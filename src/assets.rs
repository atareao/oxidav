//! F15 — embedded SPA assets and the SPA fallback.
//!
//! The React build in `web/dist` is embedded into the binary at compile time
//! with [`rust_embed`], so the shipped artifact is a single distributable
//! executable that serves the REST API (`/api/v1`), the DAV surface (`/dav`)
//! **and** the SPA.
//!
//! # Build order (mandatory)
//!
//! `rust-embed` reads `web/dist` **at compile time**, so the frontend must be
//! built **before** the backend:
//!
//! ```text
//! cd web && npm ci && npm run build   # produces web/dist
//! cargo build --release               # embeds web/dist into the binary
//! ```
//!
//! [`build.rs`](../../build.rs) enforces that ordering. In **debug** it creates
//! a placeholder (plus a `cargo:warning`) when `web/dist` is missing, so a
//! backend-only checkout still compiles. In **release** a placeholder is never
//! embedded: if there is no real `web/dist/index.html`, the build **aborts**
//! with an actionable message, so a shipped binary can never serve the
//! placeholder page. In debug builds `rust-embed` serves the files straight
//! from disk (live `web/dist`); in release they are embedded.
//!
//! # Caching
//!
//! Vite emits content-hashed bundles under `/assets/`; those are served with
//! `Cache-Control: public, max-age=31536000, immutable`. Everything else —
//! notably `index.html` — is served `Cache-Control: no-cache` so a redeploy is
//! picked up immediately (an aggressively cached `index.html` would keep
//! clients pointing at deleted hashed bundles). Every response carries an
//! `ETag` and honours `If-None-Match` with a `304 Not Modified`.
//!
//! # SPA fallback
//!
//! The SPA uses `BrowserRouter` (clean URLs), so any `GET`/`HEAD` that is not an
//! existing asset and has no file extension serves `index.html` with `200` —
//! e.g. `/calendar`. A path that *does* carry an extension but has no matching
//! asset (e.g. `/assets/missing.js`) is a genuine `404`. Paths under the
//! reserved `/api` and `/dav` namespaces never fall through to HTML.
//!
//! # Environment
//!
//! This module reads no environment variables; it only serves the embedded
//! build. Server configuration (`OXIDAV_BIND_ADDR`, `OXIDAV_DATABASE_URL`,
//! `OXIDAV_POCKET_ID_ISSUER`, `OXIDAV_OIDC_AUDIENCE`, `RUST_LOG`) is handled by
//! [`crate::config`].

use std::borrow::Cow;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use percent_encoding::percent_decode_str;
use rust_embed::RustEmbed;
use sha2::{Digest, Sha256};

/// The embedded frontend build (`web/dist`).
#[derive(RustEmbed)]
#[folder = "web/dist"]
struct SpaAssets;

/// Cache policy for Vite content-hashed bundles under `/assets/`.
const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// Cache policy for everything else (`index.html`, `favicon.svg`, …).
const CACHE_NO_CACHE: &str = "no-cache";

/// Handle for the router's fallback service.
///
/// Serves an existing embedded asset, otherwise applies the SPA fallback (see
/// the module docs). Mounted via [`axum::Router::fallback`] so it only ever sees
/// requests that matched neither `/healthz`, nor `/api/v1/*`, nor `/dav/*`.
pub async fn spa_fallback(uri: Uri, method: Method, headers: HeaderMap) -> Response {
    // Only safe, idempotent verbs may hit the asset layer.
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::NOT_FOUND.into_response();
    }

    // Percent-decode and canonicalise the path *before* classifying it, so
    // `..`/`.` segments and duplicate slashes cannot smuggle a request past the
    // reserved-namespace check (e.g. `/./api/v1/x`). A path that escapes the
    // root is a hard 404 with no HTML.
    let Some(path) = normalize_path(uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    // Defence in depth: even if routing changed, the API/DAV namespaces must
    // never answer with HTML. An unknown path there is a JSON `404`.
    if is_reserved(&path) {
        return crate::api::error::ApiError::NotFound.into_response();
    }

    if let Some(response) = asset_response(&path, &headers) {
        return response;
    }

    // A non-existent asset that carries a file extension is a real 404; a
    // clean client route is handed to the SPA.
    if has_extension(&path) {
        return StatusCode::NOT_FOUND.into_response();
    }

    match asset_response("index.html", &headers) {
        Some(response) => response,
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Build a response for an embedded asset, or `None` when it does not exist.
fn asset_response(path: &str, headers: &HeaderMap) -> Option<Response> {
    let file = SpaAssets::get(path)?;
    Some(serve_file(path, file.data, headers))
}

/// Render an asset, computing its `ETag` and honouring `If-None-Match`.
fn serve_file(path: &str, data: Cow<'static, [u8]>, headers: &HeaderMap) -> Response {
    let etag = compute_etag(&data);

    let mut response_headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(content_type_for(path)) {
        response_headers.insert(header::CONTENT_TYPE, value);
    }
    if let Ok(value) = HeaderValue::from_str(cache_control(path)) {
        response_headers.insert(header::CACHE_CONTROL, value);
    }
    if let Ok(value) = HeaderValue::from_str(&etag) {
        response_headers.insert(header::ETAG, value);
    }

    if etag_matches(headers, &etag) {
        return (StatusCode::NOT_MODIFIED, response_headers).into_response();
    }

    (
        StatusCode::OK,
        response_headers,
        Body::from(data.into_owned()),
    )
        .into_response()
}

/// Stable, strong `ETag` derived from the asset bytes (SHA-256, hex-quoted).
fn compute_etag(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("\"{hex}\"")
}

/// Whether the request's `If-None-Match` matches the resource `ETag`.
fn etag_matches(headers: &HeaderMap, etag: &str) -> bool {
    let Some(raw) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };

    raw.split(',').any(|candidate| {
        let candidate = candidate.trim();
        candidate == "*" || candidate.trim_start_matches("W/") == etag
    })
}

/// `Cache-Control` for an asset path.
///
/// Only the content-hashed bundles Vite emits under `assets/` are immutable;
/// every other file (chiefly `index.html`) must be revalidated.
fn cache_control(path: &str) -> &'static str {
    if path.starts_with("assets/") {
        CACHE_IMMUTABLE
    } else {
        CACHE_NO_CACHE
    }
}

