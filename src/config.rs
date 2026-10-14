//! Configuration loaded from environment variables.
//!
//! [`Config::from_env`] reads every setting OxiDav needs at start-up, applying
//! the documented defaults and validating mandatory values. Errors are typed
//! via [`ConfigError`].
//!
//! F17 replaced the "verify `Bearer` against JWKS" settings
//! (`OXIDAV_POCKET_ID_ISSUER`, `OXIDAV_OIDC_AUDIENCE`) with the server-side OIDC
//! Authorization Code settings below, whose names mirror `valet` exactly. None
//! of them is mandatory here: [`crate::auth::session::AuthConfig::from_config`]
//! performs the **fail-closed** validation at start-up and aborts if auth is
//! enabled while any required value is empty.

use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Environment variable holding the listen address (`SocketAddr`).
pub const ENV_BIND_ADDR: &str = "OXIDAV_BIND_ADDR";
/// Environment variable holding the SQLite connection string (consumed in F03).
pub const ENV_DATABASE_URL: &str = "OXIDAV_DATABASE_URL";
/// Environment variable holding the tracing log level / filter.
pub const ENV_LOG_LEVEL: &str = "RUST_LOG";

/// Environment variable enabling OIDC authentication (`true`/`1`; default off).
pub const ENV_AUTH_ENABLED: &str = "AUTH_ENABLED";
/// Environment variable holding the OIDC issuer URL (Pocket ID).
pub const ENV_AUTH_ISSUER_URL: &str = "AUTH_ISSUER_URL";
/// Environment variable holding the confidential OIDC client id.
pub const ENV_AUTH_CLIENT_ID: &str = "AUTH_CLIENT_ID";
/// Environment variable holding the confidential OIDC client secret.
///
/// Kept in the process environment only and never logged; it is the shared
/// secret exchanged server-side for tokens (RFC 6749 §2.1).
pub const ENV_AUTH_CLIENT_SECRET: &str = "AUTH_CLIENT_SECRET";
/// Environment variable holding the OIDC `redirect_uri`
/// (`{base}/api/auth/callback`).
pub const ENV_AUTH_REDIRECT_URL: &str = "AUTH_REDIRECT_URL";
/// Environment variable holding the optional SSO `post_logout_redirect_uri`.
pub const ENV_AUTH_POST_LOGOUT_REDIRECT_URL: &str = "AUTH_POST_LOGOUT_REDIRECT_URL";
/// Environment variable holding the HS256 secret that signs session/flow
/// cookies. Rotating it invalidates every live session.
pub const ENV_JWT_SECRET: &str = "JWT_SECRET";

/// Default listen address: all interfaces, port 8080.
pub const DEFAULT_BIND_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8080);
/// Default SQLite connection string.
pub const DEFAULT_DATABASE_URL: &str = "sqlite://data/oxidav.db";
/// Default log level / filter when `RUST_LOG` is unset.
pub const DEFAULT_LOG_LEVEL: &str = "info";

/// Runtime configuration for the OxiDav server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Address the HTTP server binds to.
    pub bind_addr: SocketAddr,
    /// Tracing filter / level (from `RUST_LOG`).
    pub log_level: String,
    /// SQLite connection string. Placeholder consumed in F03.
    pub database_url: String,
    /// Whether OIDC authentication is enabled (`AUTH_ENABLED`).
    ///
    /// When `false` (the default) the API operates as a synthetic dev user and
    /// the auth routes answer `404`; see [`crate::auth::session::AuthConfig`].
    pub auth_enabled: bool,
    /// OIDC issuer URL (`AUTH_ISSUER_URL`).
    pub auth_issuer_url: String,
    /// Confidential OIDC client id (`AUTH_CLIENT_ID`).
    pub auth_client_id: String,
    /// Confidential OIDC client secret (`AUTH_CLIENT_SECRET`).
    pub auth_client_secret: String,
    /// OIDC `redirect_uri` (`AUTH_REDIRECT_URL`).
    pub auth_redirect_url: String,
    /// Optional SSO `post_logout_redirect_uri`
    /// (`AUTH_POST_LOGOUT_REDIRECT_URL`); empty means "not configured".
    pub auth_post_logout_redirect_url: String,
    /// HS256 secret that signs the session/flow cookies (`JWT_SECRET`).
    pub jwt_secret: String,
}

