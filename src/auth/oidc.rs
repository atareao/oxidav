//! F04 — OIDC (Pocket ID) token verification.
//!
//! This module turns a `Authorization: Bearer <jwt>` header into an
//! [`AuthUser`] by verifying the token against the JWKS published by Pocket ID.
//! It never issues tokens nor manages sessions: it only consumes and rejects
//! them, **fail-closed**.
//!
//! Design highlights:
//!
//! * Only **RS256** is accepted. `alg=none` and every symmetric algorithm
//!   (`HS256`/`HS384`/`HS512`), including the "use the RSA public key as an
//!   HMAC secret" confusion trick, are rejected with `401`.
//! * Keys are cached by `kid` with a configurable TTL. An unknown `kid` forces a
//!   single JWKS refresh (key rotation) before giving up.
//! * `iss`, `aud` (when configured), `exp`, `nbf` and `iat` are validated with a
//!   clock-skew leeway.
//! * Every rejection is a `401` carrying `WWW-Authenticate: Bearer`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::header;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
use jsonwebtoken::{
    Algorithm, DecodingKey, TokenData, Validation, decode, decode_header, get_current_timestamp,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};

use crate::api::error::ApiError;
use crate::router::AppState;

/// Default lifetime of a cached JWKS document (15 minutes).
pub const DEFAULT_JWKS_TTL: Duration = Duration::from_secs(15 * 60);
/// Default clock-skew tolerance applied to `exp`, `nbf` and `iat`.
pub const DEFAULT_CLOCK_SKEW: Duration = Duration::from_secs(60);
/// Minimum delay between two JWKS refresh attempts after a failure.
///
/// Prevents a "refresh storm" when the cache TTL lapses while the IdP is down:
/// instead of hitting the IdP on every request, the cache backs off and serves
/// the last known-good keys until the window elapses.
pub const DEFAULT_REFRESH_BACKOFF: Duration = Duration::from_secs(30);

/// Static configuration for OIDC token verification.
///
/// [`jwks_uri`](Self::jwks_uri) is resolved from the IdP discovery document
/// (`{issuer}/.well-known/openid-configuration`) by [`OidcConfig::discover`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcConfig {
    /// Expected `iss` claim; also the discovery base URL.
    pub issuer: String,
    /// Expected `aud` claim (the SPA client id). When `None`, `aud` is not
    /// validated (a warning is logged).
    pub audience: Option<String>,
    /// JWKS endpoint that publishes the RSA public keys.
    pub jwks_uri: String,
}

/// The subset of the discovery document this module consumes.
#[derive(Debug, Deserialize)]
struct DiscoveryDocument {
    issuer: String,
    jwks_uri: String,
}

impl OidcConfig {
    /// Build a configuration from explicit values.
    pub fn new(
        issuer: impl Into<String>,
        audience: Option<String>,
        jwks_uri: impl Into<String>,
    ) -> Self {
        Self {
            issuer: issuer.into(),
            audience,
            jwks_uri: jwks_uri.into(),
        }
    }

