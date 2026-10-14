//! F05/F17 — authentication for the `/dav` surface.
//!
//! Native DAV clients authenticate with `Authorization: Basic base64(username:token)`,
//! where `token` is an app token minted by [`crate::auth::app_tokens`]. A request
//! that also carries a valid signed session cookie (see
//! [`crate::auth::session`]) is admitted through the browser path first, so a
//! logged-in SPA can reach its own collections with the cookie it already has.
//!
//! The [`DavAuth`] extractor resolves **both** paths to the same [`SessionUser`]
//! identity, because the CalDAV/CardDAV handlers scope collections by that
//! identity (`ensure_owner`). Either way a failure is turned into a `401`
//! carrying `WWW-Authenticate: Basic realm="oxidav"`.
//!
//! The `username` in a Basic header is informational: the real identity is the
//! token owner, read from the database. Neither the `Authorization` header nor
//! the token is ever logged.
//!
//! # Why accepting the session cookie here is safe (CSRF)
//!
//! DAV mutations (`PUT`, `DELETE`, `MKCALENDAR`, …) are state-changing requests
//! that a rogue cross-site page might try to trigger with the victim's cookie.
//! Two layers defend them:
//!
//! 1. The `oxidav_session` cookie is issued `SameSite=Lax` (see
//!    [`crate::auth::routes`]), so the browser does **not** attach it to a
//!    *cross-site* request except top-level GET navigations — precisely the
//!    non-mutating shape.
//! 2. `SameSite=Lax` only separates *sites*, not *origins*: a **sibling
//!    subdomain** of the same registrable domain (`evil.oxidav.example` vs
//!    `app.oxidav.example`) is *same-site*, so the browser would happily attach
//!    the cookie to a `PUT`/`DELETE`/`MKCALENDAR` it sends. To close that hole,
//!    a cookie-authenticated request with a **state-changing method** must also
//!    present same-origin evidence — `Sec-Fetch-Site: same-origin`/`none`, or an
//!    `Origin` matching the request authority. Without it the request is refused
//!    with `403`: the credential is valid, the *context* is not. Native DAV
//!    clients send neither header, but they authenticate with Basic, which this
//!    check never gates, so they are unaffected.
//!
//! Either way a failure is turned into a `401` carrying
//! `WWW-Authenticate: Basic realm="oxidav"` (a `403` is used for the CSRF
//! refusal above, where the credential itself was fine).

use axum::Json;
use axum::Router;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::cookie::CookieJar;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::json;

use crate::auth::app_tokens::{TokenError, parse_token, spawn_touch_last_used, verify_app_token};
use crate::auth::middleware::SessionUser;
use crate::auth::session::{SESSION_COOKIE_NAME, validate_session};
use crate::router::AppState;

/// Value of the `WWW-Authenticate` challenge sent with every `401`.
pub const WWW_AUTHENTICATE: &str = r#"Basic realm="oxidav", charset="UTF-8""#;

/// Generic message returned to clients for every failure.
///
/// Deliberately does not distinguish "unknown" from "revoked" (or any other
/// cause) so the response cannot be used to probe which tokens exist.
const GENERIC_ERROR: &str = "invalid credentials";

/// Message returned for a CSRF refusal (`403`). Deliberately generic: the
/// credential was valid, only the request context was rejected, so no internal
/// detail is disclosed.
const CROSS_ORIGIN_ERROR: &str = "cross-origin request rejected";

/// Errors produced while authenticating a DAV request with Basic Auth.
///
/// This is the F05 counterpart of the plan's `AuthError`; it is re-exported from
/// [`crate::auth`] as `BasicAuthError` to avoid clashing with the canonical
/// [`crate::auth::session::AuthError`] used by the F17 session flow.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// No `Authorization` header was present.
    #[error("cabecera Authorization ausente")]
    MissingHeader,
    /// The scheme was present but is not `Basic`.
    #[error("esquema de autenticación no soportado")]
    UnsupportedScheme,
    /// The credentials were not valid Base64.
    #[error("credenciales base64 inválidas")]
    InvalidBase64,
    /// The decoded credentials were not `username:token`.
    #[error("credenciales malformadas")]
    MalformedCredentials,
    /// Token resolution failed (unknown, revoked, mismatched or malformed).
    #[error(transparent)]
    Token(#[from] TokenError),
    /// A cookie-authenticated, state-changing request lacked same-origin
    /// evidence. Answered with `403` (credential valid, context refused).
    #[error("petición cross-origin rechazada")]
    CrossOriginRequest,
}

