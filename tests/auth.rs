//! F17 — integration tests for the OIDC auth routes and the session middleware.
//!
//! These exercise the real router built by [`build_router`]. The session cookies
//! are minted directly with `emit_session`, and the `login` test points the
//! issuer at a `wiremock` server, so nothing here needs a real IdP.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use oxidav::auth::session::{
    AuthConfig, Claims, SESSION_COOKIE_NAME, clear_caches, emit_flow_token, emit_session,
};
use oxidav::config::Config;
use oxidav::router::{AppState, build_router, offline_pool};
use serde_json::json;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const JWT_SECRET: &str = "auth-test-jwt-secret";
const FLOW_COOKIE: &str = "oxidav_oauth_flow";

fn config_with_auth(enabled: bool, issuer_url: &str) -> Config {
    Config {
        bind_addr: "127.0.0.1:0".parse().expect("valid socket addr"),
        log_level: "info".to_string(),
        database_url: "sqlite::memory:".to_string(),
        auth_enabled: enabled,
        auth_issuer_url: issuer_url.to_string(),
        auth_client_id: "oxidav-spa".into(),
        auth_client_secret: "client-secret".into(),
        auth_redirect_url: "https://oxidav.example/api/auth/callback".into(),
        auth_post_logout_redirect_url: String::new(),
        jwt_secret: JWT_SECRET.into(),
    }
}

fn auth_config(issuer_url: &str) -> Arc<AuthConfig> {
    Arc::new(AuthConfig {
        enabled: true,
        issuer_url: issuer_url.to_string(),
        client_id: "oxidav-spa".into(),
        client_secret: "client-secret".into(),
        redirect_url: "https://oxidav.example/api/auth/callback".into(),
        jwt_secret: JWT_SECRET.into(),
        post_logout_redirect_url: String::new(),
    })
}

/// Router with auth **disabled** (dev user).
fn disabled_app() -> axum::Router {
    build_router(AppState::new(
        config_with_auth(false, "https://issuer.test"),
        None,
        offline_pool(),
    ))
}

/// Router with auth **enabled**, pointed at `issuer_url`.
fn enabled_app(issuer_url: &str) -> axum::Router {
    build_router(AppState::new(
        config_with_auth(true, issuer_url),
        Some(auth_config(issuer_url)),
        offline_pool(),
    ))
}

async fn call(
    app: axum::Router,
    method: Method,
    uri: &str,
    cookie: Option<(&str, &str)>,
) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some((name, value)) = cookie {
        builder = builder.header(header::COOKIE, format!("{name}={value}"));
    }
    app.oneshot(builder.body(Body::empty()).expect("build request"))
        .await
        .expect("router responds")
}

async fn body_string(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    String::from_utf8_lossy(&bytes).into_owned()
}

fn set_cookies(response: &Response) -> Vec<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::to_string)
        .collect()
}

fn session_cookie(sub: &str) -> String {
    session_cookie_with_exp(sub, 3600)
}

/// Mint a valid `oxidav_session` value for `sub`, expiring `exp_offset` seconds
/// from now (negative for an already-expired cookie).
fn session_cookie_with_exp(sub: &str, exp_offset: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs() as i64;
    let now_usize = now as usize;
    let exp = (now + exp_offset).max(0) as usize;

    let claims = Claims {
        sub: sub.to_string(),
        email: Some("user@example.com".into()),
        name: Some("Test User".into()),
        exp,
        iat: now_usize,
        id_token: None,
    };
    emit_session(&auth_config("https://issuer.test"), &claims).expect("emit session")
}

/// Mint a valid `oxidav_session` value that retains `id_token`, so `logout` can
/// build the provider SSO end-session URL.
fn session_cookie_with_id_token(sub: &str, id_token: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs() as i64;

    let claims = Claims {
        sub: sub.to_string(),
        email: Some("user@example.com".into()),
        name: Some("Test User".into()),
        exp: (now + 3600) as usize,
        iat: now as usize,
        id_token: Some(id_token.to_string()),
    };
    emit_session(&auth_config("https://issuer.test"), &claims).expect("emit session")
}