    /// Resolve the configuration from the IdP discovery document.
    ///
    /// Fetches `{issuer}/.well-known/openid-configuration` and validates that
    /// the `issuer` it advertises matches the configured one (guards against
    /// issuer mix-up).
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::Discovery`] if the document cannot be fetched,
    /// parsed, or advertises a different issuer.
    pub async fn discover(
        client: &reqwest::Client,
        issuer: &str,
        audience: Option<String>,
    ) -> Result<Self, AuthError> {
        let base = issuer.trim_end_matches('/');
        let url = format!("{base}/.well-known/openid-configuration");

        let document: DiscoveryDocument = client
            .get(&url)
            .send()
            .await
            .map_err(|err| AuthError::Discovery(err.to_string()))?
            .error_for_status()
            .map_err(|err| AuthError::Discovery(err.to_string()))?
            .json()
            .await
            .map_err(|err| AuthError::Discovery(err.to_string()))?;

        if document.issuer.trim_end_matches('/') != base {
            return Err(AuthError::Discovery(format!(
                "issuer mismatch: expected `{base}`, discovery advertised `{}`",
                document.issuer
            )));
        }

        Ok(Self {
            issuer: base.to_string(),
            audience,
            jwks_uri: document.jwks_uri,
        })
    }
}

/// Errors produced while authenticating a request.
///
/// Every variant maps to a `401 Unauthorized` response with a
/// `WWW-Authenticate: Bearer` header (see [`IntoResponse`]).
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The `Authorization` header is missing or is not a well-formed
    /// `Bearer <token>`.
    #[error("missing or malformed Authorization header")]
    MissingToken,
    /// The token is not a structurally valid JWT.
    #[error("malformed token: {0}")]
    MalformedToken(String),
    /// The token header requested an algorithm other than RS256.
    #[error("unsupported signing algorithm; only RS256 is accepted")]
    UnsupportedAlgorithm,
    /// The token header carries no `kid`, so no key can be selected.
    #[error("token is missing the `kid` header")]
    MissingKeyId,
    /// No cached (or freshly fetched) key matches the token `kid`.
    #[error("no matching signing key for the token")]
    UnknownKey,
    /// Signature or claim validation failed.
    #[error("token signature or claims are invalid: {0}")]
    InvalidToken(String),
    /// The OIDC discovery document could not be retrieved or was invalid.
    #[error("OIDC discovery failed: {0}")]
    Discovery(String),
    /// The JWKS document could not be retrieved from the IdP.
    #[error("failed to fetch JWKS: {0}")]
    JwksFetch(String),
    /// The JWKS document was retrieved but contained no usable keys.
    #[error("failed to parse JWKS: {0}")]
    JwksParse(String),
}

impl AuthError {
    /// Whether this error reflects an **infrastructure** problem (the IdP is
    /// unreachable or misbehaving) rather than a bad client token.
    ///
    /// Infrastructure failures are logged at `error!`; client-side rejections
    /// are logged at `debug!`. Either way the response stays a generic `401`
    /// (fail-closed) so internal details never leak to the caller.
    fn is_infrastructure(&self) -> bool {
        matches!(
            self,
            AuthError::Discovery(_) | AuthError::JwksFetch(_) | AuthError::JwksParse(_)
        )
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        // Keep the detail server-side only; the client always gets a generic
        // `unauthorized` body (never issuer/JWKS/URL internals).
        if self.is_infrastructure() {
            tracing::error!(error = %self, "OIDC infrastructure failure while authenticating request");
        } else {
            tracing::debug!(error = %self, "rejected bearer token");
        }

        // Delegate to the uniform F06 error envelope so a `401` from the auth
        // middleware has the exact same body shape as every other API error
        // (`{"error":{"code":"unauthorized","message":"..."}}`).
        // `ApiError::Unauthorized` already attaches `WWW-Authenticate: Bearer`.
        ApiError::Unauthorized.into_response()
    }
}

/// The claims this module cares about.
///
/// `aud` may be a single string or an array; it is kept as raw JSON and the
/// strict comparison is delegated to [`jsonwebtoken`].
#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    /// Subject — the stable user id (`sub` claim of Pocket ID).
    pub sub: String,
    /// Issuer (`iss`).
    pub iss: String,
    /// Audience (`aud`), string or array.
    #[serde(default)]
    pub aud: Option<Value>,
    /// Expiration time (`exp`), Unix seconds.
    pub exp: u64,
    /// Not-before time (`nbf`), Unix seconds.
    #[serde(default)]
    pub nbf: Option<u64>,
    /// Issued-at time (`iat`), Unix seconds.
    #[serde(default)]
    pub iat: Option<u64>,
}

/// In-memory JWKS cache indexed by `kid`.
///
/// Holds the RSA public keys downloaded from the IdP, a TTL and the refresh
/// serialisation lock. When the TTL expires (or an unknown `kid` shows up) the
/// cache is refreshed from [`OidcConfig::jwks_uri`].
#[derive(Debug)]
pub struct JwksCache {
    config: Arc<OidcConfig>,
    client: reqwest::Client,
    ttl: Duration,
    leeway: Duration,
    state: RwLock<CacheState>,
    refresh_lock: Mutex<()>,
}

#[derive(Debug, Default)]
struct CacheState {
    keys: HashMap<String, DecodingKey>,
    fetched_at: Option<Instant>,
    /// Earliest instant at which a refresh may be retried after a failure.
    /// `None` means no pending backoff (a refresh is allowed immediately).
    retry_after: Option<Instant>,
}

impl JwksCache {
    /// Create a cache for `config` using `client` to download the JWKS.
    pub fn new(
        config: Arc<OidcConfig>,
        client: reqwest::Client,
        ttl: Duration,
        leeway: Duration,
    ) -> Self {
        Self {
            config,
            client,
            ttl,
            leeway,
            state: RwLock::new(CacheState::default()),
            refresh_lock: Mutex::new(()),
        }
    }

    /// The configuration backing this cache.
    pub fn config(&self) -> Arc<OidcConfig> {
        Arc::clone(&self.config)
    }

    /// Return the cached key for `kid`, refreshing once on a miss.
    ///
    /// Returns `None` when no key can be produced — either the cache is cold and
    /// the refresh failed (fail-closed) or the `kid` is still unknown. When a
    /// refresh fails while the IdP is down, the last known-good key for `kid`
    /// (if any) is reused and further refreshes are backed off, so a lapsed TTL
    /// does not trigger a refresh storm.
    pub async fn get_key(&self, kid: &str) -> Option<DecodingKey> {
        if let Some(key) = self.lookup_valid(kid).await {
            return Some(key);
        }

        // Miss or expired entry: refresh once (covers key rotation), unless a
        // recent failure asked us to back off.
        if self.refresh_allowed().await && self.refresh().await.is_ok() {
            return self.lookup_valid(kid).await;
        }

        // Refresh skipped or failed: fall back to the last known-good key for
        // this `kid`, if we still hold one (degraded mode while the IdP is down).
        self.lookup_any(kid).await
    }

    /// Look up `kid` in the cache, ignoring entries past their TTL.
    async fn lookup_valid(&self, kid: &str) -> Option<DecodingKey> {
        let state = self.state.read().await;
        let fetched_at = state.fetched_at?;
        if fetched_at.elapsed() > self.ttl {
            return None;
        }
        state.keys.get(kid).cloned()
    }

    /// Look up `kid` in the cache regardless of the TTL (last known-good key).
    async fn lookup_any(&self, kid: &str) -> Option<DecodingKey> {
        self.state.read().await.keys.get(kid).cloned()
    }

    /// Whether a JWKS refresh may be attempted now (no active backoff).
    async fn refresh_allowed(&self) -> bool {
        let retry_after = self.state.read().await.retry_after;
        match retry_after {
            Some(until) => Instant::now() >= until,
            None => true,
        }
    }

    /// Refresh the cached key set from the IdP.
    ///
    /// Concurrent refreshes are serialised to avoid a thundering herd. On
    /// failure the previously cached keys are left untouched and a backoff
    /// window is armed so the next attempt is delayed.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::JwksFetch`] if the download fails and
    /// [`AuthError::JwksParse`] if the document holds no usable RSA keys.
    pub async fn refresh(&self) -> Result<(), AuthError> {
        let _guard = self.refresh_lock.lock().await;

        let outcome = self.fetch_keys().await;
        let mut state = self.state.write().await;

        match outcome {
            Ok(keys) => {
                state.keys = keys;
                state.fetched_at = Some(Instant::now());
                state.retry_after = None;
                Ok(())
            }
            Err(err) => {
                // Arm the backoff so a lapsed TTL cannot cause a refresh storm.
                state.retry_after = Some(Instant::now() + DEFAULT_REFRESH_BACKOFF);
                Err(err)
            }
        }
    }

    /// Download and decode the JWKS document into a `kid`-indexed map.
    async fn fetch_keys(&self) -> Result<HashMap<String, DecodingKey>, AuthError> {
        let jwks = self.fetch_jwks().await?;
        decode_jwks(&jwks)
    }

    /// Download and deserialise the JWKS document.
    async fn fetch_jwks(&self) -> Result<JwkSet, AuthError> {
        let response = self
            .client
            .get(&self.config.jwks_uri)
            .send()
            .await
            .map_err(|err| AuthError::JwksFetch(err.to_string()))?
            .error_for_status()
            .map_err(|err| AuthError::JwksFetch(err.to_string()))?;

        response
            .json::<JwkSet>()
            .await
            .map_err(|err| AuthError::JwksParse(err.to_string()))
    }

    /// Verify a raw JWT and return its [`Claims`].
    ///
    /// Enforces RS256, key selection by `kid`, and the `iss`/`aud`/`exp`/`nbf`
    /// claims (plus a future-`iat` check) with the configured clock skew.
    ///
    /// # Errors
    ///
    /// Returns an [`AuthError`] for any structural, cryptographic or claim
    /// failure.
    pub async fn verify(&self, token: &str) -> Result<Claims, AuthError> {
        let header =
            decode_header(token).map_err(|err| AuthError::MalformedToken(err.to_string()))?;

        if header.alg != Algorithm::RS256 {
            return Err(AuthError::UnsupportedAlgorithm);
        }

        let kid = header.kid.ok_or(AuthError::MissingKeyId)?;
        let key = self.get_key(&kid).await.ok_or(AuthError::UnknownKey)?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.leeway = self.leeway.as_secs();
        validation.validate_nbf = true;
        validation.set_issuer(&[self.config.issuer.as_str()]);
        match self.config.audience.as_deref() {
            Some(audience) => validation.set_audience(&[audience]),
            None => {
                validation.validate_aud = false;
                tracing::warn!(
                    "OIDC `audience` is not configured; skipping `aud` validation (set it to the SPA client id)"
                );
            }
        }

        let data: TokenData<Claims> = decode(token, &key, &validation)
            .map_err(|err| AuthError::InvalidToken(err.to_string()))?;

        // jsonwebtoken does not validate `iat`; reject tokens issued in the
        // future beyond the tolerated skew.
        if let Some(iat) = data.claims.iat {
            let now = get_current_timestamp();
            if iat > now.saturating_add(validation.leeway) {
                return Err(AuthError::InvalidToken(
                    "token `iat` is in the future".to_string(),
                ));
            }
        }

        // An empty `sub` cannot identify a principal; reject it so downstream
        // code never sees a meaningless (and potentially confusing) identity.
        if data.claims.sub.is_empty() {
            return Err(AuthError::InvalidToken("token `sub` is empty".to_string()));
        }

        Ok(data.claims)
    }
}