/// Whether the path's final segment looks like a file (has a non-empty stem and
/// extension). Used to tell a client route (`/calendar`) from a missing asset
/// (`/assets/missing.js`).
fn has_extension(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .and_then(|segment| segment.rsplit_once('.'))
        .is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty())
}

/// Canonicalise a raw request path into a relative asset path.
///
/// The raw path is percent-decoded and then folded segment by segment:
/// empty segments (duplicate/leading/trailing slashes) and `.` are dropped,
/// and `..` pops the last real segment. Returns `None` when a `..` would step
/// above the root — the caller answers `404` without ever serving HTML.
///
/// `"/"` and `""` both normalise to the empty string (the SPA root).
fn normalize_path(raw: &str) -> Option<String> {
    let decoded = percent_decode_str(raw).decode_utf8_lossy();
    let mut segments: Vec<&str> = Vec::new();
    for segment in decoded.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                // Escaping the root (`..` with nothing to pop) is rejected.
                segments.pop()?;
            }
            other => segments.push(other),
        }
    }
    Some(segments.join("/"))
}

/// Whether the path belongs to a reserved (non-SPA) namespace.
///
/// Expects an already-[`normalize_path`]d path, so `/./api/...` cannot slip
/// through as an SPA route.
fn is_reserved(path: &str) -> bool {
    matches!(path.split('/').next(), Some("api" | "dav"))
}

/// `Content-Type` inferred from the path's extension.
fn content_type_for(path: &str) -> &'static str {
    let extension = path
        .rsplit('/')
        .next()
        .and_then(|segment| segment.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase());

    match extension.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("wasm") => "application/wasm",
        Some("xml") => "application/xml",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_by_extension() {
        assert_eq!(content_type_for("index.html"), "text/html; charset=utf-8");
        assert_eq!(
            content_type_for("assets/index-abc.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            content_type_for("assets/app.css"),
            "text/css; charset=utf-8"
        );
        assert_eq!(content_type_for("favicon.svg"), "image/svg+xml");
        assert_eq!(content_type_for("logo.PNG"), "image/png");
        assert_eq!(content_type_for("font.woff2"), "font/woff2");
        assert_eq!(content_type_for("unknown"), "application/octet-stream");
    }

    #[test]
    fn cache_policy_by_location() {
        assert_eq!(
            cache_control("assets/index-abc.js"),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(cache_control("index.html"), "no-cache");
        assert_eq!(cache_control("favicon.svg"), "no-cache");
    }

    #[test]
    fn extension_detection() {
        assert!(has_extension("assets/nope.js"));
        assert!(has_extension("a/b.tar.gz"));
        assert!(!has_extension("calendar"));
        assert!(!has_extension("calendar/"));
        assert!(!has_extension(""));
        assert!(!has_extension("dotfile."));
    }

    #[test]
    fn normalizes_dot_and_duplicate_segments() {
        assert_eq!(normalize_path("/"), Some(String::new()));
        assert_eq!(normalize_path(""), Some(String::new()));
        assert_eq!(normalize_path("/calendar"), Some("calendar".to_string()));
        assert_eq!(
            normalize_path("//assets//app.js"),
            Some("assets/app.js".to_string())
        );
        assert_eq!(normalize_path("/./api/v1/x"), Some("api/v1/x".to_string()));
        assert_eq!(
            normalize_path("/foo/../api/v1/x"),
            Some("api/v1/x".to_string())
        );
        // Percent-encoded traversal is decoded before folding.
        assert_eq!(normalize_path("/%2e%2e/secret"), None);
        assert_eq!(normalize_path("/..%2f..%2fetc"), None);
        assert_eq!(normalize_path("/../etc/passwd"), None);
    }

    #[test]
    fn reserved_namespaces() {
        assert!(is_reserved("api/v1/unknown"));
        assert!(is_reserved("api"));
        assert!(is_reserved("dav/calendars/user/"));
        assert!(!is_reserved("calendar"));
        assert!(!is_reserved(""));
    }

    #[test]
    fn etag_is_stable_and_quoted() {
        let a = compute_etag(b"hello");
        let b = compute_etag(b"hello");
        assert_eq!(a, b);
        assert!(a.starts_with('"') && a.ends_with('"'));
        assert_ne!(a, compute_etag(b"world"));
    }

    #[test]
    fn if_none_match_wildcard_and_exact() {
        let mut headers = HeaderMap::new();
        assert!(!etag_matches(&headers, "\"x\""));

        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static("*"));
        assert!(etag_matches(&headers, "\"x\""));

        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("\"y\", \"x\""),
        );
        assert!(etag_matches(&headers, "\"x\""));

        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static("W/\"x\""));
        assert!(etag_matches(&headers, "\"x\""));
    }
}