// ---------------------------------------------------------------------------
// Simulated IdP: an RSA keypair as PEM constants (no `rsa` dependency) plus a
// `wiremock` server serving discovery, token and JWKS.
//
// The fixtures have the same shape as the `src/auth/session.rs` unit tests: a
// fixed RSA key signs the fake ID tokens and its public half is served as JWKS,
// so the callback can be driven end to end without a real IdP.
// ---------------------------------------------------------------------------

/// `kid` of the single RSA key the fake IdP publishes.
const TEST_KID: &str = "test-kid";
/// Base64url modulus of the fake IdP's signing key (public half served as JWKS).
const TEST_JWKS_N: &str = "ybHbf8ju7SODIbYvLInRYmtJadSL4bsjqzO9f-87xoEtPLSPwqZzePmwZywjeN3DjygHPm5Fb7kuZRqYTGfrkWk9uIs4Z6xE_fcXiT1Qq9g9O_oPp-twq0os_--ClSmE7LZC4YESzeKHuFwHq1vRt3ud6vWbyvsNfOHQl_fue2_2gE5EnAzOIZ_zSrWrnl0LAJuT-qKBPC3k-LBvMGeXWSTZ8AgBTqMhxJxITrxgVqGwLOUOp3pyTMYop0B5YWX1m1xpK-r7Ria5t3lMc5-TNOZtb_Rxo6TNjB_Kdwzr7ZlpFcWH_-FWWwfBHKp5_jsFihtazz77sz8tTo_4QVJDHw";
/// Public exponent of the fake IdP's signing key (base64url `AQAB` = 65537).
const TEST_JWKS_E: &str = "AQAB";
/// PEM private half of the fake IdP's RSA key. A constant on purpose: the test
/// never generates a key nor pulls the deprecated `rsa` crate.
const TEST_PRIVATE_KEY_PEM: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDJsdt/yO7tI4Mh
ti8sidFia0lp1IvhuyOrM71/7zvGgS08tI/CpnN4+bBnLCN43cOPKAc+bkVvuS5l
GphMZ+uRaT24izhnrET99xeJPVCr2D07+g+n63CrSiz/74KVKYTstkLhgRLN4oe4
XAerW9G3e53q9ZvK+w184dCX9+57b/aATkScDM4hn/NKtaueXQsAm5P6ooE8LeT4
sG8wZ5dZJNnwCAFOoyHEnEhOvGBWobAs5Q6nenJMxiinQHlhZfWbXGkr6vtGJrm3
eUxzn5M05m1v9HGjpM2MH8p3DOvtmWkVxYf/4VZbB8Ecqnn+OwWKG1rPPvuzPy1O
j/hBUkMfAgMBAAECggEAFU68VRoZn6TGSCvyfN6MZ7zU0yDYrD60bHQ5W0gfRP/F
kymyHEqwXUHnkGa50p1++OMuLLrCjCSAkb7HAx9hZAm9sv7GNTGeUkQl0OJQ4O6l
vW3T9JXrX7Uk3t7jKTXrLISuuRsFLQn285OJXeGD3MHq83UCAeYWzTAf9MYFBDYm
EkKlTGUuX6O6xZEeuaieuaU9qRkUk35vm8fRwJcgvUeduX8xR+CgOVmSlvz8UY/y
c8PV+1hBHZhPFCDAmxbvaiZNFthTc/s3X/CFwaqlh2um+FQYaTxRiSmEC7KKCTaj
NvqcjVXFHh4+KVvVyRDyCuRglsJPYZdrHwgLyKmRxQKBgQD6yDGHo9dlZEIz0CSO
LJ1VfbmAKvJzAQ8oDhaxAHoAQTgk6z6oiC+j1z7ong+Yrp8U6X9ZaEDjtoQsCAW/
lhVNmHiYKayJUUsQ00zdcxDnWvv4vheQb6BERZBkXUSgsL+Ja5YwVSO7s/H9mAP+
Bw89gvplbLGkVmv9kQg5/65eGwKBgQDN5DKR7bVVoUeMJVLklYwAyJoJqt6jZ+pb
vJsN6OcNRUfLz1dqOZ1mX+wjdGunmMQ1s8FXuq1nGkgZjsNXdrHwUFPAQOIy3WjI
OOgh8C5E1b/YeVMn0G8lJYoC3nh0gvNCkeq7tMk8EszDg7BoJnkALmGXqlzAD/qO
u3jP2VsvTQKBgQC4ZBge7nYCo/wVUrZ+Hwm0AVQyi+Fmc+HsBqfij0IlC883Pgz9
J903b51etlErZ0Gqw6CSYZhMljeKlH99heG2AySwZPvqn249Oc/rh55nLbvVAhgb
aCgD6s4nLXi3Wh4K8aMleIRkkAOe/XX6AsDO7o4jow7ekXinoUrWMkvH/wKBgHOF
HQaZrAuekQlGC8trSQFLjHnuIDro3CqamRjl23WrsIvfCvnz16eQHGsMMDb51OUY
tVtmx1I5AcktO9cAJvhh6YvOG9xUNC1bGIuqOhuvHpP0Br8pCKN7+4J+lnEw5BIu
0th8qAgHuFHeuoTFSVDbtTAmiyHMFVYuQ6vDFZFlAoGBANcfxMTXA4tZg/tQUQNH
sD4oZPVlcmNFXsd9j42vgLnPo+F1xtnm2M33fNEyq9xRSdmu8OfjByimDynIINOk
GGqx5iiXrtdzoFsEVj2ueae5LFmbHJOQI/o1j6cse5nKpQs4MyQJ6sI4/d+uul6u
SestSfOxhvIiYxMrOAHQ3iRF
-----END PRIVATE KEY-----
"#;

