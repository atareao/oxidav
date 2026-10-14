//! F04/F05/F17 — Authentication.
//!
//! * [`session`] is the canonical module for F17: it implements the OIDC
//!   **server-side** Authorization Code flow (signed session/flow cookies,
//!   discovery, code exchange, ID-token validation) and owns the session
//!   [`AuthError`].
//! * [`routes`] exposes the `/api/auth/*` HTTP surface over those primitives.
//! * [`middleware`] enforces the session cookie on `/api/*` and provides the
//!   [`SessionUser`] identity extractor.
//! * [`app_tokens`] owns the revocable tokens used by native DAV clients.
//! * [`basic`] resolves a DAV request to a [`SessionUser`] from a valid session
//!   cookie (browser) or an `Authorization: Basic` app token (native clients).
//!
//! The old F04 bearer-token verification (`oidc`) was removed in F17: there is
//! **no `Bearer` support** left in the backend. Programmatic access is served by
//! the DAV app tokens instead.

pub mod app_tokens;
pub mod basic;
pub mod middleware;
pub mod routes;
pub mod session;

pub use app_tokens::{
    AppToken, ENV_TOKEN_PEPPER, TokenError, create_app_token, generate_token, get_app_token,
    hash_secret, list_app_tokens, parse_token, rename_app_token, revoke_app_token,
    spawn_touch_last_used, touch_last_used, verify_app_token, verify_secret,
};
pub use basic::{
    BasicCredentials, DavAuth, WWW_AUTHENTICATE, basic_credentials, decode_basic,
    require_basic_auth,
};
// F05's Basic-Auth error, aliased so the canonical `AuthError` name stays with
// the F17 session error below (bearer `oidc::AuthError` used to own it).
pub use basic::AuthError as BasicAuthError;
pub use middleware::{SessionUser, session_middleware};
pub use session::{
    AuthConfig, AuthError, Claims, FlowClaims, OidcDiscovery, SESSION_COOKIE_NAME,
    SESSION_TTL_SECS, StateStore, TokenResponse, UserIdentity, build_end_session_url, clear_caches,
    discover, emit_flow_token, emit_session, exchange_code, fetch_jwks, generate_state_and_nonce,
    http_client, validate_flow_token, validate_id_token, validate_session,
};