/// Index a [`JwkSet`] by `kid`, keeping only usable RSA keys.
fn decode_jwks(jwks: &JwkSet) -> Result<HashMap<String, DecodingKey>, AuthError> {
    let mut keys = HashMap::new();

    for jwk in &jwks.keys {
        let Some(kid) = jwk.common.key_id.as_deref() else {
            tracing::debug!("skipping JWKS entry without a `kid`");
            continue;
        };
        if !matches!(jwk.algorithm, AlgorithmParameters::RSA(_)) {
            tracing::warn!(kid, "skipping non-RSA JWKS key");
            continue;
        }
        match DecodingKey::from_jwk(jwk) {
            Ok(key) => {
                keys.insert(kid.to_string(), key);
            }
            Err(err) => tracing::warn!(%err, kid, "skipping unusable JWKS key"),
        }
    }

    if keys.is_empty() {
        return Err(AuthError::JwksParse(
            "JWKS contained no usable RSA keys".to_string(),
        ));
    }

    Ok(keys)
}

/// Extract the token from an `Authorization: Bearer <token>` header.
///
/// The scheme is matched case-insensitively and surrounding whitespace around
/// the token is trimmed. Returns `None` when the header is absent, uses another
/// scheme, or carries an empty token.
pub fn bearer_token(parts: &Parts) -> Option<&str> {
    let value = parts.headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    if !scheme.eq_ignore_ascii_case("Bearer") || token.is_empty() {
        return None;
    }
    Some(token)
}