impl AuthError {
    /// The `WWW-Authenticate` challenge to emit alongside the `401`.
    #[must_use]
    pub fn www_authenticate(&self) -> &'static str {
        WWW_AUTHENTICATE
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        // Log the reason internally (never the header or the token), but return
        // a generic body to the client.
        tracing::debug!(error = %self, "rejecting Basic auth request");

        // A CSRF refusal is not an authentication failure: the credential was
        // valid, only the request context was rejected. It answers `403`
        // *without* a `WWW-Authenticate` challenge, because re-authenticating
        // would not help.
        if matches!(self, AuthError::CrossOriginRequest) {
            let body = Json(json!({ "ok": false, "error": CROSS_ORIGIN_ERROR }));
            return (StatusCode::FORBIDDEN, body).into_response();
        }

        let body = Json(json!({ "ok": false, "error": GENERIC_ERROR }));
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, WWW_AUTHENTICATE)],
            body,
        )
            .into_response()
    }
}

/// Credentials extracted from an `Authorization: Basic` header.
///
/// `Debug` is implemented by hand so the app token is redacted: it must never
/// reach a log line via `{:?}`. The username is informational and safe to print.
#[derive(Clone, PartialEq, Eq)]
pub struct BasicCredentials {
    /// Informational user name supplied by the client.
    pub username: String,
    /// The app token (the part after the first `:`).
    pub token: String,
}

impl std::fmt::Debug for BasicCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BasicCredentials")
            .field("username", &self.username)
            .field("token", &"…")
            .finish()
    }
}

/// Decode a raw `Authorization` header value of the form `Basic <base64>`.
///
/// The base64 payload is decoded as UTF-8 and split at the **first** `:` into
/// `username:token`; the token is the whole remainder, so a token that itself
/// contains `:` survives intact.
///
/// # Errors
///
/// * [`AuthError::UnsupportedScheme`] — no space, or a scheme other than `Basic`.
/// * [`AuthError::InvalidBase64`] — the payload is not valid Base64.
/// * [`AuthError::MalformedCredentials`] — the payload is not UTF-8, has no `:`,
///   or carries an empty token.
pub fn decode_basic(value: &str) -> Result<BasicCredentials, AuthError> {
    let (scheme, encoded) = value.split_once(' ').ok_or(AuthError::UnsupportedScheme)?;
    if !scheme.eq_ignore_ascii_case("Basic") {
        return Err(AuthError::UnsupportedScheme);
    }

    let decoded = STANDARD
        .decode(encoded.trim())
        .map_err(|_| AuthError::InvalidBase64)?;
    let decoded = String::from_utf8(decoded).map_err(|_| AuthError::MalformedCredentials)?;

    let (username, token) = decoded
        .split_once(':')
        .ok_or(AuthError::MalformedCredentials)?;
    if token.is_empty() {
        return Err(AuthError::MalformedCredentials);
    }

    Ok(BasicCredentials {
        username: username.to_string(),
        token: token.to_string(),
    })
}

/// Extract and decode the `Authorization: Basic` header from request parts.
///
/// # Errors
///
/// [`AuthError::MissingHeader`] when the header is absent, plus every error of
/// [`decode_basic`].
pub fn basic_credentials(parts: &Parts) -> Result<BasicCredentials, AuthError> {
    let value = parts
        .headers
        .get(header::AUTHORIZATION)
        .ok_or(AuthError::MissingHeader)?;
    let value = value.to_str().map_err(|_| AuthError::InvalidBase64)?;
    decode_basic(value)
}

/// Authenticated principal for DAV routes.
///
/// Resolved from a valid session cookie when authentication is enabled, and
/// otherwise from a Basic app token. Both paths yield the same [`SessionUser`]
/// identity, so the DAV handlers are agnostic about which one authenticated the
/// request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavAuth(pub SessionUser);

