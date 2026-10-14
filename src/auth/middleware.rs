//! F17 — Session authentication middleware and request identity.
//!
//! [`session_middleware`] enforces a valid OxiDav session cookie on every
//! protected `/api/*` route when authentication is enabled. It is a
//! **pass-through** while auth is disabled (or unconfigured), so the API runs as
//! the synthetic dev user ([`SessionUser::dev`]) during development; `/api/auth/*`
//! is always exempt because those routes must be reachable before a session
//! exists.
//!
//! The verified identity is placed in the request extensions, so handlers obtain
//! it with the lightweight [`SessionUser`] extractor instead of re-reading (and
//! re-validating) the cookie on every handler.

use axum::extract::{FromRequestParts, Request, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::cookie::CookieJar;

use crate::api::error::ApiError;
use crate::auth::session::{SESSION_COOKIE_NAME, validate_session};
use crate::router::AppState;

/// Authenticated identity attached to a request by [`session_middleware`].
///
/// It carries the subset of the session claims the API and DAV surfaces consume.
/// It is **never** built from a `Bearer` token: the API fills it from the signed
/// session cookie and DAV fills it from an app token (see
/// [`crate::auth::basic`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionUser {
    /// Stable user id (the Pocket ID `sub` claim), used to scope collections.
    pub sub: String,
    /// Email claim, when the provider supplied one.
    pub email: Option<String>,
    /// Display name, when the provider supplied one.
    pub name: Option<String>,
}

impl SessionUser {
    /// The synthetic identity used while authentication is disabled.
    ///
    /// Keeps the API usable in development without an IdP: every request is the
    /// same `sub = "dev"`, exactly as `valet` does with auth disabled.
    #[must_use]
    pub fn dev() -> Self {
        Self {
            sub: "dev".to_string(),
            email: None,
            name: Some("Dev User".to_string()),
        }
    }
}

impl FromRequestParts<AppState> for SessionUser {
    /// A missing extension is a wiring bug, not a client error: the middleware
    /// always injects the identity (or the dev user) before the handler runs, so
    /// its absence maps to a generic `500` rather than a `401`.
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<SessionUser>()
            .cloned()
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

/// Enforce a valid session cookie on protected `/api/*` routes.
///
/// * Without an enabled [`crate::auth::session::AuthConfig`] the middleware is a
///   pass-through and injects [`SessionUser::dev`].
/// * `/api/auth/*` and every non-`/api/` path (the SPA and `/dav`) are exempt;
///   DAV keeps its own Basic Auth.
/// * Any other `/api/*` request must present a valid session cookie, otherwise
///   it is answered with the uniform `401` envelope.
pub async fn session_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(config) = state.auth_config.as_deref().filter(|config| config.enabled) else {
        // Auth disabled: the API runs as the dev user, untouched.
        request.extensions_mut().insert(SessionUser::dev());
        return next.run(request).await;
    };

    if is_exempt(request.uri().path()) {
        return next.run(request).await;
    }

    let jar = CookieJar::from_headers(request.headers());
    let Some(session) = jar.get(SESSION_COOKIE_NAME) else {
        return ApiError::Unauthorized.into_response();
    };

    match validate_session(config, session.value()) {
        Ok(claims) => {
            request.extensions_mut().insert(SessionUser {
                sub: claims.sub,
                email: claims.email,
                name: claims.name,
            });
            next.run(request).await
        }
        Err(_) => ApiError::Unauthorized.into_response(),
    }
}

