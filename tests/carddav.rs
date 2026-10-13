//! F08 phase B — CardDAV integration tests.
//!
//! Each test mounts the full application router over a throwaway file-backed
//! database under `/tmp/opencode`, mints a real F05 app token and presents it via
//! `Authorization: Basic`. Nothing is mocked: the F05 middleware, the SQLite
//! data layer and the DAV handlers all run for real.
//!
//! `MKCOL` / `PROPFIND` are not standard *HTTP* verbs, so they are sent as raw
//! methods via [`Method::from_bytes`] (the server registers the routes with
//! `any()`, exactly as a native client would exercise them).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, Response, StatusCode, header};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use oxidav::auth::app_tokens::create_app_token;
use oxidav::config::Config;
use oxidav::db::models::{CollectionKind, NewCollection};
use oxidav::db::{create_collection, get_card_object, init_pool};
use oxidav::router::{AppState, build_router};
use sqlx::SqlitePool;
use tower::ServiceExt;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// The Pocket ID `sub` the test app token is minted for.
const SUB: &str = "user-a";

/// A throwaway file-backed DSN under `/tmp/opencode` (never `data/oxidav.db`).
fn temp_db_url(tag: &str) -> String {
    let dir = PathBuf::from("/tmp/opencode/oxidav-tests");
    std::fs::create_dir_all(&dir).expect("create temp test dir");
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("carddav-{tag}-{}-{n}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    format!("sqlite://{}", path.display())
}

/// The application router, its pool and the Basic credentials of a valid token.
struct TestEnv {
    app: axum::Router,
    pool: SqlitePool,
    basic: String,
}

async fn test_env(tag: &str) -> TestEnv {
    let pool: SqlitePool = init_pool(&temp_db_url(tag)).await.expect("init_pool");
    let (_id, token) = create_app_token(&pool, SUB, "dav client")
        .await
        .expect("create app token");

    let config = Config {
        bind_addr: "127.0.0.1:0".parse().expect("valid socket addr"),
        log_level: "info".to_string(),
        database_url: "sqlite::memory:".to_string(),
        pocket_id_issuer: "https://id.example.com".to_string(),
        audience: None,
    };
    let app = build_router(AppState::offline_oidc(config, pool.clone()));

    TestEnv {
        app,
        pool,
        basic: basic_header(SUB, &token),
    }
}

fn basic_header(username: &str, token: &str) -> String {
    let raw = format!("{username}:{token}");
    format!("Basic {}", STANDARD.encode(raw.as_bytes()))
}

/// Build and dispatch a DAV request, authenticating with `env.basic`.
async fn dav(
    env: &TestEnv,
    method: &str,
    uri: &str,
    depth: Option<&str>,
    body: &str,
) -> Response<Body> {
    let method = Method::from_bytes(method.as_bytes()).expect("valid DAV method");
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, &env.basic)
        .header(header::CONTENT_TYPE, "application/xml; charset=utf-8");
    if let Some(depth) = depth {
        builder = builder.header("depth", depth);
    }

    env.app
        .clone()
        .oneshot(
            builder
                .body(Body::from(body.to_string()))
                .expect("build request"),
        )
        .await
        .expect("send request")
}

/// Build and dispatch a request with extra headers (conditionals, …) and a
/// `text/vcard` body, authenticating with `env.basic`.
async fn dav_with(
    env: &TestEnv,
    method: &str,
    uri: &str,
    extra: &[(&str, &str)],
    body: &str,
) -> Response<Body> {
    let method = Method::from_bytes(method.as_bytes()).expect("valid DAV method");
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, &env.basic)
        .header(header::CONTENT_TYPE, "text/vcard; charset=utf-8");
    for (name, value) in extra {
        builder = builder.header(*name, *value);
    }

    env.app
        .clone()
        .oneshot(
            builder
                .body(Body::from(body.to_string()))
                .expect("build request"),
        )
        .await
        .expect("send request")
}

/// Read a response body as UTF-8 text.
async fn body_text(response: Response<Body>) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    String::from_utf8(bytes.to_vec()).expect("UTF-8 body")
}

