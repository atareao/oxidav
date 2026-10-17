//! F06/F17 — REST API integration tests.
//!
//! The `/api/v1` router is mounted behind the F17 session middleware. The
//! helpers below mint a signed `oxidav_session` cookie with `emit_session`, so
//! these tests exercise the real cookie authentication path end to end without
//! an IdP (was: an in-memory RS256 key pair and a loopback JWKS server).

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use oxidav::auth::session::{AuthConfig, Claims, SESSION_COOKIE_NAME, emit_session};
use oxidav::auth::{TokenError, verify_app_token};
use oxidav::config::Config;
use oxidav::dav::vcf::parse_vcf;
use oxidav::db::init_pool;
use oxidav::db::models::{NewCalendarObject, NewCardObject};
use oxidav::db::{
    get_calendar_object, get_card_object, insert_calendar_object, insert_card_object,
    list_card_objects,
};
use oxidav::router::{AppState, build_router, offline_pool};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;

/// HS256 secret shared by the test session cookies and the enabling config.
const TEST_JWT_SECRET: &str = "integration-test-jwt-secret";

/// Enabling [`AuthConfig`] used by the tests.
fn test_auth_config() -> Arc<AuthConfig> {
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

/// [`Config`] matching [`test_auth_config`] (auth enabled).
fn test_config() -> Config {
    Config {
        bind_addr: "127.0.0.1:0".parse().expect("valid socket addr"),
        log_level: "info".to_string(),
        database_url: "sqlite::memory:".to_string(),
        auth_enabled: true,
        auth_issuer_url: "https://issuer.test".into(),
        auth_client_id: "oxidav-spa".into(),
        auth_client_secret: "client-secret".into(),
        auth_redirect_url: "http://localhost/api/auth/callback".into(),
        auth_post_logout_redirect_url: String::new(),
        jwt_secret: TEST_JWT_SECRET.into(),
    }
}

/// Mint a signed session cookie value for `sub`.
fn session_cookie(auth: &AuthConfig, sub: &str) -> String {
    let now = unix_now();
    let claims = Claims {
        sub: sub.to_string(),
        email: None,
        name: None,
        exp: now + 3600,
        iat: now,
        id_token: None,
    };
    emit_session(auth, &claims).expect("emit session")
}

/// Current UNIX timestamp in seconds.
fn unix_now() -> usize {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as usize)
        .unwrap_or(0)
}

/// Build an [`AppState`] with auth enabled (cookie sessions) backed by `db`.
fn state_with_auth_and_db(db: SqlitePool) -> AppState {
    AppState::new(test_config(), Some(test_auth_config()), db)
}

// ---------------------------------------------------------------------------
// Collections test harness
// ---------------------------------------------------------------------------

/// A router plus its database and the enabling auth config (for minting
/// session cookies).
struct TestEnv {
    app: axum::Router,
    pool: SqlitePool,
    auth: Arc<AuthConfig>,
}

impl TestEnv {
    async fn new(tag: &str) -> Self {
        let pool = init_pool(&common::temp_db_url("api", tag))
            .await
            .expect("init_pool");
        let auth = test_auth_config();
        let app = build_router(AppState::new(
            test_config(),
            Some(Arc::clone(&auth)),
            pool.clone(),
        ));
        Self { app, pool, auth }
    }

    /// Mint a signed session cookie for `sub`.
    fn token(&self, sub: &str) -> String {
        session_cookie(&self.auth, sub)
    }

