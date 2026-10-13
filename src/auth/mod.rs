//! F04/F05 — Authentication.
//!
//! * [`oidc`] verifies SPA bearer tokens (JWT) against Pocket ID's JWKS.
//! * [`app_tokens`] owns the revocable tokens used by native DAV clients.
//! * [`basic`] resolves `Authorization: Basic` headers to an [`oidc::AuthUser`].
//!
//! The `AuthUser { sub }` identity is defined once in [`oidc`] and reused across
//! both flows.

pub mod app_tokens;
pub mod basic;
pub mod oidc;

pub use app_tokens::{
    AppToken, ENV_TOKEN_PEPPER, TokenError, create_app_token, generate_token, get_app_token,
    hash_secret, list_app_tokens, parse_token, rename_app_token, revoke_app_token,
    spawn_touch_last_used, touch_last_used, verify_app_token, verify_secret,
};
pub use basic::{
    BasicCredentials, DavAuth, WWW_AUTHENTICATE, basic_credentials, decode_basic,
    require_basic_auth,
};
// F05's Basic-Auth error, aliased to avoid colliding with F04's `oidc::AuthError`
// (bearer-token rejection), which keeps the name `AuthError` at this level.
pub use basic::AuthError as BasicAuthError;
pub use oidc::{
    AuthError, AuthUser, Claims, DEFAULT_CLOCK_SKEW, DEFAULT_JWKS_TTL, JwksCache, OidcConfig,
    bearer_token, require_auth,
};