/// Authenticated principal extracted from a verified bearer token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthUser {
    /// Subject of the token (`sub` claim), i.e. the Pocket ID user id.
    pub sub: String,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer_token(parts).ok_or(AuthError::MissingToken)?;
        let claims = state.jwks.verify(token).await?;
        Ok(Self { sub: claims.sub })
    }
}

/// Wrap a router so every request must carry a valid bearer token.
///
/// The verified [`AuthUser`] is inserted into the request extensions, so
/// downstream handlers can read it via `Extension<AuthUser>`. Requests without
/// a valid token are rejected with `401` + `WWW-Authenticate: Bearer`.
///
/// The middleware is stateful: it needs its own [`AppState`] (to reach the
/// [`JwksCache`]) in addition to the one supplied to the router via
/// `with_state`. Because axum's stateless `middleware::from_fn` pins the
/// extractor state to `()`, the state is threaded explicitly with
/// `from_fn_with_state`; callers pass a clone of the same state they later give
/// the router.
pub fn require_auth(router: Router<AppState>, state: AppState) -> Router<AppState> {
    router.layer(axum::middleware::from_fn_with_state(
        state,
        require_auth_middleware,
    ))
}

/// Middleware backing [`require_auth`].
async fn require_auth_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AuthError> {
    let (mut parts, body) = request.into_parts();
    let user = AuthUser::from_request_parts(&mut parts, &state).await?;
    let mut request = Request::from_parts(parts, body);
    request.extensions_mut().insert(user);
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::time::Duration;

    use axum::Json;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::http::StatusCode;
    use axum::routing::get;
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::traits::PublicKeyParts;
    use rsa::{RsaPrivateKey, RsaPublicKey};
    use serde_json::json;
    use tokio::net::TcpListener;
    use tower::ServiceExt;

    use crate::config::Config;

    const ISSUER: &str = "https://issuer.test";
    const AUDIENCE: &str = "oxidav-spa";

    /// An in-memory RS256 key pair plus its public JWK representation.
    struct TestKey {
        encoding: EncodingKey,
        jwk: Value,
    }

    /// Generate a fresh RSA-2048 key pair and its public JWK.
    fn generate_key(kid: &str) -> TestKey {
        let mut rng = rand::thread_rng();
        let private = RsaPrivateKey::new(&mut rng, 2048).expect("generate RSA key");
        let public = RsaPublicKey::from(&private);

        let n = URL_SAFE_NO_PAD.encode(public.n().to_bytes_be());
        let e = URL_SAFE_NO_PAD.encode(public.e().to_bytes_be());
        let der = private.to_pkcs1_der().expect("encode PKCS#1 DER");

        TestKey {
            encoding: EncodingKey::from_rsa_der(der.as_bytes()),
            jwk: json!({
                "kty": "RSA",
                "kid": kid,
                "alg": "RS256",
                "use": "sig",
                "n": n,
                "e": e,
            }),
        }
    }

    /// Build a JWKS document from the given keys.
    fn jwks(keys: &[&TestKey]) -> Value {
        let entries: Vec<Value> = keys.iter().map(|key| key.jwk.clone()).collect();
        json!({ "keys": entries })
    }

    /// Build a claim set with the given subject, audience and expiry offset.
    fn claims(sub: &str, audience: &Value, exp_offset: i64) -> Value {
        let now = get_current_timestamp() as i64;
        json!({
            "sub": sub,
            "iss": ISSUER,
            "aud": audience,
            "exp": (now + exp_offset) as u64,
            "iat": now as u64,
        })
    }

    /// Sign `claims` with `key`, setting the `kid` header.
    fn sign(key: &TestKey, kid: &str, claims: &Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        encode(&header, claims, &key.encoding).expect("encode JWT")
    }

    /// Hand-roll an `alg=none` token (jsonwebtoken refuses to encode one).
    fn alg_none_token(claims: &Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).expect("serialize claims"));
        format!("{header}.{payload}.")
    }

    /// Spawn a loopback-only JWKS server returning `initial` at `/jwks`.
    ///
    /// Returns the URL, a handle to mutate the served document, and the task.
    async fn spawn_jwks_server(
        initial: Value,
    ) -> (String, Arc<RwLock<Value>>, tokio::task::JoinHandle<()>) {
        let document = Arc::new(RwLock::new(initial));
        let handler = {
            let document = Arc::clone(&document);
            move || {
                let document = Arc::clone(&document);
                async move {
                    let value = document.read().await.clone();
                    Json(value)
                }
            }
        };

        let app = Router::new().route("/jwks", get(handler));
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback listener");
        let addr = listener.local_addr().expect("listener address");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        (format!("http://{addr}/jwks"), document, handle)
    }

    /// Build a cache pointed at `jwks_uri` with a 5-minute TTL.
    fn cache_for(jwks_uri: &str) -> JwksCache {
        cache_with_leeway(jwks_uri, DEFAULT_CLOCK_SKEW)
    }

    fn cache_with_leeway(jwks_uri: &str, leeway: Duration) -> JwksCache {
        cache_with_ttl_and_leeway(jwks_uri, Duration::from_secs(300), leeway)
    }

    fn cache_with_ttl_and_leeway(jwks_uri: &str, ttl: Duration, leeway: Duration) -> JwksCache {
        let config = Arc::new(OidcConfig::new(
            ISSUER,
            Some(AUDIENCE.to_string()),
            jwks_uri.to_string(),
        ));
        JwksCache::new(config, reqwest::Client::new(), ttl, leeway)
    }

    fn test_config() -> Config {
        Config {
            bind_addr: "127.0.0.1:0".parse().expect("valid socket addr"),
            log_level: "info".to_string(),
            database_url: "sqlite::memory:".to_string(),
            pocket_id_issuer: ISSUER.to_string(),
            audience: Some(AUDIENCE.to_string()),
        }
    }

    fn test_state(cache: JwksCache) -> AppState {
        let config = cache.config();
        AppState::new(
            test_config(),
            config,
            Arc::new(cache),
            crate::router::offline_pool(),
        )
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

    // --- Paso 1: Authorization: Bearer parsing -------------------------------

    #[test]
    fn bearer_token_parses_valid_header() {
        let parts = parts_with_authorization(Some("Bearer abc.def.ghi"));
        assert_eq!(bearer_token(&parts), Some("abc.def.ghi"));
    }

    #[test]
    fn bearer_token_is_case_insensitive_and_trims() {
        let parts = parts_with_authorization(Some("bearer   abc.def.ghi  "));
        assert_eq!(bearer_token(&parts), Some("abc.def.ghi"));
    }

    #[test]
    fn bearer_token_rejects_missing_header() {
        let parts = parts_with_authorization(None);
        assert_eq!(bearer_token(&parts), None);
    }

    #[test]
    fn bearer_token_rejects_other_scheme() {
        let parts = parts_with_authorization(Some("Basic dXNlcjpwYXNz"));
        assert_eq!(bearer_token(&parts), None);
    }

    #[test]
    fn bearer_token_rejects_empty_token() {
        let parts = parts_with_authorization(Some("Bearer    "));
        assert_eq!(bearer_token(&parts), None);
    }

    // --- Paso 2/3: signature and algorithm -----------------------------------

    #[tokio::test]
    async fn valid_rs256_token_yields_claims() {
        let key = generate_key("k1");
        let (url, _doc, _server) = spawn_jwks_server(jwks(&[&key])).await;
        let cache = cache_for(&url);

        let token = sign(&key, "k1", &claims("user-1", &json!(AUDIENCE), 300));
        let claims = cache.verify(&token).await.expect("valid token accepted");

        assert_eq!(claims.sub, "user-1");
        assert_eq!(claims.iss, ISSUER);
    }

    #[tokio::test]
    async fn alg_none_is_rejected() {
        let key = generate_key("k1");
        let (url, _doc, _server) = spawn_jwks_server(jwks(&[&key])).await;
        let cache = cache_for(&url);

        let token = alg_none_token(&claims("user-1", &json!(AUDIENCE), 300));
        assert!(cache.verify(&token).await.is_err());
    }

    #[tokio::test]
    async fn hs256_is_rejected() {
        let key = generate_key("k1");
        let (url, _doc, _server) = spawn_jwks_server(jwks(&[&key])).await;
        let cache = cache_for(&url);

        // Symmetric token, even reusing the RSA public key bytes as the HMAC
        // secret, must be rejected (algorithm confusion).
        let secret = URL_SAFE_NO_PAD.encode(key.jwk["n"].as_str().expect("modulus"));
        let encoding = EncodingKey::from_secret(secret.as_bytes());
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("k1".to_string());
        let token = encode(&header, &claims("user-1", &json!(AUDIENCE), 300), &encoding)
            .expect("encode HS256 token");

        assert!(cache.verify(&token).await.is_err());
    }

    // --- Paso 4: claim validation --------------------------------------------

    #[tokio::test]
    async fn wrong_issuer_is_rejected() {
        let key = generate_key("k1");
        let (url, _doc, _server) = spawn_jwks_server(jwks(&[&key])).await;
        let cache = cache_for(&url);

        let mut payload = claims("user-1", &json!(AUDIENCE), 300);
        payload["iss"] = json!("https://evil.test");
        let token = sign(&key, "k1", &payload);

        assert!(cache.verify(&token).await.is_err());
    }

    #[tokio::test]
    async fn wrong_audience_is_rejected() {
        let key = generate_key("k1");
        let (url, _doc, _server) = spawn_jwks_server(jwks(&[&key])).await;
        let cache = cache_for(&url);

        let token = sign(
            &key,
            "k1",
            &claims("user-1", &json!("some-other-client"), 300),
        );
        assert!(cache.verify(&token).await.is_err());
    }

    #[tokio::test]
    async fn audience_array_containing_expected_value_is_accepted() {
        let key = generate_key("k1");
        let (url, _doc, _server) = spawn_jwks_server(jwks(&[&key])).await;
        let cache = cache_for(&url);

        let audience = json!([AUDIENCE, "another"]);
        let token = sign(&key, "k1", &claims("user-1", &audience, 300));
        assert!(cache.verify(&token).await.is_ok());
    }

    #[tokio::test]
    async fn expired_token_is_rejected() {
        let key = generate_key("k1");
        let (url, _doc, _server) = spawn_jwks_server(jwks(&[&key])).await;
        let cache = cache_for(&url);

        let token = sign(&key, "k1", &claims("user-1", &json!(AUDIENCE), -3600));
        assert!(cache.verify(&token).await.is_err());
    }

    #[tokio::test]
    async fn not_yet_valid_token_is_rejected() {
        let key = generate_key("k1");
        let (url, _doc, _server) = spawn_jwks_server(jwks(&[&key])).await;
        let cache = cache_for(&url);

        let mut payload = claims("user-1", &json!(AUDIENCE), 300);
        payload["nbf"] = json!(get_current_timestamp() + 3600);
        let token = sign(&key, "k1", &payload);

        assert!(cache.verify(&token).await.is_err());
    }

    #[tokio::test]
    async fn token_within_clock_skew_is_accepted() {
        let key = generate_key("k1");
        let (url, _doc, _server) = spawn_jwks_server(jwks(&[&key])).await;
        let cache = cache_with_leeway(&url, Duration::from_secs(60));

        // Expired 30s ago, but inside the 60s leeway window.
        let token = sign(&key, "k1", &claims("user-1", &json!(AUDIENCE), -30));
        assert!(cache.verify(&token).await.is_ok());
    }

    #[tokio::test]
    async fn future_iat_is_rejected() {
        let key = generate_key("k1");
        let (url, _doc, _server) = spawn_jwks_server(jwks(&[&key])).await;
        let cache = cache_with_leeway(&url, Duration::from_secs(60));

        let mut payload = claims("user-1", &json!(AUDIENCE), 300);
        payload["iat"] = json!(get_current_timestamp() + 3600);
        let token = sign(&key, "k1", &payload);

        assert!(cache.verify(&token).await.is_err());
    }

    // --- Paso 5: kid selection and rotation ----------------------------------

    #[tokio::test]
    async fn unknown_kid_triggers_refresh_and_succeeds() {
        let key_a = generate_key("a");
        let key_b = generate_key("b");
        let (url, document, _server) = spawn_jwks_server(jwks(&[&key_a])).await;
        let cache = cache_for(&url);

        // Warm the cache with key "a".
        let token_a = sign(&key_a, "a", &claims("user-a", &json!(AUDIENCE), 300));
        cache.verify(&token_a).await.expect("key a accepted");

        // IdP rotates: publish key "b" (cache still holds only "a").
        *document.write().await = jwks(&[&key_a, &key_b]);

        let token_b = sign(&key_b, "b", &claims("user-b", &json!(AUDIENCE), 300));
        let claims = cache.verify(&token_b).await.expect("rotation picked up");
        assert_eq!(claims.sub, "user-b");
    }

    #[tokio::test]
    async fn unknown_kid_still_absent_is_rejected() {
        let key_a = generate_key("a");
        let unknown = generate_key("z");
        let (url, _doc, _server) = spawn_jwks_server(jwks(&[&key_a])).await;
        let cache = cache_for(&url);

        let token = sign(&unknown, "z", &claims("user-z", &json!(AUDIENCE), 300));
        assert!(cache.verify(&token).await.is_err());
    }

    #[tokio::test]
    async fn empty_subject_is_rejected() {
        let key = generate_key("k1");
        let (url, _doc, _server) = spawn_jwks_server(jwks(&[&key])).await;
        let cache = cache_for(&url);

        let token = sign(&key, "k1", &claims("", &json!(AUDIENCE), 300));
        assert!(cache.verify(&token).await.is_err());
    }

    #[tokio::test]
    async fn audience_is_not_validated_when_unconfigured() {
        let key = generate_key("k1");
        let (url, _doc, _server) = spawn_jwks_server(jwks(&[&key])).await;
        // No configured audience: any `aud` value is accepted.
        let config = Arc::new(OidcConfig::new(ISSUER, None, url.clone()));
        let cache = JwksCache::new(
            config,
            reqwest::Client::new(),
            Duration::from_secs(300),
            DEFAULT_CLOCK_SKEW,
        );

        let token = sign(
            &key,
            "k1",
            &claims("user-1", &json!("unrelated-client"), 300),
        );
        assert!(cache.verify(&token).await.is_ok());
    }

    // --- Paso 6b: refresh backoff and stale-key reuse ------------------------

    #[tokio::test]
    async fn stale_key_is_reused_and_failed_refresh_is_backed_off() {
        let key = generate_key("k1");
        let (url, _doc, server) = spawn_jwks_server(jwks(&[&key])).await;
        let cache = cache_with_ttl_and_leeway(&url, Duration::from_millis(50), DEFAULT_CLOCK_SKEW);

        // Warm the cache while the IdP is reachable.
        let token = sign(&key, "k1", &claims("user-1", &json!(AUDIENCE), 300));
        assert!(cache.verify(&token).await.is_ok());

        // Take the IdP down and let the TTL lapse.
        server.abort();
        tokio::time::sleep(Duration::from_millis(150)).await;

        // The last known-good key is reused instead of failing hard...
        assert!(
            cache.get_key("k1").await.is_some(),
            "a stale key must be reused while the IdP is down"
        );
        // ...and the failed refresh armed the backoff window.
        assert!(
            cache.state.read().await.retry_after.is_some(),
            "a failed refresh must arm the backoff window"
        );
    }

    // --- Paso 6: fail-closed -------------------------------------------------

    #[tokio::test]
    async fn fail_closed_when_jwks_unreachable() {
        // Reserve a port and drop the listener so connections are refused.
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback listener");
        let addr = listener.local_addr().expect("listener address");
        drop(listener);

        let cache = cache_for(&format!("http://{addr}/jwks"));
        let key = generate_key("k1");
        let token = sign(&key, "k1", &claims("user-1", &json!(AUDIENCE), 300));

        assert!(cache.verify(&token).await.is_err());
        assert!(cache.get_key("k1").await.is_none());
    }

    #[tokio::test]
    async fn refresh_rejects_jwks_without_rsa_keys() {
        let (url, _doc, _server) = spawn_jwks_server(json!({ "keys": [] })).await;
        let cache = cache_for(&url);

        assert!(cache.refresh().await.is_err());
    }

    // --- Paso 7: extractor and middleware over a router ----------------------

    async fn protected(user: AuthUser) -> Json<Value> {
        Json(json!({ "sub": user.sub }))
    }

    async fn ok_handler() -> StatusCode {
        StatusCode::OK
    }

    #[tokio::test]
    async fn extractor_accepts_valid_token() {
        let key = generate_key("k1");
        let (url, _doc, _server) = spawn_jwks_server(jwks(&[&key])).await;
        let cache = cache_for(&url);
        let app = Router::new()
            .route("/api/v1/protected", get(protected))
            .with_state(test_state(cache));

        let token = sign(&key, "k1", &claims("user-42", &json!(AUDIENCE), 300));
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/v1/protected")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("send request");

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let value: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value, json!({ "sub": "user-42" }));
    }

    #[tokio::test]
    async fn extractor_rejects_missing_token_with_challenge() {
        let key = generate_key("k1");
        let (url, _doc, _server) = spawn_jwks_server(jwks(&[&key])).await;
        let cache = cache_for(&url);
        let app = Router::new()
            .route("/api/v1/protected", get(protected))
            .with_state(test_state(cache));

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/v1/protected")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("send request");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer")
        );
    }

    #[tokio::test]
    async fn require_auth_middleware_protects_router() {
        let key = generate_key("k1");
        let (url, _doc, _server) = spawn_jwks_server(jwks(&[&key])).await;
        let cache = cache_for(&url);
        let state = test_state(cache);

        let app = require_auth(
            Router::new().route("/api/v1/protected", get(ok_handler)),
            state.clone(),
        )
        .with_state(state);

        let token = sign(&key, "k1", &claims("user-7", &json!(AUDIENCE), 300));

        let authorized = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/v1/protected")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("send request");
        assert_eq!(authorized.status(), StatusCode::OK);

        let unauthorized = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/v1/protected")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("send request");
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            unauthorized
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer")
        );
    }

    // --- Paso 8: generic, detail-free error responses ------------------------

    #[tokio::test]
    async fn auth_error_response_is_generic_and_hides_internals() {
        let response = AuthError::JwksFetch(
            "failed to fetch JWKS: boom (http://internal.example/jwks)".to_string(),
        )
        .into_response();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer")
        );

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let value: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["error"]["code"], "unauthorized");
        assert!(value["error"]["message"].is_string());

        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains("internal.example") && !text.contains("JWKS"),
            "internal detail leaked into the response body: {text}"
        );
    }

    #[tokio::test]
    async fn unauthorized_body_uses_uniform_envelope() {
        let response = AuthError::MissingToken.into_response();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer")
        );

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let value: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["error"]["code"], "unauthorized");
        assert_eq!(value["error"]["message"], "Autenticación requerida");
        // The old `{"ok":false,...}` shape is gone.
        assert!(value.get("ok").is_none());
    }
}
