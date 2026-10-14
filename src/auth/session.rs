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
use std::sync::{Mutex, OnceLock};
use std::time::{Duration as StdDuration, Instant};

use base64::Engine;
use futures::StreamExt;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rand::RngCore;
use rand::rngs::OsRng;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Name of the HttpOnly cookie that carries the signed OxiDav session.
pub const SESSION_COOKIE_NAME: &str = "oxidav_session";

/// Default session lifetime, in seconds, used as the cookie `Max-Age`.
pub const SESSION_TTL_SECS: u64 = 60 * 60 * 8;

/// Timeout applied to every outbound OIDC HTTP request.
const HTTP_TIMEOUT_SECS: u64 = 10;
/// Maximum size (bytes) accepted for discovery/JWKS/token response bodies.
const MAX_BODY_BYTES: usize = 64 * 1024;
/// How long a fetched discovery document is served from the cache.
const DISCOVERY_TTL: StdDuration = StdDuration::from_secs(300);
/// How long a fetched JWKS document is served from the cache.
const JWKS_TTL: StdDuration = StdDuration::from_secs(300);
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

/// Fail-closed configuration for the OIDC integration.
///
/// The derived [`Default`] leaves every required setting empty (and `enabled`
/// `false`). With auth enabled and any of them unset, [`AuthConfig::validate`]
/// aborts startup instead of trusting a placeholder issuer, pointing the
/// callback at an inexistent route or adding a stray CORS origin.
#[derive(Debug, Clone, Default)]
pub struct AuthConfig {
    pub enabled: bool,
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    pub jwt_secret: String,
    /// Optional `post_logout_redirect_uri` sent to the provider on logout.
    pub post_logout_redirect_url: String,
}

impl AuthConfig {
    /// Fail-closed validation: when auth is enabled, every required setting must
    /// be present. An empty `jwt_secret` would let anyone forge a session cookie
    /// (HMAC with an empty key), so a misconfigured server must not start.
    pub fn validate(&self) -> Result<(), AuthError> {
        if !self.enabled {
            return Ok(());
        }

        let required = [
            ("AUTH_ISSUER_URL", self.issuer_url.as_str()),
            ("AUTH_CLIENT_ID", self.client_id.as_str()),
            ("AUTH_CLIENT_SECRET", self.client_secret.as_str()),
            ("AUTH_REDIRECT_URL", self.redirect_url.as_str()),
            ("JWT_SECRET", self.jwt_secret.as_str()),
        ];

        let missing: Vec<&str> = required
            .iter()
            .filter(|(_, value)| value.trim().is_empty())
            .map(|(name, _)| *name)
            .collect();

        if !missing.is_empty() {
            return Err(AuthError::ConfigError(format!(
                "auth is enabled but these required settings are empty: {}",
                missing.join(", ")
            )));
        }

        // Fail-closed scheme check. With auth enabled the discovery, the JWKS
        // fetch and the token `POST` all carry credentials (`client_secret`,
        // the authorization `code`); over plain HTTP they would travel in the
        // clear. We demand an absolute `https` URL, tolerating `http` **only**
        // on a loopback host, where the traffic never leaves the machine (a
        // local IdP in development). Parsing with `url` avoids hand-rolled
        // string checks that could be fooled by wildcards or "almost https".
        if !is_secure_url(&self.issuer_url) {
            return Err(AuthError::ConfigError(
                "AUTH_ISSUER_URL debe ser una URL absoluta con esquema `https` \
                 (se admite `http` solo si el host es loopback, p. ej. localhost): \
                 un issuer en claro enviaría el client_secret y el código de \
                 autorización sin cifrar"
                    .to_string(),
            ));
        }
        if !is_secure_url(&self.redirect_url) {
            return Err(AuthError::ConfigError(
                "AUTH_REDIRECT_URL debe ser una URL absoluta con esquema `https` \
                 (se admite `http` solo si el host es loopback): un redirect en \
                 claro devolvería el código de autorización sin cifrar"
                    .to_string(),
            ));
        }

        Ok(())
    }

    /// Build the auth configuration from the single [`crate::config::Config`]
    /// source and validate it fail-closed.
    ///
    /// # Errors
    ///
    /// [`AuthError::ConfigError`] when auth is enabled but any required setting
    /// (issuer, client id/secret, redirect URL or JWT secret) is empty.
    pub fn from_config(config: &crate::config::Config) -> Result<Self, AuthError> {
        let auth = Self {
            enabled: config.auth_enabled,
            issuer_url: config.auth_issuer_url.clone(),
            client_id: config.auth_client_id.clone(),
            client_secret: config.auth_client_secret.clone(),
            redirect_url: config.auth_redirect_url.clone(),
            jwt_secret: config.jwt_secret.clone(),
            post_logout_redirect_url: config.auth_post_logout_redirect_url.clone(),
        };
        auth.validate()?;
        Ok(auth)
    }
}

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

/// Relevant fields of the provider's `/.well-known/openid-configuration`.
#[derive(Debug, Clone, Deserialize)]
pub struct OidcDiscovery {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    #[serde(default)]
    pub end_session_endpoint: Option<String>,
}

/// Token endpoint response for the Authorization Code exchange.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub id_token: String,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
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

// ---------------------------------------------------------------------------
// OIDC client
// ---------------------------------------------------------------------------

