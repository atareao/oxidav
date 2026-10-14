//! F07/F17 — DAV surface integration tests (the `/dav` OPTIONS probe).
//!
//! Every test mounts the full application router over a throwaway file-backed
//! database under `/tmp/opencode`. The `/dav` subtree is protected by the
//! `require_basic_auth` middleware, which accepts **either** a valid session
//! cookie (when auth is enabled) **or** an `Authorization: Basic` app token
//! (minted here with the real F05 code), so both authentication paths are
//! exercised end to end (never mocked).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use oxidav::auth::app_tokens::create_app_token;
use oxidav::auth::basic::WWW_AUTHENTICATE;
use oxidav::auth::session::{AuthConfig, Claims, SESSION_COOKIE_NAME, emit_session};
use oxidav::config::Config;
use oxidav::db::init_pool;
use oxidav::router::{AppState, build_router};
use sqlx::SqlitePool;
use tower::ServiceExt;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Shared HS256 secret for the test session cookies and the enabling config.
const TEST_JWT_SECRET: &str = "dav-test-jwt-secret";

/// A throwaway file-backed DSN under `/tmp/opencode` (never `data/oxidav.db`).
fn temp_db_url(tag: &str) -> String {
    let dir = PathBuf::from("/tmp/opencode/oxidav-tests");
    std::fs::create_dir_all(&dir).expect("create temp test dir");
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("dav-{tag}-{}-{n}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    format!("sqlite://{}", path.display())
}

/// Base [`Config`] shared by every DAV test: auth disabled, no IdP wired.
fn base_config() -> Config {
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

/// An enabling [`AuthConfig`] that matches [`Config`] with auth on.
fn dav_auth_config() -> Arc<AuthConfig> {
    Arc::new(AuthConfig {
        enabled: true,
        issuer_url: "https://issuer.test".into(),
        client_id: "oxidav-spa".into(),
        client_secret: "client-secret".into(),
        redirect_url: "http://localhost/api/auth/callback".into(),
        jwt_secret: TEST_JWT_SECRET.into(),
        post_logout_redirect_url: String::new(),
    })
}

/// Current UNIX timestamp in seconds.
fn unix_now() -> usize {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as usize)
        .unwrap_or(0)
}

/// Mint a valid `oxidav_session` value for `sub`, expiring `exp_offset` seconds
/// from now (negative for an already-expired cookie).
fn session_cookie_with_exp(auth: &AuthConfig, sub: &str, exp_offset: i64) -> String {
    let now = unix_now() as i64;
    let claims = Claims {
        sub: sub.to_string(),
        email: Some("user@example.com".into()),
        name: Some("Test User".into()),
        exp: (now + exp_offset).max(0) as usize,
        iat: now as usize,
        id_token: None,
    };
    emit_session(auth, &claims).expect("emit session")
}

/// The application router plus the token material needed to authenticate.
struct TestEnv {
    app: axum::Router,
    /// `Authorization: Basic …` header value for a valid app token.
    basic: String,
    /// A Basic header that is well-formed but backed by no stored token.
    bogus_basic: String,
}

async fn test_env(tag: &str) -> TestEnv {
    let pool: SqlitePool = init_pool(&temp_db_url(tag)).await.expect("init_pool");
    let (_id, token) = create_app_token(&pool, "user-a", "dav client")
        .await
        .expect("create app token");

    let app = build_router(AppState::new(base_config(), None, pool));

    TestEnv {
        app,
        basic: basic_header("user-a", &token),
        bogus_basic: basic_header("user-a", "oxidav_deadbeefdeadbeefdeadbeefdeadbeef_secret"),
    }
}

/// A router with auth **enabled** (session cookie + Basic both accepted) plus
/// the material to authenticate through either path.
struct AuthEnv {
    app: axum::Router,
    auth: Arc<AuthConfig>,
    /// `Authorization: Basic …` header for a valid app token.
    basic: String,
    /// A well-formed Basic header backed by no stored token.
    bogus_basic: String,
}

async fn auth_env(tag: &str) -> AuthEnv {
    let pool: SqlitePool = init_pool(&temp_db_url(tag)).await.expect("init_pool");
    let (_id, token) = create_app_token(&pool, "user-a", "dav client")
        .await
        .expect("create app token");

    let config = Config {
        auth_enabled: true,
        auth_issuer_url: "https://issuer.test".into(),
        auth_client_id: "oxidav-spa".into(),
        auth_client_secret: "client-secret".into(),
        auth_redirect_url: "http://localhost/api/auth/callback".into(),
        jwt_secret: TEST_JWT_SECRET.into(),
        ..base_config()
    };
    let auth = dav_auth_config();
    let app = build_router(AppState::new(config, Some(Arc::clone(&auth)), pool));

    AuthEnv {
        app,
        auth,
        basic: basic_header("user-a", &token),
        bogus_basic: basic_header("user-a", "oxidav_deadbeefdeadbeefdeadbeefdeadbeef_secret"),
    }
}

fn basic_header(username: &str, token: &str) -> String {
    let raw = format!("{username}:{token}");
    format!("Basic {}", STANDARD.encode(raw.as_bytes()))
}

/// Issue a DAV request with an arbitrary set of headers.
async fn dav_request(
    app: &axum::Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).expect("build request"))
        .await
        .expect("send request")
}

