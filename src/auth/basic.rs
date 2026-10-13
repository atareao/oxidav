//! F05 — HTTP Basic Auth for the `/dav` surface.
//!
//! Native DAV clients authenticate with `Authorization: Basic base64(username:token)`,
//! where `token` is an app token minted by [`crate::auth::app_tokens`]. This
//! module decodes that header, resolves it to an [`AuthUser`] and turns every
//! failure into a `401` carrying `WWW-Authenticate: Basic realm="oxidav"`.
//!
//! The `username` is informational: the real identity is the token owner, read
//! from the database. Neither the `Authorization` header nor the token is ever
//! logged.

use axum::Json;
use axum::Router;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::json;

use crate::auth::app_tokens::{TokenError, parse_token, spawn_touch_last_used, verify_app_token};
use crate::auth::oidc::AuthUser;
use crate::router::AppState;

/// Value of the `WWW-Authenticate` challenge sent with every `401`.
pub const WWW_AUTHENTICATE: &str = r#"Basic realm="oxidav", charset="UTF-8""#;

/// Generic message returned to clients for every failure.
///
/// Deliberately does not distinguish "unknown" from "revoked" (or any other
/// cause) so the response cannot be used to probe which tokens exist.
const GENERIC_ERROR: &str = "invalid credentials";

/// Errors produced while authenticating a DAV request with Basic Auth.
///
/// This is the F05 counterpart of the plan's `AuthError`; it is re-exported from
/// [`crate::auth`] as `BasicAuthError` to avoid clashing with the F04
/// [`crate::auth::oidc::AuthError`] used for bearer tokens.
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

/// Authenticated principal for DAV routes, resolved from a Basic app token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavAuth(pub AuthUser);

impl FromRequestParts<AppState> for DavAuth {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let credentials = basic_credentials(parts)?;

        // Resolve the identity first; only then schedule the best-effort
        // `last_used_at` update (the id is public and safe to move into the task).
        let (id, _secret) = parse_token(&credentials.token)?;
        let user = verify_app_token(&state.db, &credentials.token).await?;
        spawn_touch_last_used(state.db.clone(), id);

        Ok(DavAuth(user))
    }
}

/// Wrap a router so every request must carry a valid Basic app token.
///
/// The verified [`AuthUser`] is inserted into the request extensions, so
/// downstream handlers can read it via `Extension<AuthUser>` (or use the
/// [`DavAuth`] extractor directly). Failures become `401` +
/// `WWW-Authenticate: Basic realm="oxidav", charset="UTF-8"`.
///
/// Like [`crate::auth::oidc::require_auth`], the middleware is stateful and
/// threads its own [`AppState`] through `from_fn_with_state`.
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
}