/// Shared, pooled HTTP client used for every OIDC request, with a bounded
/// timeout so a slow/hung provider cannot pin a request forever.
///
/// Redirects are deliberately **disabled** (F17 hardening over valet): the
/// token endpoint is reached with a `POST` that carries the confidential
/// `client_secret` in its body, and a `307`/`308` redirect re-sends both the
/// method and the body to the redirect target. Following one would leak the
/// client credential to whatever host the (possibly malicious) token endpoint
/// points at. None of the legitimate calls below needs redirects to work.
pub fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(StdDuration::from_secs(HTTP_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::none())
            .pool_max_idle_per_host(4)
            .build()
            .unwrap_or_else(|_| {
                // The primary build only fails on invalid TLS/proxy settings.
                // Retry without the pool tuning but KEEP the timeout and the
                // no-redirect policy: the previous fallback (`Client::new()`)
                // has no timeout at all and follows redirects, which would let a
                // hung provider pin the task and a redirect exfiltrate the
                // client secret.
                tracing::warn!(
                    "failed to build the OIDC HTTP client with pool tuning; \
                     retrying without it but keeping the {}s timeout",
                    HTTP_TIMEOUT_SECS
                );
                reqwest::Client::builder()
                    .timeout(StdDuration::from_secs(HTTP_TIMEOUT_SECS))
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap_or_else(|_| {
                        tracing::warn!(
                            "failed to build the OIDC HTTP client with a timeout; \
                             falling back to an unredirected client"
                        );
                        reqwest::Client::builder()
                            .redirect(reqwest::redirect::Policy::none())
                            .build()
                            .unwrap_or_else(|_| reqwest::Client::new())
                    })
            })
    })
}

/// Process-wide, TTL-bounded cache for discovery and JWKS documents so a
/// callback does not re-fetch them every time.
#[derive(Default)]
struct OidcCache {
    discovery: tokio::sync::RwLock<HashMap<String, (OidcDiscovery, Instant)>>,
    jwks: tokio::sync::RwLock<HashMap<String, (JwkSet, Instant)>>,
}

fn oidc_cache() -> &'static OidcCache {
    static CACHE: OnceLock<OidcCache> = OnceLock::new();
    CACHE.get_or_init(OidcCache::default)
}

/// Read a JSON body while refusing to buffer more than [`MAX_BODY_BYTES`].
async fn read_limited_json<T, F>(response: reqwest::Response, error: F) -> Result<T, AuthError>
where
    T: DeserializeOwned,
    F: Fn() -> AuthError,
{
    if let Some(length) = response.content_length()
        && length > MAX_BODY_BYTES as u64
    {
        return Err(error());
    }

    let mut body: Vec<u8> = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| error())?;
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(error());
        }
        body.extend_from_slice(&chunk);
    }

    serde_json::from_slice(&body).map_err(|_| error())
}

/// Fetch `{issuer}/.well-known/openid-configuration`, cached for [`DISCOVERY_TTL`].
///
/// Every endpoint that the callers later dial is required to share the issuer's
/// origin (RFC 8414): `token_endpoint` and `authorization_endpoint` here, and
/// `jwks_uri` again in [`validate_id_token`] before the fetch. The discovery
/// document is only as trustworthy as the issuer's origin — a compromised
/// document (or a MITM if the issuer were not HTTPS) could otherwise point
/// `token_endpoint` at a foreign host, and [`exchange_code`] would `POST` the
/// confidential `client_secret` to it. This origin allow-listing is a
/// hardening **on top of valet**, which only checks `jwks_uri`. On any
/// mismatch the caller gets the same opaque [`AuthError::DiscoveryFailed`]:
/// which endpoint was rejected is deliberately not revealed.
pub async fn discover(
    client: &reqwest::Client,
    issuer_url: &str,
) -> Result<OidcDiscovery, AuthError> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer_url.trim_end_matches('/')
    );

    if let Some(cached) = {
        let guard = oidc_cache().discovery.read().await;
        guard
            .get(&url)
            .filter(|(_, at)| at.elapsed() < DISCOVERY_TTL)
            .map(|(discovery, _)| discovery.clone())
    } {
        return Ok(cached);
    }

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|_| AuthError::DiscoveryFailed)?;

    if !response.status().is_success() {
        return Err(AuthError::DiscoveryFailed);
    }

    let mut discovery: OidcDiscovery =
        read_limited_json(response, || AuthError::DiscoveryFailed).await?;

    // RFC 8414 hardening (beyond valet): never trust an endpoint advertised off
    // the issuer's origin. The token endpoint is the sensitive one — it is where
    // `exchange_code` sends `client_secret` — but the authorization endpoint is
    // checked too so the whole flow stays anchored to the issuer.
    if !same_origin(issuer_url, &discovery.token_endpoint)
        || !same_origin(issuer_url, &discovery.authorization_endpoint)
    {
        return Err(AuthError::DiscoveryFailed);
    }

    // `end_session_endpoint` is optional. Unlike the endpoints above it must not
    // fail discovery when it points off-origin: the rest of the flow has to keep
    // working and logout can degrade to local. But it must not be trusted
    // either — a compromised document could aim the SSO logout at an attacker,
    // and the SPA (which only checks that the URL is an absolute `https:`) would
    // navigate there carrying the `id_token_hint` in the query. So a foreign
    // endpoint is silently dropped. Nothing sensitive (no URL, no token) is
    // logged.
    if let Some(endpoint) = discovery.end_session_endpoint.as_deref()
        && !same_origin(issuer_url, endpoint)
    {
        tracing::warn!("descartando end_session_endpoint: no comparte origen con el issuer OIDC");
        discovery.end_session_endpoint = None;
    }

    oidc_cache()
        .discovery
        .write()
        .await
        .insert(url, (discovery.clone(), Instant::now()));

    Ok(discovery)
}

/// Fetch and parse the provider's JWKS document, cached for [`JWKS_TTL`].
pub async fn fetch_jwks(client: &reqwest::Client, jwks_uri: &str) -> Result<JwkSet, AuthError> {
    if let Some(cached) = {
        let guard = oidc_cache().jwks.read().await;
        guard
            .get(jwks_uri)
            .filter(|(_, at)| at.elapsed() < JWKS_TTL)
            .map(|(jwks, _)| jwks.clone())
    } {
        return Ok(cached);
    }

    let response = client
        .get(jwks_uri)
        .send()
        .await
        .map_err(|_| AuthError::JwksUnavailable)?;

    if !response.status().is_success() {
        return Err(AuthError::JwksUnavailable);
    }

    let jwks: JwkSet = read_limited_json(response, || AuthError::JwksUnavailable).await?;

    oidc_cache()
        .jwks
        .write()
        .await
        .insert(jwks_uri.to_string(), (jwks.clone(), Instant::now()));

    Ok(jwks)
}

