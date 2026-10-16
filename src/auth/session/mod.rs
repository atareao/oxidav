//! F17 — Authentication primitives for the OIDC Authorization Code flow.
//!
//! Provides session sign/verify (HS256), `state`/`nonce` generation and
//! one-time consumption, OIDC discovery/JWKS retrieval, authorization code
//! exchange and ID token validation. All crypto primitives come from
//! `jsonwebtoken`; randomness from `rand`; HTTP from the shared `reqwest`
//! client.
//!
//! This module is the OxiDav port of `valet/src/auth.rs`: the *design* matches
//! valet (server-side confidential-client flow, signed HttpOnly cookies), while
//! the crypto backend stays on `jsonwebtoken`'s `rust_crypto` feature so the
//! static musl binary never links OpenSSL/`aws-lc-sys`.

use thiserror::Error;

mod config;
mod oidc;
mod tokens;

pub use config::AuthConfig;
pub use oidc::{
    OidcDiscovery, TokenResponse, build_end_session_url, clear_caches, discover, exchange_code,
    fetch_jwks, http_client, validate_id_token,
};
pub use tokens::{
    Claims, FlowClaims, StateStore, UserIdentity, emit_flow_token, emit_session,
    generate_state_and_nonce, validate_flow_token, validate_session,
};

/// Name of the HttpOnly cookie that carries the signed OxiDav session.
pub const SESSION_COOKIE_NAME: &str = "oxidav_session";

/// Default session lifetime, in seconds, used as the cookie `Max-Age`.
pub const SESSION_TTL_SECS: u64 = 60 * 60 * 8;

/// Small grace period for the ID token `exp` to absorb clock skew.
pub const ID_TOKEN_LEEWAY_SECS: u64 = 5;

#[derive(Error, Debug)]
pub enum AuthError {
    #[error("falta la cabecera de autorización")]
    MissingHeader,
    #[error("token inválido: {0}")]
    InvalidToken(String),
    #[error("token expirado")]
    ExpiredToken,
    #[error("error de configuración: {0}")]
    ConfigError(String),
    #[error("la sesión está malformada o tiene una firma inválida")]
    InvalidSession,
    #[error("el state OIDC falta, es desconocido o ya se ha consumido")]
    InvalidState,
    #[error("el nonce OIDC no coincide")]
    InvalidNonce,
    #[error("el emisor del token de identidad no coincide")]
    InvalidIssuer,
    #[error("la audiencia del token de identidad no coincide con el client id")]
    InvalidAudience,
    #[error("el descubrimiento OIDC ha fallado")]
    DiscoveryFailed,
    #[error("no se ha podido obtener el JWKS")]
    JwksUnavailable,
    #[error("el intercambio del código de autorización ha fallado")]
    TokenExchangeFailed,
}

/// Current UNIX timestamp in seconds, shared by the session and OIDC tests.
#[cfg(test)]
pub(super) fn now() -> usize {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_error_display() {
        let err = AuthError::MissingHeader;
        assert!(err.to_string().contains("autorización"));
        let err = AuthError::InvalidToken("bad sig".into());
        assert!(err.to_string().contains("bad sig"));
        let err = AuthError::ExpiredToken;
        assert!(err.to_string().contains("expirado"));
    }
}
