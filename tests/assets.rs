//! F15 — embedded static assets and SPA fallback integration tests.
//!
//! These tests exercise the real router built by [`build_router`] (not a
//! hand-rolled one), so they also pin down the **route priorities**: the API,
//! DAV and `healthz` surfaces must never fall through to the SPA HTML.

use std::fs;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use oxidav::config::Config;
use oxidav::router::{AppState, build_router};
use tower::ServiceExt;

fn test_state() -> AppState {
    AppState::from_config(Config {
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
    })
}

async fn get(uri: &str) -> Response {
    get_with_headers(uri, &[]).await
}

async fn get_with_headers(uri: &str, headers: &[(&str, &str)]) -> Response {
    let mut builder = Request::builder().uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(Body::empty()).expect("valid request");
    build_router(test_state())
        .oneshot(request)
        .await
        .expect("router responds")
}

fn header_str(response: &Response, name: header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

async fn body_string(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body readable");
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Whether `web/dist` holds a **real** Vite build.
///
/// A clean checkout has no `web/dist` at all: `build.rs` then writes a
/// development placeholder (debug profile only) that does not mount the React
/// app. Tests that need real hashed bundles skip themselves in that case, so
/// `cargo test` stays green on a checkout that never ran `npm run build`.
fn has_real_bundle() -> bool {
    fs::read_to_string("web/dist/index.html")
        .map(|html| html.contains("<div id=\"root\">"))
        .unwrap_or(false)
}

/// Find a content-hashed bundle emitted by Vite under `web/dist/assets`.
///
/// Callers must guard with [`has_real_bundle`]; when a real bundle is present
/// this returns the first entry with the given extension.
fn first_hashed_asset(extension: &str) -> String {
    let dir = fs::read_dir("web/dist/assets").expect("web/dist/assets exists");
    let mut found: Option<String> = None;
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(extension) {
            found = Some(name);
            break;
        }
    }
    found.unwrap_or_else(|| {
        panic!(
            "no {extension} asset under web/dist/assets; callers must guard with has_real_bundle()"
        )
    })
}

/// Send a request with an explicit method and URI through the real router.
async fn request(method: Method, uri: &str) -> Response {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .expect("valid request");
    build_router(test_state())
        .oneshot(request)
        .await
        .expect("router responds")
}

/// Assert a response is a `404` that never answers with HTML.
fn assert_no_html_404(response: &Response, context: &str) {
    assert_eq!(response.status(), StatusCode::NOT_FOUND, "{context}");
    let content_type = header_str(response, header::CONTENT_TYPE).unwrap_or_default();
    assert!(
        !content_type.starts_with("text/html"),
        "{context} must never answer with HTML, got: {content_type}"
    );
}

#[tokio::test]
async fn hashed_asset_is_served_immutable_with_correct_type() {
    if !has_real_bundle() {
        eprintln!("skipping: web/dist holds no real Vite bundle (run `cd web && npm run build`)");
        return;
    }
    let asset = first_hashed_asset(".js");
    let response = get(&format!("/assets/{asset}")).await;

    assert_eq!(response.status(), StatusCode::OK);

    let content_type = header_str(&response, header::CONTENT_TYPE).unwrap_or_default();
    assert!(
        content_type.starts_with("text/javascript"),
        "unexpected content-type: {content_type}"
    );

    let cache = header_str(&response, header::CACHE_CONTROL).unwrap_or_default();
    assert_eq!(cache, "public, max-age=31536000, immutable");

    assert!(
        header_str(&response, header::ETAG).is_some(),
        "hashed assets must carry an ETag"
    );
}

#[tokio::test]
async fn index_html_is_served_no_cache() {
    if !has_real_bundle() {
        eprintln!("skipping: web/dist holds no real Vite bundle (run `cd web && npm run build`)");
        return;
    }
    let response = get("/index.html").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header_str(&response, header::CONTENT_TYPE).as_deref(),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(
        header_str(&response, header::CACHE_CONTROL).as_deref(),
        Some("no-cache")
    );
}

#[tokio::test]
async fn root_serves_index_html() {
    if !has_real_bundle() {
        eprintln!("skipping: web/dist holds no real Vite bundle (run `cd web && npm run build`)");
        return;
    }
    let response = get("/").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header_str(&response, header::CONTENT_TYPE).as_deref(),
        Some("text/html; charset=utf-8")
    );
    let body = body_string(response).await;
    assert!(
        body.contains("<div id=\"root\">"),
        "unexpected body: {body}"
    );
}