/// Exchange an Authorization Code for tokens at the provider's token endpoint.
pub async fn exchange_code(
    client: &reqwest::Client,
    config: &AuthConfig,
    token_endpoint: &str,
    code: &str,
) -> Result<TokenResponse, AuthError> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", config.redirect_url.as_str()),
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.as_str()),
        ])
        .finish();

    let response = client
        .post(token_endpoint)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body)
        .send()
        .await
        .map_err(|_| AuthError::TokenExchangeFailed)?;

    if !response.status().is_success() {
        return Err(AuthError::TokenExchangeFailed);
    }

    read_limited_json(response, || AuthError::TokenExchangeFailed).await
}

/// Whether two URLs share scheme, host and port (RFC 8414 defense in depth).
///
/// Delegates the comparison to [`url::Url::origin`], which already returns a
/// normalized origin: the **default port is folded away** (`https://host` and
/// `https://host:443` are the same origin, likewise `http`/`:80`) via
/// `port_or_known_default`, and the parsed **host is lower-cased**, so the
/// comparison is host-case-insensitive. Re-implementing that by hand is a
/// known source of bugs, so it is deliberately *not* done here.
///
/// It is fail-closed for schemes without a tuple origin (`data:`, `file:`,
/// …): those yield a fresh opaque origin that never compares equal.
fn same_origin(a: &str, b: &str) -> bool {
    match (url::Url::parse(a), url::Url::parse(b)) {
        (Ok(a), Ok(b)) => a.origin() == b.origin(),
        _ => false,
    }
}

/// Whether `value` is an absolute URL acceptable for a credential-bearing OIDC
/// endpoint: `https` anywhere, or `http` **only** on a loopback host.
///
/// Loopback is the single tolerated plaintext case (traffic stays on the
/// machine, e.g. a local Pocket ID in development). Everything else must be
/// `https`, because the token exchange sends `client_secret` and the callback
/// returns the authorization `code`. Requiring a parsed absolute URL means a
/// relative path, a wildcard or a "javascript:" pseudo-scheme can never pass.
fn is_secure_url(value: &str) -> bool {
    match url::Url::parse(value) {
        Ok(url) => match url.scheme() {
            "https" => true,
            "http" => is_loopback_host(&url),
            _ => false,
        },
        Err(_) => false,
    }
}

/// Whether the URL's host is loopback in any reasonable spelling:
/// `localhost` (case-insensitive, optional trailing dot), any `127.0.0.0/8`
/// address, or IPv6 `::1`.
fn is_loopback_host(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain
            .trim_end_matches('.')
            .eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Claims of interest from a provider-issued ID token.
#[derive(Debug, Deserialize)]
struct IdTokenClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
}

/// Validate an ID token against the provider JWKS and the expected
/// `issuer`/`audience`/`nonce`, returning the user identity on success.
pub async fn validate_id_token(
    client: &reqwest::Client,
    jwks_uri: &str,
    id_token: &str,
    issuer_url: &str,
    client_id: &str,
    nonce: &str,
) -> Result<UserIdentity, AuthError> {
    let header = jsonwebtoken::decode_header(id_token).map_err(|_| {
        AuthError::InvalidToken("la cabecera del token de identidad está malformada".into())
    })?;
    let kid = header.kid.ok_or_else(|| {
        AuthError::InvalidToken("el token de identidad no lleva el campo kid".into())
    })?;

    // Defense in depth (RFC 8414): never trust a JWKS hosted off the issuer's
    // origin, even if discovery advertised it.
    if !same_origin(issuer_url, jwks_uri) {
        return Err(AuthError::JwksUnavailable);
    }

    let jwks = fetch_jwks(client, jwks_uri).await?;
    let jwk = jwks.find(&kid).ok_or_else(|| {
        AuthError::InvalidToken("ninguna clave del JWKS coincide con el kid".into())
    })?;

    let key = DecodingKey::from_jwk(jwk).map_err(|_| AuthError::JwksUnavailable)?;

    let mut validation = Validation::new(Algorithm::RS256);
    // Small grace period to absorb clock skew between us and the provider.
    validation.leeway = ID_TOKEN_LEEWAY_SECS;
    validation.set_issuer(&[issuer_url]);
    validation.set_audience(&[client_id]);

    let data = jsonwebtoken::decode::<IdTokenClaims>(id_token, &key, &validation).map_err(
        |err| match err.kind() {
            jsonwebtoken::errors::ErrorKind::ExpiredSignature => AuthError::ExpiredToken,
            jsonwebtoken::errors::ErrorKind::InvalidIssuer => AuthError::InvalidIssuer,
            jsonwebtoken::errors::ErrorKind::InvalidAudience => AuthError::InvalidAudience,
            _ => AuthError::InvalidToken("la firma o la validación del token han fallado".into()),
        },
    )?;

    if data.claims.nonce.as_deref() != Some(nonce) {
        return Err(AuthError::InvalidNonce);
    }

    Ok(UserIdentity {
        sub: data.claims.sub,
        email: data.claims.email,
        name: data.claims.name,
    })
}

// ---------------------------------------------------------------------------
// Logout
// ---------------------------------------------------------------------------

