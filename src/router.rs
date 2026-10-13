//! HTTP router construction.
//!
//! [`build_router`] assembles the Axum [`Router`], wiring the shared
//! [`AppState`] and the observability middleware. Feature-specific routes (DAV,
//! REST API, auth) are added by later plans (F04–F10).

use std::sync::Arc;

use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::get};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tower_http::trace::TraceLayer;

use crate::auth::basic::require_basic_auth;
use crate::auth::oidc::{DEFAULT_CLOCK_SKEW, DEFAULT_JWKS_TTL, JwksCache, OidcConfig};
use crate::config::Config;

/// Shared, cheaply-cloneable application state injected into handlers.
///
/// Carries the [`Config`], the OIDC/JWKS material needed by the bearer-token
/// extractor (`AuthUser`) and the `require_auth` middleware, and the SQLite
/// [`SqlitePool`] consumed by the DAV app-token repositories and the Basic Auth
/// extractor (`DavAuth`).
#[derive(Debug, Clone)]
pub struct AppState {
    /// Immutable runtime configuration.
    pub config: Arc<Config>,
    /// OIDC verifier configuration (issuer, audience, `jwks_uri`).
    pub oidc: Arc<OidcConfig>,
    /// Cached JWKS public keys, indexed by `kid`.
    pub jwks: Arc<JwksCache>,
    /// Shared SQLite pool (schema, DAV objects and `app_tokens`).
    pub db: SqlitePool,
}

impl AppState {
    /// Create a new application state from its parts.
    ///
    /// `oidc` and `jwks` must describe the same IdP: `jwks` is normally built
    /// with [`JwksCache::new`] from a clone of `oidc`.
    pub fn new(
        config: Config,
        oidc: Arc<OidcConfig>,
        jwks: Arc<JwksCache>,
        db: SqlitePool,
    ) -> Self {
        Self {
            config: Arc::new(config),
            oidc,
            jwks,
            db,
        }
    }

    /// Build a state with a **default, offline** OIDC/JWKS setup.
    ///
    /// No network access is performed: the `jwks_uri` is derived from
    /// `config.pocket_id_issuer` as a best-effort placeholder, and the database
    /// is an in-memory pool with no schema applied. Callers that can reach the
    /// IdP should prefer [`OidcConfig::discover`] and then [`AppState::new`];
    /// this constructor exists so the server and its tests can start without an
    /// IdP (bearer tokens are simply rejected until a real `jwks_uri` is wired).
    pub fn from_config(config: Config) -> Self {
        Self::offline_oidc(config, offline_pool())
    }

    /// Build a state with an offline OIDC/JWKS setup and an explicit pool.
    ///
    /// Like [`AppState::from_config`], but keeps a caller-provided `db` (e.g. a
    /// real pool initialised at start-up) instead of a throwaway in-memory one.
    pub fn offline_oidc(config: Config, db: SqlitePool) -> Self {
        // Normalise the issuer the same way `OidcConfig::discover` does, so a
        // configured trailing slash cannot cause an `iss` mismatch later.
        let issuer = config.pocket_id_issuer.trim_end_matches('/').to_string();
        let oidc = Arc::new(OidcConfig::new(
            issuer.clone(),
            config.audience.clone(),
            default_jwks_uri(&issuer),
        ));
        let jwks = Arc::new(JwksCache::new(
            Arc::clone(&oidc),
            reqwest::Client::new(),
            DEFAULT_JWKS_TTL,
            DEFAULT_CLOCK_SKEW,
        ));
        Self::new(config, oidc, jwks, db)
    }
}

/// Build a lazy, non-persistent in-memory pool.
///
/// Used as an offline placeholder when a real database cannot be opened (e.g.
/// during OIDC-only start-up or in tests that never touch persistence). It
/// connects lazily, so building it can never fail and requires no `unwrap`.
pub fn offline_pool() -> SqlitePool {
    SqlitePoolOptions::new().connect_lazy_with(SqliteConnectOptions::new().filename(":memory:"))
}

/// Best-effort `jwks_uri` derived from an issuer, used only when OIDC
/// discovery is unavailable. Pocket ID may publish its JWKS elsewhere; a wrong
/// value fails closed (tokens are rejected until discovery succeeds).
fn default_jwks_uri(issuer: &str) -> String {
    format!("{}/.well-known/jwks.json", issuer.trim_end_matches('/'))
}

/// Build the application router.
///
/// Mounts `GET /healthz`, the `/api/v1` REST router (behind the F04
/// `require_auth` middleware), the `/dav` CalDAV surface (behind F05
/// `require_basic_auth`) and the F15 embedded SPA, then attaches tracing. The
/// state is threaded through so handlers can be added incrementally.
///
/// # Route priorities
///
/// The SPA is registered as the router's **fallback**, so it only sees requests
/// that matched nothing else. This keeps the three surfaces cleanly separated:
///
/// * `/healthz`, `/api/v1/*` and `/dav/*` always win — they are matched before
///   the fallback and can never be answered with HTML.
/// * An unknown `/api/v1/*` path answers a JSON `404` (the `api` fallback is
///   applied *after* `require_auth`, so it works without a token); an unknown
///   `/dav/*` path stays on the DAV surface.
/// * Any other `GET`/`HEAD` goes to [`crate::assets::spa_fallback`], which
///   serves the embedded asset or, for a clean client route, `index.html`.
pub fn build_router(state: AppState) -> Router {
    let api = crate::api::router();
    let api = crate::auth::oidc::require_auth(api, state.clone());
    // Registered after the auth layer on purpose: axum only wraps the routes
    // that exist when `layer` is called, so the fallback answers a JSON 404 to
    // unauthenticated requests for unknown API paths instead of a 401.
    let api = api.fallback(api_not_found);

    // The DAV surface is mounted with `nest_service` (not `nest`) so both the
    // canonical `/dav/` and the bare `/dav` are routed into the same sub-router:
    // nesting a `Router` at `/dav` would only register `/dav`, leaving the
    // trailing-slash form the DAV clients and the tests use unmatched.
    let dav = crate::dav::router();
    let dav = require_basic_auth(dav, state.clone());
    let dav = dav.with_state(state.clone());

    Router::new()
        .route("/healthz", get(healthz))
        .nest("/api/v1", api)
        .nest_service("/dav", dav)
        .fallback(crate::assets::spa_fallback)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Liveness probe: always returns `200 OK` with `{"status":"ok"}`.
async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

/// Fallback for unknown `/api/v1/*` paths: the uniform JSON `404` envelope.
///
/// Never HTML, never a token challenge — the path simply does not exist.
async fn api_not_found() -> Response {
    crate::api::error::ApiError::NotFound.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            bind_addr: "127.0.0.1:0".parse().expect("valid socket addr"),
            log_level: "info".to_string(),
            database_url: "sqlite::memory:".to_string(),
            pocket_id_issuer: "https://id.example.com".to_string(),
            audience: None,
        }
    }

    #[tokio::test]
    async fn cloned_state_shares_config_arc() {
        let state = AppState::from_config(test_config());
        let cloned = state.clone();

        assert!(
            Arc::ptr_eq(&state.config, &cloned.config),
            "cloning AppState must share the config Arc, not deep-copy it"
        );
        assert!(
            Arc::ptr_eq(&state.oidc, &cloned.oidc),
            "cloning AppState must share the OIDC config Arc"
        );
        assert!(
            Arc::ptr_eq(&state.jwks, &cloned.jwks),
            "cloning AppState must share the JWKS cache Arc"
        );
    }
}