/// The `WWW-Authenticate` challenge of a response, or `""` when absent.
fn challenge(response: &axum::response::Response) -> String {
    response
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

async fn options(env: &TestEnv, authorization: Option<&str>) -> axum::response::Response {
    let mut builder = Request::builder().method(Method::OPTIONS).uri("/dav/");
    if let Some(value) = authorization {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    env.app
        .clone()
        .oneshot(builder.body(Body::empty()).expect("build request"))
        .await
        .expect("send request")
}

#[tokio::test]
async fn options_dav_without_basic_is_401_with_challenge() {
    let env = test_env("options-unauth").await;

    let response = options(&env, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let challenge = response
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .expect("401 must carry a WWW-Authenticate header")
        .to_str()
        .expect("ASCII challenge");
    assert_eq!(challenge, WWW_AUTHENTICATE);
    assert!(challenge.starts_with("Basic"), "{challenge}");
}

#[tokio::test]
async fn options_dav_with_an_unknown_token_is_401() {
    let env = test_env("options-bogus").await;

    let response = options(&env, Some(&env.bogus_basic)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        response.headers().get(header::WWW_AUTHENTICATE).is_some(),
        "a rejected token must also receive the Basic challenge"
    );
}

#[tokio::test]
async fn options_dav_with_valid_basic_is_200_with_dav_header() {
    let env = test_env("options-auth").await;

    let response = options(&env, Some(&env.basic)).await;
    assert_eq!(response.status(), StatusCode::OK);

    let dav = response
        .headers()
        .get("dav")
        .expect("OPTIONS must advertise the DAV compliance classes")
        .to_str()
        .expect("ASCII DAV header");
    // WebDAV classes 1 and 2 plus the CalDAV and CardDAV compliance classes.
    assert!(dav.contains("1"), "{dav}");
    assert!(dav.contains("2"), "{dav}");
    assert!(dav.contains("calendar-access"), "{dav}");
    assert!(dav.contains("addressbook"), "{dav}");

    let allow = response
        .headers()
        .get(header::ALLOW)
        .expect("OPTIONS must advertise the allowed methods")
        .to_str()
        .expect("ASCII Allow header");
    for method in [
        "OPTIONS",
        "PROPFIND",
        "REPORT",
        "PUT",
        "DELETE",
        "MKCALENDAR",
        "MKCOL",
    ] {
        assert!(allow.contains(method), "Allow lacks {method}: {allow}");
    }
}

/// Sanity check: `/dav` keeps its own authentication and never falls through to
/// the REST API's session middleware. An arbitrary `Bearer` header authenticates
/// nothing, and — with auth disabled — a session-cookie-shaped credential does
/// not sneak a caller in as the dev user either: the answer stays the DAV `401`
/// with the `Basic` challenge.
///
/// The one deliberate exception (a *valid* session cookie admitted by the
/// explicit browser path when auth is enabled) is covered by
/// [`dav_accepts_a_valid_session_cookie`].
#[tokio::test]
async fn dav_keeps_its_basic_challenge_for_api_credentials() {
    let env = test_env("dav-api-creds").await;

    for (label, value) in [
        ("arbitrary bearer", ("authorization", "Bearer not-a-jwt")),
        (
            "session-cookie-shaped credential",
            ("cookie", "oxidav_session=not-a-signed-session"),
        ),
    ] {
        let response = dav_request(&env.app, Method::OPTIONS, "/dav/", &[value]).await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "case: {label}");
        assert!(
            challenge(&response).starts_with("Basic"),
            "case: {label}: {}",
            challenge(&response)
        );
    }
}

// ---------------------------------------------------------------------------
// F17 — the session cookie is accepted alongside Basic on /dav
// ---------------------------------------------------------------------------

/// A valid session cookie authenticates a DAV request when auth is enabled, so a
/// logged-in browser can reach the DAV surface without an app token.
#[tokio::test]
async fn dav_accepts_a_valid_session_cookie() {
    let env = auth_env("dav-session-ok").await;
    let cookie = session_cookie_with_exp(&env.auth, "user-a", 3600);
    let cookie_header = format!("{SESSION_COOKIE_NAME}={cookie}");

    let response = dav_request(
        &env.app,
        Method::OPTIONS,
        "/dav/",
        &[("cookie", &cookie_header)],
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers().get("dav").is_some(),
        "OPTIONS must still advertise the DAV compliance classes"
    );
}

/// The session cookie resolves to the **session subject**: the DAV owner check
/// sees the cookie's `sub`, not the `{username}` named in the URL.
#[tokio::test]
async fn dav_session_cookie_resolves_to_the_session_subject() {
    let env = auth_env("dav-session-owner").await;
    let cookie = session_cookie_with_exp(&env.auth, "user-a", 3600);
    let cookie_header = format!("{SESSION_COOKIE_NAME}={cookie}");
    // `Method::PROPFIND` is not a named constant in `http`; build it from bytes.
    let propfind = Method::from_bytes(b"PROPFIND").expect("PROPFIND is a valid method");

    // The subject's own calendar-home is reachable (207 Multi-Status).
    let own = dav_request(
        &env.app,
        propfind.clone(),
        "/dav/calendars/user-a/",
        &[("cookie", &cookie_header)],
    )
    .await;
    assert_eq!(own.status(), StatusCode::MULTI_STATUS);

    // Another user's calendar-home is forbidden: the cookie authenticated as
    // `user-a`, not as whoever the URL names.
    let foreign = dav_request(
        &env.app,
        propfind,
        "/dav/calendars/user-b/",
        &[("cookie", &cookie_header)],
    )
    .await;
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
}

/// An expired session cookie alone does not authenticate: `/dav` answers the
/// usual `401` with the Basic challenge.
#[tokio::test]
async fn dav_rejects_an_expired_session_cookie_with_the_basic_challenge() {
    let env = auth_env("dav-session-expired").await;
    let cookie = session_cookie_with_exp(&env.auth, "user-a", -10);
    let cookie_header = format!("{SESSION_COOKIE_NAME}={cookie}");

    let response = dav_request(
        &env.app,
        Method::OPTIONS,
        "/dav/",
        &[("cookie", &cookie_header)],
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        challenge(&response).starts_with("Basic"),
        "an expired cookie must produce the same Basic-challenge 401"
    );
}

/// An invalid/expired session cookie **falls through** to Basic instead of
/// short-circuiting the request: a valid app token alongside it authenticates.
#[tokio::test]
async fn dav_expired_session_cookie_falls_back_to_basic() {
    let env = auth_env("dav-session-fallback").await;
    let cookie = session_cookie_with_exp(&env.auth, "user-a", -10);
    let cookie_header = format!("{SESSION_COOKIE_NAME}={cookie}");

    let response = dav_request(
        &env.app,
        Method::OPTIONS,
        "/dav/",
        &[
            ("cookie", &cookie_header),
            ("authorization", env.basic.as_str()),
        ],
    )
    .await;

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a bad cookie must not block the Basic path"
    );
}

/// The Basic + app-token path is unchanged when auth is enabled: native DAV
/// clients keep working exactly as before.
#[tokio::test]
async fn dav_basic_app_token_still_works_with_auth_enabled() {
    let env = auth_env("dav-basic-auth-on").await;

    let valid = dav_request(
        &env.app,
        Method::OPTIONS,
        "/dav/",
        &[("authorization", env.basic.as_str())],
    )
    .await;
    assert_eq!(valid.status(), StatusCode::OK);

    let bogus = dav_request(
        &env.app,
        Method::OPTIONS,
        "/dav/",
        &[("authorization", env.bogus_basic.as_str())],
    )
    .await;
    assert_eq!(bogus.status(), StatusCode::UNAUTHORIZED);
    assert!(challenge(&bogus).starts_with("Basic"));
}

/// With auth disabled the cookie path is inactive: `/dav` still demands Basic
/// and a cookie-shaped credential cannot slip a caller in as the dev user.
#[tokio::test]
async fn dav_requires_basic_when_auth_is_disabled() {
    let env = test_env("dav-auth-off").await;

    let no_basic = dav_request(
        &env.app,
        Method::OPTIONS,
        "/dav/",
        &[("cookie", "oxidav_session=whatever")],
    )
    .await;
    assert_eq!(no_basic.status(), StatusCode::UNAUTHORIZED);
    assert!(challenge(&no_basic).starts_with("Basic"));

    let with_basic = dav_request(
        &env.app,
        Method::OPTIONS,
        "/dav/",
        &[("authorization", env.basic.as_str())],
    )
    .await;
    assert_eq!(with_basic.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// F17 hardening — same-origin evidence for cookie-authenticated DAV mutations
// ---------------------------------------------------------------------------

/// A cookie-authenticated state-changing method without any same-origin
/// evidence is rejected with `403` (the credential is valid, the context is
/// what is refused). This closes the sibling-subdomain CSRF hole that
/// `SameSite=Lax` cannot: `evil.oxidav.example` is *same-site* with
/// `app.oxidav.example`.
#[tokio::test]
async fn dav_cookie_mutation_without_origin_evidence_is_403() {
    let env = auth_env("dav-csrf-no-evidence").await;
    let cookie = session_cookie_with_exp(&env.auth, "user-a", 3600);
    let cookie_header = format!("{SESSION_COOKIE_NAME}={cookie}");

    let response = dav_request(
        &env.app,
        Method::PUT,
        "/dav/calendars/user-a/cal/event.ics",
        &[("cookie", &cookie_header)],
    )
    .await;

    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "a cookie PUT without same-origin evidence must be a 403"
    );
}

/// `Sec-Fetch-Site: same-origin` is valid evidence: the mutation reaches the
/// handler (which answers its own `400`/`404`, never a `401`/`403`).
#[tokio::test]
async fn dav_cookie_mutation_with_sec_fetch_site_same_origin_is_allowed() {
    let env = auth_env("dav-csrf-sec-fetch").await;
    let cookie = session_cookie_with_exp(&env.auth, "user-a", 3600);
    let cookie_header = format!("{SESSION_COOKIE_NAME}={cookie}");

    let response = dav_request(
        &env.app,
        Method::PUT,
        "/dav/calendars/user-a/cal/event.ics",
        &[
            ("cookie", &cookie_header),
            ("sec-fetch-site", "same-origin"),
        ],
    )
    .await;

    assert!(
        !matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ),
        "same-origin evidence must authenticate the mutation, got {}",
        response.status()
    );
}

/// An `Origin` naming a sibling subdomain is same-site but a different origin,
/// so the mutation is refused.
#[tokio::test]
async fn dav_cookie_mutation_with_sibling_origin_is_403() {
    let env = auth_env("dav-csrf-sibling").await;
    let cookie = session_cookie_with_exp(&env.auth, "user-a", 3600);
    let cookie_header = format!("{SESSION_COOKIE_NAME}={cookie}");

    let response = dav_request(
        &env.app,
        Method::PUT,
        "/dav/calendars/user-a/cal/event.ics",
        &[
            ("cookie", &cookie_header),
            ("origin", "https://evil.oxidav.example"),
            ("host", "app.oxidav.example"),
        ],
    )
    .await;

    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "an Origin from a sibling subdomain must be refused"
    );
}

