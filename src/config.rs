//! Configuration loaded from environment variables.
//!
//! [`Config::from_env`] reads every setting OxiDav needs at start-up, applying
//! the documented defaults and validating mandatory values. Errors are typed
//! via [`ConfigError`].

use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Environment variable holding the listen address (`SocketAddr`).
pub const ENV_BIND_ADDR: &str = "OXIDAV_BIND_ADDR";
/// Environment variable holding the SQLite connection string (consumed in F03).
pub const ENV_DATABASE_URL: &str = "OXIDAV_DATABASE_URL";
/// Environment variable holding the Pocket ID OIDC issuer (mandatory, F04).
pub const ENV_POCKET_ID_ISSUER: &str = "OXIDAV_POCKET_ID_ISSUER";
/// Environment variable holding the expected OIDC `aud` claim (optional, F04).
///
/// When set, bearer tokens must carry a matching `aud`; when unset (or empty),
/// `aud` is **not** validated (a warning is logged at verification time).
pub const ENV_OIDC_AUDIENCE: &str = "OXIDAV_OIDC_AUDIENCE";
/// Environment variable holding the tracing log level / filter.
pub const ENV_LOG_LEVEL: &str = "RUST_LOG";

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
    /// Pocket ID OIDC issuer URL. Placeholder consumed in F04.
    pub pocket_id_issuer: String,
    /// Expected OIDC `aud` claim (SPA client id). When `None`, `aud` is not
    /// validated; set `OXIDAV_OIDC_AUDIENCE` to enable strict audience checks.
    pub audience: Option<String>,
}

/// Errors produced while loading [`Config`] from the environment.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// A mandatory environment variable was not set.
    #[error("missing required environment variable `{name}`")]
    MissingVar {
        /// Name of the missing variable.
        name: String,
    },
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
    /// [`ConfigError`] when a mandatory variable is missing or a value is
    /// malformed.
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

        let pocket_id_issuer =
            non_empty_env(ENV_POCKET_ID_ISSUER).ok_or_else(|| ConfigError::MissingVar {
                name: ENV_POCKET_ID_ISSUER.to_string(),
            })?;

        // Optional: an empty value means "absent" (audience not validated).
        let audience = non_empty_env(ENV_OIDC_AUDIENCE);

        Ok(Self {
            bind_addr,
            log_level,
            database_url,
            pocket_id_issuer,
            audience,
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
            env::remove_var(ENV_POCKET_ID_ISSUER);
            env::remove_var(ENV_OIDC_AUDIENCE);
            env::remove_var(ENV_LOG_LEVEL);
        }
    }

    #[test]
    fn defaults_when_optional_vars_absent() {
        let _guard = lock();
        clear_oxidav_env();
        // SAFETY: serialised by ENV_LOCK.
        unsafe { env::set_var(ENV_POCKET_ID_ISSUER, "https://id.example.com") };

        let cfg = Config::from_env().expect("config should load with defaults");

        assert_eq!(cfg.bind_addr, DEFAULT_BIND_ADDR);
        assert_eq!(cfg.database_url, DEFAULT_DATABASE_URL);
        assert_eq!(cfg.log_level, DEFAULT_LOG_LEVEL);
        assert_eq!(cfg.pocket_id_issuer, "https://id.example.com");
        assert_eq!(cfg.audience, None);
    }

    #[test]
    fn parses_explicit_env_vars() {
        let _guard = lock();
        clear_oxidav_env();
        // SAFETY: serialised by ENV_LOCK.
        unsafe {
            env::set_var(ENV_BIND_ADDR, "127.0.0.1:9999");
            env::set_var(ENV_DATABASE_URL, "sqlite:///tmp/oxidav-test.db");
            env::set_var(ENV_POCKET_ID_ISSUER, "https://issuer.example/");
            env::set_var(ENV_OIDC_AUDIENCE, "oxidav-spa");
            env::set_var(ENV_LOG_LEVEL, "debug");
        }

        let cfg = Config::from_env().expect("config should parse explicit vars");

        assert_eq!(cfg.bind_addr, "127.0.0.1:9999".parse().unwrap());
        assert_eq!(cfg.database_url, "sqlite:///tmp/oxidav-test.db");
        assert_eq!(cfg.log_level, "debug");
        assert_eq!(cfg.pocket_id_issuer, "https://issuer.example/");
        assert_eq!(cfg.audience.as_deref(), Some("oxidav-spa"));
    }

    #[test]
    fn empty_audience_is_treated_as_absent() {
        let _guard = lock();
        clear_oxidav_env();
        // SAFETY: serialised by ENV_LOCK.
        unsafe {
            env::set_var(ENV_POCKET_ID_ISSUER, "https://id.example.com");
            env::set_var(ENV_OIDC_AUDIENCE, "   ");
        }

        let cfg = Config::from_env().expect("empty audience should be ignored");

        assert_eq!(cfg.audience, None);
    }

    #[test]
    fn empty_bind_addr_falls_back_to_default() {
        let _guard = lock();
        clear_oxidav_env();
        // SAFETY: serialised by ENV_LOCK.
        unsafe {
            env::set_var(ENV_BIND_ADDR, "   ");
            env::set_var(ENV_POCKET_ID_ISSUER, "https://id.example.com");
        }

        let cfg = Config::from_env().expect("empty bind addr should use the default");

        assert_eq!(cfg.bind_addr, DEFAULT_BIND_ADDR);
    }

    #[test]
    fn missing_issuer_yields_missing_var() {
        let _guard = lock();
        clear_oxidav_env(); // issuer intentionally unset

        let err = Config::from_env().expect_err("missing issuer must fail");

        assert_eq!(
            err,
            ConfigError::MissingVar {
                name: ENV_POCKET_ID_ISSUER.to_string()
            }
        );
        assert!(err.to_string().contains(ENV_POCKET_ID_ISSUER));
    }

    #[test]
    fn invalid_bind_addr_yields_invalid_value() {
        let _guard = lock();
        clear_oxidav_env();
        // SAFETY: serialised by ENV_LOCK.
        unsafe {
            env::set_var(ENV_BIND_ADDR, "not-a-socket-addr");
            env::set_var(ENV_POCKET_ID_ISSUER, "https://id.example.com");
        }

        let err = Config::from_env().expect_err("invalid bind addr must fail");

        match err {
            ConfigError::InvalidValue { name, .. } => assert_eq!(name, ENV_BIND_ADDR),
            other => panic!("expected InvalidValue, got {other:?}"),
        }
    }
}
