//! Router integration tests.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use oxidav::config::Config;
use oxidav::router::{AppState, build_router};
use tower::ServiceExt;

fn test_state() -> AppState {
    AppState::from_config(Config {
        bind_addr: "127.0.0.1:0".parse().expect("valid socket addr"),
        log_level: "info".to_string(),
        database_url: "sqlite::memory:".to_string(),
        pocket_id_issuer: "https://id.example.com".to_string(),
        audience: None,
    })
}

#[tokio::test]
async fn healthz_returns_200_json_ok() {
    let app = build_router(test_state());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.starts_with("application/json"),
        "unexpected content-type: {content_type}"
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value, serde_json::json!({ "status": "ok" }));
}

#[tokio::test]
async fn unknown_route_returns_404() {
    let app = build_router(test_state());

    // A clean client route (`/desconocida`) now reaches the SPA fallback and
    // answers `index.html`; a path carrying a file extension that is not an
    // embedded asset remains a genuine 404. See `tests/assets.rs` for the full
    // SPA-fallback coverage.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/desconocida.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