/// An `Origin` equal to the request's own authority is valid evidence.
#[tokio::test]
async fn dav_cookie_mutation_with_own_origin_is_allowed() {
    let env = auth_env("dav-csrf-own-origin").await;
    let cookie = session_cookie_with_exp(&env.auth, "user-a", 3600);
    let cookie_header = format!("{SESSION_COOKIE_NAME}={cookie}");

    let response = dav_request(
        &env.app,
        Method::PUT,
        "/dav/calendars/user-a/cal/event.ics",
        &[
            ("cookie", &cookie_header),
            ("origin", "https://app.oxidav.example"),
            ("host", "app.oxidav.example"),
        ],
    )
    .await;

    assert!(
        !matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ),
        "the request's own Origin must authenticate the mutation, got {}",
        response.status()
    );
}

/// Reads (`PROPFIND`) are never gated, even without evidence headers.
#[tokio::test]
async fn dav_cookie_propfind_without_origin_evidence_is_allowed() {
    let env = auth_env("dav-csrf-propfind").await;
    let cookie = session_cookie_with_exp(&env.auth, "user-a", 3600);
    let cookie_header = format!("{SESSION_COOKIE_NAME}={cookie}");
    let propfind = Method::from_bytes(b"PROPFIND").expect("PROPFIND is a valid method");

    let response = dav_request(
        &env.app,
        propfind,
        "/dav/calendars/user-a/",
        &[("cookie", &cookie_header)],
    )
    .await;

    assert_eq!(
        response.status(),
        StatusCode::MULTI_STATUS,
        "a cookie PROPFIND must not require same-origin evidence"
    );
}

/// The Basic path is untouched: a native DAV client sends no browser security
/// headers and keeps working.
#[tokio::test]
async fn dav_basic_mutation_without_origin_evidence_is_allowed() {
    let env = auth_env("dav-csrf-basic").await;

    let response = dav_request(
        &env.app,
        Method::PUT,
        "/dav/calendars/user-a/cal/event.ics",
        &[("authorization", env.basic.as_str())],
    )
    .await;

    assert!(
        !matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ),
        "the Basic path must stay unaffected by the CSRF check, got {}",
        response.status()
    );
}