/// Errors produced while loading [`Config`] from the environment.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// A variable was present but could not be parsed/validated.
    #[error("invalid value for environment variable `{name}`: {reason}")]
    InvalidValue {
        /// Name of the offending variable.
        name: String,
        /// Human-readable explanation of why the value is invalid.
        reason: String,
    },
}

impl Config {
    /// Build the configuration from the process environment.
    ///
    /// Applies the documented defaults for optional variables and returns a
    /// [`ConfigError`] when a value is malformed. No variable is *mandatory*
    /// here: when `AUTH_ENABLED` is truthy,
    /// [`crate::auth::session::AuthConfig::from_config`] validates the auth
    /// settings fail-closed.
    pub fn from_env() -> Result<Self, ConfigError> {
        let bind_addr = match non_empty_env(ENV_BIND_ADDR) {
            Some(value) => {
                value
                    .parse::<SocketAddr>()
                    .map_err(|err| ConfigError::InvalidValue {
                        name: ENV_BIND_ADDR.to_string(),
                        reason: err.to_string(),
                    })?
            }
            None => DEFAULT_BIND_ADDR,
        };

        let log_level =
            non_empty_env(ENV_LOG_LEVEL).unwrap_or_else(|| DEFAULT_LOG_LEVEL.to_string());

        let database_url =
            non_empty_env(ENV_DATABASE_URL).unwrap_or_else(|| DEFAULT_DATABASE_URL.to_string());

        // `AUTH_ENABLED` is truthy only for the documented literals; anything
        // else (including a typo) leaves auth off, which the dev user surfaces.
        let auth_enabled = non_empty_env(ENV_AUTH_ENABLED)
            .is_some_and(|value| matches!(value.trim(), "true" | "1"));

        // Every auth setting defaults to the empty string: the fail-closed
        // check lives in `AuthConfig::validate`, not here.
        let auth_issuer_url = non_empty_env(ENV_AUTH_ISSUER_URL).unwrap_or_default();
        let auth_client_id = non_empty_env(ENV_AUTH_CLIENT_ID).unwrap_or_default();
        let auth_client_secret = non_empty_env(ENV_AUTH_CLIENT_SECRET).unwrap_or_default();
        let auth_redirect_url = non_empty_env(ENV_AUTH_REDIRECT_URL).unwrap_or_default();
        let auth_post_logout_redirect_url =
            non_empty_env(ENV_AUTH_POST_LOGOUT_REDIRECT_URL).unwrap_or_default();
        let jwt_secret = non_empty_env(ENV_JWT_SECRET).unwrap_or_default();

        Ok(Self {
            bind_addr,
            log_level,
            database_url,
            auth_enabled,
            auth_issuer_url,
            auth_client_id,
            auth_client_secret,
            auth_redirect_url,
            auth_post_logout_redirect_url,
            jwt_secret,
        })
    }
}