    /// Issue a request and decode the JSON body (or `Null` for an empty body).
    ///
    /// `token` is the value of the `oxidav_session` cookie (or `None` for an
    /// unauthenticated request); it is sent as `Cookie: oxidav_session=…`.
    async fn call(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header(header::COOKIE, format!("{SESSION_COOKIE_NAME}={token}"));
        }
        let body = match body {
            Some(value) => {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
                Body::from(serde_json::to_vec(&value).expect("serialize body"))
            }
            None => Body::empty(),
        };
        let request = builder.body(body).expect("build request");
        let response = self
            .app
            .clone()
            .oneshot(request)
            .await
            .expect("send request");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            // A non-JSON body is a contract regression (every endpoint must
            // answer JSON): fail loudly with the raw body instead of masking it
            // as `Null`.
            serde_json::from_slice(&bytes).unwrap_or_else(|err| {
                panic!(
                    "response body is not valid JSON ({err}); raw body: {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, value)
    }

    /// Issue a request with a raw, non-JSON body and decode the JSON response.
    ///
    /// Used by the F22 `POST .../contacts/import` endpoint, whose contract is
    /// `Content-Type: text/vcard` with the `.vcf` as the raw body.
    async fn call_body(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        content_type: &str,
        body: Vec<u8>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, content_type);
        if let Some(token) = token {
            builder = builder.header(header::COOKIE, format!("{SESSION_COOKIE_NAME}={token}"));
        }
        let request = builder.body(Body::from(body)).expect("build request");
        let response = self
            .app
            .clone()
            .oneshot(request)
            .await
            .expect("send request");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|err| {
                panic!(
                    "response body is not valid JSON ({err}); raw body: {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, value)
    }

    /// `POST /collections`, returning the created DTO.
    async fn create(&self, token: &str, body: Value) -> Value {
        let (status, value) = self
            .call(Method::POST, "/api/v1/collections", Some(token), Some(body))
            .await;
        assert_eq!(status, StatusCode::CREATED, "create failed: {value}");
        value
    }
}

fn collection_body(name: &str, kind: &str, color: Option<&str>) -> Value {
    let mut body = json!({ "name": name, "type": kind });
    if let Some(color) = color {
        body["color"] = json!(color);
    }
    body
}

#[tokio::test]
async fn me_without_cookie_returns_401() {
    let app = build_router(state_with_auth_and_db(offline_pool()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/me")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("send request");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    // F17 authenticates with a cookie: there is no `Bearer` challenge to send.
    assert!(
        response.headers().get(header::WWW_AUTHENTICATE).is_none(),
        "a cookie-authenticated 401 must not advertise a Bearer challenge"
    );

    // The 401 must use the same uniform error envelope as every other endpoint.
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let value: Value = serde_json::from_slice(&body).expect("json body");
    assert_eq!(value["error"]["code"], "unauthorized");
    assert!(value["error"]["message"].is_string());
}

#[tokio::test]
async fn me_with_valid_session_cookie_returns_sub() {
    let auth = test_auth_config();
    let app = build_router(AppState::new(
        test_config(),
        Some(Arc::clone(&auth)),
        offline_pool(),
    ));
    let cookie = session_cookie(&auth, "user-42");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/me")
                .header(header::COOKIE, format!("{SESSION_COOKIE_NAME}={cookie}"))
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("send request");

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let value: Value = serde_json::from_slice(&body).expect("json body");
    assert_eq!(value, json!({ "sub": "user-42" }));
}

// ---------------------------------------------------------------------------
// Tarea 2 — POST crea colección → aparece en GET listado
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_collections_create_then_list() {
    let env = TestEnv::new("create").await;
    let token = env.token("user-a");

    let created = env
        .create(
            &token,
            collection_body("Personal", "calendar", Some("#1677ff")),
        )
        .await;
    assert_eq!(created["name"], "Personal");
    assert_eq!(created["type"], "calendar");
    assert_eq!(created["color"], "#1677ff");
    assert!(created["id"].as_str().is_some_and(|id| id.len() == 36));

    let (status, page) = env
        .call(Method::GET, "/api/v1/collections", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(page["items"][0]["id"], created["id"]);
}

// ---------------------------------------------------------------------------
// Tarea 3 — Validación 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_collections_validation() {
    let env = TestEnv::new("validation").await;
    let token = env.token("user-a");

    let cases = [
        ("empty name", collection_body("   ", "calendar", None)),
        ("bad type", collection_body("Ok", "task", None)),
        (
            "bad color",
            collection_body("Ok", "addressbook", Some("#12345")),
        ),
    ];

    for (label, body) in cases {
        let (status, value) = env
            .call(
                Method::POST,
                "/api/v1/collections",
                Some(&token),
                Some(body),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "case: {label}");
        assert_eq!(value["error"]["code"], "validation", "case: {label}");
        assert!(value["error"]["message"].is_string(), "case: {label}");
    }

    // Nothing was persisted.
    let (_, page) = env
        .call(Method::GET, "/api/v1/collections", Some(&token), None)
        .await;
    assert_eq!(page["total"], 0);
}

// ---------------------------------------------------------------------------
// Tarea 4 — Scoping: recurso ajeno → 404
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_collections_scoping() {
    let env = TestEnv::new("scoping").await;
    let token_a = env.token("user-a");
    let token_b = env.token("user-b");

    let created = env
        .create(&token_a, collection_body("A", "calendar", None))
        .await;
    let id = created["id"].as_str().expect("id");

    let (status, _) = env
        .call(
            Method::GET,
            &format!("/api/v1/collections/{id}"),
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (_, page_b) = env
        .call(Method::GET, "/api/v1/collections", Some(&token_b), None)
        .await;
    assert_eq!(page_b["total"], 0, "B must not see A's collections");
}

// ---------------------------------------------------------------------------
// Tarea 5 — PATCH parcial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_collections_update_is_partial() {
    let env = TestEnv::new("update").await;
    let token = env.token("user-a");

    let created = env
        .create(&token, collection_body("Old", "calendar", Some("#111111")))
        .await;
    let id = created["id"].as_str().expect("id").to_string();
    let uri = format!("/api/v1/collections/{id}");

    // Change name only; colour must survive.
    let (status, patched) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({ "name": "New" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["name"], "New");
    assert_eq!(patched["color"], "#111111");

    // Change colour only; name must survive.
    let (status, patched) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({ "color": "#222222" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["name"], "New");
    assert_eq!(patched["color"], "#222222");

    // Invalid patch is rejected.
    let (status, _) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({ "name": "  " })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// Tarea 6 — PATCH/GET/DELETE sobre inexistente → 404
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_collections_not_found() {
    let env = TestEnv::new("notfound").await;
    let token = env.token("user-a");
    let uri = "/api/v1/collections/00000000-0000-4000-8000-000000000000";

    let (get_status, get_body) = env.call(Method::GET, uri, Some(&token), None).await;
    assert_eq!(get_status, StatusCode::NOT_FOUND);
    assert_eq!(get_body["error"]["code"], "not_found");

    let (patch_status, _) = env
        .call(
            Method::PATCH,
            uri,
            Some(&token),
            Some(json!({ "name": "X" })),
        )
        .await;
    assert_eq!(patch_status, StatusCode::NOT_FOUND);

    let (delete_status, _) = env.call(Method::DELETE, uri, Some(&token), None).await;
    assert_eq!(delete_status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Tarea 7 — Paginación y clamp
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_collections_pagination_and_clamp() {
    let env = TestEnv::new("pagination").await;
    let token = env.token("user-a");

    for name in ["Alpha", "Beta", "Gamma"] {
        env.create(&token, collection_body(name, "calendar", None))
            .await;
    }

    let (status, page) = env
        .call(
            Method::GET,
            "/api/v1/collections?limit=2&offset=1",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["total"], 3);
    assert_eq!(page["limit"], 2);
    assert_eq!(page["offset"], 1);
    let items = page["items"].as_array().expect("items");
    assert_eq!(items.len(), 2, "page size must match the effective limit");
    assert_eq!(items[0]["name"], "Beta");
    assert_eq!(items[1]["name"], "Gamma");

    // Non-positive limit falls back to the default and is echoed back.
    let (_, page) = env
        .call(
            Method::GET,
            "/api/v1/collections?limit=0",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(page["limit"], 20);

    // Oversized limit is clamped to the ceiling and reflected.
    let (_, page) = env
        .call(
            Method::GET,
            "/api/v1/collections?limit=100000",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(page["limit"], 200);
}

// ---------------------------------------------------------------------------
// Tarea 8 — DELETE elimina (cascadea objetos) y segundo DELETE → 404
// ---------------------------------------------------------------------------

/// Count the rows in `table` belonging to `collection_id`.
async fn count_objects(pool: &SqlitePool, table: &str, collection_id: &str) -> i64 {
    sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM {table} WHERE collection_id = ?"
    ))
    .bind(collection_id)
    .fetch_one(pool)
    .await
    .expect("count objects")
}

#[tokio::test]
async fn api_collections_delete_cascades_and_is_not_idempotent() {
    let env = TestEnv::new("delete").await;
    let token = env.token("user-a");

    let created = env
        .create(&token, collection_body("Doomed", "calendar", None))
        .await;
    let id = created["id"].as_str().expect("id").to_string();

    // Seed one calendar object and one card object in the collection.
    insert_calendar_object(
        &env.pool,
        &NewCalendarObject {
            id: "evt-1".to_string(),
            collection_id: id.clone(),
            uid: "uid-evt-1".to_string(),
            summary: Some("Event".to_string()),
            description: None,
            dtstart: Some(1_700_000_000_000),
            dtend: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            location: None,
            url: None,
            rrule: None,
            raw_payload: "BEGIN:VCALENDAR\nEND:VCALENDAR".to_string(),
        },
    )
    .await
    .expect("insert event");
    insert_card_object(
        &env.pool,
        &NewCardObject {
            id: "card-1".to_string(),
            collection_id: id.clone(),
            uid: "uid-card-1".to_string(),
            fn_: Some("Ada".to_string()),
            email: None,
            phone: None,
            organization: None,
            notes: None,
            raw_payload: "BEGIN:VCARD\nEND:VCARD".to_string(),
        },
    )
    .await
    .expect("insert card");
    assert_eq!(count_objects(&env.pool, "calendar_objects", &id).await, 1);
    assert_eq!(count_objects(&env.pool, "card_objects", &id).await, 1);

    let (status, body) = env
        .call(
            Method::DELETE,
            &format!("/api/v1/collections/{id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null, "204 must have no body");

    // Cascaded deletes.
    assert_eq!(count_objects(&env.pool, "calendar_objects", &id).await, 0);
    assert_eq!(count_objects(&env.pool, "card_objects", &id).await, 0);

    // Gone from the listing.
    let (_, page) = env
        .call(Method::GET, "/api/v1/collections", Some(&token), None)
        .await;
    assert_eq!(page["total"], 0);

    // A second delete is an explicit 404.
    let (status, _) = env
        .call(
            Method::DELETE,
            &format!("/api/v1/collections/{id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Tarea 9 — Envoltura de error uniforme y no filtración interna
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_error_envelope_is_uniform_and_leaks_nothing() {
    let env = TestEnv::new("envelope").await;
    let token = env.token("user-a");

    // 400: validation.
    let (status, value) = env
        .call(
            Method::POST,
            "/api/v1/collections",
            Some(&token),
            Some(collection_body("", "calendar", None)),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"]["code"], "validation");
    let keys: Vec<&String> = value["error"]
        .as_object()
        .expect("error object")
        .keys()
        .collect();
    assert_eq!(keys.len(), 2, "only code and message are exposed");

    // 404: not found, same shape.
    let (status, value) = env
        .call(
            Method::GET,
            "/api/v1/collections/missing",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(value["error"]["code"], "not_found");
    assert!(value["error"]["message"].is_string());

    // 500: simulated database failure — generic message, no internal detail.
    env.pool.close().await;
    let (status, value) = env
        .call(Method::GET, "/api/v1/collections", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(value["error"]["code"], "internal");
    assert_eq!(value["error"]["message"], "Error interno del servidor");
    let rendered = value.to_string();
    assert!(
        !rendered.contains("database") && !rendered.contains("Query"),
        "internal detail leaked: {rendered}"
    );
}

// ---------------------------------------------------------------------------
// F10 — Search API (`/api/v1/search`)
// ---------------------------------------------------------------------------

/// Seed `n` calendar objects matching "reunion" into `collection_id`.
async fn seed_events(pool: &SqlitePool, collection_id: &str, n: usize) {
    for i in 0..n {
        insert_calendar_object(
            pool,
            &NewCalendarObject {
                id: format!("evt-{i}"),
                collection_id: collection_id.to_string(),
                uid: format!("uid-evt-{i}"),
                summary: Some("reunion importante".to_string()),
                description: None,
                dtstart: Some(1_700_000_000_000),
                dtend: None,
                dtstart_tzid: None,
                dtend_tzid: None,
                location: None,
                url: None,
                rrule: None,
                raw_payload: format!("BEGIN:VCALENDAR\nSUMMARY:reunion {i}\nEND:VCALENDAR"),
            },
        )
        .await
        .expect("insert event");
    }
}

/// Tarea 2 — `GET /api/v1/search/events`: scoped DTOs, bm25 order, no payload.
#[tokio::test]
async fn search_events_endpoint_is_scoped_and_ranked() {
    let env = TestEnv::new("search-events").await;
    let token_a = env.token("user-a");
    let token_b = env.token("user-b");

    let col_a = env
        .create(&token_a, collection_body("A", "calendar", None))
        .await;
    let cid_a = col_a["id"].as_str().expect("id").to_string();
    let col_b = env
        .create(&token_b, collection_body("B", "calendar", None))
        .await;
    let cid_b = col_b["id"].as_str().expect("id").to_string();

    // "many" repeats the term thrice → more relevant under bm25 ASC.
    insert_calendar_object(
        &env.pool,
        &NewCalendarObject {
            id: "evt-many".to_string(),
            collection_id: cid_a.clone(),
            uid: "uid-many".to_string(),
            summary: Some("reunion reunion reunion".to_string()),
            description: None,
            dtstart: Some(1_700_000_000_000),
            dtend: Some(1_700_000_600_000),
            dtstart_tzid: None,
            dtend_tzid: None,
            location: None,
            url: None,
            rrule: None,
            raw_payload: "BEGIN:VCALENDAR\nSECRET-A\nEND:VCALENDAR".to_string(),
        },
    )
    .await
    .expect("evt many");
    insert_calendar_object(
        &env.pool,
        &NewCalendarObject {
            id: "evt-few".to_string(),
            collection_id: cid_a.clone(),
            uid: "uid-few".to_string(),
            summary: Some("reunion equipo".to_string()),
            description: None,
            dtstart: None,
            dtend: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            location: None,
            url: None,
            rrule: None,
            raw_payload: "BEGIN:VCALENDAR\nEND:VCALENDAR".to_string(),
        },
    )
    .await
    .expect("evt few");
    // A foreign object user-a must never see.
    insert_calendar_object(
        &env.pool,
        &NewCalendarObject {
            id: "evt-b".to_string(),
            collection_id: cid_b,
            uid: "uid-b".to_string(),
            summary: Some("reunion secreta".to_string()),
            description: None,
            dtstart: None,
            dtend: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            location: None,
            url: None,
            rrule: None,
            raw_payload: "BEGIN:VCALENDAR\nEND:VCALENDAR".to_string(),
        },
    )
    .await
    .expect("evt b");

    let (status, value) = env
        .call(
            Method::GET,
            "/api/v1/search/events?q=reunion",
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let items = value.as_array().expect("array");
    assert_eq!(items.len(), 2, "user-a sees only its own two events");
    assert_eq!(items[0]["id"], "evt-many", "bm25 ASC: most relevant first");
    assert_eq!(items[1]["id"], "evt-few");
    assert_eq!(items[0]["uid"], "uid-many");
    assert_eq!(items[0]["collection_id"], cid_a);
    assert_eq!(items[0]["summary"], "reunion reunion reunion");
    assert_eq!(items[0]["dtstart"], 1_700_000_000_000_i64);
    assert_eq!(items[0]["dtend"], 1_700_000_600_000_i64);
    assert!(
        items[0].get("raw_payload").is_none(),
        "raw_payload must not be exposed"
    );
    assert!(
        !value.to_string().contains("SECRET-A"),
        "raw .ics payload leaked"
    );

    let (_, value_b) = env
        .call(
            Method::GET,
            "/api/v1/search/events?q=reunion",
            Some(&token_b),
            None,
        )
        .await;
    let items_b = value_b.as_array().expect("array");
    assert_eq!(items_b.len(), 1, "user-b sees only its own event");
    assert_eq!(items_b[0]["id"], "evt-b");
}

/// Tarea 3 — `GET /api/v1/search/contacts`: scoped DTOs, no `.vcf` payload.
#[tokio::test]
async fn search_contacts_endpoint_is_scoped() {
    let env = TestEnv::new("search-contacts").await;
    let token_a = env.token("user-a");
    let token_b = env.token("user-b");

    let book_a = env
        .create(&token_a, collection_body("Abook", "addressbook", None))
        .await;
    let bid_a = book_a["id"].as_str().expect("id").to_string();
    let book_b = env
        .create(&token_b, collection_body("Bbook", "addressbook", None))
        .await;
    let bid_b = book_b["id"].as_str().expect("id").to_string();

    insert_card_object(
        &env.pool,
        &NewCardObject {
            id: "card-a".to_string(),
            collection_id: bid_a.clone(),
            uid: "uid-ca".to_string(),
            fn_: Some("Ada Lovelace".to_string()),
            email: Some("ada@example.com".to_string()),
            phone: Some("+34600000000".to_string()),
            organization: Some("Analytical".to_string()),
            notes: None,
            raw_payload: "BEGIN:VCARD\nSECRET-VCF\nEND:VCARD".to_string(),
        },
    )
    .await
    .expect("card a");
    // Distinct FN so a leak would be obvious; both still match "Lovelace".
    insert_card_object(
        &env.pool,
        &NewCardObject {
            id: "card-b".to_string(),
            collection_id: bid_b,
            uid: "uid-cb".to_string(),
            fn_: Some("Grace Lovelace".to_string()),
            email: None,
            phone: None,
            organization: None,
            notes: None,
            raw_payload: "BEGIN:VCARD\nEND:VCARD".to_string(),
        },
    )
    .await
    .expect("card b");

    let (status, value) = env
        .call(
            Method::GET,
            "/api/v1/search/contacts?q=Lovelace",
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let items = value.as_array().expect("array");
    assert_eq!(items.len(), 1, "user-a sees only its own contact");
    assert_eq!(items[0]["id"], "card-a");
    assert_eq!(items[0]["uid"], "uid-ca");
    assert_eq!(items[0]["collection_id"], bid_a);
    assert_eq!(items[0]["fn"], "Ada Lovelace");
    assert_eq!(items[0]["email"], "ada@example.com");
    assert_eq!(items[0]["phone"], "+34600000000");
    assert_eq!(items[0]["organization"], "Analytical");
    assert!(
        items[0].get("raw_payload").is_none(),
        "raw_payload must not be exposed"
    );
    assert!(
        !value.to_string().contains("SECRET-VCF"),
        "raw .vcf payload leaked"
    );

    let (_, value_b) = env
        .call(
            Method::GET,
            "/api/v1/search/contacts?q=Lovelace",
            Some(&token_b),
            None,
        )
        .await;
    let items_b = value_b.as_array().expect("array");
    assert_eq!(items_b.len(), 1);
    assert_eq!(items_b[0]["id"], "card-b");
}

/// Tarea 4 — short/empty query is `200` + empty list (trigram limitation).
#[tokio::test]
async fn search_short_query_returns_empty_list() {
    let env = TestEnv::new("search-short").await;
    let token = env.token("user-a");
    let col = env
        .create(&token, collection_body("A", "calendar", None))
        .await;
    let cid = col["id"].as_str().expect("id").to_string();
    insert_calendar_object(
        &env.pool,
        &NewCalendarObject {
            id: "evt-1".to_string(),
            collection_id: cid,
            uid: "uid-1".to_string(),
            summary: Some("reunion".to_string()),
            description: None,
            dtstart: None,
            dtend: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            location: None,
            url: None,
            rrule: None,
            raw_payload: "BEGIN:VCALENDAR\nEND:VCALENDAR".to_string(),
        },
    )
    .await
    .expect("event");

    for uri in [
        "/api/v1/search/events?q=",
        "/api/v1/search/events?q=ab",
        "/api/v1/search/events",
        "/api/v1/search/contacts?q=",
        "/api/v1/search/contacts?q=ab",
    ] {
        let (status, value) = env.call(Method::GET, uri, Some(&token), None).await;
        assert_eq!(status, StatusCode::OK, "uri: {uri}");
        assert_eq!(
            value.as_array().map(Vec::len),
            Some(0),
            "uri: {uri} must answer []"
        );
    }

    // The aggregate endpoint shares the rule and always answers both keys.
    let (status, value) = env
        .call(Method::GET, "/api/v1/search?q=ab", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["events"].as_array().map(Vec::len), Some(0));
    assert_eq!(value["contacts"].as_array().map(Vec::len), Some(0));
}

/// Tarea 5 — `limit` clamp (to `MAX_SEARCH_LIMIT`), default and `400`.
#[tokio::test]
async fn search_limit_clamp_and_default() {
    let env = TestEnv::new("search-limit").await;
    let token = env.token("user-a");
    let col = env
        .create(&token, collection_body("Big", "calendar", None))
        .await;
    let cid = col["id"].as_str().expect("id").to_string();
    seed_events(&env.pool, &cid, 205).await;

    // Oversized limit is clamped to MAX_SEARCH_LIMIT (200).
    let (status, value) = env
        .call(
            Method::GET,
            "/api/v1/search/events?q=reunion&limit=100000",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value.as_array().map(Vec::len), Some(200), "clamped to 200");

    // Absent limit uses the documented default (20).
    let (status, value) = env
        .call(
            Method::GET,
            "/api/v1/search/events?q=reunion",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value.as_array().map(Vec::len), Some(20), "default is 20");

    // Non-positive limit clamps to 1 (never SQLite's unbounded `LIMIT -1`).
    let (status, value) = env
        .call(
            Method::GET,
            "/api/v1/search/events?q=reunion&limit=0",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value.as_array().map(Vec::len), Some(1));

    // A non-numeric limit is a uniform validation error.
    let (status, value) = env
        .call(
            Method::GET,
            "/api/v1/search/events?q=reunion&limit=abc",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"]["code"], "validation");
}

/// Tarea 6 — internal failures use the envelope and leak nothing.
#[tokio::test]
async fn search_error_envelope_leaks_nothing() {
    let env = TestEnv::new("search-envelope").await;
    let token = env.token("user-a");

    env.pool.close().await;
    let (status, value) = env
        .call(
            Method::GET,
            "/api/v1/search/events?q=reunion",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(value["error"]["code"], "internal");
    assert_eq!(value["error"]["message"], "Error interno del servidor");
    let rendered = value.to_string();
    assert!(
        !rendered.contains("database") && !rendered.contains("Query"),
        "internal detail leaked: {rendered}"
    );
}

/// Regresión (review F10, MAYOR 1) — `q` no debe interpretarse como gramática
/// FTS5. Comillas, operadores y paréntesis se neutralizan como texto literal, así
/// que ninguna entrada de usuario puede producir un `500`.
#[tokio::test]
async fn search_hostile_query_never_returns_500() {
    let env = TestEnv::new("search-hostile").await;
    let token = env.token("user-a");
    let col = env
        .create(&token, collection_body("A", "calendar", None))
        .await;
    let cid = col["id"].as_str().expect("id").to_string();
    let book = env
        .create(&token, collection_body("Book", "addressbook", None))
        .await;
    let bid = book["id"].as_str().expect("id").to_string();

    insert_calendar_object(
        &env.pool,
        &NewCalendarObject {
            id: "evt-1".to_string(),
            collection_id: cid,
            uid: "uid-1".to_string(),
            summary: Some("reunion importante".to_string()),
            description: None,
            dtstart: None,
            dtend: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            location: None,
            url: None,
            rrule: None,
            raw_payload: "BEGIN:VCALENDAR\nEND:VCALENDAR".to_string(),
        },
    )
    .await
    .expect("event");
    insert_card_object(
        &env.pool,
        &NewCardObject {
            id: "card-1".to_string(),
            collection_id: bid,
            uid: "uid-c".to_string(),
            fn_: Some("Ada Lovelace".to_string()),
            email: None,
            phone: None,
            organization: None,
            notes: None,
            raw_payload: "BEGIN:VCARD\nEND:VCARD".to_string(),
        },
    )
    .await
    .expect("card");

    // Percent-encoded hostile inputs: an unterminated quote, a bare operator,
    // an unbalanced parenthesis, a glob, etc. None may 500.
    let hostile = [
        "a%22b",              // a"b
        "reunion%22",         // reunion"
        "reunion%20AND",      // reunion AND
        "foo%28",             // foo(
        "a%20OR%20b",         // a OR b
        "NOT%20x",            // NOT x
        "%2A",                // *
        "%22%22%22",          // """
        "reunion%20NEAR%2F2", // reunion NEAR/2
    ];

    for q in hostile {
        for base in [
            "/api/v1/search/events",
            "/api/v1/search/contacts",
            "/api/v1/search",
        ] {
            let uri = format!("{base}?q={q}");
            let (status, value) = env.call(Method::GET, &uri, Some(&token), None).await;
            assert_eq!(status, StatusCode::OK, "uri: {uri} -> {value}");
        }
    }

    // Escaping must not break a legitimate query: the literal phrase still
    // matches its substring.
    let (status, value) = env
        .call(
            Method::GET,
            "/api/v1/search/events?q=reunion",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value.as_array().map(Vec::len), Some(1));
    assert_eq!(value[0]["id"], "evt-1");
}

/// Tarea 7 — no/invalid token → `401` on every search endpoint.
#[tokio::test]
async fn search_unauthorized_without_token() {
    let env = TestEnv::new("search-401").await;

    for uri in [
        "/api/v1/search/events?q=reunion",
        "/api/v1/search/contacts?q=reunion",
        "/api/v1/search?q=reunion",
    ] {
        let (status, value) = env.call(Method::GET, uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "uri: {uri}");
        assert_eq!(value["error"]["code"], "unauthorized");
    }

    let (status, _) = env
        .call(
            Method::GET,
            "/api/v1/search/events?q=reunion",
            Some("not-a-jwt"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Tarea 8 — aggregate endpoint returns both sets, scoped.
#[tokio::test]
async fn search_aggregate_returns_both_sets() {
    let env = TestEnv::new("search-aggregate").await;
    let token = env.token("user-a");
    let col = env
        .create(&token, collection_body("A", "calendar", None))
        .await;
    let cid = col["id"].as_str().expect("id").to_string();
    let book = env
        .create(&token, collection_body("Book", "addressbook", None))
        .await;
    let bid = book["id"].as_str().expect("id").to_string();

    insert_calendar_object(
        &env.pool,
        &NewCalendarObject {
            id: "evt-1".to_string(),
            collection_id: cid,
            uid: "uid-e".to_string(),
            summary: Some("zeta meeting".to_string()),
            description: None,
            dtstart: None,
            dtend: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            location: None,
            url: None,
            rrule: None,
            raw_payload: "BEGIN:VCALENDAR\nEND:VCALENDAR".to_string(),
        },
    )
    .await
    .expect("event");
    insert_card_object(
        &env.pool,
        &NewCardObject {
            id: "card-1".to_string(),
            collection_id: bid,
            uid: "uid-c".to_string(),
            fn_: Some("Zeta Person".to_string()),
            email: None,
            phone: None,
            organization: None,
            notes: None,
            raw_payload: "BEGIN:VCARD\nEND:VCARD".to_string(),
        },
    )
    .await
    .expect("card");

    let (status, value) = env
        .call(Method::GET, "/api/v1/search?q=zeta", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["events"].as_array().map(Vec::len), Some(1));
    assert_eq!(value["contacts"].as_array().map(Vec::len), Some(1));
    assert_eq!(value["events"][0]["id"], "evt-1");
    assert_eq!(value["contacts"][0]["id"], "card-1");
    assert_eq!(value["contacts"][0]["fn"], "Zeta Person");
}

// ---------------------------------------------------------------------------
// F11 — Calendar event REST endpoints (`/api/v1/collections/{id}/events`)
// ---------------------------------------------------------------------------

/// Create a `calendar` collection for `token` and return its id.
async fn calendar_collection_id(env: &TestEnv, token: &str, name: &str) -> String {
    let created = env
        .create(token, collection_body(name, "calendar", None))
        .await;
    created["id"].as_str().expect("collection id").to_string()
}

/// Tareas 2–5 — scoped CRUD: create (`201`), list (`200`), read (`200`),
/// patch (`200`, new ETag, recomposed `.ics`), delete (`204`); user B never
/// reaches user A's collection (anti-IDOR); `raw_payload` is never exposed.
#[tokio::test]
async fn events_crud_is_scoped_and_hides_raw_payload() {
    let env = TestEnv::new("events-crud").await;
    let token_a = env.token("user-a");
    let token_b = env.token("user-b");
    let cid = calendar_collection_id(&env, &token_a, "A").await;
    let base = format!("/api/v1/collections/{cid}/events");

    // Create.
    let (status, created) = env
        .call(
            Method::POST,
            &base,
            Some(&token_a),
            Some(json!({
                "summary": "Reunión",
                "description": "line1\nline2; with, punctuation",
                "dtstart": 1_700_000_000_000_i64,
                "dtend": 1_700_000_600_000_i64,
                "rrule": "FREQ=WEEKLY;COUNT=3",
                "uid": "evt-uid-1",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {created}");
    assert_eq!(created["uid"], "evt-uid-1");
    assert_eq!(created["collection_id"], cid);
    assert_eq!(created["summary"], "Reunión");
    assert_eq!(created["dtstart"], 1_700_000_000_000_i64);
    assert_eq!(created["dtend"], 1_700_000_600_000_i64);
    assert_eq!(created["rrule"], "FREQ=WEEKLY;COUNT=3");
    assert!(created["etag"].as_str().is_some_and(|e| !e.is_empty()));
    assert!(
        created.get("raw_payload").is_none(),
        "raw_payload must not be exposed"
    );
    assert!(
        !created.to_string().contains("BEGIN:VCALENDAR"),
        "raw .ics leaked"
    );
    let etag_before = created["etag"].as_str().expect("etag").to_string();

    // List.
    let (status, list) = env.call(Method::GET, &base, Some(&token_a), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = list.as_array().expect("array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["uid"], "evt-uid-1");
    assert!(items[0].get("raw_payload").is_none());
    assert!(!list.to_string().contains("BEGIN:VCALENDAR"));

    // Read one.
    let one_uri = format!("{base}/evt-uid-1");
    let (status, one) = env.call(Method::GET, &one_uri, Some(&token_a), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(one["uid"], "evt-uid-1");
    assert!(one.get("raw_payload").is_none());

    // Patch: the `.ics` is recomposed and a fresh ETag is written.
    let (status, patched) = env
        .call(
            Method::PATCH,
            &one_uri,
            Some(&token_a),
            Some(json!({ "summary": "Renombrado" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "patch failed: {patched}");
    assert_eq!(patched["summary"], "Renombrado");
    assert_eq!(
        patched["dtstart"], 1_700_000_000_000_i64,
        "dtstart must survive a partial patch"
    );
    assert_ne!(
        patched["etag"].as_str(),
        Some(etag_before.as_str()),
        "the etag must change after an edit"
    );
    assert!(patched.get("raw_payload").is_none());

    // Anti-IDOR: user B cannot reach user A's collection at all.
    let (status, _) = env.call(Method::GET, &base, Some(&token_b), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "B must not list A's events");
    let (status, _) = env.call(Method::GET, &one_uri, Some(&token_b), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "B must not read A's event");
    let (status, _) = env
        .call(
            Method::PATCH,
            &one_uri,
            Some(&token_b),
            Some(json!({ "summary": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "B must not edit A's event");
    let (status, _) = env
        .call(Method::DELETE, &one_uri, Some(&token_b), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "B must not delete A's event");

    // Delete as the owner: explicit `204`, then gone.
    let (status, body) = env
        .call(Method::DELETE, &one_uri, Some(&token_a), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null, "204 must have no body");
    let (status, _) = env.call(Method::GET, &one_uri, Some(&token_a), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Tarea 3 — invalid input is a uniform `400`, including the `rrule` CRLF
/// injection regression (Bloqueante 1 del review F11).
#[tokio::test]
async fn events_validation_rejects_bad_input() {
    let env = TestEnv::new("events-validation").await;
    let token = env.token("user-a");
    let cid = calendar_collection_id(&env, &token, "A").await;
    let base = format!("/api/v1/collections/{cid}/events");

    let cases = [
        (
            "inverted range",
            json!({ "summary": "x", "dtstart": 1000, "dtend": 999 }),
        ),
        (
            "blank summary",
            json!({ "summary": "   ", "dtstart": 1000 }),
        ),
        ("missing dtstart", json!({ "summary": "x" })),
        (
            "rrule CRLF injection",
            json!({
                "summary": "x",
                "dtstart": 1000,
                "rrule": "FREQ=DAILY\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:evil@x",
            }),
        ),
        (
            "rrule bare newline",
            json!({ "summary": "x", "dtstart": 1000, "rrule": "FREQ=DAILY\nX" }),
        ),
        (
            "rrule control char",
            json!({ "summary": "x", "dtstart": 1000, "rrule": "FREQ=DAILY\u{0007}" }),
        ),
        (
            "rrule bad charset",
            json!({ "summary": "x", "dtstart": 1000, "rrule": "FREQ=DAILY:{oops}" }),
        ),
    ];

    for (label, body) in cases {
        let (status, value) = env
            .call(Method::POST, &base, Some(&token), Some(body))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "case: {label} -> {value}");
        assert_eq!(value["error"]["code"], "validation", "case: {label}");
        assert!(value["error"]["message"].is_string(), "case: {label}");
    }

    // Nothing was persisted by the rejected requests.
    let (_, list) = env.call(Method::GET, &base, Some(&token), None).await;
    assert_eq!(list.as_array().map(Vec::len), Some(0));

    // A valid event, then a patch regression on the same fields.
    let (status, created) = env
        .call(
            Method::POST,
            &base,
            Some(&token),
            Some(json!({ "summary": "keep", "dtstart": 1000 })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let uid = created["uid"].as_str().expect("uid").to_string();
    let one_uri = format!("{base}/{uid}");

    // Empty patch is a `400`.
    let (status, _) = env
        .call(Method::PATCH, &one_uri, Some(&token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Patch with an injected rrule is rejected too.
    let (status, value) = env
        .call(
            Method::PATCH,
            &one_uri,
            Some(&token),
            Some(json!({ "rrule": "FREQ=DAILY\r\nEND:VEVENT" })),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "patch injection -> {value}"
    );
    assert_eq!(value["error"]["code"], "validation");

    // The stored event is untouched by the rejected patch.
    let (_, one) = env.call(Method::GET, &one_uri, Some(&token), None).await;
    assert_eq!(one["rrule"], Value::Null);
}

/// F18 Pasos 15–17 — a `tzid` anchors an event to an IANA zone end to end: an
/// unknown zone is a `400`, a valid one is stored canonically and exposed, a
/// `PATCH` that only changes the zone is accepted, and the ms instants are
/// untouched throughout.
#[tokio::test]
async fn events_tzid_is_validated_canonicalised_and_exposed() {
    let env = TestEnv::new("events-tzid").await;
    let token = env.token("user-a");
    let cid = calendar_collection_id(&env, &token, "A").await;
    let base = format!("/api/v1/collections/{cid}/events");

    // An unknown zone is rejected with the uniform `400` envelope.
    let (status, value) = env
        .call(
            Method::POST,
            &base,
            Some(&token),
            Some(json!({
                "summary": "x",
                "dtstart": 1_700_000_000_000_i64,
                "tzid": "Mars/Olympus",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "unknown tzid -> {value}");
    assert_eq!(value["error"]["code"], "validation");

    // A valid zone is stored (canonically) and exposed; the instants are the
    // same UTC milliseconds as before.
    let (status, created) = env
        .call(
            Method::POST,
            &base,
            Some(&token),
            Some(json!({
                "summary": "Reunión",
                "dtstart": 1_700_000_000_000_i64,
                "dtend": 1_700_000_600_000_i64,
                "tzid": "Europe/Madrid",
                "uid": "evt-tz-1",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {created}");
    assert_eq!(created["dtstart_tzid"], "Europe/Madrid");
    assert_eq!(created["dtend_tzid"], "Europe/Madrid");
    assert_eq!(created["dtstart"], 1_700_000_000_000_i64);
    assert_eq!(created["dtend"], 1_700_000_600_000_i64);

    let one_uri = format!("{base}/evt-tz-1");
    let (status, one) = env.call(Method::GET, &one_uri, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(one["dtstart_tzid"], "Europe/Madrid");
    assert_eq!(one["dtstart"], 1_700_000_000_000_i64);

    // A `PATCH` that only changes the zone must be accepted (it is not "empty")
    // and replaces both anchors.
    let (status, patched) = env
        .call(
            Method::PATCH,
            &one_uri,
            Some(&token),
            Some(json!({ "tzid": "Asia/Tokyo" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "tzid-only patch failed: {patched}");
    assert_eq!(patched["dtstart_tzid"], "Asia/Tokyo");
    assert_eq!(patched["dtend_tzid"], "Asia/Tokyo");
    assert_eq!(
        patched["dtstart"], 1_700_000_000_000_i64,
        "a zone change must not move the instant"
    );

    // An invalid zone on `PATCH` is a `400` that leaves the event untouched.
    let (status, value) = env
        .call(
            Method::PATCH,
            &one_uri,
            Some(&token),
            Some(json!({ "tzid": "Mars/Olympus" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "bad patch tzid -> {value}");
    let (_, after) = env.call(Method::GET, &one_uri, Some(&token), None).await;
    assert_eq!(after["dtstart_tzid"], "Asia/Tokyo");
}

/// F18 review — an absurd instant (`i64` extremes) is rejected with the uniform
/// `400` instead of being handed to the composition path, with or without a
/// `tzid`.
#[tokio::test]
async fn events_reject_out_of_range_instants() {
    let env = TestEnv::new("events-range").await;
    let token = env.token("user-a");
    let cid = calendar_collection_id(&env, &token, "A").await;
    let base = format!("/api/v1/collections/{cid}/events");

    let cases = [
        (
            "dtstart i64::MAX",
            json!({ "summary": "x", "dtstart": i64::MAX }),
        ),
        (
            "dtstart i64::MIN",
            json!({ "summary": "x", "dtstart": i64::MIN }),
        ),
        (
            "dtend absurd",
            json!({ "summary": "x", "dtstart": 1_700_000_000_000_i64, "dtend": i64::MAX }),
        ),
        (
            "dtstart absurd with tzid",
            json!({ "summary": "x", "dtstart": i64::MAX, "tzid": "Europe/Madrid" }),
        ),
    ];

    for (label, body) in cases {
        let (status, value) = env
            .call(Method::POST, &base, Some(&token), Some(body))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "case: {label} -> {value}");
        assert_eq!(value["error"]["code"], "validation", "case: {label}");
    }

    // Nothing was persisted by the rejected requests.
    let (_, list) = env.call(Method::GET, &base, Some(&token), None).await;
    assert_eq!(list.as_array().map(Vec::len), Some(0));
}

/// F18 Paso 17 — regression: without `tzid` the event keeps the pre-F18
/// behaviour — the anchors are `null` and the composed `.ics` carries no
/// `VTIMEZONE` block.
#[tokio::test]
async fn events_without_tzid_keep_the_utc_behaviour() {
    let env = TestEnv::new("events-no-tzid").await;
    let token = env.token("user-a");
    let cid = calendar_collection_id(&env, &token, "A").await;
    let base = format!("/api/v1/collections/{cid}/events");

    let (status, created) = env
        .call(
            Method::POST,
            &base,
            Some(&token),
            Some(json!({
                "summary": "Sin zona",
                "dtstart": 1_700_000_000_000_i64,
                "uid": "evt-no-tz",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {created}");
    assert_eq!(created["dtstart_tzid"], Value::Null);
    assert_eq!(created["dtend_tzid"], Value::Null);

    let stored = get_calendar_object(&env.pool, &cid, "evt-no-tz")
        .await
        .expect("query")
        .expect("persisted");
    assert!(stored.dtstart_tzid.is_none());
    assert!(
        !stored.raw_payload.contains("VTIMEZONE"),
        "no zone anchor must mean no VTIMEZONE: {}",
        stored.raw_payload
    );
}

/// Tarea 4 — a collection that is missing, foreign or not a `calendar` is a
/// `404` (address books are never leaked).
#[tokio::test]
async fn events_require_a_calendar_collection() {
    let env = TestEnv::new("events-collection").await;
    let token = env.token("user-a");

    let book = env
        .create(&token, collection_body("Book", "addressbook", None))
        .await;
    let bid = book["id"].as_str().expect("id").to_string();
    let book_base = format!("/api/v1/collections/{bid}/events");
    let (status, value) = env.call(Method::GET, &book_base, Some(&token), None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "address book must be invisible"
    );
    assert_eq!(value["error"]["code"], "not_found");
    let (status, _) = env
        .call(
            Method::POST,
            &book_base,
            Some(&token),
            Some(json!({ "summary": "x", "dtstart": 1 })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let missing = "/api/v1/collections/00000000-0000-4000-8000-000000000000/events";
    let (status, _) = env.call(Method::GET, missing, Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Tarea 6 — no/invalid session cookie → `401` with the uniform envelope.
#[tokio::test]
async fn events_require_authentication() {
    let env = TestEnv::new("events-401").await;
    let base = "/api/v1/collections/whatever/events";
    let one = "/api/v1/collections/whatever/events/uid";

    let cases = [
        (Method::GET, base),
        (Method::POST, base),
        (Method::GET, one),
        (Method::PATCH, one),
        (Method::DELETE, one),
    ];
    for (method, uri) in cases {
        let (status, value) = env.call(method, uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "uri: {uri}");
        assert_eq!(value["error"]["code"], "unauthorized", "uri: {uri}");
    }

    let (status, _) = env.call(Method::GET, base, Some("not-a-jwt"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Mayor — the optional `[start, end)` window filters by overlap, `limit` /
/// `offset` paginate with a documented clamp, and a bad window is a `400`.
#[tokio::test]
async fn events_range_filter_and_pagination() {
    let env = TestEnv::new("events-range").await;
    let token = env.token("user-a");
    let cid = calendar_collection_id(&env, &token, "Range").await;
    let base = format!("/api/v1/collections/{cid}/events");

    // Pinned uids make the `ORDER BY uid ASC` listing deterministic.
    let seeds = [
        ("a", 1_000_i64, Some(2_000_i64)),
        ("b", 5_000_i64, Some(6_000_i64)),
        ("c", 10_000_i64, None),
    ];
    for (uid, start, end) in seeds {
        let mut body = json!({ "summary": uid, "dtstart": start, "uid": uid });
        if let Some(end) = end {
            body["dtend"] = json!(end);
        }
        let (status, value) = env
            .call(Method::POST, &base, Some(&token), Some(body))
            .await;
        assert_eq!(status, StatusCode::CREATED, "seed {uid}: {value}");
    }

    // Unfiltered: everything.
    let (status, all) = env.call(Method::GET, &base, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(all.as_array().map(Vec::len), Some(3));

    // Window overlapping `a` (1000..2000) and `b` (5000..6000), not `c`.
    let (status, win) = env
        .call(
            Method::GET,
            &format!("{base}?start=1500&end=5500"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let uids: Vec<&str> = win
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|item| item["uid"].as_str())
        .collect();
    assert_eq!(uids, vec!["a", "b"], "overlap window: {win}");

    // A window that touches nothing.
    let (_, win) = env
        .call(
            Method::GET,
            &format!("{base}?start=2500&end=4500"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(win.as_array().map(Vec::len), Some(0));

    // A point event (`c`, no DTEND) inside a later window is included.
    let (_, win) = env
        .call(
            Method::GET,
            &format!("{base}?start=9000&end=11000"),
            Some(&token),
            None,
        )
        .await;
    let items = win.as_array().expect("array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["uid"], "c");

    // Pagination: `order by uid`, offset 1 picks `b`.
    let (status, page) = env
        .call(
            Method::GET,
            &format!("{base}?limit=1&offset=1"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let items = page.as_array().expect("array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["uid"], "b");

    // Oversized limit is clamped (not rejected) and still a bare array.
    let (status, page) = env
        .call(
            Method::GET,
            &format!("{base}?limit=100000"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page.as_array().map(Vec::len), Some(3));

    // Non-numeric pagination is a uniform `400`.
    let (status, value) = env
        .call(
            Method::GET,
            &format!("{base}?limit=abc"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"]["code"], "validation");

    // Non-numeric range bound is a uniform `400`.
    let (status, value) = env
        .call(
            Method::GET,
            &format!("{base}?start=nope"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"]["code"], "validation");

    // An inverted window is rejected.
    let (status, _) = env
        .call(
            Method::GET,
            &format!("{base}?start=5000&end=1000"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// F24 (PLAN-028) — `PATCH` preserves the stored `.ics`; `LOCATION`/`URL`
// ---------------------------------------------------------------------------

/// A realistic `VCALENDAR` from a native client: it carries a `VALARM` (with its
/// own `SUMMARY`), two `ATTENDEE`, `CATEGORIES`, `LOCATION`, `DESCRIPTION` and an
/// `X-` property. As of F25 `CATEGORIES`/`LOCATION`/`DESCRIPTION` are modelled;
/// the `VALARM`, the `ATTENDEE`s and the `X-` property are the genuinely
/// unmodelled content a `PATCH` must preserve.
fn rich_ics(uid: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//oxidav//test//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:{uid}\r\n\
         DTSTAMP:20240101T000000Z\r\n\
         SUMMARY:Original\r\n\
         DESCRIPTION:Linea uno\r\n\
         LOCATION:Sala 3\r\n\
         CATEGORIES:trabajo,personal\r\n\
         X-CUSTOM:keep-me\r\n\
         ATTENDEE;CN=Ada:mailto:ada@example.org\r\n\
         ATTENDEE;CN=Alan:mailto:alan@example.org\r\n\
         BEGIN:VALARM\r\n\
         ACTION:DISPLAY\r\n\
         TRIGGER:-PT15M\r\n\
         SUMMARY:Recordatorio\r\n\
         END:VALARM\r\n\
         DTSTART:20240315T100000Z\r\n\
         DTEND:20240315T110000Z\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    )
}

/// A minimal `VCALENDAR` whose event carries a `URL` line, used to prove the
/// verbatim emission and the scheme allowlist at the API boundary.
fn ics_with_url(uid: &str, url: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//oxidav//test//EN\r\n\
         BEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:Original\r\n\
         LOCATION:Sala 3\r\nURL:{url}\r\n\
         DTSTART:20240315T100000Z\r\nDTEND:20240315T110000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

/// Seed a calendar object through the repository (bypassing the DAV `PUT`) so a
/// test can set the stored `raw_payload` and the denormalised columns
/// **independently** — the only way to reproduce a legacy row whose `location`
/// column is `NULL` while its payload still carries `LOCATION` (E14).
async fn seed_event(
    env: &TestEnv,
    collection_id: &str,
    id: &str,
    uid: &str,
    location: Option<&str>,
    url: Option<&str>,
    raw: &str,
) {
    insert_calendar_object(
        &env.pool,
        &NewCalendarObject {
            id: id.to_string(),
            collection_id: collection_id.to_string(),
            uid: uid.to_string(),
            summary: Some("Original".to_string()),
            description: None,
            location: location.map(str::to_string),
            url: url.map(str::to_string),
            dtstart: Some(1_710_496_800_000),
            dtend: Some(1_710_497_200_000),
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            raw_payload: raw.to_string(),
        },
    )
    .await
    .expect("seed event");
}

/// Read the stored `raw_payload` of a calendar event.
async fn stored_event_raw(env: &TestEnv, collection_id: &str, uid: &str) -> String {
    get_calendar_object(&env.pool, collection_id, uid)
        .await
        .expect("read event")
        .expect("event exists")
        .raw_payload
}

/// E1 — the flag test: a `PATCH` changing only the title keeps every **unmodelled**
/// property of a rich `.ics` (`VALARM`, both `ATTENDEE`, `X-`) and the modelled
/// `LOCATION` byte for byte, refreshes `DTSTAMP` and rewrites only those two
/// lines. As of F25 `CATEGORIES` is **modelled** (it is reconciled like the other
/// modelled fields), so until the API inherits it from the payload a `PATCH` that
/// does not mention it drops the line; that inheritance is a later step of
/// PLAN-029, not this one.
#[tokio::test]
async fn patch_preserves_unmodelled_properties_of_a_rich_ics() {
    let env = TestEnv::new("f24-preserve-rich").await;
    let token = env.token("user-a");
    let cid = calendar_collection_id(&env, &token, "A").await;
    let uid = "rich-1@oxidav";
    let original = rich_ics(uid);
    seed_event(&env, &cid, "evt-rich", uid, Some("Sala 3"), None, &original).await;

    let uri = format!("/api/v1/collections/{cid}/events/{uid}");
    let (status, body) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({ "summary": "Nuevo" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "patch failed: {body}");
    assert_eq!(body["summary"], "Nuevo");
    assert_eq!(body["location"], "Sala 3");

    let stored = stored_event_raw(&env, &cid, uid).await;
    for needle in [
        "BEGIN:VALARM",
        "SUMMARY:Recordatorio",
        "END:VALARM",
        "ATTENDEE;CN=Ada:mailto:ada@example.org",
        "ATTENDEE;CN=Alan:mailto:alan@example.org",
        "X-CUSTOM:keep-me",
        "LOCATION:Sala 3",
        "DESCRIPTION:Linea uno",
        "SUMMARY:Nuevo",
    ] {
        assert!(stored.contains(needle), "missing {needle:?}: {stored}");
    }
    // F25: `CATEGORIES` is modelled now, so until the API inherits it from the
    // payload (a later PLAN-029 step) a `PATCH` drops the line. This test keeps
    // promising only what is genuinely unmodelled, plus the modelled `LOCATION`.
    assert!(
        !stored.contains("CATEGORIES:"),
        "F25: CATEGORIES is modelled and dropped until the API inherits it: {stored}"
    );
    assert!(
        !stored.contains("DTSTAMP:20240101T000000Z"),
        "DTSTAMP must be refreshed: {stored}"
    );

    // Only the SUMMARY and DTSTAMP lines differ, line for line: everything else
    // (the whole VALARM, the second ATTENDEE, the folding) is byte-identical. The
    // F25-modelled `CATEGORIES` line is set aside from both sides, since a later
    // step will make the API inherit it and restore it here.
    let before: Vec<&str> = original
        .lines()
        .filter(|line| !line.starts_with("CATEGORIES:"))
        .collect();
    let after: Vec<&str> = stored
        .lines()
        .filter(|line| !line.starts_with("CATEGORIES:"))
        .collect();
    assert_eq!(before.len(), after.len(), "line count changed:\n{stored}");
    let diffs: Vec<&str> = before
        .iter()
        .zip(&after)
        .filter(|(before, after)| before != after)
        .map(|(before, _)| *before)
        .collect();
    assert_eq!(diffs.len(), 2, "only SUMMARY/DTSTAMP may change:\n{stored}");
    assert!(
        diffs.iter().any(|line| line.starts_with("SUMMARY:")),
        "{diffs:?}"
    );
    assert!(
        diffs.iter().any(|line| line.starts_with("DTSTAMP:")),
        "{diffs:?}"
    );
}

/// E14 — the D1 rule: a legacy row whose `location` column is `NULL` but whose
/// payload carries `LOCATION:Sala 3` (a row written before migration `0005`)
/// keeps its location across a `PATCH` that does not mention it, and the DTO
/// reports it.
#[tokio::test]
async fn patch_on_a_legacy_row_keeps_the_location_of_its_payload() {
    let env = TestEnv::new("f24-legacy-location").await;
    let token = env.token("user-a");
    let cid = calendar_collection_id(&env, &token, "A").await;
    let uid = "legacy-1@oxidav";
    // `location` column NULL, payload carries `LOCATION`.
    let original = rich_ics(uid);
    seed_event(&env, &cid, "evt-legacy", uid, None, None, &original).await;

    let uri = format!("/api/v1/collections/{cid}/events/{uid}");
    let (status, body) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({ "summary": "Renombrado" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "patch failed: {body}");
    assert_eq!(
        body["location"], "Sala 3",
        "the column was NULL but the payload had LOCATION"
    );

    let stored = stored_event_raw(&env, &cid, uid).await;
    assert!(
        stored.contains("LOCATION:Sala 3"),
        "LOCATION lost: {stored}"
    );

    // The column is healed to agree with the reconciled payload.
    let row = get_calendar_object(&env.pool, &cid, uid)
        .await
        .expect("read event")
        .expect("event exists");
    assert_eq!(row.location.as_deref(), Some("Sala 3"));
}

/// E3 — a blank `location` clears the field: `200`, the DTO reports `null` and
/// no `LOCATION:` line survives in the payload.
#[tokio::test]
async fn patch_with_a_blank_location_clears_it() {
    let env = TestEnv::new("f24-clear-location").await;
    let token = env.token("user-a");
    let cid = calendar_collection_id(&env, &token, "A").await;
    let uid = "clr-1@oxidav";
    seed_event(
        &env,
        &cid,
        "evt-clr",
        uid,
        Some("Sala 3"),
        None,
        &rich_ics(uid),
    )
    .await;

    let uri = format!("/api/v1/collections/{cid}/events/{uid}");
    let (status, body) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({ "location": "" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "patch failed: {body}");
    assert_eq!(body["location"], Value::Null, "location must be cleared");

    let stored = stored_event_raw(&env, &cid, uid).await;
    assert!(
        !stored.contains("LOCATION:"),
        "a LOCATION line survived: {stored}"
    );
}

/// E10 — `POST` with `location` and `url`: `201`, the DTO carries both, the
/// payload escapes the free-text `LOCATION` and emits the `URL` verbatim.
#[tokio::test]
async fn create_with_location_and_url() {
    let env = TestEnv::new("f24-create-loc-url").await;
    let token = env.token("user-a");
    let cid = calendar_collection_id(&env, &token, "A").await;
    let base = format!("/api/v1/collections/{cid}/events");

    let (status, created) = env
        .call(
            Method::POST,
            &base,
            Some(&token),
            Some(json!({
                "summary": "Con sitio",
                "dtstart": 1_710_496_800_000_i64,
                "dtend": 1_710_497_200_000_i64,
                "uid": "loc-url-1",
                "location": "Sala 3; planta 2, ala \\norte",
                "url": "https://ejemplo.org/a?x=1&y=2,3",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {created}");
    assert_eq!(created["location"], "Sala 3; planta 2, ala \\norte");
    assert_eq!(created["url"], "https://ejemplo.org/a?x=1&y=2,3");

    let stored = stored_event_raw(&env, &cid, "loc-url-1").await;
    // Free text: `;`, `,` and `\` are escaped on the wire.
    assert!(
        stored.contains("LOCATION:Sala 3\\; planta 2\\, ala \\\\norte"),
        "LOCATION not escaped: {stored}"
    );
    // A URI: emitted verbatim, its comma untouched.
    assert!(
        stored.contains("URL:https://ejemplo.org/a?x=1&y=2,3"),
        "URL not verbatim: {stored}"
    );
}

/// E5 — the `url` scheme allowlist: a hostile scheme is a `400` and the stored
/// payload does not change; `https://` is accepted and emitted verbatim.
#[tokio::test]
async fn url_scheme_allowlist() {
    let env = TestEnv::new("f24-url-allowlist").await;
    let token = env.token("user-a");
    let cid = calendar_collection_id(&env, &token, "A").await;
    let uid = "url-1@oxidav";
    seed_event(
        &env,
        &cid,
        "evt-url",
        uid,
        Some("Sala 3"),
        Some("https://ok.example/x"),
        &ics_with_url(uid, "https://ok.example/x"),
    )
    .await;

    let uri = format!("/api/v1/collections/{cid}/events/{uid}");
    let before = stored_event_raw(&env, &cid, uid).await;

    for hostile in [
        "javascript:alert(1)",
        "data:text/html,x",
        "file:///etc/passwd",
        "https://x\ny",
    ] {
        let (status, body) = env
            .call(
                Method::PATCH,
                &uri,
                Some(&token),
                Some(json!({ "url": hostile })),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{hostile:?}: {body}");
        assert_eq!(body["error"]["code"], "validation", "{hostile:?}: {body}");
        assert_eq!(
            stored_event_raw(&env, &cid, uid).await,
            before,
            "payload changed for {hostile:?}"
        );
    }

    let (status, body) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({ "url": "https://ejemplo.org/x" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "valid url rejected: {body}");
    assert_eq!(body["url"], "https://ejemplo.org/x");
    let after = stored_event_raw(&env, &cid, uid).await;
    assert!(
        after.contains("URL:https://ejemplo.org/x"),
        "URL not stored verbatim: {after}"
    );
}

/// E9 (API half) — a DAV `PUT` of an `.ics` carrying `LOCATION`/`URL` is
/// denormalised, and the REST `GET` reports both.
#[tokio::test]
async fn put_then_api_get_returns_location_and_url() {
    let env = TestEnv::new("f24-put-api-get").await;
    let token = env.token("user-a");
    let cid = calendar_collection_id(&env, &token, "A").await;

    let uid = "dav-lu-1@oxidav";
    let ics = ics_with_url(uid, "https://ejemplo.org/x");
    let request = Request::builder()
        .method(Method::PUT)
        .uri(format!("/dav/calendars/user-a/{cid}/{uid}.ics"))
        .header(header::COOKIE, format!("{SESSION_COOKIE_NAME}={token}"))
        .header(header::CONTENT_TYPE, "text/calendar; charset=utf-8")
        // A cookie-authenticated, state-changing DAV request must prove
        // same-origin (F17 hardening).
        .header("sec-fetch-site", "same-origin")
        .body(Body::from(ics))
        .expect("build PUT");
    let response = env.app.clone().oneshot(request).await.expect("send PUT");
    assert_eq!(response.status(), StatusCode::CREATED);

    let uri = format!("/api/v1/collections/{cid}/events/{uid}");
    let (status, body) = env.call(Method::GET, &uri, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "get failed: {body}");
    assert_eq!(body["location"], "Sala 3");
    assert_eq!(body["url"], "https://ejemplo.org/x");
}

// ---------------------------------------------------------------------------
// F12 — Contact REST endpoints (`/api/v1/collections/{id}/contacts`)
// ---------------------------------------------------------------------------

/// Create an `addressbook` collection for `token` and return its id.
async fn addressbook_collection_id(env: &TestEnv, token: &str, name: &str) -> String {
    let created = env
        .create(token, collection_body(name, "addressbook", None))
        .await;
    created["id"].as_str().expect("collection id").to_string()
}

/// Tareas 2–5 — scoped CRUD: create (`201`), list (`200`), read (`200`),
/// patch (`200`, new ETag, recomposed `.vcf`), delete (`204`); user B never
/// reaches user A's address book (anti-IDOR); `raw_payload` is never exposed.
#[tokio::test]
async fn contacts_crud_is_scoped_and_hides_raw_payload() {
    let env = TestEnv::new("contacts-crud").await;
    let token_a = env.token("user-a");
    let token_b = env.token("user-b");
    let cid = addressbook_collection_id(&env, &token_a, "A").await;
    let base = format!("/api/v1/collections/{cid}/contacts");

    // Create.
    let (status, created) = env
        .call(
            Method::POST,
            &base,
            Some(&token_a),
            Some(json!({
                "fn": "Ada Lovelace",
                "email": "ada@example.com",
                "phone": "+34600000000",
                "organization": "Analytical",
                "uid": "contact-uid-1",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {created}");
    assert_eq!(created["uid"], "contact-uid-1");
    assert_eq!(created["collection_id"], cid);
    assert_eq!(created["fn"], "Ada Lovelace");
    assert_eq!(created["full_name"], "Ada Lovelace");
    assert_eq!(created["email"], "ada@example.com");
    assert_eq!(created["phone"], "+34600000000");
    assert_eq!(created["organization"], "Analytical");
    assert!(created["etag"].as_str().is_some_and(|e| !e.is_empty()));
    assert!(
        created.get("raw_payload").is_none(),
        "raw_payload must not be exposed"
    );
    assert!(
        !created.to_string().contains("BEGIN:VCARD"),
        "raw .vcf leaked"
    );
    let etag_before = created["etag"].as_str().expect("etag").to_string();

    // List.
    let (status, list) = env.call(Method::GET, &base, Some(&token_a), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = list.as_array().expect("array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["uid"], "contact-uid-1");
    assert!(items[0].get("raw_payload").is_none());
    assert!(!list.to_string().contains("BEGIN:VCARD"));

    // Read one.
    let one_uri = format!("{base}/contact-uid-1");
    let (status, one) = env.call(Method::GET, &one_uri, Some(&token_a), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(one["uid"], "contact-uid-1");
    assert!(one.get("raw_payload").is_none());

    // Patch: the `.vcf` is recomposed and a fresh ETag is written.
    let (status, patched) = env
        .call(
            Method::PATCH,
            &one_uri,
            Some(&token_a),
            Some(json!({ "fn": "Ada King" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "patch failed: {patched}");
    assert_eq!(patched["fn"], "Ada King");
    assert_eq!(
        patched["email"], "ada@example.com",
        "email must survive a partial patch"
    );
    assert_ne!(
        patched["etag"].as_str(),
        Some(etag_before.as_str()),
        "the etag must change after an edit"
    );
    assert!(patched.get("raw_payload").is_none());

    // Anti-IDOR: user B cannot reach user A's address book at all.
    let (status, _) = env.call(Method::GET, &base, Some(&token_b), None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "B must not list A's contacts"
    );
    let (status, _) = env.call(Method::GET, &one_uri, Some(&token_b), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "B must not read A's contact");
    let (status, _) = env
        .call(
            Method::PATCH,
            &one_uri,
            Some(&token_b),
            Some(json!({ "fn": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "B must not edit A's contact");
    let (status, _) = env
        .call(Method::DELETE, &one_uri, Some(&token_b), None)
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "B must not delete A's contact"
    );

    // Delete as the owner: explicit `204`, then gone.
    let (status, body) = env
        .call(Method::DELETE, &one_uri, Some(&token_a), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null, "204 must have no body");
    let (status, _) = env.call(Method::GET, &one_uri, Some(&token_a), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Tarea 3 — invalid input is a uniform `400` (no `500`), including a CRLF /
/// control-character injection in any free-text field.
#[tokio::test]
async fn contacts_validation_rejects_bad_input() {
    let env = TestEnv::new("contacts-validation").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "A").await;
    let base = format!("/api/v1/collections/{cid}/contacts");

    let cases = [
        ("blank fn", json!({ "fn": "   " })),
        ("missing fn", json!({ "email": "a@b.com" })),
        ("fn CRLF injection", json!({ "fn": "Ada\r\nX-EVIL:1" })),
        ("fn control char", json!({ "fn": "Ada\u{0007}" })),
        (
            "email CRLF injection",
            json!({ "fn": "Ada", "email": "a@b.com\r\nX:1" }),
        ),
        (
            "organization newline",
            json!({ "fn": "Ada", "organization": "Acme\n_Injected" }),
        ),
        ("uid with slash", json!({ "fn": "Ada", "uid": "a/b" })),
    ];

    for (label, body) in cases {
        let (status, value) = env
            .call(Method::POST, &base, Some(&token), Some(body))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "case: {label} -> {value}");
        assert_eq!(value["error"]["code"], "validation", "case: {label}");
        assert!(value["error"]["message"].is_string(), "case: {label}");
    }

    // A malformed payload (unknown shape) is also a uniform `400`, never a `500`.
    let (status, value) = env
        .call(
            Method::POST,
            &base,
            Some(&token),
            Some(json!({ "unexpected": true })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"]["code"], "validation");

    // Nothing was persisted by the rejected requests.
    let (_, list) = env.call(Method::GET, &base, Some(&token), None).await;
    assert_eq!(list.as_array().map(Vec::len), Some(0));

    // A valid contact, then regression checks on `PATCH`.
    let (status, created) = env
        .call(
            Method::POST,
            &base,
            Some(&token),
            Some(json!({ "fn": "keep" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let uid = created["uid"].as_str().expect("uid").to_string();
    let one_uri = format!("{base}/{uid}");

    // Empty patch is a `400`.
    let (status, _) = env
        .call(Method::PATCH, &one_uri, Some(&token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Patch with an injected fn is rejected too.
    let (status, value) = env
        .call(
            Method::PATCH,
            &one_uri,
            Some(&token),
            Some(json!({ "fn": "Ada\r\nX-EVIL:1" })),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "patch injection -> {value}"
    );
    assert_eq!(value["error"]["code"], "validation");

    // The stored contact is untouched by the rejected patch.
    let (_, one) = env.call(Method::GET, &one_uri, Some(&token), None).await;
    assert_eq!(one["fn"], "keep");
}

/// Two `POST`s that pin the **same** `uid` in one address book: the second is a
/// `409 Conflict` with the uniform envelope (`error.code == "conflict"`), and
/// only one contact is stored.
#[tokio::test]
async fn contacts_duplicate_uid_is_a_conflict() {
    let env = TestEnv::new("contacts-conflict").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;
    let base = format!("/api/v1/collections/{cid}/contacts");
    let body = json!({ "fn": "Ada", "uid": "dup-uid" });

    let (status, created) = env
        .call(Method::POST, &base, Some(&token), Some(body.clone()))
        .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "first create failed: {created}"
    );
    assert_eq!(created["uid"], "dup-uid");

    let (status, value) = env
        .call(Method::POST, &base, Some(&token), Some(body))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "second create: {value}");
    assert_eq!(value["error"]["code"], "conflict");

    let (_, list) = env.call(Method::GET, &base, Some(&token), None).await;
    assert_eq!(list.as_array().map(Vec::len), Some(1));
}

/// Regression (M1): a contact whose **stored** `uid` carries a real newline (as
/// CardDAV may persist it) must not be recomposed into a `raw_payload` with an
/// injected content line. The `PATCH` fails with `500` and the stored payload is
/// left untouched.
#[tokio::test]
async fn contacts_patch_rejects_a_stored_uid_with_a_newline() {
    let env = TestEnv::new("contacts-stored-uid").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;

    let stored_payload =
        "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:eviluid\r\nFN:Evil\r\nEND:VCARD\r\n".to_string();
    insert_card_object(
        &env.pool,
        &NewCardObject {
            id: "card-evil".to_string(),
            collection_id: cid.clone(),
            uid: "evil\nX-INJECTED:1".to_string(),
            fn_: Some("Evil".to_string()),
            email: None,
            phone: None,
            organization: None,
            notes: None,
            raw_payload: stored_payload.clone(),
        },
    )
    .await
    .expect("seed a card with an invalid stored uid");

    // The `{uid}` path segment carries the same (percent-encoded) newline.
    let uri = format!("/api/v1/collections/{cid}/contacts/evil%0AX-INJECTED:1");
    let (status, value) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({ "fn": "Injected" })),
        )
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "patch: {value}");
    assert_eq!(value["error"]["code"], "internal");

    let raw: String = sqlx::query_scalar("SELECT raw_payload FROM card_objects WHERE id = ?1")
        .bind("card-evil")
        .fetch_one(&env.pool)
        .await
        .expect("read raw_payload");
    assert_eq!(raw, stored_payload, "the stored payload must be untouched");
    assert!(
        !raw.contains("X-INJECTED"),
        "an injected content line reached raw_payload: {raw}"
    );
}

/// Tarea 4 — a collection that is missing, foreign or not an `addressbook` is a
/// `404` (calendars are never leaked).
#[tokio::test]
async fn contacts_require_an_addressbook_collection() {
    let env = TestEnv::new("contacts-collection").await;
    let token = env.token("user-a");

    let cal = env
        .create(&token, collection_body("Cal", "calendar", None))
        .await;
    let cid = cal["id"].as_str().expect("id").to_string();
    let cal_base = format!("/api/v1/collections/{cid}/contacts");
    let (status, value) = env.call(Method::GET, &cal_base, Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "calendar must be invisible");
    assert_eq!(value["error"]["code"], "not_found");
    let (status, _) = env
        .call(
            Method::POST,
            &cal_base,
            Some(&token),
            Some(json!({ "fn": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let missing = "/api/v1/collections/00000000-0000-4000-8000-000000000000/contacts";
    let (status, _) = env.call(Method::GET, missing, Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Tarea 6 — no/invalid session cookie → `401` with the uniform envelope.
#[tokio::test]
async fn contacts_require_authentication() {
    let env = TestEnv::new("contacts-401").await;
    let base = "/api/v1/collections/whatever/contacts";
    let one = "/api/v1/collections/whatever/contacts/uid";

    let cases = [
        (Method::GET, base),
        (Method::POST, base),
        (Method::GET, one),
        (Method::PATCH, one),
        (Method::DELETE, one),
    ];
    for (method, uri) in cases {
        let (status, value) = env.call(method, uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "uri: {uri}");
        assert_eq!(value["error"]["code"], "unauthorized", "uri: {uri}");
    }

    let (status, _) = env.call(Method::GET, base, Some("not-a-jwt"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Tarea 7 — `limit` / `offset` paginate with the documented clamp, and a
/// non-numeric page parameter is a uniform `400`.
#[tokio::test]
async fn contacts_pagination_clamps() {
    let env = TestEnv::new("contacts-pagination").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;
    let base = format!("/api/v1/collections/{cid}/contacts");

    // Pinned uids make the `ORDER BY uid ASC` listing deterministic.
    for uid in ["a", "b", "c"] {
        let (status, value) = env
            .call(
                Method::POST,
                &base,
                Some(&token),
                Some(json!({ "fn": uid, "uid": uid })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "seed {uid}: {value}");
    }

    // Unfiltered: everything.
    let (status, all) = env.call(Method::GET, &base, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(all.as_array().map(Vec::len), Some(3));

    // `limit=1&offset=1` picks `b`.
    let (status, page) = env
        .call(
            Method::GET,
            &format!("{base}?limit=1&offset=1"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let items = page.as_array().expect("array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["uid"], "b");

    // Oversized limit is clamped (not rejected) and still a bare array.
    let (status, page) = env
        .call(
            Method::GET,
            &format!("{base}?limit=100000"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page.as_array().map(Vec::len), Some(3));

    // Non-numeric pagination is a uniform `400`.
    let (status, value) = env
        .call(
            Method::GET,
            &format!("{base}?limit=abc"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"]["code"], "validation");
}

/// Tarea 9 — a database failure surfaces as the uniform `500` envelope without
/// leaking internal detail.
#[tokio::test]
async fn contacts_error_envelope_leaks_nothing() {
    let env = TestEnv::new("contacts-envelope").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;

    env.pool.close().await;
    let (status, value) = env
        .call(
            Method::GET,
            &format!("/api/v1/collections/{cid}/contacts"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(value["error"]["code"], "internal");
    assert_eq!(value["error"]["message"], "Error interno del servidor");
    let rendered = value.to_string();
    assert!(
        !rendered.contains("database") && !rendered.contains("Query"),
        "internal detail leaked: {rendered}"
    );
}

// ---------------------------------------------------------------------------
// F13 — App token REST endpoints (`/api/v1/app-tokens`)
// ---------------------------------------------------------------------------

/// `POST /api/v1/app-tokens` body.
fn app_token_body(name: &str) -> Value {
    json!({ "name": name })
}

/// Tareas 1–3 — the listing is scoped to the caller, exposes only metadata, and
/// never leaks the secret or its digest. Anti-IDOR: user B's token is invisible
/// to user A.
#[tokio::test]
async fn app_tokens_list_is_scoped_and_hides_secret() {
    let env = TestEnv::new("app-tokens-list").await;
    let token_a = env.token("user-a");
    let token_b = env.token("user-b");

    let mut own_ids = Vec::new();
    for name in ["iPhone", "Thunderbird"] {
        let (status, created) = env
            .call(
                Method::POST,
                "/api/v1/app-tokens",
                Some(&token_a),
                Some(app_token_body(name)),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "create failed: {created}");
        own_ids.push(created["id"].as_str().expect("id").to_string());
    }

    let (status, foreign) = env
        .call(
            Method::POST,
            "/api/v1/app-tokens",
            Some(&token_b),
            Some(app_token_body("Foreign")),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let foreign_id = foreign["id"].as_str().expect("id").to_string();
    let foreign_secret = foreign["token"].as_str().expect("token").to_string();

    // A sees exactly its own two tokens.
    let (status, list) = env
        .call(Method::GET, "/api/v1/app-tokens", Some(&token_a), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let items = list.as_array().expect("array");
    assert_eq!(items.len(), 2, "A must see only its own tokens");
    for item in items {
        assert!(item["id"].is_string());
        assert!(item["name"].is_string());
        assert!(
            item["created_at"]
                .as_str()
                .is_some_and(|s| s.ends_with('Z')),
            "created_at must be ISO-8601 UTC: {item}"
        );
        assert!(item["last_used_at"].is_null(), "never used -> null");
        assert!(item.get("token").is_none(), "token must not be listed");
        assert!(
            item.get("secret_hash").is_none(),
            "secret_hash must not be listed"
        );
    }
    let ids: Vec<&str> = items.iter().filter_map(|i| i["id"].as_str()).collect();
    assert!(own_ids.iter().all(|id| ids.contains(&id.as_str())));
    assert!(!ids.contains(&foreign_id.as_str()), "B's id leaked into A");
    let rendered = list.to_string();
    assert!(
        !rendered.contains(&foreign_secret) && !rendered.contains("oxidav_"),
        "the secret leaked into the listing: {rendered}"
    );

    // B sees exactly its own one token.
    let (_, list_b) = env
        .call(Method::GET, "/api/v1/app-tokens", Some(&token_b), None)
        .await;
    let items_b = list_b.as_array().expect("array");
    assert_eq!(items_b.len(), 1);
    assert_eq!(items_b[0]["id"], foreign_id);
}

/// Tarea 3 — creation returns the full token `oxidav_<id>_<secret>` exactly
/// once, and the returned value authenticates.
#[tokio::test]
async fn app_tokens_create_returns_secret_once_and_verifies() {
    let env = TestEnv::new("app-tokens-create").await;
    let token = env.token("user-a");

    let (status, created) = env
        .call(
            Method::POST,
            "/api/v1/app-tokens",
            Some(&token),
            Some(app_token_body("  iPhone  ")),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {created}");
    assert_eq!(created["name"], "iPhone", "name must be trimmed");
    assert!(
        created["created_at"]
            .as_str()
            .is_some_and(|s| s.ends_with('Z')),
        "created_at must be ISO-8601 UTC"
    );

    let id = created["id"].as_str().expect("id");
    let full = created["token"].as_str().expect("token");
    assert!(full.starts_with("oxidav_"), "token shape: {full}");
    assert!(
        full.starts_with(&format!("oxidav_{id}_")),
        "token embeds id"
    );

    // The clear-text secret works against the F05 verifier.
    let user = verify_app_token(&env.pool, full).await.expect("verify");
    assert_eq!(user.sub, "user-a");

    // The listing never repeats the secret.
    let (_, list) = env
        .call(Method::GET, "/api/v1/app-tokens", Some(&token), None)
        .await;
    let rendered = list.to_string();
    assert!(!rendered.contains(full), "secret re-exposed in listing");
    assert!(
        !rendered.contains("oxidav_"),
        "token prefix leaked: {rendered}"
    );
}

/// Tarea 4 — invalid payloads are a uniform `400`; a missing/invalid session
/// cookie is a `401`; the error envelope is `{error:{code,message}}`.
#[tokio::test]
async fn app_tokens_validation_and_unauthorized() {
    let env = TestEnv::new("app-tokens-validation").await;
    let token = env.token("user-a");

    for (label, body) in [
        ("blank name", app_token_body("   ")),
        ("missing name", json!({})),
        ("wrong type", json!({ "name": 42 })),
    ] {
        let (status, value) = env
            .call(Method::POST, "/api/v1/app-tokens", Some(&token), Some(body))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "case: {label}");
        assert_eq!(value["error"]["code"], "validation", "case: {label}");
        let keys = value["error"].as_object().expect("error object").len();
        assert_eq!(keys, 2, "only code and message are exposed");
    }

    // Nothing was persisted by the rejected requests.
    let (_, list) = env
        .call(Method::GET, "/api/v1/app-tokens", Some(&token), None)
        .await;
    assert_eq!(list.as_array().map(Vec::len), Some(0));

    // No/invalid token → 401 with the uniform envelope on every verb.
    for (method, uri) in [
        (Method::GET, "/api/v1/app-tokens"),
        (Method::POST, "/api/v1/app-tokens"),
        (Method::PATCH, "/api/v1/app-tokens/some-id"),
        (Method::DELETE, "/api/v1/app-tokens/some-id"),
    ] {
        let (status, value) = env.call(method, uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "uri: {uri}");
        assert_eq!(value["error"]["code"], "unauthorized", "uri: {uri}");
    }
    let (status, _) = env
        .call(Method::GET, "/api/v1/app-tokens", Some("not-a-jwt"), None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Tareas 2 & 5 — revocation is scoped and not idempotent at the API boundary:
/// revoking one's own token is `204` (and it stops verifying); repeating it is a
/// `404`; a foreign or unknown id is a `404` and leaves the other user's token
/// intact.
#[tokio::test]
async fn app_tokens_revoke_is_scoped() {
    let env = TestEnv::new("app-tokens-revoke").await;
    let token_a = env.token("user-a");
    let token_b = env.token("user-b");

    // A's own token.
    let (_, created_a) = env
        .call(
            Method::POST,
            "/api/v1/app-tokens",
            Some(&token_a),
            Some(app_token_body("A-phone")),
        )
        .await;
    let id_a = created_a["id"].as_str().expect("id").to_string();
    let raw_a = created_a["token"].as_str().expect("token").to_string();

    // B's token, which A must never be able to revoke.
    let (_, created_b) = env
        .call(
            Method::POST,
            "/api/v1/app-tokens",
            Some(&token_b),
            Some(app_token_body("B-phone")),
        )
        .await;
    let id_b = created_b["id"].as_str().expect("id").to_string();
    let raw_b = created_b["token"].as_str().expect("token").to_string();

    // Anti-IDOR: A cannot revoke B's token.
    let (status, value) = env
        .call(
            Method::DELETE,
            &format!("/api/v1/app-tokens/{id_b}"),
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "A revoked B's token! {value}"
    );
    assert!(
        verify_app_token(&env.pool, &raw_b).await.is_ok(),
        "B's token must survive A"
    );

    // Unknown id → 404.
    let (status, _) = env
        .call(
            Method::DELETE,
            "/api/v1/app-tokens/deadbeefdeadbeefdeadbeefdeadbeef",
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Owner revokes: 204, no body, and the token stops verifying.
    let (status, body) = env
        .call(
            Method::DELETE,
            &format!("/api/v1/app-tokens/{id_a}"),
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null, "204 must have no body");
    assert!(matches!(
        verify_app_token(&env.pool, &raw_a).await,
        Err(TokenError::Revoked)
    ));

    // Revoked tokens disappear from the listing.
    let (_, list) = env
        .call(Method::GET, "/api/v1/app-tokens", Some(&token_a), None)
        .await;
    assert_eq!(list.as_array().map(Vec::len), Some(0));

    // A second delete on the now-revoked token is an explicit 404.
    let (status, _) = env
        .call(
            Method::DELETE,
            &format!("/api/v1/app-tokens/{id_a}"),
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Tarea 6 — renaming is scoped and returns only metadata; a blank name is a
/// `400`; an unknown, foreign or revoked id is a `404`.
#[tokio::test]
async fn app_tokens_rename_is_scoped() {
    let env = TestEnv::new("app-tokens-rename").await;
    let token_a = env.token("user-a");
    let token_b = env.token("user-b");

    let (_, created_a) = env
        .call(
            Method::POST,
            "/api/v1/app-tokens",
            Some(&token_a),
            Some(app_token_body("Old")),
        )
        .await;
    let id_a = created_a["id"].as_str().expect("id").to_string();

    let (_, created_b) = env
        .call(
            Method::POST,
            "/api/v1/app-tokens",
            Some(&token_b),
            Some(app_token_body("B")),
        )
        .await;
    let id_b = created_b["id"].as_str().expect("id").to_string();

    // Rename own: 200 with updated metadata only.
    let uri = format!("/api/v1/app-tokens/{id_a}");
    let (status, renamed) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token_a),
            Some(app_token_body("New")),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "rename failed: {renamed}");
    assert_eq!(renamed["name"], "New");
    assert_eq!(renamed["id"], id_a);
    assert!(renamed.get("token").is_none());
    assert!(renamed.get("secret_hash").is_none());

    // The listing reflects the new name.
    let (_, list) = env
        .call(Method::GET, "/api/v1/app-tokens", Some(&token_a), None)
        .await;
    assert_eq!(list[0]["name"], "New");

    // Blank name → 400.
    let (status, value) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token_a),
            Some(app_token_body("  ")),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"]["code"], "validation");

    // Unknown id → 404.
    let (status, _) = env
        .call(
            Method::PATCH,
            "/api/v1/app-tokens/deadbeefdeadbeefdeadbeefdeadbeef",
            Some(&token_a),
            Some(app_token_body("x")),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Anti-IDOR: A cannot rename B's token.
    let (status, _) = env
        .call(
            Method::PATCH,
            &format!("/api/v1/app-tokens/{id_b}"),
            Some(&token_a),
            Some(app_token_body("hijacked")),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A revoked token cannot be renamed either.
    let (status, _) = env.call(Method::DELETE, &uri, Some(&token_a), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token_a),
            Some(app_token_body("after-revoke")),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Tarea 4/9 — a database failure surfaces as the uniform `500` envelope and
/// leaks nothing internal.
#[tokio::test]
async fn app_tokens_error_envelope_leaks_nothing() {
    let env = TestEnv::new("app-tokens-envelope").await;
    let token = env.token("user-a");

    env.pool.close().await;
    let (status, value) = env
        .call(Method::GET, "/api/v1/app-tokens", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(value["error"]["code"], "internal");
    assert_eq!(value["error"]["message"], "Error interno del servidor");
    let rendered = value.to_string();
    assert!(
        !rendered.contains("database") && !rendered.contains("Query"),
        "internal detail leaked: {rendered}"
    );
}

// ---------------------------------------------------------------------------
// F22 — full vCard support (contracts 4–7 of PLAN-023).
//
// RED: these integration tests pin the JSON contract the API layer must grow
// (`ContactDetailDto`, the structured request fields, the surgical property
// merge and the `text/vcard` import endpoint). They are HTTP + JSON only: the
// sole crate-internal import is the already-public `parse_vcf`.
// ---------------------------------------------------------------------------

/// Base64 body of a tiny JPEG, exactly as macOS Contacts writes an embedded
/// `PHOTO` (`PHOTO;ENCODING=b;TYPE=JPEG:<base64>`).
const APPLE_PHOTO_B64: &str = "/9j/4AAQSkZJRgABAQAAAQ==";

/// An Apple-style vCard, stored verbatim as CardDAV would store it.
///
/// `FN` is deliberately the **last** content line before `END:VCARD`: the merge
/// moves an edited property to just before the terminator, so this seed makes
/// "replace the `FN` line in place" and "the merge output" byte-identical.
fn apple_card() -> String {
    let lines: Vec<String> = vec![
        "BEGIN:VCARD".to_string(),
        "VERSION:4.0".to_string(),
        "UID:apple-uid@oxidav".to_string(),
        format!("PHOTO;ENCODING=b;TYPE=JPEG:{APPLE_PHOTO_B64}"),
        "BDAY:--0203".to_string(),
        "item1.TEL;TYPE=cell:+34600000000".to_string(),
        "item1.X-ABLabel:iPhone".to_string(),
        "X-SOCIALPROFILE;TYPE=home:https://example.com/ada".to_string(),
        "REV:20240115T101530Z".to_string(),
        "NICKNAME:Ada".to_string(),
        "N:Lovelace;Ada;;;".to_string(),
        "ORG:Analytical Engines".to_string(),
        "EMAIL:ada@example.com".to_string(),
        "ADR;TYPE=home:;;221B Baker St;London;;NW1;UK".to_string(),
        "NOTE:first line of a folded".to_string(),
        " note that continues here".to_string(),
        "FN:Ada Lovelace".to_string(),
        "END:VCARD".to_string(),
    ];
    lines.join("\r\n") + "\r\n"
}

/// A card with several multi-value groups and `FN` last, for the merge tests.
fn multi_card() -> String {
    [
        "BEGIN:VCARD",
        "VERSION:4.0",
        "UID:multi-uid@oxidav",
        "TEL;TYPE=cell:+34600000001",
        "TEL;TYPE=work:+34900000002",
        "EMAIL;TYPE=home:multi@example.com",
        "EMAIL;TYPE=work:multi@work.example",
        "ADR;TYPE=home:;;1 Home St;Madrid;;28001;ES",
        "ADR;TYPE=work:;;2 Work Ave;Madrid;;28002;ES",
        "FN:Multi Person",
        "END:VCARD",
    ]
    .join("\r\n")
        + "\r\n"
}

/// A minimal, well-formed card.
fn plain_card(uid: &str, formatted_name: &str) -> String {
    format!("BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:{formatted_name}\r\nEND:VCARD\r\n")
}

/// A large valid card whose `NOTE` is folded at 74 octets per physical line.
fn big_card(index: usize, note_len: usize) -> String {
    let repeated = "0123456789".repeat(note_len / 10 + 1);
    let note = &repeated[..note_len];
    let mut out = String::new();
    out.push_str("BEGIN:VCARD\r\nVERSION:4.0\r\n");
    out.push_str(&format!("UID:big-{index}@oxidav\r\nFN:Big {index}\r\n"));
    for (i, chunk) in note.as_bytes().chunks(74).enumerate() {
        let chunk = std::str::from_utf8(chunk).expect("ascii note");
        if i == 0 {
            out.push_str(&format!("NOTE:{chunk}\r\n"));
        } else {
            out.push_str(&format!(" {chunk}\r\n"));
        }
    }
    out.push_str("END:VCARD\r\n");
    out
}

/// Seed a `card_objects` row with a raw payload, verbatim (as CardDAV does).
async fn seed_card(env: &TestEnv, collection_id: &str, id: &str, uid: &str, raw: &str) {
    insert_card_object(
        &env.pool,
        &NewCardObject {
            id: id.to_string(),
            collection_id: collection_id.to_string(),
            uid: uid.to_string(),
            fn_: None,
            email: None,
            phone: None,
            organization: None,
            notes: None,
            raw_payload: raw.to_string(),
        },
    )
    .await
    .expect("seed card");
}

/// Read the stored `raw_payload` of a card.
async fn stored_raw(env: &TestEnv, collection_id: &str, uid: &str) -> String {
    get_card_object(&env.pool, collection_id, uid)
        .await
        .expect("read card")
        .expect("card exists")
        .raw_payload
}

/// Invariant (decision 1): the denormalised columns equal `parse_vcf(raw_payload)`.
async fn assert_columns_agree(env: &TestEnv, collection_id: &str, uid: &str) {
    let row = get_card_object(&env.pool, collection_id, uid)
        .await
        .expect("read card")
        .expect("card exists");
    let parsed = parse_vcf(&row.raw_payload).expect("payload parses");
    assert_eq!(row.uid, parsed.uid);
    assert_eq!(row.fn_.as_deref(), Some(parsed.fn_.as_str()));
    assert_eq!(row.email, parsed.email);
    assert_eq!(row.phone, parsed.phone);
    assert_eq!(row.organization, parsed.organization);
    assert_eq!(row.notes, parsed.notes);
}

/// Criterio de aceptación 1 — un `PATCH` de `fn` sobre una tarjeta importada por
/// CardDAV deja el `raw_payload` **idéntico** salvo la línea `FN`.
#[tokio::test]
async fn carddav_imported_contact_survives_a_name_patch() {
    let env = TestEnv::new("f22-preserve").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;

    let original = apple_card();
    seed_card(&env, &cid, "card-apple", "apple-uid@oxidav", &original).await;

    let uri = format!("/api/v1/collections/{cid}/contacts/apple-uid@oxidav");
    let (status, body) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({ "fn": "Ada King" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "patch failed: {body}");

    let raw = stored_raw(&env, &cid, "apple-uid@oxidav").await;
    let expected = original.replace("FN:Ada Lovelace", "FN:Ada King");
    assert_eq!(raw, expected, "the PATCH did not preserve the payload");

    // (a) PHOTO byte-for-byte, (b) BDAY not normalised to a synthetic year,
    // (c) the grouped label survives, (d) the folded NOTE is not unfolded.
    assert!(
        raw.contains(&format!("PHOTO;ENCODING=b;TYPE=JPEG:{APPLE_PHOTO_B64}")),
        "PHOTO changed: {raw}"
    );
    assert!(raw.contains("BDAY:--0203"), "BDAY changed: {raw}");
    assert!(!raw.contains("00000203"), "BDAY was normalised: {raw}");
    assert!(raw.contains("item1.X-ABLabel:iPhone"), "label lost: {raw}");
    assert!(
        raw.contains("item1.TEL;TYPE=cell:+34600000000"),
        "grouped TEL lost: {raw}"
    );
    assert!(
        raw.contains("NOTE:first line of a folded\r\n note that continues here"),
        "the folded NOTE was unfolded: {raw}"
    );
}

/// Criterio de aceptación 2 — el `GET` de detalle devuelve todos los grupos.
#[tokio::test]
async fn contact_detail_returns_every_group() {
    let env = TestEnv::new("f22-detail").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;

    seed_card(&env, &cid, "card-apple", "apple-uid@oxidav", &apple_card()).await;

    let uri = format!("/api/v1/collections/{cid}/contacts/apple-uid@oxidav");
    let (status, d) = env.call(Method::GET, &uri, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "detail failed: {d}");

    assert_eq!(d["id"], "card-apple");
    assert_eq!(d["uid"], "apple-uid@oxidav");
    assert_eq!(d["collection_id"], cid);
    assert_eq!(d["fn"], "Ada Lovelace");
    assert_eq!(d["full_name"], "Ada Lovelace");
    assert!(d["etag"].as_str().is_some_and(|e| !e.is_empty()));
    assert!(d["created_at"].is_i64());
    assert!(d["updated_at"].is_i64());
    assert_eq!(d["version"], "4.0");
    assert_eq!(d["rev"], "20240115T101530Z");

    assert_eq!(d["name"]["family"], "Lovelace");
    assert_eq!(d["name"]["given"], "Ada");
    assert_eq!(d["name"]["additional"], json!([]));
    assert_eq!(d["name"]["prefix"], json!([]));
    assert_eq!(d["name"]["suffix"], json!([]));

    assert_eq!(d["nicknames"], json!(["Ada"]));

    let photo = &d["photos"][0];
    assert_eq!(photo["kind"], "data");
    assert_eq!(photo["media_type"], "image/jpeg");
    assert_eq!(
        photo["data_uri"],
        format!("data:image/jpeg;base64,{APPLE_PHOTO_B64}")
    );
    assert!(photo.get("uri").is_none(), "the data variant must omit uri");

    assert_eq!(d["birthday"], "--0203");
    assert_eq!(d["anniversary"], Value::Null);
    assert_eq!(d["gender"], Value::Null);

    let tel = &d["tels"][0];
    assert_eq!(tel["value"], "+34600000000");
    assert_eq!(tel["types"], json!(["cell"]));
    assert_eq!(tel["pref"], Value::Null);

    assert_eq!(d["emails"][0]["value"], "ada@example.com");
    assert_eq!(d["impps"], json!([]));
    let address = &d["addresses"][0];
    assert_eq!(address["street"], "221B Baker St");
    assert_eq!(address["locality"], "London");
    assert_eq!(address["country"], "UK");
    assert_eq!(address["types"], json!(["home"]));
    assert_eq!(d["geos"], json!([]));
    assert_eq!(d["timezones"], json!([]));
    assert_eq!(d["titles"], json!([]));
    assert_eq!(d["roles"], json!([]));
    assert_eq!(d["organizations"], json!([["Analytical Engines"]]));
    assert_eq!(d["logos"], json!([]));
    assert_eq!(
        d["notes"],
        json!(["first line of a foldednote that continues here"])
    );
    assert_eq!(d["categories"], json!([]));
    assert_eq!(d["urls"], json!([]));
    assert_eq!(d["sounds"], json!([]));

    let others = d["others"].as_array().expect("others array");
    let label = others
        .iter()
        .find(|property| property["name"] == "X-ABLabel")
        .expect("X-ABLabel must reach `others`");
    assert_eq!(label["group"], "item1");
    assert_eq!(label["value"], "iPhone");
    assert_eq!(label["parameters"], json!([]));
    let social = others
        .iter()
        .find(|property| property["name"] == "X-SOCIALPROFILE")
        .expect("X-SOCIALPROFILE must reach `others`");
    assert!(social["value"].is_string());
}

/// Decisión 7 — un grupo `PATCH`ado presente sustituye **todas** sus apariciones.
#[tokio::test]
async fn patch_replaces_a_multi_value_group() {
    let env = TestEnv::new("f22-tels").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;

    seed_card(&env, &cid, "card-multi", "multi-uid@oxidav", &multi_card()).await;

    let uri = format!("/api/v1/collections/{cid}/contacts/multi-uid@oxidav");
    let (status, body) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({
                "tels": [
                    { "value": "+34111", "types": ["work"], "pref": 2 },
                    { "value": "+34222", "types": ["home"] },
                ],
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "patch failed: {body}");

    let raw = stored_raw(&env, &cid, "multi-uid@oxidav").await;
    // The two old `TEL` lines are gone; the two new ones land just before the
    // terminator; every other line keeps its position byte-for-byte.
    let expected = [
        "BEGIN:VCARD",
        "VERSION:4.0",
        "UID:multi-uid@oxidav",
        "EMAIL;TYPE=home:multi@example.com",
        "EMAIL;TYPE=work:multi@work.example",
        "ADR;TYPE=home:;;1 Home St;Madrid;;28001;ES",
        "ADR;TYPE=work:;;2 Work Ave;Madrid;;28002;ES",
        "FN:Multi Person",
        "TEL;TYPE=work;PREF=2:+34111",
        "TEL;TYPE=home:+34222",
        "END:VCARD",
    ]
    .join("\r\n")
        + "\r\n";
    assert_eq!(raw, expected);
    assert!(!raw.contains("+34600000001"), "an old TEL survived: {raw}");
    assert!(!raw.contains("+34900000002"), "an old TEL survived: {raw}");
}

/// Decisión 7 — un array vacío **borra** la propiedad.
#[tokio::test]
async fn patch_with_an_empty_array_deletes_the_property() {
    let env = TestEnv::new("f22-delete").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;

    seed_card(&env, &cid, "card-multi", "multi-uid@oxidav", &multi_card()).await;

    let uri = format!("/api/v1/collections/{cid}/contacts/multi-uid@oxidav");
    let (status, body) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({ "tels": [] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "patch failed: {body}");

    let raw = stored_raw(&env, &cid, "multi-uid@oxidav").await;
    let expected = [
        "BEGIN:VCARD",
        "VERSION:4.0",
        "UID:multi-uid@oxidav",
        "EMAIL;TYPE=home:multi@example.com",
        "EMAIL;TYPE=work:multi@work.example",
        "ADR;TYPE=home:;;1 Home St;Madrid;;28001;ES",
        "ADR;TYPE=work:;;2 Work Ave;Madrid;;28002;ES",
        "FN:Multi Person",
        "END:VCARD",
    ]
    .join("\r\n")
        + "\r\n";
    assert_eq!(raw, expected);
    assert!(!raw.contains("TEL"), "a TEL survived: {raw}");
}

/// A card carrying one occurrence of every structured/scalar property the
/// "replace with nothing deletes" contract can remove, with a marker `NOTE` and
/// the `FN` last so the merge can move a replacement to the end.
fn deletable_card() -> String {
    [
        "BEGIN:VCARD",
        "VERSION:4.0",
        "UID:delete-uid@oxidav",
        "N:Lovelace;Ada;;;",
        "GENDER:F;female",
        "BDAY:--0203",
        "ANNIVERSARY:20100101",
        "NOTE:keep me byte for byte",
        "FN:Ada Lovelace",
        "END:VCARD",
    ]
    .join("\r\n")
        + "\r\n"
}

/// Decisión 7 (extendida) — un `gender` presente y **totalmente vacío** borra el
/// `GENDER`; el resto del payload sigue byte a byte.
#[tokio::test]
async fn patch_with_an_empty_gender_deletes_the_property() {
    let env = TestEnv::new("f22-delete-gender").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;

    let original = deletable_card();
    let expected = original.replace("GENDER:F;female\r\n", "");
    assert_ne!(original, expected, "the seed must carry a GENDER");

    // Both encodings of "totally empty": an empty object and explicit nulls.
    for (uid, body) in [
        ("delete-gender-a@oxidav", json!({ "gender": {} })),
        (
            "delete-gender-b@oxidav",
            json!({ "gender": { "sex": null, "identity": null } }),
        ),
    ] {
        let card = original.replace("delete-uid@oxidav", uid);
        let expected_card = expected.replace("delete-uid@oxidav", uid);
        seed_card(&env, &cid, &format!("seed-{uid}"), uid, &card).await;

        let uri = format!("/api/v1/collections/{cid}/contacts/{uid}");
        let (status, body_out) = env
            .call(Method::PATCH, &uri, Some(&token), Some(body))
            .await;
        assert_eq!(status, StatusCode::OK, "patch failed: {body_out}");

        let raw = stored_raw(&env, &cid, uid).await;
        assert!(!raw.contains("GENDER"), "GENDER survived: {raw}");
        assert_eq!(raw, expected_card, "the rest of the payload changed");
    }
}

/// Decisión 7 (extendida) — un `name` presente y **totalmente vacío** borra el
/// `N`; el resto del payload sigue byte a byte.
#[tokio::test]
async fn patch_with_an_empty_name_deletes_the_property() {
    let env = TestEnv::new("f22-delete-name").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;

    let original = deletable_card();
    let expected = original.replace("N:Lovelace;Ada;;;\r\n", "");
    assert_ne!(original, expected, "the seed must carry an N");
    seed_card(&env, &cid, "seed-name", "delete-uid@oxidav", &original).await;

    let uri = format!("/api/v1/collections/{cid}/contacts/delete-uid@oxidav");
    let (status, body) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({ "name": {} })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "patch failed: {body}");

    let raw = stored_raw(&env, &cid, "delete-uid@oxidav").await;
    assert!(!raw.contains("N:Lovelace"), "N survived: {raw}");
    assert_eq!(raw, expected, "the rest of the payload changed");
}

/// Decisión 7 (extendida) — un `birthday` vacío tras `trim` borra el `BDAY`; el
/// resto del payload sigue byte a byte.
#[tokio::test]
async fn patch_with_a_blank_birthday_deletes_the_property() {
    let env = TestEnv::new("f22-delete-bday").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;

    let original = deletable_card();
    let expected = original.replace("BDAY:--0203\r\n", "");
    assert_ne!(original, expected, "the seed must carry a BDAY");
    seed_card(&env, &cid, "seed-bday", "delete-uid@oxidav", &original).await;

    let uri = format!("/api/v1/collections/{cid}/contacts/delete-uid@oxidav");
    let (status, body) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({ "birthday": "  " })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "patch failed: {body}");

    let raw = stored_raw(&env, &cid, "delete-uid@oxidav").await;
    assert!(!raw.contains("BDAY"), "BDAY survived: {raw}");
    assert_eq!(raw, expected, "the rest of the payload changed");
}

/// Decisión 7 (extendida) — un `anniversary` vacío tras `trim` borra el
/// `ANNIVERSARY`; el resto del payload sigue byte a byte.
#[tokio::test]
async fn patch_with_a_blank_anniversary_deletes_the_property() {
    let env = TestEnv::new("f22-delete-anniv").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;

    let original = deletable_card();
    let expected = original.replace("ANNIVERSARY:20100101\r\n", "");
    assert_ne!(original, expected, "the seed must carry an ANNIVERSARY");
    seed_card(&env, &cid, "seed-anniv", "delete-uid@oxidav", &original).await;

    let uri = format!("/api/v1/collections/{cid}/contacts/delete-uid@oxidav");
    let (status, body) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({ "anniversary": "" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "patch failed: {body}");

    let raw = stored_raw(&env, &cid, "delete-uid@oxidav").await;
    assert!(!raw.contains("ANNIVERSARY"), "ANNIVERSARY survived: {raw}");
    assert_eq!(raw, expected, "the rest of the payload changed");
}

/// Decisión 7 (control) — un `gender` **parcialmente** relleno **reemplaza** la
/// línea (no la borra).
#[tokio::test]
async fn patch_with_a_partial_gender_replaces_the_property() {
    let env = TestEnv::new("f22-partial-gender").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;

    let original = deletable_card();
    seed_card(&env, &cid, "seed-partial", "delete-uid@oxidav", &original).await;

    let uri = format!("/api/v1/collections/{cid}/contacts/delete-uid@oxidav");
    let (status, body) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({ "gender": { "sex": "M", "identity": null } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "patch failed: {body}");

    // The old `GENDER` line is gone from its position; the replacement is
    // inserted just before the terminator (`FN` is last). Every other line keeps
    // its position byte-for-byte.
    let expected = [
        "BEGIN:VCARD",
        "VERSION:4.0",
        "UID:delete-uid@oxidav",
        "N:Lovelace;Ada;;;",
        "BDAY:--0203",
        "ANNIVERSARY:20100101",
        "NOTE:keep me byte for byte",
        "FN:Ada Lovelace",
        "GENDER:M",
        "END:VCARD",
    ]
    .join("\r\n")
        + "\r\n";

    let raw = stored_raw(&env, &cid, "delete-uid@oxidav").await;
    assert!(
        !raw.contains("GENDER:F;female"),
        "the old GENDER survived: {raw}"
    );
    assert!(
        raw.contains("GENDER:M"),
        "the replacement is missing: {raw}"
    );
    assert_eq!(raw, expected, "the payload does not match the replacement");
}

/// Decisión 7 — una propiedad ausente del `PATCH` no se toca.
#[tokio::test]
async fn patch_leaves_absent_properties_alone() {
    let env = TestEnv::new("f22-absent").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;

    let original = multi_card();
    seed_card(&env, &cid, "card-multi", "multi-uid@oxidav", &original).await;

    let uri = format!("/api/v1/collections/{cid}/contacts/multi-uid@oxidav");
    let (status, body) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({ "fn": "Rebuilt Name" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "patch failed: {body}");

    let raw = stored_raw(&env, &cid, "multi-uid@oxidav").await;
    assert_eq!(raw, original.replace("FN:Multi Person", "FN:Rebuilt Name"));
    for line in [
        "TEL;TYPE=cell:+34600000001",
        "TEL;TYPE=work:+34900000002",
        "EMAIL;TYPE=home:multi@example.com",
        "EMAIL;TYPE=work:multi@work.example",
        "ADR;TYPE=home:;;1 Home St;Madrid;;28001;ES",
        "ADR;TYPE=work:;;2 Work Ave;Madrid;;28002;ES",
    ] {
        assert!(raw.contains(line), "an absent property changed: {line}");
    }
}

/// Decisión 3 — `create` compone la tarjeta completa (esqueleto + merge) y el
/// detalle la devuelve.
#[tokio::test]
async fn create_composes_the_full_card() {
    let env = TestEnv::new("f22-create").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;
    let base = format!("/api/v1/collections/{cid}/contacts");

    let (status, created) = env
        .call(
            Method::POST,
            &base,
            Some(&token),
            Some(json!({
                "fn": "Ada Lovelace",
                "name": { "family": "Lovelace", "given": "Ada" },
                "emails": [ { "value": "ada@example.com", "types": ["work"], "pref": 1 } ],
                "tels": [ { "value": "+34600000000", "types": ["cell"] } ],
                "notes": ["first programmer"],
                "categories": [["Friends", "Family"]],
                "urls": ["https://example.com/ada"],
                "anniversary": "20100101",
                "gender": { "sex": "F" },
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {created}");
    let uid = created["uid"].as_str().expect("uid").to_string();
    assert!(created.get("raw_payload").is_none());

    let raw = stored_raw(&env, &cid, &uid).await;
    assert!(raw.starts_with("BEGIN:VCARD\r\n"), "payload: {raw}");
    assert!(raw.contains("VERSION:4.0"), "payload: {raw}");
    assert!(raw.contains(&format!("UID:{uid}")), "payload: {raw}");
    assert!(
        raw.contains("EMAIL;TYPE=work;PREF=1:ada@example.com"),
        "payload: {raw}"
    );
    assert!(raw.contains("TEL;TYPE=cell:+34600000000"), "payload: {raw}");

    let uri = format!("{base}/{uid}");
    let (status, d) = env.call(Method::GET, &uri, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "detail failed: {d}");
    assert_eq!(d["version"], "4.0");
    assert_eq!(d["uid"], uid);
    assert_eq!(d["name"]["family"], "Lovelace");
    assert_eq!(d["emails"][0]["value"], "ada@example.com");
    assert_eq!(d["emails"][0]["types"], json!(["work"]));
    assert_eq!(d["emails"][0]["pref"], 1);
    assert_eq!(d["tels"][0]["value"], "+34600000000");
    assert_eq!(d["notes"], json!(["first programmer"]));
    assert_eq!(d["categories"], json!([["Friends", "Family"]]));
    assert!(
        d["urls"][0]
            .as_str()
            .is_some_and(|url| url.starts_with("https://example.com/ada")),
        "urls: {}",
        d["urls"]
    );
    assert_eq!(d["anniversary"], "20100101");
    assert_eq!(d["gender"]["sex"], "F");
}

/// Decisión 1 — las columnas desnormalizadas coinciden con `parse_vcf` tras
/// crear y tras parchear un grupo multivalor.
#[tokio::test]
async fn stored_columns_agree_with_the_payload() {
    let env = TestEnv::new("f22-columns").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;
    let base = format!("/api/v1/collections/{cid}/contacts");

    let (status, created) = env
        .call(
            Method::POST,
            &base,
            Some(&token),
            Some(json!({
                "fn": "Ada Lovelace",
                "email": "ada@example.com",
                "phone": "+34600000000",
                "organization": "Analytical Engines",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {created}");
    let uid = created["uid"].as_str().expect("uid").to_string();
    assert_columns_agree(&env, &cid, &uid).await;

    // Patching a *new* multi-value group re-derives the legacy columns from the
    // resulting payload.
    let uri = format!("{base}/{uid}");
    let (status, body) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({
                "emails": [ { "value": "new@example.com" } ],
                "tels": [ { "value": "+34999888777" } ],
                "notes": ["updated note"],
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "patch failed: {body}");

    let row = get_card_object(&env.pool, &cid, &uid)
        .await
        .expect("read card")
        .expect("card exists");
    assert_eq!(row.email.as_deref(), Some("new@example.com"));
    assert_eq!(row.phone.as_deref(), Some("+34999888777"));
    assert_eq!(row.notes.as_deref(), Some("updated note"));
    assert_columns_agree(&env, &cid, &uid).await;
}

/// Decisión 7 — el escalar heredado y su array en la misma petición es un `400`.
#[tokio::test]
async fn legacy_scalar_and_array_together_are_rejected() {
    let env = TestEnv::new("f22-scalar-array").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;
    let base = format!("/api/v1/collections/{cid}/contacts");

    let cases = [
        json!({ "fn": "Ada", "email": "a@b.com", "emails": [ { "value": "c@d.com" } ] }),
        json!({ "fn": "Ada", "phone": "+34", "tels": [ { "value": "+35" } ] }),
        json!({ "fn": "Ada", "organization": "Acme", "organizations": [ ["Beta"] ] }),
    ];

    for body in cases {
        let (status, value) = env
            .call(Method::POST, &base, Some(&token), Some(body.clone()))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body {body} -> {value}");
        assert_eq!(value["error"]["code"], "validation", "body {body}");
        assert!(value["error"]["message"].is_string(), "body {body}");
    }

    // Nothing was persisted by the rejected requests.
    let (_, list) = env.call(Method::GET, &base, Some(&token), None).await;
    assert_eq!(list.as_array().map(Vec::len), Some(0));
}

/// Decisión 8 — todo campo nuevo rechaza con `400` (nunca `500`).
#[tokio::test]
async fn new_field_validation_rejects_with_400() {
    let env = TestEnv::new("f22-validation").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;
    let base = format!("/api/v1/collections/{cid}/contacts");

    let too_many_tels = json!(
        (0..40)
            .map(|i| json!({ "value": format!("+34{i}") }))
            .collect::<Vec<_>>()
    );

    let cases = [
        (
            "type with space",
            json!({ "fn": "Ada", "tels": [ { "value": "+34", "types": ["wo rk"] } ] }),
        ),
        (
            "type with colon",
            json!({ "fn": "Ada", "tels": [ { "value": "+34", "types": ["a:b"] } ] }),
        ),
        (
            "pref zero",
            json!({ "fn": "Ada", "emails": [ { "value": "a@b.com", "pref": 0 } ] }),
        ),
        (
            "pref 101",
            json!({ "fn": "Ada", "emails": [ { "value": "a@b.com", "pref": 101 } ] }),
        ),
        (
            "control char in nicknames",
            json!({ "fn": "Ada", "nicknames": ["a\u{0007}b"] }),
        ),
        (
            "control char in notes",
            json!({ "fn": "Ada", "notes": ["a\u{0007}b"] }),
        ),
        (
            "control char in address street",
            json!({ "fn": "Ada", "addresses": [ { "street": "a\u{0007}b" } ] }),
        ),
        (
            "control char in urls",
            json!({ "fn": "Ada", "urls": ["https://x/\u{0007}"] }),
        ),
        (
            "control char in geos",
            json!({ "fn": "Ada", "geos": ["geo:1,2\u{0007}"] }),
        ),
        (
            "control char in timezones",
            json!({ "fn": "Ada", "timezones": ["America/New_York\u{0007}"] }),
        ),
        (
            "control char in name family",
            json!({ "fn": "Ada", "name": { "family": "a\u{0007}b" } }),
        ),
        (
            "control char in name prefix",
            json!({ "fn": "Ada", "name": { "prefix": ["Dr.\u{0007}"] } }),
        ),
        (
            "control char in gender identity",
            json!({ "fn": "Ada", "gender": { "sex": "F", "identity": "a\u{0007}b" } }),
        ),
        (
            "overlong birthday",
            json!({ "fn": "Ada", "birthday": "x".repeat(1000) }),
        ),
        (
            "too many tels",
            json!({ "fn": "Ada", "tels": too_many_tels }),
        ),
    ];

    for (label, body) in cases {
        let (status, value) = env
            .call(Method::POST, &base, Some(&token), Some(body))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "case: {label} -> {value}");
        assert_eq!(value["error"]["code"], "validation", "case: {label}");
        assert!(value["error"]["message"].is_string(), "case: {label}");
    }
}

/// Decisión 4 — el `uid` es inmutable en `PATCH`; `VERSION` no se toca.
#[tokio::test]
async fn patch_cannot_change_the_uid() {
    let env = TestEnv::new("f22-uid").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;
    let base = format!("/api/v1/collections/{cid}/contacts");

    let (status, created) = env
        .call(
            Method::POST,
            &base,
            Some(&token),
            Some(json!({ "fn": "Ada", "uid": "fixed-uid@oxidav", "phone": "+34" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {created}");

    let uri = format!("{base}/fixed-uid@oxidav");

    // A normal PATCH must leave `VERSION` and `UID` untouched.
    let (status, body) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({ "fn": "Renamed" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "patch failed: {body}");
    let raw = stored_raw(&env, &cid, "fixed-uid@oxidav").await;
    assert!(raw.contains("VERSION:4.0"), "VERSION changed: {raw}");
    assert!(raw.contains("UID:fixed-uid@oxidav"), "UID changed: {raw}");

    // Changing the `uid` is rejected (chosen contract: a `400`, never a silent
    // ignore).
    let (status, value) = env
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(json!({ "fn": "X", "uid": "other-uid" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "uid patch -> {value}");
    assert_eq!(value["error"]["code"], "validation");
    let raw_after = stored_raw(&env, &cid, "fixed-uid@oxidav").await;
    assert_eq!(
        raw_after, raw,
        "a rejected patch must not touch the payload"
    );
}

/// Contrato 6 — `POST .../contacts/import` guarda cada tarjeta verbatim.
#[tokio::test]
async fn import_stores_cards_verbatim() {
    let env = TestEnv::new("f22-import").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;
    let uri = format!("/api/v1/collections/{cid}/contacts/import");

    let card1 = apple_card();
    let card2 = plain_card("imp-2@oxidav", "Second Person");
    let card3 = plain_card("imp-3@oxidav", "Third Person");
    let file = format!("{card1}{card2}{card3}");

    let (status, value) = env
        .call_body(
            Method::POST,
            &uri,
            Some(&token),
            "text/vcard",
            file.clone().into_bytes(),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "import failed: {value}");
    assert_eq!(
        value.as_array().map(Vec::len),
        Some(3),
        "expected three created cards: {value}"
    );

    assert_eq!(stored_raw(&env, &cid, "apple-uid@oxidav").await, card1);
    assert_eq!(stored_raw(&env, &cid, "imp-2@oxidav").await, card2);
    assert_eq!(stored_raw(&env, &cid, "imp-3@oxidav").await, card3);

    // The detail view preserves the embedded PHOTO.
    let one = format!("/api/v1/collections/{cid}/contacts/apple-uid@oxidav");
    let (status, d) = env.call(Method::GET, &one, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(d["photos"][0]["kind"], "data");
    assert_eq!(
        d["photos"][0]["data_uri"],
        format!("data:image/jpeg;base64,{APPLE_PHOTO_B64}")
    );
}

/// Contrato 6 — un `.vcf` válido de más de 3 MB se importa (el límite por
/// defecto de axum, 2 MB, lo rechazaría: este test obliga a subir el límite).
#[tokio::test]
async fn import_accepts_a_large_body() {
    let env = TestEnv::new("f22-import-large").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;
    let uri = format!("/api/v1/collections/{cid}/contacts/import");

    let mut file = String::new();
    let mut i = 0usize;
    while file.len() <= 3 * 1024 * 1024 {
        file.push_str(&big_card(i, 200_000));
        i += 1;
    }
    assert!(file.len() > 3 * 1024 * 1024, "the body must exceed 3 MB");

    let (status, value) = env
        .call_body(
            Method::POST,
            &uri,
            Some(&token),
            "text/vcard",
            file.into_bytes(),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "large import failed: {value}");
    assert_eq!(
        stored_raw(&env, &cid, "big-0@oxidav").await,
        big_card(0, 200_000)
    );
}

/// Contrato 6 — un cuerpo sin tarjetas es un `400` con el sobre uniforme.
#[tokio::test]
async fn import_rejects_a_payload_without_cards() {
    let env = TestEnv::new("f22-import-bad").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;
    let uri = format!("/api/v1/collections/{cid}/contacts/import");

    let (status, value) = env
        .call_body(
            Method::POST,
            &uri,
            Some(&token),
            "text/vcard",
            b"garbage".to_vec(),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "garbage import -> {value}");
    assert_eq!(value["error"]["code"], "validation");
    assert!(value["error"]["message"].is_string());
}

/// FIX-2 — dos tarjetas con el **mismo** `uid` en un fichero: `409` y **cero**
/// filas escritas (la importación no es parcial).
#[tokio::test]
async fn import_rejects_duplicate_uids_within_the_file() {
    let env = TestEnv::new("f22-import-dup").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;
    let uri = format!("/api/v1/collections/{cid}/contacts/import");

    let file = format!(
        "{}{}",
        plain_card("dup@oxidav", "First"),
        plain_card("dup@oxidav", "Second"),
    );

    let (status, value) = env
        .call_body(
            Method::POST,
            &uri,
            Some(&token),
            "text/vcard",
            file.into_bytes(),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "duplicate import -> {value}");
    assert_eq!(value["error"]["code"], "conflict");

    let rows = list_card_objects(&env.pool, &cid).await.expect("list");
    assert_eq!(rows.len(), 0, "a rejected import must write nothing");
}

/// FIX-2 — un `uid` que ya existe en la colección: `409` y **cero** filas
/// nuevas (ni siquiera las tarjetas válidas que lo preceden).
#[tokio::test]
async fn import_rejects_an_existing_uid_without_writing() {
    let env = TestEnv::new("f22-import-existing").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;
    let uri = format!("/api/v1/collections/{cid}/contacts/import");

    seed_card(
        &env,
        &cid,
        "card-taken",
        "taken@oxidav",
        &plain_card("taken@oxidav", "Taken"),
    )
    .await;

    let file = format!(
        "{}{}",
        plain_card("fresh@oxidav", "Fresh"),
        plain_card("taken@oxidav", "Clash"),
    );

    let (status, value) = env
        .call_body(
            Method::POST,
            &uri,
            Some(&token),
            "text/vcard",
            file.into_bytes(),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "existing uid -> {value}");
    assert_eq!(value["error"]["code"], "conflict");

    let rows = list_card_objects(&env.pool, &cid).await.expect("list");
    assert_eq!(rows.len(), 1, "only the seeded card must remain");
    assert!(
        get_card_object(&env.pool, &cid, "fresh@oxidav")
            .await
            .expect("get")
            .is_none(),
        "no partial import: the fresh card must not be stored"
    );
}

/// FIX-2 — el camino feliz de tres tarjetas sigue siendo `201` con las tres.
#[tokio::test]
async fn import_three_cards_is_atomic_and_created() {
    let env = TestEnv::new("f22-import-three").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;
    let uri = format!("/api/v1/collections/{cid}/contacts/import");

    let file = format!(
        "{}{}{}",
        plain_card("one@oxidav", "One"),
        plain_card("two@oxidav", "Two"),
        plain_card("three@oxidav", "Three"),
    );

    let (status, value) = env
        .call_body(
            Method::POST,
            &uri,
            Some(&token),
            "text/vcard",
            file.into_bytes(),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "import failed: {value}");
    assert_eq!(value.as_array().map(Vec::len), Some(3));

    let rows = list_card_objects(&env.pool, &cid).await.expect("list");
    assert_eq!(rows.len(), 3, "all three cards must be stored");
}

/// FIX-3 — la importación persiste el `uid` **parseado** (desescapado), no el
/// crudo: `UID:a\,b` deja la columna igual que `parse_vcf(payload).uid`.
#[tokio::test]
async fn import_persists_the_parsed_uid() {
    let env = TestEnv::new("f22-import-uid").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;
    let uri = format!("/api/v1/collections/{cid}/contacts/import");

    let card = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:a\\,b\r\nFN:Comma Uid\r\nEND:VCARD\r\n";
    let parsed = parse_vcf(card).expect("parse");
    assert_eq!(parsed.uid, "a,b", "the parser unescapes the comma");

    let (status, value) = env
        .call_body(
            Method::POST,
            &uri,
            Some(&token),
            "text/vcard",
            card.as_bytes().to_vec(),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "import failed: {value}");

    let row = get_card_object(&env.pool, &cid, &parsed.uid)
        .await
        .expect("get")
        .expect("row exists");
    assert_eq!(row.uid, parse_vcf(&row.raw_payload).expect("reparse").uid);
    assert_eq!(row.uid, "a,b");
    assert_columns_agree(&env, &cid, &parsed.uid).await;
}

/// FIX-3 — un `UID:a\Nb` (que desescapa a un salto de línea real) → `400` y
/// **ninguna** fila escrita.
#[tokio::test]
async fn import_rejects_a_uid_that_unescapes_to_a_newline() {
    let env = TestEnv::new("f22-import-uid-nl").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;
    let uri = format!("/api/v1/collections/{cid}/contacts/import");

    let card = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:a\\Nb\r\nFN:Newline Uid\r\nEND:VCARD\r\n";
    assert!(
        parse_vcf(card).expect("parse").uid.contains('\n'),
        "the parser turns \\N into a real line feed"
    );

    let (status, value) = env
        .call_body(
            Method::POST,
            &uri,
            Some(&token),
            "text/vcard",
            card.as_bytes().to_vec(),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "newline uid -> {value}");
    assert_eq!(value["error"]["code"], "validation");

    let rows = list_card_objects(&env.pool, &cid).await.expect("list");
    assert_eq!(rows.len(), 0, "no row must be written for an invalid uid");
}

/// FIX-3 — la creación persiste el `uid` **parseado** y rechaza el que desescapa
/// a un salto de línea.
#[tokio::test]
async fn create_persists_the_parsed_uid() {
    let env = TestEnv::new("f22-create-uid").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;
    let base = format!("/api/v1/collections/{cid}/contacts");

    // JSON `"a\\,b"` is the value `a\,b`, which the parser unescapes to `a,b`.
    let (status, created) = env
        .call(
            Method::POST,
            &base,
            Some(&token),
            Some(json!({ "fn": "Comma", "uid": "a\\,b" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {created}");
    assert_eq!(created["uid"], "a,b");

    let row = get_card_object(&env.pool, &cid, "a,b")
        .await
        .expect("get")
        .expect("row exists");
    assert_eq!(row.uid, parse_vcf(&row.raw_payload).expect("reparse").uid);

    // A uid that unescapes to a real line feed is a `400`, and nothing is stored.
    let (status, value) = env
        .call(
            Method::POST,
            &base,
            Some(&token),
            Some(json!({ "fn": "Newline", "uid": "a\\Nb" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "newline uid -> {value}");
    assert_eq!(value["error"]["code"], "validation");
}

/// FIX-1 — las sub-listas del `N` separan por `,` sin escapar y round-trip.
#[tokio::test]
async fn name_sub_lists_are_comma_separated_and_round_trip() {
    let env = TestEnv::new("f22-name-lists").await;
    let token = env.token("user-a");
    let cid = addressbook_collection_id(&env, &token, "Book").await;
    let base = format!("/api/v1/collections/{cid}/contacts");

    let (status, created) = env
        .call(
            Method::POST,
            &base,
            Some(&token),
            Some(json!({
                "fn": "Ada Lovelace",
                "name": {
                    "family": "Lovelace",
                    "given": "Ada",
                    "additional": ["Byron", "King"],
                    "prefix": ["Dr.", "Prof."],
                    "suffix": ["Jr.", "PhD"],
                },
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {created}");
    let uid = created["uid"].as_str().expect("uid").to_string();

    let raw = stored_raw(&env, &cid, &uid).await;
    assert!(
        raw.contains("N:Lovelace;Ada;Byron,King;Dr.,Prof.;Jr.,PhD\r\n"),
        "sub-lists must be comma-separated without escaping: {raw}"
    );
    assert!(
        !raw.contains("Dr.\\,Prof."),
        "the list separator must not be escaped: {raw}"
    );

    let uri = format!("{base}/{uid}");
    let (status, d) = env.call(Method::GET, &uri, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "detail failed: {d}");
    assert_eq!(d["name"]["additional"], json!(["Byron", "King"]));
    assert_eq!(d["name"]["prefix"], json!(["Dr.", "Prof."]));
    assert_eq!(d["name"]["suffix"], json!(["Jr.", "PhD"]));
}
