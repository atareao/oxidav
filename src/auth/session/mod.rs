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

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration as StdDuration, Instant};

use base64::Engine;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use thiserror::Error;

mod config;
mod oidc;

pub use config::AuthConfig;
pub use oidc::{
    OidcDiscovery, TokenResponse, build_end_session_url, clear_caches, discover, exchange_code,
    fetch_jwks, http_client, validate_id_token,
};

/// Name of the HttpOnly cookie that carries the signed OxiDav session.
pub const SESSION_COOKIE_NAME: &str = "oxidav_session";

/// Default session lifetime, in seconds, used as the cookie `Max-Age`.
pub const SESSION_TTL_SECS: u64 = 60 * 60 * 8;

/// Lifetime of an issued `state`. Matches the flow-cookie TTL
/// (`FLOW_TTL_SECS` in `src/routes/auth.rs`): a pending state older than this
/// is abandoned and must not be consumable.
const FLOW_TTL: StdDuration = StdDuration::from_secs(600);
/// Hard upper bound on pending (issued but unconsumed) `state` entries.
///
/// `/api/auth/login` is unauthenticated, so without a cap an attacker could
/// abandon logins indefinitely and grow the map without bound (memory DoS).
const MAX_PENDING_STATES: usize = 1024;
/// Small grace period for the ID token `exp` to absorb clock skew.
pub const ID_TOKEN_LEEWAY_SECS: u64 = 5;

/// Claims of the short-lived, signed OIDC flow cookie.
///
/// The `state`/`nonce` pair is signed with the session secret (HS256) so it
/// cannot be planted by another origin/subdomain (cookie tossing), and it is
/// additionally consumed exactly once through [`StateStore`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowClaims {
    pub state: String,
    pub nonce: String,
    pub exp: usize,
    pub iat: usize,
}

/// Claims stored inside the signed OxiDav session cookie.
///
/// Beyond the user identity (`sub`/`email`/`name`) and the `exp`/`iat`
/// bookkeeping, the session retains the provider-issued ID token in the
/// `id_token` claim so it can be used as `id_token_hint` on SSO logout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub exp: usize,
    pub iat: usize,
    /// ID token issued by the provider, retained for `end_session` logout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
}

/// Identity extracted from a successfully validated OIDC ID token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserIdentity {
    pub sub: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

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

// ---------------------------------------------------------------------------
// Session cookie
// ---------------------------------------------------------------------------

/// Sign a session token (HS256 with `config.jwt_secret`) carrying `claims`.
pub fn emit_session(config: &AuthConfig, claims: &Claims) -> Result<String, AuthError> {
    let header = Header::new(Algorithm::HS256);
    let key = EncodingKey::from_secret(config.jwt_secret.as_bytes());
    jsonwebtoken::encode(&header, claims, &key).map_err(|e| AuthError::ConfigError(e.to_string()))
}

/// Verify a session token's HS256 signature and expiration, returning its
/// claims. A missing, tampered or expired token MUST be rejected.
pub fn validate_session(config: &AuthConfig, token: &str) -> Result<Claims, AuthError> {
    let key = DecodingKey::from_secret(config.jwt_secret.as_bytes());
    let mut validation = Validation::new(Algorithm::HS256);
    // No grace period: an expired session must be rejected immediately so the
    // `/api/auth/me` contract (an expired cookie is unauthenticated) holds.
    validation.leeway = 0;

    jsonwebtoken::decode::<Claims>(token, &key, &validation)
        .map(|data| data.claims)
        .map_err(|err| match err.kind() {
            jsonwebtoken::errors::ErrorKind::ExpiredSignature => AuthError::ExpiredToken,
            _ => AuthError::InvalidSession,
        })
}

// ---------------------------------------------------------------------------
// OIDC flow cookie (signed state/nonce)
// ---------------------------------------------------------------------------

