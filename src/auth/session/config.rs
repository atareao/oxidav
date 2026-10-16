//! OIDC integration configuration for [`crate::auth::session`].
//!
//! Owns [`AuthConfig`] — the fail-closed settings that drive the OIDC
//! Authorization Code flow — and the URL validators it is built on. The
//! fail-closed policy is unchanged: with auth enabled every required `AUTH_*`
//! setting must be present and the issuer/redirect URLs must be absolute
//! `https:` (or `http` on a loopback host), otherwise [`AuthConfig::validate`]
//! rejects the configuration (and the caller aborts startup with it).

use super::AuthError;

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

// ---------------------------------------------------------------------------
// URL validators
// ---------------------------------------------------------------------------

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
pub(super) fn same_origin(a: &str, b: &str) -> bool {
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

/// Build a fully-populated, valid [`AuthConfig`] for tests.
///
/// It lives at module scope (not inside `mod tests`) so both this module's
/// tests and the parent `session` tests — JWT/cookie round-trips and logout
/// URL building — share one fixture instead of duplicating it.
#[cfg(test)]
pub(super) fn test_config() -> AuthConfig {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