/// Current UNIX timestamp in seconds.
fn now() -> usize {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs() as usize
}

/// The fake IdP's JWKS document: one RS256 key matching [`TEST_PRIVATE_KEY_PEM`].
fn jwks_json() -> String {
    format!(
        r#"{{"keys":[{{"kty":"RSA","use":"sig","kid":"{TEST_KID}","alg":"RS256","n":"{TEST_JWKS_N}","e":"{TEST_JWKS_E}"}}]}}"#
    )
}

/// Sign a fake `id_token` (RS256) with the fixed test key, bound to
/// `issuer`/`audience`/`nonce`.
fn sign_id_token(issuer: &str, audience: &str, nonce: &str, exp: usize) -> String {
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};

    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(TEST_KID.into());

    let claims = json!({
        "iss": issuer,
        "aud": audience,
        "sub": "user-123",
        "email": "user@example.com",
        "name": "Test User",
        "nonce": nonce,
        "iat": now(),
        "exp": exp,
    });

    encode(
        &header,
        &claims,
        &EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY_PEM.as_bytes())
            .expect("test RSA key must parse"),
    )
    .expect("test ID token must encode")
}

/// Mount the discovery document on `server`. Every advertised endpoint shares
/// the mock's origin, as RFC 8414 requires; `with_end_session` additionally
/// advertises an `end_session_endpoint`.
async fn mount_discovery(server: &MockServer, with_end_session: bool) {
    // Discovery is memoized in a process-wide, URL-keyed cache. Every test here
    // mounts a fresh mock IdP on an ephemeral port; a recycled port could serve
    // a stale document from a sibling test. Clear before mounting so no test can
    // observe another's cache entry.
    clear_caches().await;

    let base = server.uri();
    let body = if with_end_session {
        format!(
            r#"{{"authorization_endpoint":"{base}/authorize","token_endpoint":"{base}/token","jwks_uri":"{base}/jwks","end_session_endpoint":"{base}/logout"}}"#
        )
    } else {
        format!(
            r#"{{"authorization_endpoint":"{base}/authorize","token_endpoint":"{base}/token","jwks_uri":"{base}/jwks"}}"#
        )
    };

    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(body),
        )
        .mount(server)
        .await;
}