/// Sign a short-lived flow token (HS256 with `config.jwt_secret`) carrying the
/// `state` and `nonce` issued by `login`.
pub fn emit_flow_token(
    config: &AuthConfig,
    state: &str,
    nonce: &str,
    ttl_secs: u64,
) -> Result<String, AuthError> {
    let now = unix_now();
    let claims = FlowClaims {
        state: state.to_string(),
        nonce: nonce.to_string(),
        iat: now,
        exp: now + ttl_secs as usize,
    };
    let header = Header::new(Algorithm::HS256);
    let key = EncodingKey::from_secret(config.jwt_secret.as_bytes());
    jsonwebtoken::encode(&header, &claims, &key).map_err(|e| AuthError::ConfigError(e.to_string()))
}

/// Verify a flow token's HS256 signature and expiration.
pub fn validate_flow_token(config: &AuthConfig, token: &str) -> Result<FlowClaims, AuthError> {
    let key = DecodingKey::from_secret(config.jwt_secret.as_bytes());
    let mut validation = Validation::new(Algorithm::HS256);
    validation.leeway = 0;

    jsonwebtoken::decode::<FlowClaims>(token, &key, &validation)
        .map(|data| data.claims)
        .map_err(|_| AuthError::InvalidState)
}

// ---------------------------------------------------------------------------
// State / nonce
// ---------------------------------------------------------------------------

/// Generate a fresh `(state, nonce)` pair from a CSPRNG (base64url, no pad).
pub fn generate_state_and_nonce() -> (String, String) {
    let mut state_bytes = [0u8; 32];
    let mut nonce_bytes = [0u8; 32];
    // Same CSPRNG pattern as `app_tokens`: `OsRng` (rand 0.8), which is what the
    // rest of the crate already uses instead of pulling a second RNG stack.
    OsRng.fill_bytes(&mut state_bytes);
    OsRng.fill_bytes(&mut nonce_bytes);

    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    (engine.encode(state_bytes), engine.encode(nonce_bytes))
}

/// Current UNIX timestamp in seconds.
fn unix_now() -> usize {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as usize)
        .unwrap_or(0)
}

/// One-time in-memory store mapping an issued `state` to its `nonce`.
///
/// The OIDC flow persists these in flow cookies; this helper exposes the
/// issue/consume semantics the flow relies on: a `state` may be consumed at
/// most once, which the `Mutex` guarantees across concurrent requests.
///
/// Pending states are bounded in both time ([`FLOW_TTL`]) and count
/// ([`MAX_PENDING_STATES`]) because `/api/auth/login` is unauthenticated, and
/// they are stored with their issue instant so stale entries can be purged.
#[derive(Debug, Default)]
pub struct StateStore {
    states: Mutex<HashMap<String, (String, Instant)>>,
}