/// Build the provider `end_session_endpoint` URL including `id_token_hint`.
///
/// Returns `None` when the session has no ID token (local logout only). When
/// `config.post_logout_redirect_url` is non-empty it is added as
/// `post_logout_redirect_uri`.
pub fn build_end_session_url(
    config: &AuthConfig,
    end_session_endpoint: &str,
    id_token: Option<&str>,
) -> Option<String> {
    let id_token = id_token?;
    let mut url = url::Url::parse(end_session_endpoint).ok()?;

    {
        let mut query = url.query_pairs_mut();
        query.append_pair("id_token_hint", id_token);
        if !config.post_logout_redirect_url.is_empty() {
            query.append_pair("post_logout_redirect_uri", &config.post_logout_redirect_url);
        }
    }

    Some(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // RSA key pair used only by the unit tests. It signs the fake ID tokens and
    // its public half is served as a JWKS document to `validate_id_token`.
    const TEST_KID: &str = "test-kid";
    const TEST_JWKS_N: &str = "ybHbf8ju7SODIbYvLInRYmtJadSL4bsjqzO9f-87xoEtPLSPwqZzePmwZywjeN3DjygHPm5Fb7kuZRqYTGfrkWk9uIs4Z6xE_fcXiT1Qq9g9O_oPp-twq0os_--ClSmE7LZC4YESzeKHuFwHq1vRt3ud6vWbyvsNfOHQl_fue2_2gE5EnAzOIZ_zSrWrnl0LAJuT-qKBPC3k-LBvMGeXWSTZ8AgBTqMhxJxITrxgVqGwLOUOp3pyTMYop0B5YWX1m1xpK-r7Ria5t3lMc5-TNOZtb_Rxo6TNjB_Kdwzr7ZlpFcWH_-FWWwfBHKp5_jsFihtazz77sz8tTo_4QVJDHw";
    const TEST_JWKS_E: &str = "AQAB";
    const TEST_PRIVATE_KEY_PEM: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDJsdt/yO7tI4Mh
ti8sidFia0lp1IvhuyOrM71/7zvGgS08tI/CpnN4+bBnLCN43cOPKAc+bkVvuS5l
GphMZ+uRaT24izhnrET99xeJPVCr2D07+g+n63CrSiz/74KVKYTstkLhgRLN4oe4
XAerW9G3e53q9ZvK+w184dCX9+57b/aATkScDM4hn/NKtaueXQsAm5P6ooE8LeT4
sG8wZ5dZJNnwCAFOoyHEnEhOvGBWobAs5Q6nenJMxiinQHlhZfWbXGkr6vtGJrm3
eUxzn5M05m1v9HGjpM2MH8p3DOvtmWkVxYf/4VZbB8Ecqnn+OwWKG1rPPvuzPy1O
j/hBUkMfAgMBAAECggEAFU68VRoZn6TGSCvyfN6MZ7zU0yDYrD60bHQ5W0gfRP/F
kymyHEqwXUHnkGa50p1++OMuLLrCjCSAkb7HAx9hZAm9sv7GNTGeUkQl0OJQ4O6l
vW3T9JXrX7Uk3t7jKTXrLISuuRsFLQn285OJXeGD3MHq83UCAeYWzTAf9MYFBDYm
EkKlTGUuX6O6xZEeuaieuaU9qRkUk35vm8fRwJcgvUeduX8xR+CgOVmSlvz8UY/y
c8PV+1hBHZhPFCDAmxbvaiZNFthTc/s3X/CFwaqlh2um+FQYaTxRiSmEC7KKCTaj
NvqcjVXFHh4+KVvVyRDyCuRglsJPYZdrHwgLyKmRxQKBgQD6yDGHo9dlZEIz0CSO
LJ1VfbmAKvJzAQ8oDhaxAHoAQTgk6z6oiC+j1z7ong+Yrp8U6X9ZaEDjtoQsCAW/
lhVNmHiYKayJUUsQ00zdcxDnWvv4vheQb6BERZBkXUSgsL+Ja5YwVSO7s/H9mAP+
Bw89gvplbLGkVmv9kQg5/65eGwKBgQDN5DKR7bVVoUeMJVLklYwAyJoJqt6jZ+pb
vJsN6OcNRUfLz1dqOZ1mX+wjdGunmMQ1s8FXuq1nGkgZjsNXdrHwUFPAQOIy3WjI
OOgh8C5E1b/YeVMn0G8lJYoC3nh0gvNCkeq7tMk8EszDg7BoJnkALmGXqlzAD/qO
u3jP2VsvTQKBgQC4ZBge7nYCo/wVUrZ+Hwm0AVQyi+Fmc+HsBqfij0IlC883Pgz9
J903b51etlErZ0Gqw6CSYZhMljeKlH99heG2AySwZPvqn249Oc/rh55nLbvVAhgb
aCgD6s4nLXi3Wh4K8aMleIRkkAOe/XX6AsDO7o4jow7ekXinoUrWMkvH/wKBgHOF
HQaZrAuekQlGC8trSQFLjHnuIDro3CqamRjl23WrsIvfCvnz16eQHGsMMDb51OUY
tVtmx1I5AcktO9cAJvhh6YvOG9xUNC1bGIuqOhuvHpP0Br8pCKN7+4J+lnEw5BIu
0th8qAgHuFHeuoTFSVDbtTAmiyHMFVYuQ6vDFZFlAoGBANcfxMTXA4tZg/tQUQNH
sD4oZPVlcmNFXsd9j42vgLnPo+F1xtnm2M33fNEyq9xRSdmu8OfjByimDynIINOk
GGqx5iiXrtdzoFsEVj2ueae5LFmbHJOQI/o1j6cse5nKpQs4MyQJ6sI4/d+uul6u
SestSfOxhvIiYxMrOAHQ3iRF
-----END PRIVATE KEY-----
"#;

    fn now() -> usize {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize
    }

    fn test_config() -> AuthConfig {
        AuthConfig {
            enabled: true,
            issuer_url: "https://issuer.example".into(),
            client_id: "oxidav-client".into(),
            client_secret: "client-secret".into(),
            redirect_url: "http://localhost:3000/api/auth/callback".into(),
            jwt_secret: "unit-test-jwt-secret".into(),
            post_logout_redirect_url: String::new(),
        }
    }

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
    // AuthConfig / Claims baseline
    // -----------------------------------------------------------------

    #[test]
    fn test_auth_config_default() {
        let config = AuthConfig::default();
        assert!(!config.enabled);
        // Fail-closed defaults: every required setting is empty, so with auth
        // enabled an unset variable makes `validate()` abort startup.
        assert_eq!(config.issuer_url, "");
        assert_eq!(config.redirect_url, "");
        assert_eq!(config.post_logout_redirect_url, "");
    }

    #[test]
    fn test_auth_config_validate_is_noop_when_disabled() {
        // Disabled auth tolerates empty settings (dev mode).
        assert!(AuthConfig::default().validate().is_ok());
    }

    #[test]
    fn test_auth_config_validate_fails_closed_when_enabled_and_incomplete() {
        let mut config = test_config();
        config.jwt_secret = String::new();
        assert!(
            config.validate().is_err(),
            "an empty JWT_SECRET with auth enabled must fail validation"
        );

        let mut config = test_config();
        config.client_id = String::new();
        assert!(
            config.validate().is_err(),
            "an empty client id with auth enabled must fail validation"
        );

        let mut config = test_config();
        config.client_secret = String::new();
        assert!(
            config.validate().is_err(),
            "an empty client secret with auth enabled must fail validation"
        );

        let mut config = test_config();
        config.redirect_url = String::new();
        assert!(
            config.validate().is_err(),
            "an empty redirect URL with auth enabled must fail validation"
        );

        let mut config = test_config();
        config.issuer_url = "  ".into();
        assert!(
            config.validate().is_err(),
            "a blank issuer with auth enabled must fail validation"
        );
    }

    #[test]
    fn test_auth_config_validate_ok_with_complete_config() {
        assert!(test_config().validate().is_ok());
    }

    /// The issuer and the redirect may be `https` anywhere, and `http` **only**
    /// on a loopback host (where the traffic never leaves the machine).
    #[test]
    fn test_auth_config_validate_accepts_https_and_loopback_http() {
        // `test_config` already uses an https issuer and an http localhost
        // redirect: it must be accepted.
        assert!(test_config().validate().is_ok());

        for loopback in [
            "http://localhost:9000",
            "http://127.0.0.1",
            "http://127.0.0.2:8080",
            "http://[::1]:8080",
            "https://issuer.example",
        ] {
            let mut config = test_config();
            config.issuer_url = loopback.into();
            assert!(
                config.validate().is_ok(),
                "issuer `{loopback}` must be accepted"
            );
        }

        for loopback in [
            "http://localhost:3000/api/auth/callback",
            "http://127.0.0.1/api/auth/callback",
        ] {
            let mut config = test_config();
            config.redirect_url = loopback.into();
            assert!(
                config.validate().is_ok(),
                "redirect `{loopback}` must be accepted"
            );
        }
    }

    /// A plaintext, non-loopback issuer/redirect must fail validation: the
    /// token `POST` carries `client_secret` and the callback carries the
    /// authorization `code`, both of which would travel in the clear.
    #[test]
    fn test_auth_config_validate_rejects_plaintext_non_loopback() {
        let mut config = test_config();
        config.issuer_url = "http://issuer.example".into();
        assert!(
            config.validate().is_err(),
            "an http non-loopback issuer must fail validation"
        );

        let mut config = test_config();
        config.redirect_url = "http://oxidav.example/api/auth/callback".into();
        assert!(
            config.validate().is_err(),
            "an http non-loopback redirect must fail validation"
        );
    }

    /// Relative, malformed or non-http(s) URLs must be rejected: the check is
    /// "a real absolute URL with an acceptable scheme", never a string
    /// comparison or a wildcard.
    #[test]
    fn test_auth_config_validate_rejects_relative_or_unsupported_schemes() {
        for bad in [
            "not a url",
            "/relative/path",
            "ftp://issuer.example",
            "javascript:alert(1)",
            "//issuer.example",
        ] {
            let mut config = test_config();
            config.issuer_url = bad.into();
            assert!(
                config.validate().is_err(),
                "issuer `{bad}` must fail validation"
            );
        }

        let mut config = test_config();
        config.redirect_url = "javascript:alert(1)".into();
        assert!(
            config.validate().is_err(),
            "a javascript: redirect must fail validation"
        );
    }

    #[test]
    fn test_auth_config_from_config_maps_fields_and_validates() {
        let config = crate::config::Config {
            bind_addr: "127.0.0.1:0".parse().expect("valid socket addr"),
            log_level: "info".into(),
            database_url: "sqlite::memory:".into(),
            auth_enabled: true,
            auth_issuer_url: "https://issuer.example".into(),
            auth_client_id: "oxidav-client".into(),
            auth_client_secret: "client-secret".into(),
            auth_redirect_url: "https://oxidav.example/api/auth/callback".into(),
            auth_post_logout_redirect_url: "https://oxidav.example/exit".into(),
            jwt_secret: "unit-test-jwt-secret".into(),
        };

        let auth = AuthConfig::from_config(&config).expect("complete config must map");
        assert!(auth.enabled);
        assert_eq!(auth.issuer_url, "https://issuer.example");
        assert_eq!(auth.client_id, "oxidav-client");
        assert_eq!(
            auth.redirect_url,
            "https://oxidav.example/api/auth/callback"
        );
        assert_eq!(auth.post_logout_redirect_url, "https://oxidav.example/exit");
        assert_eq!(auth.jwt_secret, "unit-test-jwt-secret");
    }

    #[test]
    fn test_auth_config_from_config_fails_closed_when_enabled_and_incomplete() {
        let config = crate::config::Config {
            bind_addr: "127.0.0.1:0".parse().expect("valid socket addr"),
            log_level: "info".into(),
            database_url: "sqlite::memory:".into(),
            auth_enabled: true,
            auth_issuer_url: "https://issuer.example".into(),
            auth_client_id: "oxidav-client".into(),
            auth_client_secret: String::new(),
            auth_redirect_url: "https://oxidav.example/api/auth/callback".into(),
            auth_post_logout_redirect_url: String::new(),
            jwt_secret: "unit-test-jwt-secret".into(),
        };

        assert!(
            AuthConfig::from_config(&config).is_err(),
            "an empty client secret with auth enabled must fail closed"
        );

        let disabled = crate::config::Config {
            auth_enabled: false,
            ..config
        };
        assert!(
            AuthConfig::from_config(&disabled).is_ok(),
            "a disabled config is valid even with empty settings"
        );
    }

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

    // -----------------------------------------------------------------
    // ID token validation (wiremock)
    // -----------------------------------------------------------------

    fn jwks_json() -> String {
        format!(
            r#"{{"keys":[{{"kty":"RSA","use":"sig","kid":"{TEST_KID}","alg":"RS256","n":"{TEST_JWKS_N}","e":"{TEST_JWKS_E}"}}]}}"#
        )
    }

    fn b64url(bytes: &[u8]) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }

    fn sign_id_token(issuer: &str, audience: &str, nonce: &str, exp: usize) -> String {
        use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};

        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(TEST_KID.into());

        let claims = json!({
            "iss": issuer,
            "aud": audience,
            "sub": "user-123",
            "email": "user@example.com",
            "name": "Test User",
            "nonce": nonce,
            "iat": now(),
            "exp": exp,
        });

        encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY_PEM.as_bytes())
                .expect("test RSA key must parse"),
        )
        .expect("test ID token must encode")
    }

    async fn jwks_server() -> (wiremock::MockServer, String) {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(jwks_json()),
            )
            .mount(&server)
            .await;

        let jwks_uri = format!("{}/jwks", server.uri());
        (server, jwks_uri)
    }

    #[tokio::test]
    async fn test_validate_id_token_accepts_valid_token() {
        let (server, jwks_uri) = jwks_server().await;
        let client = reqwest::Client::new();
        let issuer = server.uri();
        let client_id = "oxidav-client";
        let nonce = "nonce-abc";

        let id_token = sign_id_token(&issuer, client_id, nonce, now() + 300);

        let identity = validate_id_token(&client, &jwks_uri, &id_token, &issuer, client_id, nonce)
            .await
            .expect("a valid ID token must be accepted");

        assert_eq!(identity.sub, "user-123");
        assert_eq!(identity.email.as_deref(), Some("user@example.com"));
        assert_eq!(identity.name.as_deref(), Some("Test User"));
    }

    #[tokio::test]
    async fn test_validate_id_token_rejects_wrong_issuer() {
        let (server, jwks_uri) = jwks_server().await;
        let client = reqwest::Client::new();
        let issuer = server.uri();
        let nonce = "nonce-abc";

        let id_token = sign_id_token("https://evil.example", "oxidav-client", nonce, now() + 300);

        assert!(
            validate_id_token(
                &client,
                &jwks_uri,
                &id_token,
                &issuer,
                "oxidav-client",
                nonce,
            )
            .await
            .is_err(),
            "an ID token with the wrong issuer must be rejected"
        );
    }

    #[tokio::test]
    async fn test_validate_id_token_rejects_wrong_audience() {
        let (server, jwks_uri) = jwks_server().await;
        let client = reqwest::Client::new();
        let issuer = server.uri();
        let nonce = "nonce-abc";

        let id_token = sign_id_token(&issuer, "another-client", nonce, now() + 300);

        assert!(
            validate_id_token(
                &client,
                &jwks_uri,
                &id_token,
                &issuer,
                "oxidav-client",
                nonce,
            )
            .await
            .is_err(),
            "an ID token with the wrong audience must be rejected"
        );
    }

    #[tokio::test]
    async fn test_validate_id_token_rejects_expired_token() {
        let (server, jwks_uri) = jwks_server().await;
        let client = reqwest::Client::new();
        let issuer = server.uri();
        let nonce = "nonce-abc";

        // Beyond the (small) leeway, so it must be rejected.
        let id_token = sign_id_token(&issuer, "oxidav-client", nonce, now() - 60);

        assert!(
            validate_id_token(
                &client,
                &jwks_uri,
                &id_token,
                &issuer,
                "oxidav-client",
                nonce,
            )
            .await
            .is_err(),
            "an expired ID token must be rejected"
        );
    }

    #[tokio::test]
    async fn test_validate_id_token_rejects_wrong_nonce() {
        let (server, jwks_uri) = jwks_server().await;
        let client = reqwest::Client::new();
        let issuer = server.uri();

        let id_token = sign_id_token(&issuer, "oxidav-client", "other-nonce", now() + 300);

        assert!(
            validate_id_token(
                &client,
                &jwks_uri,
                &id_token,
                &issuer,
                "oxidav-client",
                "expected-nonce",
            )
            .await
            .is_err(),
            "an ID token with the wrong nonce must be rejected"
        );
    }

    #[tokio::test]
    async fn test_validate_id_token_rejects_bad_signature() {
        let (server, jwks_uri) = jwks_server().await;
        let client = reqwest::Client::new();
        let issuer = server.uri();
        let nonce = "nonce-abc";

        let valid = sign_id_token(&issuer, "oxidav-client", nonce, now() + 300);
        let mut parts: Vec<String> = valid.split('.').map(str::to_string).collect();
        let sig = parts.last_mut().unwrap();
        let mut chars: Vec<char> = sig.chars().collect();
        let last = chars.pop().unwrap();
        chars.push(if last == 'a' { 'b' } else { 'a' });
        *sig = chars.into_iter().collect();
        let tampered = parts.join(".");

        assert!(
            validate_id_token(
                &client,
                &jwks_uri,
                &tampered,
                &issuer,
                "oxidav-client",
                nonce,
            )
            .await
            .is_err(),
            "an ID token with a bad signature must be rejected"
        );
    }

    /// `alg=none` (an unsigned token) must never be trusted.
    #[tokio::test]
    async fn test_validate_id_token_rejects_alg_none() {
        let (server, jwks_uri) = jwks_server().await;
        let client = reqwest::Client::new();
        let issuer = server.uri();

        let header =
            b64url(format!(r#"{{"alg":"none","typ":"JWT","kid":"{TEST_KID}"}}"#).as_bytes());
        let payload = b64url(
            format!(
                r#"{{"iss":"{issuer}","aud":"oxidav-client","sub":"user-123","nonce":"nonce-abc","exp":{}}}"#,
                now() + 300
            )
            .as_bytes(),
        );
        let token = format!("{header}.{payload}.");

        assert!(
            validate_id_token(
                &client,
                &jwks_uri,
                &token,
                &issuer,
                "oxidav-client",
                "nonce-abc"
            )
            .await
            .is_err(),
            "an unsigned (alg=none) token must be rejected"
        );
    }

    /// Algorithm confusion: an HS256 token signed with the (public) RSA modulus
    /// as secret must be rejected because only RS256 is accepted.
    #[tokio::test]
    async fn test_validate_id_token_rejects_hs256_algorithm_confusion() {
        use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};

        let (server, jwks_uri) = jwks_server().await;
        let client = reqwest::Client::new();
        let issuer = server.uri();

        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some(TEST_KID.into());
        let claims = json!({
            "iss": issuer,
            "aud": "oxidav-client",
            "sub": "user-123",
            "nonce": "nonce-abc",
            "exp": now() + 300,
        });
        let token = encode(
            &header,
            &claims,
            &EncodingKey::from_secret(TEST_JWKS_N.as_bytes()),
        )
        .expect("HS256 token must encode");

        assert!(
            validate_id_token(
                &client,
                &jwks_uri,
                &token,
                &issuer,
                "oxidav-client",
                "nonce-abc"
            )
            .await
            .is_err(),
            "an HS256 algorithm-confusion token must be rejected"
        );
    }

    /// A `jwks_uri` advertised on a different origin than the issuer must be
    /// refused before any fetch (RFC 8414 defense in depth).
    #[tokio::test]
    async fn test_validate_id_token_rejects_jwks_on_another_origin() {
        let (server, _jwks_uri) = jwks_server().await;
        let client = reqwest::Client::new();
        let issuer = server.uri();

        let id_token = sign_id_token(&issuer, "oxidav-client", "nonce-abc", now() + 300);

        assert!(
            validate_id_token(
                &client,
                "https://evil.example/jwks",
                &id_token,
                &issuer,
                "oxidav-client",
                "nonce-abc",
            )
            .await
            .is_err(),
            "a JWKS on a foreign origin must be rejected"
        );
    }

    /// An ID token without a `kid` cannot be matched to a JWKS key and must be
    /// rejected before any network access.
    #[tokio::test]
    async fn test_validate_id_token_rejects_missing_kid() {
        use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};

        let client = reqwest::Client::new();
        let issuer = "https://issuer.example";
        let nonce = "nonce-abc";

        // `Header::new` leaves `kid` unset, so the encoded token carries none.
        let header = Header::new(Algorithm::RS256);
        assert!(header.kid.is_none(), "the fixture must omit the kid");
        let claims = json!({
            "iss": issuer,
            "aud": "oxidav-client",
            "sub": "user-123",
            "nonce": nonce,
            "exp": now() + 300,
        });
        let token = encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY_PEM.as_bytes())
                .expect("test RSA key must parse"),
        )
        .expect("test ID token must encode");

        let err = validate_id_token(
            &client,
            "https://issuer.example/jwks",
            &token,
            issuer,
            "oxidav-client",
            nonce,
        )
        .await
        .expect_err("an ID token without a kid must be rejected");

        // The internal message is Spanish, coherent with the rest of `src/auth/`.
        assert!(
            err.to_string().contains("kid"),
            "the rejection should name the missing kid: {err}"
        );
    }

    /// `same_origin` must fold away the scheme's default port, so `https://h`
    /// and `https://h:443` (resp. `http`/`:80`) are one origin, while a
    /// non-default port stays distinct.
    #[test]
    fn test_same_origin_normalizes_default_ports() {
        assert!(
            same_origin("https://host.example", "https://host.example:443"),
            "an explicit https default port is the same origin"
        );
        assert!(
            same_origin("http://host.example", "http://host.example:80"),
            "an explicit http default port is the same origin"
        );
        assert!(
            !same_origin("https://host.example", "https://host.example:8443"),
            "a non-default port is a different origin"
        );
    }

    /// Host comparison must be case-insensitive (DNS names are not
    /// case-sensitive), so `Host.Example` and `host.example` are one origin.
    #[test]
    fn test_same_origin_is_host_case_insensitive() {
        assert!(
            same_origin("https://Host.Example", "https://host.example"),
            "host case must not affect the origin"
        );
        assert!(
            same_origin("https://host.example/a", "https://HOST.example/b"),
            "host case must not affect the origin, regardless of path"
        );
    }

    /// A discovery document whose `token_endpoint` is advertised off the
    /// issuer's origin must be refused: `exchange_code` would otherwise `POST`
    /// the confidential `client_secret` to a foreign host (RFC 8414 hardening).
    #[tokio::test]
    async fn test_discover_rejects_token_endpoint_on_another_origin() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let document = json!({
            "authorization_endpoint": format!("{}/authorize", server.uri()),
            "token_endpoint": "https://evil.example/token",
            "jwks_uri": format!("{}/jwks", server.uri()),
        })
        .to_string();

        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(document),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        assert!(
            discover(&client, &server.uri()).await.is_err(),
            "a token_endpoint on a foreign origin must be rejected"
        );
    }

    /// Likewise, an `authorization_endpoint` off the issuer's origin must be
    /// refused so the whole flow stays anchored to the issuer.
    #[tokio::test]
    async fn test_discover_rejects_authorization_endpoint_on_another_origin() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let document = json!({
            "authorization_endpoint": "https://evil.example/authorize",
            "token_endpoint": format!("{}/token", server.uri()),
            "jwks_uri": format!("{}/jwks", server.uri()),
        })
        .to_string();

        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(document),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        assert!(
            discover(&client, &server.uri()).await.is_err(),
            "an authorization_endpoint on a foreign origin must be rejected"
        );
    }

    /// Response bodies are size-capped: a well-formed but oversized discovery
    /// document must be rejected. Here the mock sets `content-length`, so this
    /// exercises the header branch of the cap. The endpoints must be on the
    /// mock server's origin or the (new) origin check would reject first.
    #[tokio::test]
    async fn test_discover_rejects_oversized_body() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let padded = json!({
            "authorization_endpoint": format!("{}/authorize", server.uri()),
            "token_endpoint": format!("{}/token", server.uri()),
            "jwks_uri": format!("{}/jwks", server.uri()),
            "padding": "x".repeat(MAX_BODY_BYTES + 1024),
        })
        .to_string();

        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(padded),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        assert!(
            discover(&client, &server.uri()).await.is_err(),
            "an oversized discovery body must be rejected"
        );
    }

    /// Response bodies are also size-capped when the server streams them
    /// **without** `content-length` (`Transfer-Encoding: chunked`). This
    /// exercises the `body.len() + chunk.len() > MAX_BODY_BYTES` branch of
    /// [`read_limited_json`] — the one the `reqwest` `stream` feature exists
    /// for — rather than the `content-length` header branch.
    #[tokio::test]
    async fn test_discover_rejects_oversized_chunked_body() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let padded = json!({
            "authorization_endpoint": format!("{}/authorize", server.uri()),
            "token_endpoint": format!("{}/token", server.uri()),
            "jwks_uri": format!("{}/jwks", server.uri()),
            "padding": "x".repeat(MAX_BODY_BYTES + 1024),
        })
        .to_string();

        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    // Force a chunked response: hyper then emits no
                    // `content-length`, so only the streaming branch can stop it.
                    .insert_header("transfer-encoding", "chunked")
                    .set_body_string(padded),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/.well-known/openid-configuration", server.uri());

        // Prove the fixture really serves the body chunked: a raw request must
        // see no `content-length`. If a future wiremock/hyper started adding
        // one, this test would silently degrade into the header-branch test.
        let probe = client.get(&url).send().await.expect("probe response");
        assert!(
            probe.content_length().is_none(),
            "the chunked fixture must not advertise a content-length"
        );

        assert!(
            discover(&client, &server.uri()).await.is_err(),
            "an oversized chunked discovery body must be rejected"
        );
    }

    // -----------------------------------------------------------------
    // Discovery: end_session_endpoint origin
    // -----------------------------------------------------------------

    /// A discovery document may advertise `end_session_endpoint` on a foreign
    /// origin. The document is still accepted (logout is optional and must not
    /// break the whole flow), but the endpoint is **dropped**: otherwise the
    /// SPA would navigate to the attacker with `id_token_hint` in the query.
    #[tokio::test]
    async fn test_discover_drops_end_session_endpoint_on_another_origin() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let document = json!({
            "authorization_endpoint": format!("{}/authorize", server.uri()),
            "token_endpoint": format!("{}/token", server.uri()),
            "jwks_uri": format!("{}/jwks", server.uri()),
            "end_session_endpoint": "https://attacker.example/logout",
        })
        .to_string();

        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(document),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let discovery = discover(&client, &server.uri())
            .await
            .expect("a foreign end_session_endpoint must not fail discovery");

        assert!(
            discovery.end_session_endpoint.is_none(),
            "an end_session_endpoint off the issuer's origin must be dropped"
        );
    }

    /// A same-origin `end_session_endpoint` is preserved so SSO logout keeps
    /// working.
    #[tokio::test]
    async fn test_discover_keeps_same_origin_end_session_endpoint() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let document = json!({
            "authorization_endpoint": format!("{}/authorize", server.uri()),
            "token_endpoint": format!("{}/token", server.uri()),
            "jwks_uri": format!("{}/jwks", server.uri()),
            "end_session_endpoint": format!("{}/logout", server.uri()),
        })
        .to_string();

        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(document),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let discovery = discover(&client, &server.uri())
            .await
            .expect("a same-origin end_session_endpoint must be accepted");

        assert_eq!(
            discovery.end_session_endpoint.as_deref(),
            Some(format!("{}/logout", server.uri()).as_str()),
            "a same-origin end_session_endpoint must be preserved"
        );
    }

    // -----------------------------------------------------------------
    // Logout URL building
    // -----------------------------------------------------------------

    #[test]
    fn test_build_end_session_url_includes_id_token_hint() {
        let config = test_config();
        let url = build_end_session_url(
            &config,
            "https://issuer.example/end-session",
            Some("provider-id-token"),
        )
        .expect("with an ID token the end-session URL must be built");

        assert!(url.starts_with("https://issuer.example/end-session"));
        assert!(
            url.contains("id_token_hint=provider-id-token"),
            "URL must carry id_token_hint: {url}"
        );
    }

    #[test]
    fn test_build_end_session_url_includes_post_logout_redirect_when_configured() {
        let mut config = test_config();
        config.post_logout_redirect_url = "http://localhost:5173/exit".into();

        let url = build_end_session_url(
            &config,
            "https://issuer.example/end-session",
            Some("provider-id-token"),
        )
        .expect("with an ID token the end-session URL must be built");

        assert!(
            url.contains("post_logout_redirect_uri"),
            "URL must carry post_logout_redirect_uri when configured: {url}"
        );
        assert!(
            url.contains("localhost"),
            "URL must embed the configured value: {url}"
        );
    }

    #[test]
    fn test_build_end_session_url_without_id_token_is_none() {
        let config = test_config();
        assert!(
            build_end_session_url(&config, "https://issuer.example/end-session", None).is_none(),
            "without an ID token there is no SSO end-session URL"
        );
    }
}