#[tokio::test]
async fn svg_asset_is_served() {
    if !has_real_bundle() {
        eprintln!("skipping: web/dist holds no real Vite bundle (run `cd web && npm run build`)");
        return;
    }
    let response = get("/favicon.svg").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header_str(&response, header::CONTENT_TYPE).as_deref(),
        Some("image/svg+xml")
    );
}

#[tokio::test]
async fn matching_if_none_match_returns_304() {
    if !has_real_bundle() {
        eprintln!("skipping: web/dist holds no real Vite bundle (run `cd web && npm run build`)");
        return;
    }
    let asset = first_hashed_asset(".js");
    let uri = format!("/assets/{asset}");

    let first = get(&uri).await;
    let etag = header_str(&first, header::ETAG).expect("asset has an ETag");

    let second = get_with_headers(&uri, &[("if-none-match", etag.as_str())]).await;

    assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(
        header_str(&second, header::ETAG).as_deref(),
        Some(etag.as_str())
    );
}

#[tokio::test]
async fn unknown_client_route_falls_back_to_index_html() {
    if !has_real_bundle() {
        eprintln!("skipping: web/dist holds no real Vite bundle (run `cd web && npm run build`)");
        return;
    }
    let response = get("/calendar").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header_str(&response, header::CONTENT_TYPE).as_deref(),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(
        header_str(&response, header::CACHE_CONTROL).as_deref(),
        Some("no-cache")
    );
    let body = body_string(response).await;
    assert!(
        body.contains("<div id=\"root\">"),
        "unexpected body: {body}"
    );
}

#[tokio::test]
async fn missing_asset_with_extension_returns_404() {
    let response = get("/assets/nope.js").await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let content_type = header_str(&response, header::CONTENT_TYPE).unwrap_or_default();
    assert!(
        !content_type.starts_with("text/html"),
        "a missing asset must not be answered with HTML: {content_type}"
    );
}

#[tokio::test]
async fn unknown_api_path_returns_json_404_not_html() {
    let response = get("/api/v1/unknown").await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let content_type = header_str(&response, header::CONTENT_TYPE).unwrap_or_default();
    assert!(
        content_type.starts_with("application/json"),
        "unknown API path must answer JSON, got: {content_type}"
    );

    let body = body_string(response).await;
    let value: serde_json::Value = serde_json::from_str(&body).expect("valid JSON envelope");
    assert_eq!(value["error"]["code"], "not_found");
}

#[tokio::test]
async fn unknown_dav_path_does_not_return_html() {
    let response = get("/dav/unknown").await;

    let content_type = header_str(&response, header::CONTENT_TYPE).unwrap_or_default();
    assert!(
        !content_type.starts_with("text/html"),
        "the DAV surface must never answer with HTML, got: {content_type}"
    );
    assert_ne!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn healthz_still_returns_ok() {
    let response = get("/healthz").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        header_str(&response, header::CONTENT_TYPE)
            .unwrap_or_default()
            .starts_with("application/json")
    );
}

#[tokio::test]
async fn traversal_paths_are_404_without_html() {
    for uri in [
        "/../etc/passwd",
        "/%2e%2e/secret",
        "/..%2f..%2fetc",
        "/a/../../etc/passwd",
    ] {
        let response = get(uri).await;
        assert_no_html_404(&response, uri);
    }
}

#[tokio::test]
async fn dotted_segments_stay_in_reserved_namespace() {
    for uri in ["/./api/v1/x", "/foo/../api/v1/x", "/.//api/v1/x"] {
        let response = get(uri).await;
        assert_no_html_404(&response, uri);
    }
}

#[tokio::test]
async fn api_outside_v1_does_not_return_html() {
    let response = get("/api/unknown").await;
    assert_no_html_404(&response, "/api/unknown");
}

#[tokio::test]
async fn non_get_head_methods_do_not_return_html() {
    for method in [
        Method::POST,
        Method::PUT,
        Method::DELETE,
        Method::PATCH,
        Method::OPTIONS,
    ] {
        let response = request(method.clone(), "/calendar").await;
        let content_type = header_str(&response, header::CONTENT_TYPE).unwrap_or_default();
        assert!(
            !content_type.starts_with("text/html"),
            "{method} /calendar must never answer with HTML, got: {content_type}"
        );
    }
}

#[tokio::test]
async fn head_existing_asset_returns_200_and_headers() {
    let response = request(Method::HEAD, "/index.html").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header_str(&response, header::CONTENT_TYPE).as_deref(),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(
        header_str(&response, header::CACHE_CONTROL).as_deref(),
        Some("no-cache")
    );
    assert!(
        header_str(&response, header::ETAG).is_some(),
        "an existing asset must carry an ETag"
    );
}