impl StateStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Issue a new `(state, nonce)` pair and remember it for later consumption.
    pub fn issue(&self) -> (String, String) {
        self.issue_with_now(Instant::now())
    }

    /// Consume a `state`, returning the associated `nonce` exactly once.
    pub fn consume(&self, state: &str) -> Option<String> {
        self.consume_with_now(state, Instant::now())
    }

    /// Issue against an explicit clock instant; `issue` delegates here with
    /// `Instant::now()`. Kept as an ordinary (not `#[cfg(test)]`) private
    /// method so the production path and the tests share one implementation.
    fn issue_with_now(&self, now: Instant) -> (String, String) {
        let (state, nonce) = generate_state_and_nonce();
        // A poisoned mutex must not be fatal: the map is plain data and its
        // invariant (single use) does not depend on poison state, so recover
        // the guard instead of panicking forever.
        let mut states = self
            .states
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Purge states whose flow TTL has already elapsed (abandoned logins).
        states.retain(|_, (_, at)| now.saturating_duration_since(*at) < FLOW_TTL);

        // Defensive cap: even inside the TTL an attacker could flood the
        // unauthenticated `/login` endpoint. Evict the oldest pending state (by
        // issue instant, i.e. the smallest `Instant`) before inserting so the
        // map never exceeds `MAX_PENDING_STATES` entries.
        if states.len() >= MAX_PENDING_STATES
            && let Some(oldest) = states
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(state, _)| state.clone())
        {
            states.remove(&oldest);
        }

        states.insert(state.clone(), (nonce.clone(), now));
        (state, nonce)
    }

    /// Consume against an explicit clock instant; `consume` delegates here with
    /// `Instant::now()`. A state past its TTL is treated as absent.
    fn consume_with_now(&self, state: &str, now: Instant) -> Option<String> {
        let mut states = self
            .states
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let expired = match states.get(state) {
            Some((_, at)) => now.saturating_duration_since(*at) >= FLOW_TTL,
            None => return None,
        };

        if expired {
            // Drop the stale entry so the map does not grow forever.
            states.remove(state);
            return None;
        }

        states.remove(state).map(|(nonce, _)| nonce)
    }
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
    use super::config::test_config;
    use super::*;

    fn test_claims() -> Claims {
        Claims {
            sub: "user-123".into(),
            email: Some("user@example.com".into()),
            name: Some("Test User".into()),
            exp: now() + 3600,
            iat: now(),
            id_token: Some("provider-id-token".into()),
        }
    }

    // -----------------------------------------------------------------
    // Claims baseline
    // -----------------------------------------------------------------

    #[test]
    fn test_claims_creation() {
        let claims = test_claims();
        assert_eq!(claims.sub, "user-123");
        assert_eq!(claims.email.as_deref(), Some("user@example.com"));
        assert_eq!(claims.name.as_deref(), Some("Test User"));
        assert_eq!(claims.id_token.as_deref(), Some("provider-id-token"));
    }

    #[test]
    fn test_auth_error_display() {
        let err = AuthError::MissingHeader;
        assert!(err.to_string().contains("autorización"));
        let err = AuthError::InvalidToken("bad sig".into());
        assert!(err.to_string().contains("bad sig"));
        let err = AuthError::ExpiredToken;
        assert!(err.to_string().contains("expirado"));
    }

    #[test]
    fn test_claims_serialization() {
        let claims = Claims {
            sub: "user-1".into(),
            email: None,
            name: None,
            exp: 9999999999,
            iat: 1000000000,
            id_token: None,
        };
        let json = serde_json::to_value(&claims).unwrap();
        assert_eq!(json["sub"], "user-1");
    }

    // -----------------------------------------------------------------
    // Session cookie
    // -----------------------------------------------------------------

    #[test]
    fn test_emit_and_validate_session_roundtrip() {
        let config = test_config();
        let claims = test_claims();

        let token = emit_session(&config, &claims).expect("emit_session should succeed");
        let decoded = validate_session(&config, &token).expect("valid session should decode");

        assert_eq!(decoded.sub, "user-123");
        assert_eq!(decoded.email.as_deref(), Some("user@example.com"));
        assert_eq!(decoded.name.as_deref(), Some("Test User"));
        assert_eq!(decoded.id_token.as_deref(), Some("provider-id-token"));
    }

    #[test]
    fn test_validate_session_rejects_expired_token() {
        let config = test_config();
        let mut claims = test_claims();
        claims.exp = now() - 10;

        let token = emit_session(&config, &claims).expect("emit_session should succeed");
        assert!(
            validate_session(&config, &token).is_err(),
            "expired session must be rejected"
        );
    }

    #[test]
    fn test_validate_session_rejects_tampered_signature() {
        let config = test_config();
        let token = emit_session(&config, &test_claims()).expect("emit_session should succeed");

        // Flip the last character of the signature segment.
        let mut parts: Vec<String> = token.split('.').map(str::to_string).collect();
        let sig = parts.last_mut().expect("JWT must have a signature segment");
        let mut chars: Vec<char> = sig.chars().collect();
        let last = chars.pop().unwrap();
        chars.push(if last == 'a' { 'b' } else { 'a' });
        *sig = chars.into_iter().collect();
        let tampered = parts.join(".");

        assert!(
            validate_session(&config, &tampered).is_err(),
            "tampered session signature must be rejected"
        );
    }

    // -----------------------------------------------------------------
    // OIDC flow cookie
    // -----------------------------------------------------------------

    #[test]
    fn test_flow_token_roundtrip_carries_state_and_nonce() {
        let config = test_config();
        let token =
            emit_flow_token(&config, "state-1", "nonce-1", 600).expect("flow token encodes");
        let claims = validate_flow_token(&config, &token).expect("flow token decodes");
        assert_eq!(claims.state, "state-1");
        assert_eq!(claims.nonce, "nonce-1");
    }

    #[test]
    fn test_flow_token_rejects_tampered_signature() {
        let config = test_config();
        let token = emit_flow_token(&config, "state-1", "nonce-1", 600).unwrap();

        let mut parts: Vec<String> = token.split('.').map(str::to_string).collect();
        let sig = parts.last_mut().unwrap();
        let mut chars: Vec<char> = sig.chars().collect();
        let last = chars.pop().unwrap();
        chars.push(if last == 'a' { 'b' } else { 'a' });
        *sig = chars.into_iter().collect();
        let tampered = parts.join(".");

        assert!(
            validate_flow_token(&config, &tampered).is_err(),
            "a tampered flow cookie must be rejected"
        );
    }

    #[test]
    fn test_flow_token_rejects_expired() {
        use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};

        let config = test_config();
        let now = unix_now();
        let expired = FlowClaims {
            state: "state-1".into(),
            nonce: "nonce-1".into(),
            iat: now - 20,
            exp: now - 10,
        };
        let token = encode(
            &Header::new(Algorithm::HS256),
            &expired,
            &EncodingKey::from_secret(config.jwt_secret.as_bytes()),
        )
        .unwrap();

        assert!(
            validate_flow_token(&config, &token).is_err(),
            "an expired flow cookie must be rejected"
        );
    }

    // -----------------------------------------------------------------
    // state / nonce
    // -----------------------------------------------------------------

    #[test]
    fn test_generate_state_and_nonce_are_distinct() {
        let (state_a, nonce_a) = generate_state_and_nonce();
        let (state_b, nonce_b) = generate_state_and_nonce();

        assert!(!state_a.is_empty(), "state must not be empty");
        assert!(!nonce_a.is_empty(), "nonce must not be empty");
        assert_ne!(state_a, state_b, "two states must differ");
        assert_ne!(nonce_a, nonce_b, "two nonces must differ");
    }

    #[test]
    fn test_state_store_consumes_state_once() {
        let store = StateStore::new();
        let (state, nonce) = store.issue();

        assert_eq!(
            store.consume(&state).as_deref(),
            Some(nonce.as_str()),
            "first consumption returns the associated nonce"
        );
        assert!(
            store.consume(&state).is_none(),
            "a state must not be accepted twice"
        );
    }

    // -----------------------------------------------------------------
    // StateStore TTL / defensive cap
    // -----------------------------------------------------------------

    #[test]
    fn state_expires_after_ttl() {
        let store = StateStore::new();
        let now = Instant::now();
        let (state, _nonce) = store.issue_with_now(now);

        assert!(
            store
                .consume_with_now(&state, now + FLOW_TTL + StdDuration::from_secs(1))
                .is_none(),
            "a state older than the flow TTL must not be consumable"
        );
    }

    #[test]
    fn state_within_ttl_is_consumed_once() {
        let store = StateStore::new();
        let now = Instant::now();
        let (state, nonce) = store.issue_with_now(now);

        assert_eq!(
            store
                .consume_with_now(&state, now + FLOW_TTL - StdDuration::from_secs(1))
                .as_deref(),
            Some(nonce.as_str()),
            "a state within the TTL must still return its nonce"
        );
        assert!(
            store.consume_with_now(&state, now).is_none(),
            "a state must never be consumed twice"
        );
    }

    #[test]
    fn state_store_enforces_cap() {
        let store = StateStore::new();
        let base = Instant::now();

        let mut last_state = String::new();
        for i in 0..(MAX_PENDING_STATES + 10) {
            let (state, _nonce) = store.issue_with_now(base + StdDuration::from_millis(i as u64));
            last_state = state;
        }

        let pending = store
            .states
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len();
        assert!(
            pending <= MAX_PENDING_STATES,
            "the pending-state store must stay bounded, got {pending}"
        );

        // The most recently issued state must survive the oldest-first eviction.
        assert!(
            store
                .states
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains_key(&last_state),
            "the most recently issued state must still be present"
        );
    }
}