impl FromRequestParts<AppState> for DavAuth {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // 1. Browser path: a valid session cookie wins when auth is enabled. An
        //    *invalid/expired* cookie yields `None` here rather than an error, so
        //    the request simply falls through to Basic (and ends as the same
        //    Basic-challenge 401 if Basic does not authenticate either).
        if let Some(user) = session_user_from_cookie(parts, state) {
            // A state-changing request authenticated by the cookie must prove it
            // is same-origin: `SameSite=Lax` only separates sites, so a sibling
            // subdomain of the same registrable domain would otherwise be able to
            // drive mutations with the victim's cookie attached.
            if is_state_changing(&parts.method) && !has_same_origin_evidence(parts) {
                return Err(AuthError::CrossOriginRequest);
            }
            return Ok(DavAuth(user));
        }

        // 2. Native-client path: `Authorization: Basic` with an app token.
        let credentials = basic_credentials(parts)?;

        // Resolve the identity first; only then schedule the best-effort
        // `last_used_at` update (the id is public and safe to move into the task).
        let (id, _secret) = parse_token(&credentials.token)?;
        let user = verify_app_token(&state.db, &credentials.token).await?;
        spawn_touch_last_used(state.db.clone(), id);

        Ok(DavAuth(user))
    }
}

/// Resolve the request's identity from a valid session cookie, if any.
///
/// Returns `None` — never an error — unless auth is enabled *and* the cookie is
/// present *and* its signature and expiry verify. Falling back to `None` is what
/// lets an invalid/expired cookie degrade into the Basic path instead of
/// short-circuiting with a different `401`.
fn session_user_from_cookie(parts: &Parts, state: &AppState) -> Option<SessionUser> {
    let config = state
        .auth_config
        .as_deref()
        .filter(|config| config.enabled)?;

    let jar = CookieJar::from_headers(&parts.headers);
    let session = jar.get(SESSION_COOKIE_NAME)?;

    let claims = validate_session(config, session.value()).ok()?;
    Some(SessionUser {
        sub: claims.sub,
        email: claims.email,
        name: claims.name,
    })
}

/// Whether `method` changes server state, and therefore needs same-origin
/// evidence when a request is authenticated by the session cookie.
///
/// The safe/read verbs `GET`, `HEAD`, `OPTIONS` and the DAV read verbs
/// `PROPFIND`/`REPORT` are excluded; everything else (including
/// `PUT`/`DELETE`/`MKCALENDAR`/`MKCOL`) is treated as mutating.
fn is_state_changing(method: &Method) -> bool {
    !matches!(
        method.as_str(),
        "GET" | "HEAD" | "OPTIONS" | "PROPFIND" | "REPORT"
    )
}

/// Whether the request carries same-origin evidence acceptable for a
/// cookie-authenticated mutation.
///
/// Accepts either the modern `Sec-Fetch-Site` signal (`same-origin` or `none`),
/// or a legacy `Origin` header equal to the request's own authority. It never
/// trusts `Sec-Fetch-Site: same-site` — that is exactly the sibling-subdomain
/// case being defended against.
fn has_same_origin_evidence(parts: &Parts) -> bool {
    if let Some(site) = parts
        .headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
    {
        let site = site.trim();
        if site.eq_ignore_ascii_case("same-origin") || site.eq_ignore_ascii_case("none") {
            return true;
        }
    }

    match (
        parts
            .headers
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok()),
        parts
            .headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok()),
    ) {
        (Some(origin), Some(host)) => origin_matches_host(origin, host),
        _ => false,
    }
}

/// Whether the `Origin` header names the same authority as the request's `Host`.
///
/// Only `http`/`https` origins count (an opaque `null` origin never matches) and
/// the host comparison is case-insensitive. When the `Host` carries an explicit
/// port, the origin's effective port must match it.
fn origin_matches_host(origin: &str, host_header: &str) -> bool {
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(origin_host) = url.host_str() else {
        return false;
    };

    // Split the `Host` header into host and optional port. An IPv6 authority is
    // bracketed (`[::1]:8443`), so only treat a trailing `:digits` as a port.
    let (host, port) = match host_header.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            (h, p.parse::<u16>().ok())
        }
        _ => (host_header, None),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let origin_host = origin_host.trim_start_matches('[').trim_end_matches(']');

    if !origin_host.eq_ignore_ascii_case(host) {
        return false;
    }

    match port {
        Some(port) => url.port_or_known_default() == Some(port),
        None => true,
    }
}