/// Read an environment variable, treating unset and empty as absent.
fn non_empty_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Environment variables are process-global; serialise the tests that
    /// mutate them so they cannot interleave.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn clear_oxidav_env() {
        // SAFETY: serialised by ENV_LOCK; no other thread reads env concurrently.
        unsafe {
            env::remove_var(ENV_BIND_ADDR);
            env::remove_var(ENV_DATABASE_URL);
            env::remove_var(ENV_LOG_LEVEL);
            env::remove_var(ENV_AUTH_ENABLED);
            env::remove_var(ENV_AUTH_ISSUER_URL);
            env::remove_var(ENV_AUTH_CLIENT_ID);
            env::remove_var(ENV_AUTH_CLIENT_SECRET);
            env::remove_var(ENV_AUTH_REDIRECT_URL);
            env::remove_var(ENV_AUTH_POST_LOGOUT_REDIRECT_URL);
            env::remove_var(ENV_JWT_SECRET);
        }
    }

    #[test]
    fn defaults_when_optional_vars_absent() {
        let _guard = lock();
        clear_oxidav_env();

        let cfg = Config::from_env().expect("config should load with defaults");

        assert_eq!(cfg.bind_addr, DEFAULT_BIND_ADDR);
        assert_eq!(cfg.database_url, DEFAULT_DATABASE_URL);
        assert_eq!(cfg.log_level, DEFAULT_LOG_LEVEL);
        assert!(!cfg.auth_enabled, "auth must be off by default");
        assert_eq!(cfg.auth_issuer_url, "");
        assert_eq!(cfg.auth_client_id, "");
        assert_eq!(cfg.auth_client_secret, "");
        assert_eq!(cfg.auth_redirect_url, "");
        assert_eq!(cfg.auth_post_logout_redirect_url, "");
        assert_eq!(cfg.jwt_secret, "");
    }

    #[test]
    fn parses_explicit_env_vars() {
        let _guard = lock();
        clear_oxidav_env();
        // SAFETY: serialised by ENV_LOCK.
        unsafe {
            env::set_var(ENV_BIND_ADDR, "127.0.0.1:9999");
            env::set_var(ENV_DATABASE_URL, "sqlite:///tmp/oxidav-test.db");
            env::set_var(ENV_LOG_LEVEL, "debug");
            env::set_var(ENV_AUTH_ENABLED, "true");
            env::set_var(ENV_AUTH_ISSUER_URL, "https://issuer.example/");
            env::set_var(ENV_AUTH_CLIENT_ID, "oxidav-client");
            env::set_var(ENV_AUTH_CLIENT_SECRET, "s3cr3t");
            env::set_var(
                ENV_AUTH_REDIRECT_URL,
                "https://oxidav.example/api/auth/callback",
            );
            env::set_var(
                ENV_AUTH_POST_LOGOUT_REDIRECT_URL,
                "https://oxidav.example/exit",
            );
            env::set_var(ENV_JWT_SECRET, "session-signing-key");
        }

        let cfg = Config::from_env().expect("config should parse explicit vars");

        assert_eq!(cfg.bind_addr, "127.0.0.1:9999".parse().unwrap());
        assert_eq!(cfg.database_url, "sqlite:///tmp/oxidav-test.db");
        assert_eq!(cfg.log_level, "debug");
        assert!(cfg.auth_enabled);
        assert_eq!(cfg.auth_issuer_url, "https://issuer.example/");
        assert_eq!(cfg.auth_client_id, "oxidav-client");
        assert_eq!(cfg.auth_client_secret, "s3cr3t");
        assert_eq!(
            cfg.auth_redirect_url,
            "https://oxidav.example/api/auth/callback"
        );
        assert_eq!(
            cfg.auth_post_logout_redirect_url,
            "https://oxidav.example/exit"
        );
        assert_eq!(cfg.jwt_secret, "session-signing-key");
    }

    #[test]
    fn auth_enabled_accepts_one_and_rejects_other_values() {
        let _guard = lock();

        for (raw, expected) in [
            ("1", true),
            ("true", true),
            ("false", false),
            ("yes", false),
        ] {
            clear_oxidav_env();
            // SAFETY: serialised by ENV_LOCK.
            unsafe { env::set_var(ENV_AUTH_ENABLED, raw) };

            let cfg = Config::from_env().expect("config should load");
            assert_eq!(cfg.auth_enabled, expected, "AUTH_ENABLED={raw}");
        }
    }

    #[test]
    fn empty_bind_addr_falls_back_to_default() {
        let _guard = lock();
        clear_oxidav_env();
        // SAFETY: serialised by ENV_LOCK.
        unsafe { env::set_var(ENV_BIND_ADDR, "   ") };

        let cfg = Config::from_env().expect("empty bind addr should use the default");

        assert_eq!(cfg.bind_addr, DEFAULT_BIND_ADDR);
    }

    #[test]
    fn invalid_bind_addr_yields_invalid_value() {
        let _guard = lock();
        clear_oxidav_env();
        // SAFETY: serialised by ENV_LOCK.
        unsafe { env::set_var(ENV_BIND_ADDR, "not-a-socket-addr") };

        let err = Config::from_env().expect_err("invalid bind addr must fail");

        let ConfigError::InvalidValue { name, .. } = err;
        assert_eq!(name, ENV_BIND_ADDR);
    }
}
