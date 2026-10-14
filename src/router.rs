//! HTTP router construction.
//!
//! [`build_router`] assembles the Axum [`Router`], wiring the shared
//! [`AppState`], the F17 session middleware and the observability layer.

use std::sync::Arc;

use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::get};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tower_http::trace::TraceLayer;

use crate::auth::basic::require_basic_auth;
use crate::auth::middleware::session_middleware;
use crate::auth::session::AuthConfig;
use crate::config::Config;

/// Shared, cheaply-cloneable application state injected into handlers.
///
/// Carries the [`Config`], the optional F17 OIDC [`AuthConfig`] (the session
/// middleware and the `/api/auth/*` routes consume it) and the SQLite
/// [`SqlitePool`] used by the DAV/API repositories.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Immutable runtime configuration.
    pub config: Arc<Config>,
    /// OIDC session configuration. `None` means "no IdP wired" (dev user);
    /// an [`AuthConfig`] with `enabled == false` behaves like `None`.
    pub auth_config: Option<Arc<AuthConfig>>,
    /// Shared SQLite pool (schema, DAV objects and `app_tokens`).
    pub db: SqlitePool,
}

impl AppState {
    /// Create a new application state from its parts.
    ///
    /// `auth_config` is normally a clone of the [`AuthConfig`] validated at
    /// start-up with [`AuthConfig::from_config`](crate::auth::session::AuthConfig::from_config).
    #[must_use]
    pub fn new(config: Config, auth_config: Option<Arc<AuthConfig>>, db: SqlitePool) -> Self {
        Self {
            config: Arc::new(config),
            auth_config,
            db,
        }
    }

    /// Build an offline state with **no** auth configuration.
    ///
    /// No network or database is required: the pool is an in-memory placeholder.
    /// `auth_config` is `None`, so the session middleware is a pass-through and
    /// the API runs as the dev user. This is the constructor used by tests;
    /// production builds the state through [`AppState::new`] with an
    /// [`AuthConfig`] validated fail-closed.
    #[must_use]
    pub fn from_config(config: Config) -> Self {
        Self::new(config, None, offline_pool())
    }
}

/// Build a lazy, non-persistent in-memory pool.
///
/// Used as an offline placeholder when a real database cannot be opened (e.g.
/// in tests that never touch persistence). It connects lazily, so building it
/// can never fail and requires no `unwrap`.
#[must_use]
pub fn offline_pool() -> SqlitePool {
    SqlitePoolOptions::new().connect_lazy_with(SqliteConnectOptions::new().filename(":memory:"))
}

/// Build the application router.
///
/// Mounts `GET /healthz`, the `/api/auth/*` routes, the `/api/v1` REST router,
/// the `/dav` CalDAV surface (behind `require_basic_auth`: session cookie or
/// Basic app token) and the F15
/// embedded SPA. The F17 [`session_middleware`] is applied **globally** and
/// decides per request what to protect: it is a pass-through for everything
/// outside `/api/` (the SPA, `/dav` and `/healthz`) and for `/api/auth/*`, and
/// requires a valid session cookie for the rest of `/api/*` (`/api/v1` included).
///
/// # Route priorities
///
/// The SPA is registered as the router's **fallback**, so it only sees requests
/// that matched nothing else. This keeps the three surfaces cleanly separated:
///
/// * `/healthz`, `/api/auth/*`, `/api/v1/*` and `/dav/*` always win — they are
///   matched before the fallback and can never be answered with HTML.
/// * An unknown `/api/v1/*` path answers a JSON `404` (the `api` fallback is
///   its own sub-router's fallback). With auth enabled this only happens once a
///   session is presented; unauthenticated it is a `401` from the middleware.
/// * Any other `GET`/`HEAD` goes to [`crate::assets::spa_fallback`], which
///   serves the embedded asset or, for a clean client route, `index.html`.
pub fn build_router(state: AppState) -> Router {
    let api = crate::api::router().fallback(api_not_found);

    // The DAV surface is mounted with `nest_service` (not `nest`) so both the
    // canonical `/dav/` and the bare `/dav` are routed into the same sub-router:
    // nesting a `Router` at `/dav` would only register `/dav`, leaving the
    // trailing-slash form the DAV clients and the tests use unmatched.
    let dav = crate::dav::router();
    let dav = require_basic_auth(dav, state.clone());
    let dav = dav.with_state(state.clone());

    Router::new()
        .route("/healthz", get(healthz))
        .merge(crate::auth::routes::routes())
        .nest("/api/v1", api)
        .nest_service("/dav", dav)
        .fallback(crate::assets::spa_fallback)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            session_middleware,
        ))
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
            auth_enabled: false,
            auth_issuer_url: String::new(),
            auth_client_id: String::new(),
            auth_client_secret: String::new(),
            auth_redirect_url: String::new(),
            auth_post_logout_redirect_url: String::new(),
            jwt_secret: String::new(),
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
            state.auth_config.is_none(),
            "the offline constructor wires no auth config"
        );
    }
}