/// Whether `path` is exempt from the session requirement.
///
/// The decision is made on **segment boundaries**, after a minimal
/// normalization (repeated slashes collapsed, `.`/`..` segments resolved), so a
/// naive prefix such as `/api/authfoo` can never inherit the auth exemption and
/// a `..`-flavoured path cannot sneak into it.
///
/// * Anything **outside `/api`** is exempt: the SPA, `/dav` (which has its own
///   Basic Auth), `/healthz` and any future non-API route. The gate exists to
///   decide which routes **of `/api`** need a session, nothing else.
/// * `/api/auth` and everything under `/api/auth/` are exempt, because those
///   routes must be reachable before a session exists.
/// * Every other `/api/*` path is protected.
fn is_exempt(path: &str) -> bool {
    let path = normalize_path(path);

    // Non-API surfaces keep their exemption, whatever their namespace: only
    // `/api` routes are gated, so `/healthz`, `/healthzfoo` or any future
    // non-API path are all exempt.
    if path != "/api" && !path.starts_with("/api/") {
        return true;
    }

    // The auth routes must be reachable without a session.
    path == "/api/auth" || path.starts_with("/api/auth/")
}

/// Collapse repeated slashes and drop `.`/`..` segments, keeping the path
/// absolute. This is a *normalization for comparison only*: it mirrors what a
/// well-behaved router would resolve, so the exemption cannot be bypassed with
/// `/api/../api/auth/../../v1/me` or `//api/auth/../v1/me`.
fn normalize_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }

    let mut normalized = String::new();
    if absolute {
        normalized.push('/');
    }
    normalized.push_str(&segments.join("/"));
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_user_has_the_documented_identity() {
        let user = SessionUser::dev();
        assert_eq!(user.sub, "dev");
        assert_eq!(user.email, None);
        assert_eq!(user.name.as_deref(), Some("Dev User"));
    }

    #[test]
    fn non_api_and_auth_paths_are_exempt() {
        // Non-API surfaces (SPA, health, DAV) never require a session.
        assert!(is_exempt("/healthz"));
        assert!(is_exempt("/dav/calendars/user-a/"));
        assert!(is_exempt("/some/spa/route"));
        // The auth routes must be reachable without a session.
        assert!(is_exempt("/api/auth/login"));
        assert!(is_exempt("/api/auth/callback"));
        // The REST API is protected.
        assert!(!is_exempt("/api/v1/collections"));
        assert!(!is_exempt("/api/v1/me"));
        assert!(!is_exempt("/api/unknown"));
    }

    /// Exemption is decided on **segment boundaries**, not naive prefixes, so a
    /// future `/api/authfoo` can never inherit the auth exemption.
    #[test]
    fn is_exempt_respects_segment_boundaries() {
        // The bare prefix `/api/auth` is exempt, but only the exact segment.
        assert!(is_exempt("/api/auth"));
        assert!(is_exempt("/api/auth/"));
        assert!(is_exempt("/api/auth/login"));
        assert!(is_exempt("/api/auth/callback"));
        // Not a segment boundary: `authfoo` is a different segment.
        assert!(!is_exempt("/api/authfoo"));
        assert!(!is_exempt("/api/authz/login"));
        // Every non-`/api` path is exempt, whatever its namespace: the gate only
        // decides among `/api` routes, so `/healthz` and anything that merely
        // resembles it stay out of the session requirement.
        assert!(is_exempt("/healthz"));
        assert!(is_exempt("/healthzfoo"));
        assert!(is_exempt("/healthz/ready"));
    }

    /// Path normalization collapses repeated slashes and drops `.`/`..`
    /// segments before the exemption decision, so a traversal-flavoured path
    /// cannot sneak into the auth exemption.
    #[test]
    fn is_exempt_normalizes_before_deciding() {
        // Repeated slashes and a `.` segment still resolve to a real auth path.
        assert!(is_exempt("/api//auth/login"));
        assert!(is_exempt("/api/./auth/login"));
        assert!(is_exempt("/api/auth/../auth/login"));
        // A `..` that escapes the auth prefix must NOT be exempt.
        assert!(!is_exempt("/api/auth/../v1/me"));
        assert!(!is_exempt("//api/auth/../v1/me"));
        // Non-API surfaces keep their exemption after normalization.
        assert!(is_exempt("/dav/calendars/user-a/"));
        assert!(is_exempt("/some/spa/route"));
    }
}