/// The `Location` header of a `201`, or an empty string when absent.
fn location(response: &Response<Body>) -> String {
    response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// The `ETag` header of a response, or an empty string when absent.
fn etag(response: &Response<Body>) -> String {
    response
        .headers()
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// Create an address book collection owned by [`SUB`] with a fixed id.
async fn make_addressbook(env: &TestEnv, id: &str) {
    create_collection(
        &env.pool,
        &NewCollection {
            id: id.to_string(),
            user_id: SUB.to_string(),
            name: "Contactos".to_string(),
            kind: CollectionKind::Addressbook,
            color: "#1677ff".to_string(),
        },
    )
    .await
    .expect("create address book");
}

/// The canonical URL of the addressbook-home.
fn home_uri() -> String {
    format!("/dav/addressbooks/{SUB}/")
}

/// The canonical URL of the `col-contacts` address book.
fn collection_uri() -> String {
    format!("/dav/addressbooks/{SUB}/col-contacts/")
}

/// The canonical URL of a resource named after `uid` in `col-contacts`.
fn resource_uri(uid: &str) -> String {
    format!("/dav/addressbooks/{SUB}/col-contacts/{uid}.vcf")
}

/// A minimal, valid vCard 4.0 with a single `FN`, `EMAIL` and `TEL`.
fn vcf_card(uid: &str, fn_: &str, email: &str) -> String {
    format!(
        "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:{fn_}\r\n\
         EMAIL:{email}\r\nTEL:+34600000000\r\nORG:Analytical Engines;Research\r\n\
         NOTE:contacto\r\nEND:VCARD\r\n"
    )
}

// ---------------------------------------------------------------------------
// OPTIONS
// ---------------------------------------------------------------------------

#[tokio::test]
async fn options_advertises_addressbook_capability() {
    let env = test_env("options").await;

    let response = dav(&env, "OPTIONS", "/dav/", None, "").await;
    assert_eq!(response.status(), StatusCode::OK);

    let dav_header = response
        .headers()
        .get("dav")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(dav_header.contains("addressbook"), "{dav_header}");
    assert!(dav_header.contains("calendar-access"), "{dav_header}");

    let allow = response
        .headers()
        .get(header::ALLOW)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(allow.contains("MKCOL"), "{allow}");
}

// ---------------------------------------------------------------------------
// MKCOL
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mkcol_creates_an_addressbook_adopting_the_segment() {
    let env = test_env("mkcol-segment").await;

    let uri = format!("/dav/addressbooks/{SUB}/mybook/");
    let created = dav(&env, "MKCOL", &uri, None, "").await;
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(location(&created), uri);

    // The new collection is discoverable through the home PROPFIND.
    let propfind = dav(&env, "PROPFIND", &home_uri(), Some("1"), "").await;
    assert_eq!(propfind.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(propfind).await;
    assert!(xml.contains(&uri), "home PROPFIND lacks {uri}: {xml}");
    assert!(
        xml.contains("<card:addressbook/>"),
        "collection must be typed as an address book: {xml}"
    );
    assert!(xml.contains("getctag"), "{xml}");
}

#[tokio::test]
async fn mkcol_on_the_home_mints_an_id() {
    let env = test_env("mkcol-home").await;

    let created = dav(&env, "MKCOL", &home_uri(), None, "").await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let loc = location(&created);
    assert!(loc.starts_with(&home_uri()), "{loc}");
    assert!(loc.ends_with('/'), "{loc}");
}

#[tokio::test]
async fn mkcol_accepts_an_extended_body_with_a_display_name() {
    let env = test_env("mkcol-body").await;

    let body = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
        <d:mkcol xmlns:d=\"DAV:\" xmlns:card=\"urn:ietf:params:xml:ns:carddav\">\
        <d:set><d:prop><d:resourcetype><d:collection/><card:addressbook/></d:resourcetype>\
        <d:displayname>Trabajo</d:displayname></d:prop></d:set></d:mkcol>";
    let uri = format!("/dav/addressbooks/{SUB}/work/");
    let created = dav(&env, "MKCOL", &uri, None, body).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(location(&created), uri);

    let propfind = dav(&env, "PROPFIND", &uri, Some("0"), "").await;
    assert_eq!(propfind.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(propfind).await;
    assert!(xml.contains("Trabajo"), "{xml}");
    assert!(xml.contains("<card:addressbook/>"), "{xml}");
}

#[tokio::test]
async fn mkcol_on_an_existing_path_is_405() {
    let env = test_env("mkcol-repeat").await;

    let uri = format!("/dav/addressbooks/{SUB}/mybook/");
    let first = dav(&env, "MKCOL", &uri, None, "").await;
    assert_eq!(first.status(), StatusCode::CREATED);

    let second = dav(&env, "MKCOL", &uri, None, "").await;
    assert_eq!(second.status(), StatusCode::METHOD_NOT_ALLOWED);
}

// ---------------------------------------------------------------------------
// PROPFIND
// ---------------------------------------------------------------------------

#[tokio::test]
async fn propfind_collection_lists_resources_with_getetag() {
    let env = test_env("collection-list").await;
    make_addressbook(&env, "col-contacts").await;

    let created = dav_with(
        &env,
        "PUT",
        &resource_uri("ada@oxidav"),
        &[],
        &vcf_card("ada@oxidav", "Ada Lovelace", "ada@example.com"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let response = dav(&env, "PROPFIND", &collection_uri(), Some("1"), "").await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(response).await;

    assert!(xml.contains("ada@oxidav.vcf"), "{xml}");
    assert!(xml.contains("getetag"), "{xml}");
    assert!(xml.contains("getcontenttype"), "{xml}");
    assert!(xml.contains("text/vcard"), "{xml}");
    assert!(xml.contains("address-data"), "{xml}");
}

#[tokio::test]
async fn propfind_resource_returns_its_properties() {
    let env = test_env("resource").await;
    make_addressbook(&env, "col-contacts").await;

    let created = dav_with(
        &env,
        "PUT",
        &resource_uri("ada@oxidav"),
        &[],
        &vcf_card("ada@oxidav", "Ada Lovelace", "ada@example.com"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let response = dav(&env, "PROPFIND", &resource_uri("ada@oxidav"), Some("0"), "").await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(response).await;

    assert!(xml.contains("getetag"), "{xml}");
    assert!(xml.contains("text/vcard"), "{xml}");
    assert!(xml.contains("FN:Ada Lovelace"), "{xml}");
}

#[tokio::test]
async fn propfind_propname_returns_names_without_values() {
    let env = test_env("propname").await;
    make_addressbook(&env, "col-contacts").await;

    let body = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
        <d:propfind xmlns:d=\"DAV:\"><d:propname/></d:propfind>";
    let response = dav(&env, "PROPFIND", &collection_uri(), Some("0"), body).await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(response).await;

    assert!(xml.contains("<d:resourcetype/>"), "{xml}");
    assert!(xml.contains("<d:sync-token/>"), "{xml}");
    assert!(
        !xml.contains("<d:displayname>Contactos</d:displayname>"),
        "{xml}"
    );
}

// ---------------------------------------------------------------------------
// PUT / GET / DELETE
// ---------------------------------------------------------------------------

#[tokio::test]
async fn put_creates_a_card_with_201_and_denormalises_it() {
    let env = test_env("put-create").await;
    make_addressbook(&env, "col-contacts").await;

    let vcf = vcf_card("ada@oxidav", "Ada Lovelace", "ada@example.com");
    let response = dav_with(&env, "PUT", &resource_uri("ada@oxidav"), &[], &vcf).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert!(!etag(&response).is_empty(), "201 must carry an ETag");

    let stored = get_card_object(&env.pool, "col-contacts", "ada@oxidav")
        .await
        .expect("query")
        .expect("the card must be persisted");
    assert_eq!(stored.uid, "ada@oxidav");
    assert_eq!(stored.fn_.as_deref(), Some("Ada Lovelace"));
    assert_eq!(stored.email.as_deref(), Some("ada@example.com"));
    assert_eq!(stored.phone.as_deref(), Some("+34600000000"));
    assert_eq!(
        stored.organization.as_deref(),
        Some("Analytical Engines; Research")
    );
    assert_eq!(stored.notes.as_deref(), Some("contacto"));
    assert_eq!(
        stored.raw_payload, vcf,
        "the raw .vcf must be kept verbatim"
    );
}

#[tokio::test]
async fn put_of_the_same_uid_updates_with_204_and_a_new_etag() {
    let env = test_env("put-update").await;
    make_addressbook(&env, "col-contacts").await;

    let created = dav_with(
        &env,
        "PUT",
        &resource_uri("ada@oxidav"),
        &[],
        &vcf_card("ada@oxidav", "Ada Lovelace", "ada@example.com"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let first_etag = etag(&created);

    let updated = dav_with(
        &env,
        "PUT",
        &resource_uri("ada@oxidav"),
        &[],
        &vcf_card("ada@oxidav", "Ada Byron", "ada@example.com"),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::NO_CONTENT);
    assert!(!etag(&updated).is_empty(), "204 must carry an ETag");
    assert_ne!(first_etag, etag(&updated), "the ETag must change");

    let stored = get_card_object(&env.pool, "col-contacts", "ada@oxidav")
        .await
        .expect("query")
        .expect("present");
    assert_eq!(stored.fn_.as_deref(), Some("Ada Byron"));
}

#[tokio::test]
async fn put_with_a_mismatched_body_uid_is_409() {
    let env = test_env("put-uid").await;
    make_addressbook(&env, "col-contacts").await;

    let response = dav_with(
        &env,
        "PUT",
        &resource_uri("ada@oxidav"),
        &[],
        &vcf_card("another@oxidav", "Otra", "otra@example.com"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn if_none_match_star_on_an_existing_card_is_412() {
    let env = test_env("if-none-match").await;
    make_addressbook(&env, "col-contacts").await;

    let first = dav_with(
        &env,
        "PUT",
        &resource_uri("ada@oxidav"),
        &[],
        &vcf_card("ada@oxidav", "Ada", "ada@example.com"),
    )
    .await;
    assert_eq!(first.status(), StatusCode::CREATED);

    let conflict = dav_with(
        &env,
        "PUT",
        &resource_uri("ada@oxidav"),
        &[("If-None-Match", "*")],
        &vcf_card("ada@oxidav", "Ada 2", "ada@example.com"),
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::PRECONDITION_FAILED);

    // The stored payload is untouched.
    let stored = get_card_object(&env.pool, "col-contacts", "ada@oxidav")
        .await
        .expect("query")
        .expect("present");
    assert_eq!(stored.fn_.as_deref(), Some("Ada"));
}

#[tokio::test]
async fn if_match_with_a_stale_etag_is_412() {
    let env = test_env("if-match").await;
    make_addressbook(&env, "col-contacts").await;

    let first = dav_with(
        &env,
        "PUT",
        &resource_uri("ada@oxidav"),
        &[],
        &vcf_card("ada@oxidav", "Ada", "ada@example.com"),
    )
    .await;
    assert_eq!(first.status(), StatusCode::CREATED);

    let stale = dav_with(
        &env,
        "PUT",
        &resource_uri("ada@oxidav"),
        &[("If-Match", "\"0000000000000000\"")],
        &vcf_card("ada@oxidav", "Ada 2", "ada@example.com"),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
}

#[tokio::test]
async fn get_returns_the_vcf_and_its_etag() {
    let env = test_env("get").await;
    make_addressbook(&env, "col-contacts").await;

    let vcf = vcf_card("ada@oxidav", "Ada Lovelace", "ada@example.com");
    let created = dav_with(&env, "PUT", &resource_uri("ada@oxidav"), &[], &vcf).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let put_etag = etag(&created);

    let fetched = dav_with(&env, "GET", &resource_uri("ada@oxidav"), &[], "").await;
    assert_eq!(fetched.status(), StatusCode::OK);
    let content_type = fetched
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(content_type.starts_with("text/vcard"), "{content_type}");
    assert_eq!(etag(&fetched), put_etag, "GET must echo the stored ETag");
    assert_eq!(
        body_text(fetched).await,
        vcf,
        "GET must return the raw .vcf"
    );
}

#[tokio::test]
async fn delete_removes_the_card_and_then_get_is_404() {
    let env = test_env("delete").await;
    make_addressbook(&env, "col-contacts").await;

    let created = dav_with(
        &env,
        "PUT",
        &resource_uri("ada@oxidav"),
        &[],
        &vcf_card("ada@oxidav", "Ada", "ada@example.com"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let deleted = dav_with(&env, "DELETE", &resource_uri("ada@oxidav"), &[], "").await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    let after = dav_with(&env, "GET", &resource_uri("ada@oxidav"), &[], "").await;
    assert_eq!(after.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_of_a_missing_card_is_404() {
    let env = test_env("delete-missing").await;
    make_addressbook(&env, "col-contacts").await;

    let response = dav_with(&env, "DELETE", &resource_uri("nope@oxidav"), &[], "").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Authorization / collection kind
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_foreign_username_is_forbidden() {
    let env = test_env("foreign").await;

    let response = dav(&env, "PROPFIND", "/dav/addressbooks/user-b/", Some("1"), "").await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_calendar_reached_as_an_addressbook_is_404() {
    let env = test_env("calendar-kind").await;

    // A CalDAV calendar owned by the caller, sitting in the same table.
    create_collection(
        &env.pool,
        &NewCollection {
            id: "col-cal".to_string(),
            user_id: SUB.to_string(),
            name: "Agenda".to_string(),
            kind: CollectionKind::Calendar,
            color: "#1677ff".to_string(),
        },
    )
    .await
    .expect("create calendar");

    let propfind = dav(
        &env,
        "PROPFIND",
        &format!("/dav/addressbooks/{SUB}/col-cal/"),
        Some("0"),
        "",
    )
    .await;
    assert_eq!(propfind.status(), StatusCode::NOT_FOUND);

    let put = dav_with(
        &env,
        "PUT",
        &format!("/dav/addressbooks/{SUB}/col-cal/ada@oxidav.vcf"),
        &[],
        &vcf_card("ada@oxidav", "Ada", "ada@example.com"),
    )
    .await;
    assert_eq!(put.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Phase C — REPORT addressbook-query / addressbook-multiget
// ---------------------------------------------------------------------------

/// An `addressbook-query` body with an optional `FN` `text-match` term.
///
/// When `term` is `None` the filter is omitted, meaning "every card".
fn addressbook_query_body(term: Option<&str>) -> String {
    let filter = match term {
        Some(term) => format!(
            "<card:filter><card:prop-filter name=\"FN\">\
             <card:text-match match-type=\"contains\">{term}</card:text-match>\
             </card:prop-filter></card:filter>"
        ),
        None => String::new(),
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <card:addressbook-query xmlns:d=\"DAV:\" xmlns:card=\"urn:ietf:params:xml:ns:carddav\">\
         <d:prop><d:getetag/><card:address-data/></d:prop>{filter}</card:addressbook-query>"
    )
}

/// An `addressbook-multiget` body requesting `hrefs` in order.
fn addressbook_multiget_body(hrefs: &[&str]) -> String {
    let list: String = hrefs
        .iter()
        .map(|href| format!("<d:href>{href}</d:href>"))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <card:addressbook-multiget xmlns:d=\"DAV:\" xmlns:card=\"urn:ietf:params:xml:ns:carddav\">\
         <d:prop><d:getetag/><card:address-data/></d:prop>{list}</card:addressbook-multiget>"
    )
}

/// PUT a card into `col-contacts` and assert it was stored.
async fn put_card(env: &TestEnv, uid: &str, fn_: &str, email: &str) {
    let response = dav_with(
        env,
        "PUT",
        &resource_uri(uid),
        &[],
        &vcf_card(uid, fn_, email),
    )
    .await;
    assert!(
        response.status().is_success(),
        "PUT {uid} failed: {}",
        response.status()
    );
}

#[tokio::test]
async fn addressbook_query_filters_by_fn_and_email() {
    let env = test_env("report-query").await;
    make_addressbook(&env, "col-contacts").await;

    put_card(&env, "ada@oxidav", "Ada Lovelace", "ada@example.com").await;
    put_card(&env, "grace@oxidav", "Grace Hopper", "grace@example.com").await;
    put_card(&env, "alan@oxidav", "Alan Turing", "alan@example.com").await;

    // A term contained in `FN` selects only the matching card.
    let by_fn = dav(
        &env,
        "REPORT",
        &collection_uri(),
        Some("1"),
        &addressbook_query_body(Some("Ada")),
    )
    .await;
    assert_eq!(by_fn.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(by_fn).await;
    assert!(xml.contains("ada@oxidav.vcf"), "{xml}");
    assert!(xml.contains("address-data"), "{xml}");
    assert!(xml.contains("getetag"), "{xml}");
    assert!(xml.contains("FN:Ada Lovelace"), "{xml}");
    assert!(!xml.contains("grace@oxidav"), "{xml}");
    assert!(!xml.contains("alan@oxidav"), "{xml}");

    // A term contained in `EMAIL` selects only the matching card.
    let by_email = dav(
        &env,
        "REPORT",
        &collection_uri(),
        Some("1"),
        &addressbook_query_body(Some("grace@example.com")),
    )
    .await;
    assert_eq!(by_email.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(by_email).await;
    assert!(xml.contains("grace@oxidav.vcf"), "{xml}");
    assert!(!xml.contains("ada@oxidav"), "{xml}");
    assert!(!xml.contains("alan@oxidav"), "{xml}");
}

#[tokio::test]
async fn addressbook_query_without_filter_returns_every_card() {
    let env = test_env("report-query-all").await;
    make_addressbook(&env, "col-contacts").await;
    put_card(&env, "ada@oxidav", "Ada", "ada@example.com").await;
    put_card(&env, "grace@oxidav", "Grace", "grace@example.com").await;

    // A body without a filter, and a completely empty body, both select every
    // card of the collection.
    for body in [addressbook_query_body(None), String::new()] {
        let response = dav(&env, "REPORT", &collection_uri(), Some("1"), &body).await;
        assert_eq!(response.status(), StatusCode::MULTI_STATUS);
        let xml = body_text(response).await;
        assert!(xml.contains("ada@oxidav.vcf"), "{xml}");
        assert!(xml.contains("grace@oxidav.vcf"), "{xml}");
        assert!(xml.contains("address-data"), "{xml}");
    }
}

#[tokio::test]
async fn addressbook_multiget_returns_existing_and_404_for_missing() {
    let env = test_env("report-multiget").await;
    make_addressbook(&env, "col-contacts").await;
    put_card(&env, "ada@oxidav", "Ada Lovelace", "ada@example.com").await;
    put_card(&env, "grace@oxidav", "Grace Hopper", "grace@example.com").await;

    let existing_a = resource_uri("ada@oxidav");
    let existing_g = resource_uri("grace@oxidav");
    let missing = resource_uri("nobody@oxidav");

    let body = addressbook_multiget_body(&[&existing_a, &missing, &existing_g]);
    let response = dav(&env, "REPORT", &collection_uri(), Some("1"), &body).await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(response).await;

    assert!(xml.contains("ada@oxidav.vcf"), "{xml}");
    assert!(xml.contains("FN:Ada Lovelace"), "{xml}");
    assert!(xml.contains("grace@oxidav.vcf"), "{xml}");
    assert!(xml.contains("address-data"), "{xml}");
    assert!(xml.contains("getetag"), "{xml}");
    // The missing href gets its own 404 response.
    assert!(xml.contains(&missing), "{xml}");
    assert!(xml.contains("HTTP/1.1 404 Not Found"), "{xml}");
}

#[tokio::test]
async fn report_with_an_unsupported_body_is_400() {
    let env = test_env("report-bad").await;
    make_addressbook(&env, "col-contacts").await;

    let body = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
        <c:calendar-query xmlns:c=\"urn:ietf:params:xml:ns:caldav\"/>";
    let response = dav(&env, "REPORT", &collection_uri(), Some("1"), body).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn report_on_a_foreign_addressbook_is_forbidden() {
    let env = test_env("report-foreign").await;

    let response = dav(
        &env,
        "REPORT",
        "/dav/addressbooks/user-b/col-contacts/",
        Some("1"),
        &addressbook_query_body(None),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// Phase B (F09) — REPORT sync-collection (RFC 6578)
// ---------------------------------------------------------------------------

/// A `REPORT sync-collection` body.
///
/// `token` is emitted as `<d:sync-token>` when present (`None` = initial sync);
/// `sync_level` is emitted as `<d:sync-level>` when present. A `getetag` prop is
/// always requested so live resources carry their validator.
fn sync_body(token: Option<&str>, sync_level: Option<&str>) -> String {
    let token = token
        .map(|t| format!("<d:sync-token>{t}</d:sync-token>"))
        .unwrap_or_default();
    let level = sync_level
        .map(|l| format!("<d:sync-level>{l}</d:sync-level>"))
        .unwrap_or_default();
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <d:sync-collection xmlns:d=\"DAV:\">{token}{level}\
         <d:prop><d:getetag/></d:prop></d:sync-collection>"
    )
}

/// The `<d:sync-token>` value of a multistatus, or an empty string when absent.
fn sync_token_of(xml: &str) -> String {
    const OPEN: &str = "<d:sync-token>";
    const CLOSE: &str = "</d:sync-token>";
    let start = xml.find(OPEN).map(|i| i + OPEN.len());
    let end = xml.find(CLOSE);
    match (start, end) {
        (Some(s), Some(e)) if s <= e => xml[s..e].to_string(),
        _ => String::new(),
    }
}

/// The `<d:response>…</d:response>` block whose `<d:href>` contains `needle`, or
/// `None` when the multistatus has no such response.
///
/// Lets a test associate a per-resource status (e.g. a tombstone `404`) with the
/// exact `href` it is reported for, rather than matching the status anywhere in
/// the document.
fn response_block_for(xml: &str, needle: &str) -> Option<String> {
    const OPEN: &str = "<d:response>";
    const CLOSE: &str = "</d:response>";
    let mut rest = xml;
    while let Some(start) = rest.find(OPEN) {
        let after = &rest[start..];
        let end = after.find(CLOSE)?;
        let block = &after[..end + CLOSE.len()];
        if block.contains(needle) {
            return Some(block.to_string());
        }
        rest = &after[end + CLOSE.len()..];
    }
    None
}

/// Dispatch a `REPORT sync-collection` against `uri` at depth `0` (RFC 6578
/// pins `sync-collection` to `Depth: 0`).
async fn report_sync(env: &TestEnv, uri: &str, body: &str) -> Response<Body> {
    dav(env, "REPORT", uri, Some("0"), body).await
}

#[tokio::test]
async fn sync_initial_returns_every_resource_with_a_token() {
    let env = test_env("sync-initial").await;
    make_addressbook(&env, "col-contacts").await;
    put_card(&env, "ada@oxidav", "Ada Lovelace", "ada@example.com").await;
    put_card(&env, "grace@oxidav", "Grace Hopper", "grace@example.com").await;

    let response = report_sync(&env, &collection_uri(), &sync_body(None, None)).await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);

    let xml = body_text(response).await;
    assert!(xml.contains("ada@oxidav.vcf"), "{xml}");
    assert!(xml.contains("grace@oxidav.vcf"), "{xml}");
    assert!(xml.contains("getetag"), "{xml}");
    assert!(xml.contains("<d:sync-token>"), "{xml}");
    assert!(!sync_token_of(&xml).is_empty(), "{xml}");
}

#[tokio::test]
async fn sync_incremental_reports_created_updated_and_deleted() {
    let env = test_env("sync-incremental").await;
    make_addressbook(&env, "col-contacts").await;
    put_card(&env, "ada@oxidav", "Ada Lovelace", "ada@example.com").await;
    put_card(&env, "grace@oxidav", "Grace Hopper", "grace@example.com").await;

    let initial = report_sync(&env, &collection_uri(), &sync_body(None, None)).await;
    assert_eq!(initial.status(), StatusCode::MULTI_STATUS);
    let token = sync_token_of(&body_text(initial).await);
    assert!(!token.is_empty());

    // One resource created, one edited, one deleted after the initial sync.
    put_card(&env, "alan@oxidav", "Alan Turing", "alan@example.com").await;
    put_card(&env, "ada@oxidav", "Ada Byron", "ada@example.com").await;
    let deleted = dav_with(&env, "DELETE", &resource_uri("grace@oxidav"), &[], "").await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    let response = report_sync(&env, &collection_uri(), &sync_body(Some(&token), None)).await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);

    let xml = body_text(response).await;
    // Added / updated resources are `200` with their `getetag`.
    assert!(xml.contains("alan@oxidav.vcf"), "{xml}");
    assert!(xml.contains("ada@oxidav.vcf"), "{xml}");
    assert!(xml.contains("getetag"), "{xml}");
    // The deleted resource is a tombstone: the response block whose `href` is
    // exactly the deleted resource is the one carrying the `404`.
    let tombstone = response_block_for(&xml, &resource_uri("grace@oxidav"))
        .expect("a multistatus response for the deleted resource");
    assert!(tombstone.contains("404 Not Found"), "{tombstone}");
    assert!(!tombstone.contains("getetag"), "{tombstone}");
    // A new token is issued for the next round.
    let new_token = sync_token_of(&xml);
    assert!(!new_token.is_empty(), "{xml}");
    assert_ne!(new_token, token);
}

#[tokio::test]
async fn sync_without_changes_returns_an_empty_multistatus() {
    let env = test_env("sync-noop").await;
    make_addressbook(&env, "col-contacts").await;
    put_card(&env, "ada@oxidav", "Ada Lovelace", "ada@example.com").await;

    let initial = report_sync(&env, &collection_uri(), &sync_body(None, None)).await;
    assert_eq!(initial.status(), StatusCode::MULTI_STATUS);
    let token = sync_token_of(&body_text(initial).await);
    assert!(!token.is_empty());

    // A second sync with nothing changed reports no resources and the same token.
    let response = report_sync(&env, &collection_uri(), &sync_body(Some(&token), None)).await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);

    let xml = body_text(response).await;
    assert!(!xml.contains("<d:response>"), "must be empty: {xml}");
    assert_eq!(sync_token_of(&xml), token, "{xml}");
}

#[tokio::test]
async fn sync_with_an_unknown_token_is_forbidden() {
    let env = test_env("sync-bad-token").await;
    make_addressbook(&env, "col-contacts").await;
    put_card(&env, "ada@oxidav", "Ada Lovelace", "ada@example.com").await;

    // A token the server never issued (ahead of the collection's counter) is a
    // failed precondition.
    let ahead = report_sync(
        &env,
        &collection_uri(),
        &sync_body(Some("http://oxidav/ns/sync/999999"), None),
    )
    .await;
    assert_eq!(ahead.status(), StatusCode::FORBIDDEN);

    // A syntactically invalid token is likewise refused.
    let malformed = report_sync(
        &env,
        &collection_uri(),
        &sync_body(Some("not-a-token"), None),
    )
    .await;
    assert_eq!(malformed.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn sync_with_level_infinite_is_400() {
    let env = test_env("sync-infinite").await;
    make_addressbook(&env, "col-contacts").await;

    let response = report_sync(&env, &collection_uri(), &sync_body(None, Some("infinite"))).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sync_with_depth_one_is_400() {
    let env = test_env("sync-depth-one").await;
    make_addressbook(&env, "col-contacts").await;
    put_card(&env, "ada@oxidav", "Ada", "ada@example.com").await;

    // An explicit `Depth: 1` is out of contract for `sync-collection`.
    let response = dav(
        &env,
        "REPORT",
        &collection_uri(),
        Some("1"),
        &sync_body(None, None),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn propfind_collection_advertises_supported_report_set() {
    let env = test_env("supported-report-set").await;
    make_addressbook(&env, "col-contacts").await;

    // Ask explicitly for `DAV:` `supported-report-set`.
    let body = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
        <d:propfind xmlns:d=\"DAV:\"><d:prop>\
        <d:supported-report-set/></d:prop></d:propfind>";
    let response = dav(&env, "PROPFIND", &collection_uri(), Some("0"), body).await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(response).await;

    assert!(xml.contains("supported-report-set"), "{xml}");
    assert!(xml.contains("<d:sync-collection/>"), "{xml}");
}
