//! F07 — DAV surface integration tests (phase A: the `/dav` OPTIONS probe).
//!
//! Every test mounts the full application router over a throwaway file-backed
//! database under `/tmp/opencode`, mints a real F05 app token and presents it
//! via `Authorization: Basic`, so the F05 `require_basic_auth` middleware is
//! exercised end to end (never mocked).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use oxidav::auth::app_tokens::create_app_token;
use oxidav::auth::basic::WWW_AUTHENTICATE;
use oxidav::config::Config;
use oxidav::db::init_pool;
use oxidav::router::{AppState, build_router};
use sqlx::SqlitePool;
use tower::ServiceExt;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A throwaway file-backed DSN under `/tmp/opencode` (never `data/oxidav.db`).
fn temp_db_url(tag: &str) -> String {
    let dir = PathBuf::from("/tmp/opencode/oxidav-tests");
    std::fs::create_dir_all(&dir).expect("create temp test dir");
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("dav-{tag}-{}-{n}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    format!("sqlite://{}", path.display())
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

    let config = Config {
        bind_addr: "127.0.0.1:0".parse().expect("valid socket addr"),
        log_level: "info".to_string(),
        database_url: "sqlite::memory:".to_string(),
        pocket_id_issuer: "https://id.example.com".to_string(),
        audience: None,
    };
    let app = build_router(AppState::offline_oidc(config, pool));

    TestEnv {
        app,
        basic: basic_header("user-a", &token),
        bogus_basic: basic_header("user-a", "oxidav_deadbeefdeadbeefdeadbeefdeadbeef_secret"),
    }
}

fn basic_header(username: &str, token: &str) -> String {
    let raw = format!("{username}:{token}");
    format!("Basic {}", STANDARD.encode(raw.as_bytes()))
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

/// Sanity check: the DAV subtree stays unreachable for a bearer-only client
/// (it speaks Basic), so `/dav/` never accidentally falls through to the REST
/// API's bearer middleware.
#[tokio::test]
async fn dav_is_not_guarded_by_the_bearer_flow() {
    let env = test_env("dav-bearer").await;

    let response = env
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::OPTIONS)
                .uri("/dav/")
                .header(header::AUTHORIZATION, "Bearer not-a-jwt")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("send request");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let challenge = response
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(challenge.starts_with("Basic"), "{challenge}");
}
