//! OxiDav binary entry point.
//!
//! Loads the configuration, initialises structured logging, builds the router
//! and serves it with a graceful shutdown on `SIGINT`/`SIGTERM`.

use std::error::Error;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use oxidav::auth::oidc::{DEFAULT_CLOCK_SKEW, DEFAULT_JWKS_TTL, JwksCache, OidcConfig};
use oxidav::config::Config;
use oxidav::db::init_pool;
use oxidav::router::{AppState, build_router, offline_pool};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

/// Upper bound on the start-up OIDC discovery request. Keeping it short means a
/// slow or unreachable IdP cannot stall server start-up.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = Config::from_env()?;

    init_tracing(&config.log_level);

    let bind_addr = config.bind_addr;
    let state = build_state(config).await;
    let app = build_router(state);

    let listener = TcpListener::bind(bind_addr).await?;
    tracing::info!(bind_addr = %bind_addr, "OxiDav listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("OxiDav stopped cleanly");
    Ok(())
}

/// Build the shared [`AppState`], opening the database and resolving the OIDC
/// `jwks_uri` from the IdP discovery document.
///
/// Both steps are **best-effort**: a database that cannot be opened degrades to
/// an offline pool (DAV authentication then fails closed) and OIDC discovery
/// failures fall back to [`AppState::offline_oidc`] with a derived `jwks_uri`.
/// Neither blocks nor fails start-up.
async fn build_state(config: Config) -> AppState {
    let db = match init_pool(&config.database_url).await {
        Ok(pool) => pool,
        Err(err) => {
            tracing::error!(
                %err,
                "failed to initialise the database; DAV authentication will fail closed"
            );
            offline_pool()
        }
    };

    let client = match reqwest::Client::builder()
        .timeout(DISCOVERY_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            tracing::warn!(%err, "could not build the OIDC HTTP client; starting offline");
            return AppState::offline_oidc(config, db);
        }
    };

    match OidcConfig::discover(&client, &config.pocket_id_issuer, config.audience.clone()).await {
        Ok(oidc) => {
            tracing::info!(
                issuer = %oidc.issuer,
                jwks_uri = %oidc.jwks_uri,
                audience = oidc.audience.as_deref().unwrap_or("<unset>"),
                "OIDC discovery succeeded"
            );
            let oidc = Arc::new(oidc);
            let jwks = Arc::new(JwksCache::new(
                Arc::clone(&oidc),
                client,
                DEFAULT_JWKS_TTL,
                DEFAULT_CLOCK_SKEW,
            ));
            AppState::new(config, oidc, jwks, db)
        }
        Err(err) => {
            tracing::warn!(
                %err,
                "OIDC discovery failed at start-up; continuing offline \
                 (bearer tokens are rejected until the IdP is reachable)"
            );
            AppState::offline_oidc(config, db)
        }
    }
}

/// Initialise `tracing-subscriber` with the `env-filter` layer, falling back to
/// `info` when the configured filter fails to parse.
fn init_tracing(log_level: &str) {
    let filter = EnvFilter::try_new(log_level).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();
}

/// Await a signal-installation future and resolve only on success.
///
/// If installing the handler fails, the failure is logged and the future
/// **never completes**, so the enclosing `tokio::select!` cannot trigger a
/// spurious shutdown. `signal` names the signal for the log message.
async fn await_or_pend<F>(installation: F, signal: &str)
where
    F: Future<Output = std::io::Result<()>>,
{
    match installation.await {
        Ok(()) => {}
        Err(err) => {
            tracing::error!(
                %err,
                signal,
                "failed to install signal handler; shutdown via this signal is disabled"
            );
            std::future::pending::<()>().await;
        }
    }
}

/// Resolve when the process receives `SIGINT` (Ctrl+C) or `SIGTERM`.
///
/// Individual handler-installation failures are logged and swallowed so the
/// server keeps running rather than shutting down.
async fn shutdown_signal() {
    let ctrl_c = await_or_pend(
        async {
            tokio::signal::ctrl_c().await?;
            Ok(())
        },
        "SIGINT",
    );

    #[cfg(unix)]
    let terminate = await_or_pend(
        async {
            let mut stream =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            stream.recv().await;
            Ok(())
        },
        "SIGTERM",
    );

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    tracing::info!("shutdown signal received, draining connections");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A failed handler installation must NOT resolve the wait future, otherwise
    /// `tokio::select!` would trigger a spurious shutdown.
    #[tokio::test]
    async fn failed_handler_install_never_resolves() {
        let outcome = tokio::time::timeout(
            Duration::from_millis(50),
            await_or_pend(async { Err(std::io::Error::other("boom")) }, "TEST"),
        )
        .await;

        assert!(
            outcome.is_err(),
            "error path must not complete (would trigger shutdown)"
        );
    }

    /// A successfully delivered signal must resolve the wait future.
    #[tokio::test]
    async fn successful_signal_resolves() {
        let outcome = tokio::time::timeout(
            Duration::from_millis(50),
            await_or_pend(async { Ok(()) }, "TEST"),
        )
        .await;

        assert!(outcome.is_ok(), "success path should complete");
    }
}