/// Wrap a router so every request must authenticate as a DAV principal.
///
/// The request may authenticate either with a valid session cookie (when auth is
/// enabled) or with a Basic app token; see [`DavAuth`] for the precedence. The
/// resolved [`SessionUser`] is inserted into the request extensions, so
/// downstream handlers can read it with the [`SessionUser`] extractor directly
/// (or via `Extension<SessionUser>`). Failures become `401` +
/// `WWW-Authenticate: Basic realm="oxidav", charset="UTF-8"`.
///
/// Like [`crate::auth::middleware::session_middleware`], the middleware is
/// stateful and threads its own [`AppState`] through `from_fn_with_state`.
pub fn require_basic_auth(router: Router<AppState>, state: AppState) -> Router<AppState> {
    router.layer(axum::middleware::from_fn_with_state(
        state,
        basic_auth_middleware,
    ))
}

/// Middleware backing [`require_basic_auth`].
async fn basic_auth_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AuthError> {
    let (mut parts, body) = request.into_parts();
    let DavAuth(user) = DavAuth::from_request_parts(&mut parts, &state).await?;
    let mut request = Request::from_parts(parts, body);
    request.extensions_mut().insert(user);
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::http::Request as HttpRequest;

    fn encode(raw: &str) -> String {
        STANDARD.encode(raw.as_bytes())
    }

    fn parts_with_authorization(value: Option<&str>) -> Parts {
        let mut builder = HttpRequest::builder();
        if let Some(value) = value {
            builder = builder.header(header::AUTHORIZATION, value);
        }
        let request = builder.body(()).expect("build request");
        let (parts, ()) = request.into_parts();
        parts
    }

    #[test]
    fn decode_basic_accepts_valid_credentials() {
        let value = format!("Basic {}", encode("alice:oxidav_abc_def"));
        let creds = decode_basic(&value).expect("valid header");
        assert_eq!(creds.username, "alice");
        assert_eq!(creds.token, "oxidav_abc_def");
    }

    #[test]
    fn decode_basic_is_scheme_case_insensitive() {
        let value = format!("basic {}", encode("alice:oxidav_abc_def"));
        assert!(decode_basic(&value).is_ok());
    }

    #[test]
    fn decode_basic_keeps_colons_in_the_token() {
        let value = format!("Basic {}", encode("alice:oxidav_abc_def:extra"));
        let creds = decode_basic(&value).expect("valid header");
        assert_eq!(creds.username, "alice");
        assert_eq!(creds.token, "oxidav_abc_def:extra");
    }

    #[test]
    fn decode_basic_rejects_bad_input() {
        // Invalid base64.
        assert!(matches!(
            decode_basic("Basic !!!not-base64!!!"),
            Err(AuthError::InvalidBase64)
        ));
        // Missing separator.
        assert!(matches!(
            decode_basic(&format!("Basic {}", encode("alice"))),
            Err(AuthError::MalformedCredentials)
        ));
        // Empty token.
        assert!(matches!(
            decode_basic(&format!("Basic {}", encode("alice:"))),
            Err(AuthError::MalformedCredentials)
        ));
        // Wrong scheme / no scheme.
        assert!(matches!(
            decode_basic(&format!("Bearer {}", encode("alice:token"))),
            Err(AuthError::UnsupportedScheme)
        ));
        assert!(matches!(
            decode_basic("nospace"),
            Err(AuthError::UnsupportedScheme)
        ));
    }

    #[test]
    fn basic_credentials_reads_the_header() {
        let value = format!("Basic {}", encode("alice:oxidav_abc_def"));
        let parts = parts_with_authorization(Some(&value));
        assert!(basic_credentials(&parts).is_ok());

        let missing = parts_with_authorization(None);
        assert!(matches!(
            basic_credentials(&missing),
            Err(AuthError::MissingHeader)
        ));
    }

    #[test]
    fn errors_expose_the_basic_challenge() {
        // Exercise the real `IntoResponse` path, not a copy of the constant:
        // a 401 must carry the Basic challenge in the `WWW-Authenticate` header.
        let response = AuthError::MissingHeader.into_response();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let challenge = response
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .expect("401 must carry a WWW-Authenticate header");
        assert_eq!(
            challenge.to_str().expect("ASCII challenge"),
            WWW_AUTHENTICATE
        );
    }

    /// Build request `Parts` from a method and a set of headers, to exercise
    /// the CSRF helpers directly.
    fn parts_with(method: &str, headers: &[(&str, &str)]) -> Parts {
        let mut builder = HttpRequest::builder().method(method);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder.body(()).expect("build request");
        let (parts, ()) = request.into_parts();
        parts
    }

    /// Only truly state-changing verbs are gated; DAV reads (PROPFIND/REPORT)
    /// and the safe HTTP verbs are not.
    #[test]
    fn state_changing_methods_are_classified() {
        for method in ["PUT", "DELETE", "MKCALENDAR", "MKCOL", "POST", "PATCH"] {
            let parts = parts_with(method, &[]);
            assert!(
                is_state_changing(&parts.method),
                "`{method}` mutates state and must be gated"
            );
        }
        for method in ["GET", "HEAD", "OPTIONS", "PROPFIND", "REPORT"] {
            let parts = parts_with(method, &[]);
            assert!(
                !is_state_changing(&parts.method),
                "`{method}` is a read and must not be gated"
            );
        }
    }

    /// `Sec-Fetch-Site: same-origin` (or `none`) is accepted as evidence.
    #[test]
    fn same_origin_evidence_accepts_sec_fetch_site() {
        assert!(has_same_origin_evidence(&parts_with(
            "PUT",
            &[("sec-fetch-site", "same-origin")]
        )));
        assert!(has_same_origin_evidence(&parts_with(
            "PUT",
            &[("sec-fetch-site", "none")]
        )));
        assert!(!has_same_origin_evidence(&parts_with(
            "PUT",
            &[("sec-fetch-site", "cross-site")]
        )));
        assert!(!has_same_origin_evidence(&parts_with(
            "PUT",
            &[("sec-fetch-site", "same-site")]
        )));
    }

    /// Without a modern `Sec-Fetch-Site`, the `Origin` header is compared
    /// against the request authority: a sibling subdomain is rejected, the
    /// request's own origin is accepted.
    #[test]
    fn same_origin_evidence_compares_origin_with_host() {
        // Own origin.
        assert!(has_same_origin_evidence(&parts_with(
            "PUT",
            &[
                ("origin", "https://app.oxidav.example"),
                ("host", "app.oxidav.example"),
            ]
        )));
        // Sibling subdomain is same-site but a different origin.
        assert!(!has_same_origin_evidence(&parts_with(
            "PUT",
            &[
                ("origin", "https://evil.oxidav.example"),
                ("host", "app.oxidav.example"),
            ]
        )));
        // Origin with an explicit port must match the Host port.
        assert!(has_same_origin_evidence(&parts_with(
            "PUT",
            &[
                ("origin", "https://app.oxidav.example:8443"),
                ("host", "app.oxidav.example:8443"),
            ]
        )));
        assert!(!has_same_origin_evidence(&parts_with(
            "PUT",
            &[
                ("origin", "https://app.oxidav.example:8443"),
                ("host", "app.oxidav.example:9443"),
            ]
        )));
        // A null/opaque Origin is never evidence, and Origin alone is not
        // enough without a Host to compare against.
        assert!(!has_same_origin_evidence(&parts_with(
            "PUT",
            &[("origin", "null"), ("host", "app.oxidav.example")]
        )));
        assert!(!has_same_origin_evidence(&parts_with(
            "PUT",
            &[("origin", "https://app.oxidav.example")]
        )));
        // No evidence headers at all.
        assert!(!has_same_origin_evidence(&parts_with("PUT", &[])));
    }
}