/// Mount the token endpoint. `Some(id_token)` makes the exchange succeed;
/// `None` makes it fail with `400` (a failed authorization-code exchange).
async fn mount_token_endpoint(server: &MockServer, id_token: Option<&str>) {
    let template = match id_token {
        Some(id_token) => ResponseTemplate::new(200)
            .insert_header("content-type", "application/json")
            .set_body_string(format!(r#"{{"id_token":"{id_token}"}}"#)),
        None => ResponseTemplate::new(400)
            .insert_header("content-type", "application/json")
            .set_body_string(r#"{"error":"invalid_grant"}"#),
    };

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(template)
        .mount(server)
        .await;
}

/// Mount the JWKS document on `server`.
async fn mount_jwks(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/jwks"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(jwks_json()),
        )
        .mount(server)
        .await;
}

/// Run `GET /api/auth/login` against `app` and return the issued flow cookie
/// value together with the `state`/`nonce` embedded in the authorization URL.
async fn login_flow(app: &axum::Router) -> (String, String, String) {
    let response = call(app.clone(), Method::GET, "/api/auth/login", None).await;
    assert_eq!(response.status(), StatusCode::FOUND, "login must redirect");

    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("login must carry a Location header");
    let url = url::Url::parse(location).expect("the authorization URL must parse");
    let params: std::collections::HashMap<_, _> = url.query_pairs().collect();

    let state = params
        .get("state")
        .map(ToString::to_string)
        .expect("the authorization URL must carry a state");
    let nonce = params
        .get("nonce")
        .map(ToString::to_string)
        .expect("the authorization URL must carry a nonce");

    let flow = set_cookies(&response)
        .into_iter()
        .find(|cookie| cookie.starts_with(FLOW_COOKIE))
        .expect("login must set the flow cookie");

    (cookie_value(&flow), state, nonce)
}

/// Drive a full, successful `login` → `callback` against a fresh mock IdP and
/// return `(app, server, flow_cookie_value, state)` so a test can replay the
/// callback. `server` must be kept alive by the caller so the mocks stay up.
async fn successful_callback_setup() -> (axum::Router, MockServer, String, String) {
    let server = MockServer::start().await;
    mount_discovery(&server, false).await;
    mount_jwks(&server).await;

    let app = enabled_app(&server.uri());
    let (flow, state, nonce) = login_flow(&app).await;

    let id_token = sign_id_token(&server.uri(), "oxidav-spa", &nonce, now() + 300);
    mount_token_endpoint(&server, Some(&id_token)).await;

    (app, server, flow, state)
}

/// The `name=value` value of a single `Set-Cookie` line.
fn cookie_value(set_cookie: &str) -> String {
    set_cookie
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .map(|(_, value)| value.to_string())
        .expect("a Set-Cookie line is name=value")
}

/// Flip the last character of a JWT's signature so it no longer verifies.
fn tamper_signature(token: &str) -> String {
    let mut parts: Vec<String> = token.split('.').map(str::to_string).collect();
    let signature = parts.last_mut().expect("JWT has a signature segment");
    let mut chars: Vec<char> = signature.chars().collect();
    let last = chars.pop().expect("signature is non-empty");
    chars.push(if last == 'a' { 'b' } else { 'a' });
    *signature = chars.into_iter().collect();
    parts.join(".")
}

/// The callback URI for a given `state`/`code` pair.
fn callback_uri(state: &str, code: &str) -> String {
    format!("/api/auth/callback?state={state}&code={code}")
}

// ---------------------------------------------------------------------------
// GET /api/auth/login
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_is_404_when_auth_is_disabled() {
    let response = call(disabled_app(), Method::GET, "/api/auth/login", None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn login_redirects_and_sets_the_flow_cookie_when_enabled() {
    let server = MockServer::start().await;
    clear_caches().await;
    let discovery = format!(
        r#"{{"authorization_endpoint":"{}/authorize","token_endpoint":"{}/token","jwks_uri":"{}/jwks"}}"#,
        server.uri(),
        server.uri(),
        server.uri()
    );
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(discovery),
        )
        .mount(&server)
        .await;

    let response = call(
        enabled_app(&server.uri()),
        Method::GET,
        "/api/auth/login",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::FOUND);
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("a redirect must carry a Location header");
    assert!(
        location.contains("response_type=code"),
        "authorization URL: {location}"
    );
    assert!(location.contains("client_id=oxidav-spa"));

    let flow = set_cookies(&response)
        .into_iter()
        .find(|cookie| cookie.starts_with(FLOW_COOKIE))
        .expect("login must set the flow cookie");
    assert!(
        flow.contains("HttpOnly"),
        "flow cookie must be HttpOnly: {flow}"
    );
    assert!(
        flow.contains("Secure"),
        "flow cookie must be Secure: {flow}"
    );
    assert!(
        flow.contains("SameSite=Lax"),
        "flow cookie must be SameSite=Lax: {flow}"
    );
    assert!(flow.contains("Path=/"), "flow cookie must be Path=/");
}

// ---------------------------------------------------------------------------
// GET /api/auth/callback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn callback_without_flow_cookie_is_401_and_clears_the_flow_cookie() {
    let response = call(
        enabled_app("https://issuer.test"),
        Method::GET,
        "/api/auth/callback?state=whatever&code=abc",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let cleared = set_cookies(&response)
        .into_iter()
        .any(|cookie| cookie.starts_with(FLOW_COOKIE) && cookie.contains("Max-Age=0"));
    assert!(
        cleared,
        "the callback must always clear the one-time flow cookie"
    );
}

/// The contract says the callback clears the flow cookie on **every** outcome
/// (it is one-time). With auth disabled the route answers `404` but must still
/// clear it.
#[tokio::test]
async fn callback_clears_the_flow_cookie_when_auth_is_disabled() {
    let response = call(
        disabled_app(),
        Method::GET,
        "/api/auth/callback?state=x&code=y",
        Some((FLOW_COOKIE, "leftover-flow-token")),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let cleared = set_cookies(&response)
        .into_iter()
        .any(|cookie| cookie.starts_with(FLOW_COOKIE) && cookie.contains("Max-Age=0"));
    assert!(
        cleared,
        "the disabled callback must still clear the one-time flow cookie"
    );
}

// ---------------------------------------------------------------------------
// GET /api/auth/me
// ---------------------------------------------------------------------------

#[tokio::test]
async fn me_returns_the_dev_user_when_auth_is_disabled() {
    let response = call(disabled_app(), Method::GET, "/api/auth/me", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    let value: serde_json::Value = serde_json::from_str(&body).expect("JSON body");
    assert_eq!(value["sub"], "dev");
    assert_eq!(value["name"], "Dev User");
    assert!(value["email"].is_null());
}

#[tokio::test]
async fn me_without_cookie_is_401_when_enabled() {
    let response = call(
        enabled_app("https://issuer.test"),
        Method::GET,
        "/api/auth/me",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn me_with_a_valid_cookie_returns_the_identity() {
    let cookie = session_cookie("user-42");
    let response = call(
        enabled_app("https://issuer.test"),
        Method::GET,
        "/api/auth/me",
        Some((SESSION_COOKIE_NAME, &cookie)),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    let value: serde_json::Value = serde_json::from_str(&body).expect("JSON body");
    assert_eq!(value["sub"], "user-42");
    assert_eq!(value["email"], "user@example.com");
    assert_eq!(value["name"], "Test User");
}

// ---------------------------------------------------------------------------
// POST /api/auth/logout
// ---------------------------------------------------------------------------

#[tokio::test]
async fn logout_returns_a_null_end_session_url_without_id_token() {
    let cookie = session_cookie("user-42");
    let response = call(
        enabled_app("https://issuer.test"),
        Method::POST,
        "/api/auth/logout",
        Some((SESSION_COOKIE_NAME, &cookie)),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let cleared = set_cookies(&response)
        .into_iter()
        .any(|cookie| cookie.starts_with(SESSION_COOKIE_NAME) && cookie.contains("Max-Age=0"));
    assert!(cleared, "logout must clear the session cookie");

    let body = body_string(response).await;
    let value: serde_json::Value = serde_json::from_str(&body).expect("JSON body");
    assert!(
        value["end_session_url"].is_null(),
        "a session without an id_token has no SSO logout URL: {value}"
    );
}

// ---------------------------------------------------------------------------
// Session middleware
// ---------------------------------------------------------------------------

#[tokio::test]
async fn healthz_is_exempt_from_the_session_requirement() {
    let response = call(
        enabled_app("https://issuer.test"),
        Method::GET,
        "/healthz",
        None,
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "healthz must not require a session"
    );
}

#[tokio::test]
async fn auth_routes_are_exempt_from_the_session_requirement() {
    // `/api/auth/callback` must reach its handler without a session: proof is
    // that it answers its own 401 AND clears the flow cookie (the middleware
    // would answer a bare JSON 401 and set no cookie).
    let response = call(
        enabled_app("https://issuer.test"),
        Method::GET,
        "/api/auth/callback?state=x&code=y",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        !set_cookies(&response).is_empty(),
        "an exempt auth route must reach its handler (which clears the flow cookie)"
    );
}

#[tokio::test]
async fn api_v1_requires_a_session_when_enabled() {
    let unauthorized = call(
        enabled_app("https://issuer.test"),
        Method::GET,
        "/api/v1/me",
        None,
    )
    .await;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let cookie = session_cookie("user-7");
    let authorized = call(
        enabled_app("https://issuer.test"),
        Method::GET,
        "/api/v1/me",
        Some((SESSION_COOKIE_NAME, &cookie)),
    )
    .await;
    assert_eq!(authorized.status(), StatusCode::OK);
}

#[tokio::test]
async fn api_v1_rejects_an_expired_session() {
    let cookie = session_cookie_with_exp("user-7", -10);
    let response = call(
        enabled_app("https://issuer.test"),
        Method::GET,
        "/api/v1/me",
        Some((SESSION_COOKIE_NAME, &cookie)),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "an expired session cookie must be rejected immediately"
    );
}

// ---------------------------------------------------------------------------
// GET /api/auth/callback — central invariants (simulated IdP)
// ---------------------------------------------------------------------------

/// Happy path: a full login → callback with a real (mock) IdP establishes the
/// session, redirects the SPA to `/` and clears the one-time flow cookie.
#[tokio::test]
async fn callback_happy_path_redirects_to_root_and_sets_the_session_cookie() {
    let (app, _server, flow, state) = successful_callback_setup().await;

    let response = call(
        app,
        Method::GET,
        &callback_uri(&state, "auth-code"),
        Some((FLOW_COOKIE, &flow)),
    )
    .await;

    assert_eq!(response.status(), StatusCode::FOUND);
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok());
    assert_eq!(
        location,
        Some("/"),
        "the callback must redirect to the SPA root"
    );

    let cookies = set_cookies(&response);
    let session = cookies
        .iter()
        .find(|cookie| cookie.starts_with(SESSION_COOKIE_NAME))
        .expect("the callback must set the session cookie");
    assert!(
        session.contains("HttpOnly"),
        "session cookie must be HttpOnly: {session}"
    );
    assert!(
        session.contains("Secure"),
        "session cookie must be Secure: {session}"
    );
    assert!(
        session.contains("SameSite=Lax"),
        "session cookie must be SameSite=Lax: {session}"
    );
    assert!(
        session.contains("Max-Age=28800"),
        "session cookie must last 8 h (28800 s): {session}"
    );
    assert!(
        cookies
            .iter()
            .any(|cookie| cookie.starts_with(FLOW_COOKIE) && cookie.contains("Max-Age=0")),
        "the callback must clear the one-time flow cookie"
    );
}

/// A `state` is consumed exactly once: replaying the same successful callback
/// must be rejected. This is the route-level proof of the single-use invariant.
#[tokio::test]
async fn callback_rejects_a_reused_state() {
    let (app, _server, flow, state) = successful_callback_setup().await;

    let first = call(
        app.clone(),
        Method::GET,
        &callback_uri(&state, "auth-code"),
        Some((FLOW_COOKIE, &flow)),
    )
    .await;
    assert_eq!(
        first.status(),
        StatusCode::FOUND,
        "the first callback succeeds"
    );

    // The browser already dropped the flow cookie, but resend it: the `state`
    // itself is consumed, so the replay must fail regardless of the cookie.
    let replay = call(
        app,
        Method::GET,
        &callback_uri(&state, "auth-code"),
        Some((FLOW_COOKIE, &flow)),
    )
    .await;
    assert_eq!(
        replay.status(),
        StatusCode::UNAUTHORIZED,
        "a replayed state must be rejected"
    );
}

/// A flow cookie with a tampered signature must be rejected before any network
/// call (it is an HS256 token signed with the session secret).
#[tokio::test]
async fn callback_rejects_a_flow_cookie_with_an_invalid_signature() {
    let server = MockServer::start().await;
    // No IdP mocks are needed: the forged cookie is refused first.
    let valid = emit_flow_token(&auth_config(&server.uri()), "state-x", "nonce-x", 600)
        .expect("a flow token must encode");
    let forged = tamper_signature(&valid);

    let response = call(
        enabled_app(&server.uri()),
        Method::GET,
        &callback_uri("state-x", "code"),
        Some((FLOW_COOKIE, &forged)),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// A valid flow cookie whose `state` does not match the `state` query parameter
/// must be rejected: the two must agree, otherwise an attacker could splice a
/// valid cookie onto a foreign callback.
#[tokio::test]
async fn callback_rejects_a_flow_cookie_whose_state_param_differs() {
    let server = MockServer::start().await;
    let flow = emit_flow_token(&auth_config(&server.uri()), "state-A", "nonce-A", 600)
        .expect("a flow token must encode");

    let response = call(
        enabled_app(&server.uri()),
        Method::GET,
        &callback_uri("state-B", "code"),
        Some((FLOW_COOKIE, &flow)),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// The `state` is consumed **before** the network exchange. With a failing
/// token endpoint the first callback is a `401`; replaying the same state is
/// also a `401` **and** never reaches the token endpoint again — proving the
/// state was spent before the exchange, not after it.
#[tokio::test]
async fn callback_consumes_the_state_before_the_token_exchange() {
    let server = MockServer::start().await;
    mount_discovery(&server, false).await;
    // The exchange fails (`400`), so no JWKS and no session are ever reached.
    mount_token_endpoint(&server, None).await;

    let app = enabled_app(&server.uri());
    let (flow, state, _nonce) = login_flow(&app).await;

    let first = call(
        app.clone(),
        Method::GET,
        &callback_uri(&state, "code"),
        Some((FLOW_COOKIE, &flow)),
    )
    .await;
    assert_eq!(
        first.status(),
        StatusCode::UNAUTHORIZED,
        "a failed token exchange must be 401"
    );

    let replay = call(
        app,
        Method::GET,
        &callback_uri(&state, "code"),
        Some((FLOW_COOKIE, &flow)),
    )
    .await;
    assert_eq!(
        replay.status(),
        StatusCode::UNAUTHORIZED,
        "the replayed state must also be 401"
    );

    let token_calls = server
        .received_requests()
        .await
        .expect("wiremock records requests")
        .iter()
        .filter(|request| request.url.path() == "/token")
        .count();
    assert_eq!(
        token_calls, 1,
        "the state must be consumed before the exchange: the token endpoint \
         must be hit exactly once"
    );
}

// ---------------------------------------------------------------------------
// POST /api/auth/logout — SSO end-session URL
// ---------------------------------------------------------------------------

/// When the session retained an `id_token` and the provider advertises an
/// `end_session_endpoint`, `logout` returns the SSO logout URL carrying the
/// token as `id_token_hint`.
#[tokio::test]
async fn logout_with_an_id_token_returns_the_end_session_url() {
    let server = MockServer::start().await;
    mount_discovery(&server, true).await;

    let cookie = session_cookie_with_id_token("user-42", "sso-id-token");
    let response = call(
        enabled_app(&server.uri()),
        Method::POST,
        "/api/auth/logout",
        Some((SESSION_COOKIE_NAME, &cookie)),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    let value: serde_json::Value = serde_json::from_str(&body).expect("JSON body");

    let url = value["end_session_url"]
        .as_str()
        .expect("a session with an id_token must produce an end_session_url");
    assert!(
        url.starts_with(&format!("{}/logout", server.uri())),
        "the URL must target the provider end_session_endpoint: {url}"
    );
    assert!(
        url.contains("id_token_hint=sso-id-token"),
        "the URL must carry the id_token as id_token_hint: {url}"
    );
}
