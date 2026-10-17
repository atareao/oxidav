//! F07 phase B1 — CalDAV integration tests for `MKCALENDAR` and `PROPFIND`.
//!
//! Each test mounts the full application router over a throwaway file-backed
//! database under `/tmp/opencode`, mints a real F05 app token and presents it via
//! `Authorization: Basic`. Nothing is mocked: the F05 middleware, the SQLite
//! data layer and the DAV handlers all run for real.
//!
//! `MKCALENDAR` / `PROPFIND` are not standard *HTTP* verbs, so they are sent as
//! raw methods via [`Method::from_bytes`] (the server registers the routes with
//! `any()`, exactly as a native client would exercise them).

mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, Response, StatusCode, header};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use oxidav::auth::app_tokens::create_app_token;
use oxidav::config::Config;
use oxidav::db::models::{CollectionKind, NewCalendarObject, NewCollection};
use oxidav::db::{create_collection, get_calendar_object, init_pool, insert_calendar_object};
use oxidav::router::{AppState, build_router};
use sqlx::SqlitePool;
use tower::ServiceExt;

/// The Pocket ID `sub` the test app token is minted for.
const SUB: &str = "user-a";

/// The subject the REST API assumes when auth is disabled
/// ([`oxidav::auth::SessionUser::dev`]).
const DEV_SUB: &str = "dev";

/// The application router, its pool and the Basic credentials of a valid token.
struct TestEnv {
    app: axum::Router,
    pool: SqlitePool,
    basic: String,
}

async fn test_env(tag: &str) -> TestEnv {
    test_env_for(tag, SUB).await
}

/// Like [`test_env`] but mints the DAV app token (and therefore the DAV
/// identity) for `sub`.
///
/// The REST API runs as the dev subject when auth is disabled
/// ([`oxidav::auth::SessionUser::dev`], `sub = "dev"`), so a DAV→REST
/// round-trip must authenticate the DAV side — and own the collection — as
/// `"dev"` to make both surfaces address the same identity.
async fn test_env_for(tag: &str, sub: &str) -> TestEnv {
    let pool: SqlitePool = init_pool(&common::temp_db_url("caldav", tag))
        .await
        .expect("init_pool");
    let (_id, token) = create_app_token(&pool, sub, "dav client")
        .await
        .expect("create app token");

    let config = Config {
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
    };
    let app = build_router(AppState::new(config, None, pool.clone()));

    TestEnv {
        app,
        pool,
        basic: basic_header(sub, &token),
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

/// Build and dispatch a request with arbitrary extra headers (conditionals,
/// `Depth`, …) and a `text/calendar` body, authenticating with `env.basic`.
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
        .header(header::CONTENT_TYPE, "text/calendar; charset=utf-8");
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

/// Create a calendar collection owned by [`SUB`] with a fixed id.
async fn make_calendar(env: &TestEnv, id: &str) {
    make_calendar_owned(env, id, SUB).await;
}

/// Create a calendar collection with a fixed id, owned by `owner`.
async fn make_calendar_owned(env: &TestEnv, id: &str, owner: &str) {
    create_collection(
        &env.pool,
        &NewCollection {
            id: id.to_string(),
            user_id: owner.to_string(),
            name: "Trabajo".to_string(),
            kind: CollectionKind::Calendar,
            color: "#1677ff".to_string(),
        },
    )
    .await
    .expect("create collection");
}

/// A minimal, valid `VCALENDAR` with a single `VEVENT`.
fn ics_event(uid: &str, summary: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//oxidav//test//EN\r\n\
         BEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:{summary}\r\n\
         DTSTART:20240315T100000Z\r\nDTEND:20240315T110000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

/// A `VCALENDAR` whose `VEVENT` anchors `DTSTART`/`DTEND` to `Europe/Madrid`.
///
/// 2024-03-15 is before the 2024 DST change (2024-03-31), so `Europe/Madrid` is
/// CET (UTC+01:00): `10:00` local is `09:00` UTC (`1_710_493_200_000` ms).
fn ics_event_madrid(uid: &str, summary: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//oxidav//test//EN\r\n\
         BEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:{summary}\r\n\
         DTSTART;TZID=Europe/Madrid:20240315T100000\r\n\
         DTEND;TZID=Europe/Madrid:20240315T110000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

/// A `VCALENDAR`/`VEVENT` carrying `LOCATION` and `URL`, used to exercise the DAV
/// denormalisation of both columns (F24 scenario E9, storage half).
fn ics_event_with_location_and_url(uid: &str, summary: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//oxidav//test//EN\r\n\
         BEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:{summary}\r\n\
         LOCATION:Sala 3\r\nURL:https://ejemplo.org/x\r\n\
         DTSTART:20240315T100000Z\r\nDTEND:20240315T110000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

/// The canonical URL of the `evt-1@oxidav` resource in `col-work`.
fn resource_uri() -> String {
    format!("/dav/calendars/{SUB}/col-work/evt-1@oxidav.ics")
}

#[tokio::test]
async fn mkcalendar_returns_201_and_appears_in_the_home_propfind() {
    let env = test_env("mkcalendar").await;

    let created = dav(
        &env,
        "MKCALENDAR",
        &format!("/dav/calendars/{SUB}/"),
        None,
        "",
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let location = location(&created);
    assert!(
        location.starts_with(&format!("/dav/calendars/{SUB}/")),
        "Location must point at the new collection: {location}"
    );
    assert!(location.ends_with('/'), "{location}");

    // The collection is now discoverable through the calendar-home PROPFIND.
    let propfind = dav(
        &env,
        "PROPFIND",
        &format!("/dav/calendars/{SUB}/"),
        Some("1"),
        "",
    )
    .await;
    assert_eq!(propfind.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(propfind).await;
    assert!(
        xml.contains(&location),
        "home PROPFIND lacks {location}: {xml}"
    );
    assert!(
        xml.contains("<c:calendar/>"),
        "collection must be typed as a calendar: {xml}"
    );
    assert!(
        xml.contains("getctag"),
        "collection must expose getctag: {xml}"
    );
}

#[tokio::test]
async fn propfind_home_lists_every_collection() {
    let env = test_env("home-list").await;

    // Two collections, one created over DAV, one directly through the data layer.
    for (id, name) in [("col-work", "Trabajo"), ("col-home", "Personal")] {
        create_collection(
            &env.pool,
            &NewCollection {
                id: id.to_string(),
                user_id: SUB.to_string(),
                name: name.to_string(),
                kind: CollectionKind::Calendar,
                color: "#1677ff".to_string(),
            },
        )
        .await
        .expect("create collection");
    }

    let response = dav(
        &env,
        "PROPFIND",
        &format!("/dav/calendars/{SUB}/"),
        Some("1"),
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(response).await;

    assert!(xml.contains("/dav/calendars/user-a/col-work/"), "{xml}");
    assert!(xml.contains("/dav/calendars/user-a/col-home/"), "{xml}");
    assert!(xml.contains("Trabajo"), "{xml}");
    assert!(xml.contains("Personal"), "{xml}");
}

#[tokio::test]
async fn propfind_collection_lists_resources_with_getetag() {
    let env = test_env("collection-list").await;

    create_collection(
        &env.pool,
        &NewCollection {
            id: "col-work".to_string(),
            user_id: SUB.to_string(),
            name: "Trabajo".to_string(),
            kind: CollectionKind::Calendar,
            color: "#1677ff".to_string(),
        },
    )
    .await
    .expect("create collection");

    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//oxidav//test//EN\r\n\
               BEGIN:VEVENT\r\nUID:evt-1@oxidav\r\nSUMMARY:Reunión\r\n\
               DTSTART:20240315T100000Z\r\nDTEND:20240315T110000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let stored = insert_calendar_object(
        &env.pool,
        &NewCalendarObject {
            id: "obj-1".to_string(),
            collection_id: "col-work".to_string(),
            uid: "evt-1@oxidav".to_string(),
            summary: Some("Reunión".to_string()),
            description: None,
            dtstart: Some(1_710_496_800_000),
            dtend: Some(1_710_500_400_000),
            dtstart_tzid: None,
            dtend_tzid: None,
            location: None,
            url: None,
            rrule: None,
            raw_payload: ics.to_string(),
        },
    )
    .await
    .expect("insert calendar object");

    let response = dav(
        &env,
        "PROPFIND",
        &format!("/dav/calendars/{SUB}/col-work/"),
        Some("1"),
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(response).await;

    assert!(xml.contains("evt-1@oxidav.ics"), "{xml}");
    assert!(xml.contains("getetag"), "{xml}");
    // The serialiser escapes the ETag's surrounding quotes as `&quot;`, so match
    // the opaque hex digest rather than the quoted form.
    assert!(
        xml.contains(stored.etag.trim_matches('"')),
        "the resource ETag must be present: {xml}"
    );
    assert!(xml.contains("text/calendar"), "{xml}");
}

#[tokio::test]
async fn propfind_depth_zero_hides_collection_members() {
    let env = test_env("depth-zero").await;

    create_collection(
        &env.pool,
        &NewCollection {
            id: "col-work".to_string(),
            user_id: SUB.to_string(),
            name: "Trabajo".to_string(),
            kind: CollectionKind::Calendar,
            color: "#1677ff".to_string(),
        },
    )
    .await
    .expect("create collection");
    insert_calendar_object(
        &env.pool,
        &NewCalendarObject {
            id: "obj-1".to_string(),
            collection_id: "col-work".to_string(),
            uid: "evt-1@oxidav".to_string(),
            summary: None,
            description: None,
            dtstart: Some(1_710_496_800_000),
            dtend: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            location: None,
            url: None,
            rrule: None,
            raw_payload: "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n".to_string(),
        },
    )
    .await
    .expect("insert calendar object");

    let response = dav(
        &env,
        "PROPFIND",
        &format!("/dav/calendars/{SUB}/col-work/"),
        Some("0"),
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(response).await;

    // The collection itself is returned, but not its members.
    assert!(xml.contains("/dav/calendars/user-a/col-work/"), "{xml}");
    assert!(!xml.contains("evt-1@oxidav.ics"), "{xml}");
}

#[tokio::test]
async fn propfind_a_missing_resource_is_404() {
    let env = test_env("missing-resource").await;

    create_collection(
        &env.pool,
        &NewCollection {
            id: "col-work".to_string(),
            user_id: SUB.to_string(),
            name: "Trabajo".to_string(),
            kind: CollectionKind::Calendar,
            color: "#1677ff".to_string(),
        },
    )
    .await
    .expect("create collection");

    let response = dav(
        &env,
        "PROPFIND",
        &format!("/dav/calendars/{SUB}/col-work/does-not-exist.ics"),
        Some("0"),
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn propfind_resource_returns_etag_and_calendar_data() {
    let env = test_env("resource").await;

    create_collection(
        &env.pool,
        &NewCollection {
            id: "col-work".to_string(),
            user_id: SUB.to_string(),
            name: "Trabajo".to_string(),
            kind: CollectionKind::Calendar,
            color: "#1677ff".to_string(),
        },
    )
    .await
    .expect("create collection");
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//oxidav//test//EN\r\n\
               BEGIN:VEVENT\r\nUID:evt-1@oxidav\r\nSUMMARY:Reunión\r\n\
               DTSTART:20240315T100000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let stored = insert_calendar_object(
        &env.pool,
        &NewCalendarObject {
            id: "obj-1".to_string(),
            collection_id: "col-work".to_string(),
            uid: "evt-1@oxidav".to_string(),
            summary: Some("Reunión".to_string()),
            description: None,
            dtstart: Some(1_710_496_800_000),
            dtend: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            location: None,
            url: None,
            rrule: None,
            raw_payload: ics.to_string(),
        },
    )
    .await
    .expect("insert calendar object");

    let response = dav(
        &env,
        "PROPFIND",
        &format!("/dav/calendars/{SUB}/col-work/evt-1@oxidav.ics"),
        Some("0"),
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(response).await;

    // The ETag's quotes are XML-escaped; match the opaque hex digest.
    assert!(xml.contains(stored.etag.trim_matches('"')), "{xml}");
    assert!(xml.contains("getcontenttype"), "{xml}");
    assert!(xml.contains("calendar-data"), "{xml}");
    assert!(xml.contains("DTSTART:20240315T100000Z"), "{xml}");
}

#[tokio::test]
async fn a_foreign_username_is_forbidden() {
    let env = test_env("foreign").await;

    // The token belongs to `user-a`; addressing another user's home is a 403.
    let response = dav(&env, "PROPFIND", "/dav/calendars/user-b/", Some("1"), "").await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_unknown_method_is_405() {
    let env = test_env("method-not-allowed").await;

    // `PATCH` is a real HTTP method the DAV tree does not answer.
    let response = dav(&env, "PATCH", &format!("/dav/calendars/{SUB}/"), None, "").await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

// ---------------------------------------------------------------------------
// Phase B2a — PUT / GET / DELETE with ETags and conditional requests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn put_creates_a_resource_with_201_and_denormalises_it() {
    let env = test_env("put-create").await;
    make_calendar(&env, "col-work").await;

    let ics = ics_event("evt-1@oxidav", "Reunión");
    let response = dav_with(&env, "PUT", &resource_uri(), &[], &ics).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert!(!etag(&response).is_empty(), "201 must carry an ETag");

    let stored = get_calendar_object(&env.pool, "col-work", "evt-1@oxidav")
        .await
        .expect("query")
        .expect("the object must be persisted");
    assert_eq!(stored.uid, "evt-1@oxidav");
    assert_eq!(stored.summary.as_deref(), Some("Reunión"));
    assert_eq!(stored.dtstart, Some(1_710_496_800_000));
    assert_eq!(stored.dtend, Some(1_710_500_400_000));
    assert_eq!(
        stored.raw_payload, ics,
        "the raw .ics must be kept verbatim"
    );
}

/// F24 scenario E9 (storage half) — a `PUT` carrying `LOCATION` and `URL`
/// denormalises both into their columns, while the `GET` still returns the raw
/// `.ics` **byte-for-byte** (the flow keeps storing the payload verbatim).
#[tokio::test]
async fn put_denormalises_location_and_url_and_get_stays_verbatim() {
    let env = test_env("put-location-url").await;
    make_calendar(&env, "col-work").await;

    let ics = ics_event_with_location_and_url("evt-lu@oxidav", "Reunión");
    let response = dav_with(&env, "PUT", &uri_for("evt-lu@oxidav"), &[], &ics).await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let stored = get_calendar_object(&env.pool, "col-work", "evt-lu@oxidav")
        .await
        .expect("query")
        .expect("the object must be persisted");
    assert_eq!(stored.location.as_deref(), Some("Sala 3"));
    assert_eq!(stored.url.as_deref(), Some("https://ejemplo.org/x"));

    let fetched = dav_with(&env, "GET", &uri_for("evt-lu@oxidav"), &[], "").await;
    assert_eq!(fetched.status(), StatusCode::OK);
    assert_eq!(
        body_text(fetched).await,
        ics,
        "GET must return the raw .ics byte-for-byte"
    );
}

/// F18 Paso 13 — a `PUT` with `DTSTART;TZID=…` stores the correct UTC instant
/// (10:00 `Europe/Madrid` == 09:00 UTC) **and** the parsed zone, and the raw
/// `.ics` is still kept verbatim.
#[tokio::test]
async fn put_with_tzid_stores_the_utc_instant_and_the_zone() {
    let env = test_env("put-tzid").await;
    make_calendar(&env, "col-work").await;

    let ics = ics_event_madrid("evt-tz@oxidav", "Reunión");
    let response = dav_with(&env, "PUT", &uri_for("evt-tz@oxidav"), &[], &ics).await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let stored = get_calendar_object(&env.pool, "col-work", "evt-tz@oxidav")
        .await
        .expect("query")
        .expect("the object must be persisted");
    // 2024-03-15 10:00 Europe/Madrid (CET, UTC+1) == 09:00 UTC.
    assert_eq!(stored.dtstart, Some(1_710_493_200_000));
    assert_eq!(stored.dtstart_tzid.as_deref(), Some("Europe/Madrid"));
    assert_eq!(stored.dtend_tzid.as_deref(), Some("Europe/Madrid"));
    assert_eq!(
        stored.raw_payload, ics,
        "the raw .ics must be kept verbatim"
    );
}

/// F18 Paso 13 — round-trip DAV→REST: after the `PUT`, the REST `EventDto`
/// reports the parsed zone and the correct UTC instant.
#[tokio::test]
async fn put_with_tzid_round_trips_through_the_rest_api() {
    // The REST API runs as the dev subject when auth is disabled, so the DAV
    // side authenticates (and the collection is owned) as `dev` too.
    let env = test_env_for("tz-roundtrip", DEV_SUB).await;
    make_calendar_owned(&env, "col-work", DEV_SUB).await;

    let ics = ics_event_madrid("evt-tz-1", "Reunión");
    let dav_uri = format!("/dav/calendars/{DEV_SUB}/col-work/evt-tz-1.ics");
    let created = dav_with(&env, "PUT", &dav_uri, &[], &ics).await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/collections/col-work/events/evt-tz-1")
        .body(Body::empty())
        .expect("build request");
    let response = env
        .app
        .clone()
        .oneshot(request)
        .await
        .expect("send request");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON body");
    assert_eq!(value["dtstart_tzid"], "Europe/Madrid");
    assert_eq!(value["dtend_tzid"], "Europe/Madrid");
    assert_eq!(value["dtstart"], 1_710_493_200_000_i64);
}

#[tokio::test]
async fn put_of_the_same_uid_updates_with_204_and_a_new_etag() {
    let env = test_env("put-update").await;
    make_calendar(&env, "col-work").await;

    let created = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[],
        &ics_event("evt-1@oxidav", "Primera"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let first_etag = etag(&created);

    let updated = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[],
        &ics_event("evt-1@oxidav", "Segunda"),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::NO_CONTENT);
    let second_etag = etag(&updated);
    assert!(!second_etag.is_empty(), "204 must carry an ETag");
    assert_ne!(
        first_etag, second_etag,
        "the ETag must change after an update"
    );

    let stored = get_calendar_object(&env.pool, "col-work", "evt-1@oxidav")
        .await
        .expect("query")
        .expect("present");
    assert_eq!(stored.summary.as_deref(), Some("Segunda"));
}

#[tokio::test]
async fn if_none_match_star_on_an_existing_resource_is_412() {
    let env = test_env("put-if-none-match").await;
    make_calendar(&env, "col-work").await;

    let first = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[],
        &ics_event("evt-1@oxidav", "Uno"),
    )
    .await;
    assert_eq!(first.status(), StatusCode::CREATED);

    let conflict = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[("If-None-Match", "*")],
        &ics_event("evt-1@oxidav", "Dos"),
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::PRECONDITION_FAILED);

    // The stored payload is untouched.
    let stored = get_calendar_object(&env.pool, "col-work", "evt-1@oxidav")
        .await
        .expect("query")
        .expect("present");
    assert_eq!(stored.summary.as_deref(), Some("Uno"));
}

#[tokio::test]
async fn if_match_with_a_stale_etag_is_412() {
    let env = test_env("put-if-match").await;
    make_calendar(&env, "col-work").await;

    let first = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[],
        &ics_event("evt-1@oxidav", "Uno"),
    )
    .await;
    assert_eq!(first.status(), StatusCode::CREATED);

    let stale = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[("If-Match", "\"0000000000000000\"")],
        &ics_event("evt-1@oxidav", "Dos"),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
}

#[tokio::test]
async fn get_returns_the_ics_and_its_etag() {
    let env = test_env("get-resource").await;
    make_calendar(&env, "col-work").await;

    let ics = ics_event("evt-1@oxidav", "Reunión");
    let created = dav_with(&env, "PUT", &resource_uri(), &[], &ics).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let put_etag = etag(&created);

    let fetched = dav_with(&env, "GET", &resource_uri(), &[], "").await;
    assert_eq!(fetched.status(), StatusCode::OK);
    let content_type = fetched
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(content_type.starts_with("text/calendar"), "{content_type}");
    assert_eq!(etag(&fetched), put_etag, "GET must echo the stored ETag");
    assert_eq!(
        body_text(fetched).await,
        ics,
        "GET must return the raw .ics"
    );
}

#[tokio::test]
async fn delete_removes_the_resource_and_then_get_is_404() {
    let env = test_env("delete-then-get").await;
    make_calendar(&env, "col-work").await;

    let created = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[],
        &ics_event("evt-1@oxidav", "Uno"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let deleted = dav_with(&env, "DELETE", &resource_uri(), &[], "").await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    let after = dav_with(&env, "GET", &resource_uri(), &[], "").await;
    assert_eq!(after.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_of_a_missing_resource_is_404() {
    let env = test_env("delete-missing").await;
    make_calendar(&env, "col-work").await;

    let response = dav_with(&env, "DELETE", &resource_uri(), &[], "").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_with_a_mismatched_if_match_is_412() {
    let env = test_env("delete-if-match").await;
    make_calendar(&env, "col-work").await;

    let created = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[],
        &ics_event("evt-1@oxidav", "Uno"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let conflict = dav_with(
        &env,
        "DELETE",
        &resource_uri(),
        &[("If-Match", "\"deadbeefdeadbeef\"")],
        "",
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::PRECONDITION_FAILED);

    // The resource survives a rejected delete.
    let still_there = dav_with(&env, "GET", &resource_uri(), &[], "").await;
    assert_eq!(still_there.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Phase B2b — REPORT calendar-query with a time-range filter
// ---------------------------------------------------------------------------

/// A `VCALENDAR`/`VEVENT` with explicit `DTSTART` and optional `DTEND`, both in
/// iCalendar UTC form (`YYYYMMDDTHHMMSSZ`).
fn ics_with_times(uid: &str, summary: &str, dtstart: &str, dtend: Option<&str>) -> String {
    let dtend_line = dtend
        .map(|end| format!("DTEND:{end}\r\n"))
        .unwrap_or_default();
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//oxidav//test//EN\r\n\
         BEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:{summary}\r\n\
         DTSTART:{dtstart}\r\n{dtend_line}END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

/// The canonical URL of a resource named after `uid` in `col-work`.
fn uri_for(uid: &str) -> String {
    format!("/dav/calendars/{SUB}/col-work/{uid}.ics")
}

/// A `REPORT calendar-query` body with an optional `time-range`.
///
/// When `range` is `Some((start, end))` both attributes are emitted; when it is
/// `None` the filter carries no `time-range`, meaning "every resource".
fn calendar_query_body(range: Option<(&str, &str)>) -> String {
    let filter = match range {
        Some((start, end)) => format!(
            "<c:filter><c:comp-filter name=\"VCALENDAR\"><c:comp-filter name=\"VEVENT\">\
             <c:time-range start=\"{start}\" end=\"{end}\"/>\
             </c:comp-filter></c:comp-filter></c:filter>"
        ),
        None => String::new(),
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <c:calendar-query xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\">\
         <d:prop><d:getetag/><c:calendar-data/></d:prop>{filter}</c:calendar-query>"
    )
}

/// PUT a `.ics` resource into `col-work` and assert it was stored.
async fn put_ics(env: &TestEnv, uid: &str, ics: &str) {
    let response = dav_with(env, "PUT", &uri_for(uid), &[], ics).await;
    assert!(
        response.status().is_success(),
        "PUT {uid} failed: {}",
        response.status()
    );
}

#[tokio::test]
async fn report_time_range_returns_only_overlapping_events() {
    let env = test_env("report-overlap").await;
    make_calendar(&env, "col-work").await;

    // One event before, one inside and one after the queried window.
    put_ics(
        &env,
        "evt-jan",
        &ics_with_times(
            "evt-jan",
            "Enero",
            "20240115T100000Z",
            Some("20240115T110000Z"),
        ),
    )
    .await;
    put_ics(
        &env,
        "evt-mar",
        &ics_with_times(
            "evt-mar",
            "Marzo",
            "20240315T100000Z",
            Some("20240315T110000Z"),
        ),
    )
    .await;
    put_ics(
        &env,
        "evt-may",
        &ics_with_times(
            "evt-may",
            "Mayo",
            "20240515T100000Z",
            Some("20240515T110000Z"),
        ),
    )
    .await;

    let body = calendar_query_body(Some(("20240301T000000Z", "20240401T000000Z")));
    let response = dav(
        &env,
        "REPORT",
        &format!("/dav/calendars/{SUB}/col-work/"),
        Some("1"),
        &body,
    )
    .await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);

    let xml = body_text(response).await;
    assert!(xml.contains("evt-mar.ics"), "{xml}");
    assert!(!xml.contains("evt-jan"), "January must be excluded: {xml}");
    assert!(!xml.contains("evt-may"), "May must be excluded: {xml}");
    // The matching resource carries `getetag` and the raw `calendar-data`.
    assert!(xml.contains("getetag"), "{xml}");
    assert!(xml.contains("calendar-data"), "{xml}");
    assert!(xml.contains("DTSTART:20240315T100000Z"), "{xml}");
    assert!(xml.contains("UID:evt-mar"), "{xml}");
}

#[tokio::test]
async fn report_without_time_range_returns_every_resource() {
    let env = test_env("report-all").await;
    make_calendar(&env, "col-work").await;

    put_ics(
        &env,
        "evt-one",
        &ics_with_times("evt-one", "Uno", "20240115T100000Z", None),
    )
    .await;
    put_ics(
        &env,
        "evt-two",
        &ics_with_times("evt-two", "Dos", "20240620T080000Z", None),
    )
    .await;

    let response = dav(
        &env,
        "REPORT",
        &format!("/dav/calendars/{SUB}/col-work/"),
        Some("1"),
        &calendar_query_body(None),
    )
    .await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);

    let xml = body_text(response).await;
    assert!(xml.contains("evt-one.ics"), "{xml}");
    assert!(xml.contains("evt-two.ics"), "{xml}");
    assert!(xml.contains("getetag"), "{xml}");
    assert!(xml.contains("calendar-data"), "{xml}");
}

#[tokio::test]
async fn report_with_a_non_overlapping_range_is_an_empty_multistatus() {
    let env = test_env("report-empty").await;
    make_calendar(&env, "col-work").await;

    put_ics(
        &env,
        "evt-mar",
        &ics_with_times(
            "evt-mar",
            "Marzo",
            "20240315T100000Z",
            Some("20240315T110000Z"),
        ),
    )
    .await;

    // A window in June does not overlap the March event.
    let body = calendar_query_body(Some(("20240601T000000Z", "20240701T000000Z")));
    let response = dav(
        &env,
        "REPORT",
        &format!("/dav/calendars/{SUB}/col-work/"),
        Some("1"),
        &body,
    )
    .await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);

    let xml = body_text(response).await;
    assert!(xml.contains("<d:multistatus"), "{xml}");
    assert!(!xml.contains("evt-mar"), "must be empty: {xml}");
}

#[tokio::test]
async fn report_treats_an_event_without_dtend_as_an_instant() {
    let env = test_env("report-point").await;
    make_calendar(&env, "col-work").await;

    // No DTEND: a zero-duration instant on 2024-03-15 at 10:00 UTC.
    put_ics(
        &env,
        "evt-point",
        &ics_with_times("evt-point", "Instante", "20240315T100000Z", None),
    )
    .await;

    // A window that contains the instant matches it.
    let inside = dav(
        &env,
        "REPORT",
        &format!("/dav/calendars/{SUB}/col-work/"),
        Some("1"),
        &calendar_query_body(Some(("20240315T000000Z", "20240316T000000Z"))),
    )
    .await;
    assert_eq!(inside.status(), StatusCode::MULTI_STATUS);
    assert!(body_text(inside).await.contains("evt-point.ics"));

    // A window entirely before the instant does not.
    let before = dav(
        &env,
        "REPORT",
        &format!("/dav/calendars/{SUB}/col-work/"),
        Some("1"),
        &calendar_query_body(Some(("20240301T000000Z", "20240315T000000Z"))),
    )
    .await;
    assert_eq!(before.status(), StatusCode::MULTI_STATUS);
    assert!(!body_text(before).await.contains("evt-point"));
}

#[tokio::test]
async fn report_on_a_foreign_calendar_is_forbidden() {
    let env = test_env("report-foreign").await;

    let response = dav(
        &env,
        "REPORT",
        "/dav/calendars/user-b/col-work/",
        Some("1"),
        &calendar_query_body(None),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// Phase B3 — PROPFIND property negotiation (allprop / propname / <d:prop>)
// ---------------------------------------------------------------------------

/// A `PROPFIND` body selecting property names only.
const PROPNAME_BODY: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
     <d:propfind xmlns:d=\"DAV:\"><d:propname/></d:propfind>";

/// A `PROPFIND` body requesting an explicit set of properties.
fn prop_body(inner: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <d:propfind xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\">\
         <d:prop>{inner}</d:prop></d:propfind>"
    )
}

#[tokio::test]
async fn propfind_allprop_returns_every_cached_property() {
    let env = test_env("propfind-allprop").await;
    make_calendar(&env, "col-work").await;

    let body = "<?xml version=\"1.0\" encoding=\"utf-8\"?><d:propfind xmlns:d=\"DAV:\"><d:allprop/></d:propfind>";
    let response = dav(
        &env,
        "PROPFIND",
        &format!("/dav/calendars/{SUB}/col-work/"),
        Some("0"),
        body,
    )
    .await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(response).await;

    // The full collection set comes back.
    assert!(xml.contains("resourcetype"), "{xml}");
    assert!(xml.contains("displayname"), "{xml}");
    assert!(xml.contains("getctag"), "{xml}");
    assert!(xml.contains("sync-token"), "{xml}");
    assert!(xml.contains("<c:calendar/>"), "{xml}");
}

#[tokio::test]
async fn propfind_with_an_empty_body_is_allprop() {
    let env = test_env("propfind-empty").await;
    make_calendar(&env, "col-work").await;

    let response = dav(
        &env,
        "PROPFIND",
        &format!("/dav/calendars/{SUB}/col-work/"),
        Some("0"),
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(response).await;

    assert!(xml.contains("resourcetype"), "{xml}");
    assert!(xml.contains("displayname"), "{xml}");
    assert!(xml.contains("sync-token"), "{xml}");
}

#[tokio::test]
async fn propfind_propname_returns_names_without_values() {
    let env = test_env("propfind-propname").await;
    make_calendar(&env, "col-work").await;

    let response = dav(
        &env,
        "PROPFIND",
        &format!("/dav/calendars/{SUB}/col-work/"),
        Some("0"),
        PROPNAME_BODY,
    )
    .await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(response).await;

    // Names are present ...
    assert!(xml.contains("<d:resourcetype/>"), "{xml}");
    assert!(xml.contains("<d:displayname/>"), "{xml}");
    assert!(xml.contains("<d:sync-token/>"), "{xml}");
    // ... but without their values.
    assert!(
        !xml.contains("<d:displayname>Trabajo</d:displayname>"),
        "{xml}"
    );
    assert!(!xml.contains("getctag\">"), "{xml}");
}

#[tokio::test]
async fn propfind_explicit_prop_returns_only_the_requested_properties() {
    let env = test_env("propfind-explicit").await;
    make_calendar(&env, "col-work").await;

    let body = prop_body("<d:resourcetype/><d:sync-token/>");
    let response = dav(
        &env,
        "PROPFIND",
        &format!("/dav/calendars/{SUB}/col-work/"),
        Some("0"),
        &body,
    )
    .await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(response).await;

    // Only the requested ones.
    assert!(xml.contains("resourcetype"), "{xml}");
    assert!(xml.contains("sync-token"), "{xml}");
    // Not the ones that were not requested.
    assert!(!xml.contains("displayname"), "{xml}");
    assert!(!xml.contains("getctag"), "{xml}");
}

#[tokio::test]
async fn propfind_unknown_requested_property_is_reported_as_404() {
    let env = test_env("propfind-not-found").await;
    make_calendar(&env, "col-work").await;

    let body = prop_body("<d:resourcetype/><d:does-not-exist/>");
    let response = dav(
        &env,
        "PROPFIND",
        &format!("/dav/calendars/{SUB}/col-work/"),
        Some("0"),
        &body,
    )
    .await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(response).await;

    // The defined property is in the `200` propstat, the unknown one is in the
    // `404` propstat.
    assert!(
        xml.contains("<d:status>HTTP/1.1 200 OK</d:status>"),
        "{xml}"
    );
    assert!(
        xml.contains("<d:status>HTTP/1.1 404 Not Found</d:status>"),
        "{xml}"
    );
    assert!(xml.contains("does-not-exist"), "{xml}");
    assert!(xml.contains("resourcetype"), "{xml}");
}

#[tokio::test]
async fn propfind_requested_resource_property_returns_getetag() {
    let env = test_env("propfind-res-getetag").await;
    make_calendar(&env, "col-work").await;

    let created = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[],
        &ics_event("evt-1@oxidav", "Reunión"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let body = prop_body("<d:getetag/><d:sync-token/>");
    let response = dav(&env, "PROPFIND", &resource_uri(), Some("0"), &body).await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(response).await;

    // `getetag` is defined on a resource; `sync-token` is not.
    assert!(xml.contains("getetag"), "{xml}");
    assert!(
        xml.contains("<d:status>HTTP/1.1 404 Not Found</d:status>"),
        "{xml}"
    );
    // `calendar-data` was not requested and must not leak.
    assert!(!xml.contains("calendar-data"), "{xml}");
}

// ---------------------------------------------------------------------------
// Phase B3 — strong vs. weak ETag comparison for If-Match / If-None-Match
// ---------------------------------------------------------------------------

#[tokio::test]
async fn if_match_with_a_weak_validator_is_412_even_when_the_etag_matches() {
    let env = test_env("put-if-match-weak").await;
    make_calendar(&env, "col-work").await;

    let created = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[],
        &ics_event("evt-1@oxidav", "Uno"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let stored_etag = etag(&created);

    // A weak validator matching the current ETag must NOT satisfy If-Match.
    let weak = format!("W/{stored_etag}");
    let response = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[("If-Match", &weak)],
        &ics_event("evt-1@oxidav", "Dos"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);

    // The stored payload is untouched.
    let stored = get_calendar_object(&env.pool, "col-work", "evt-1@oxidav")
        .await
        .expect("query")
        .expect("present");
    assert_eq!(stored.summary.as_deref(), Some("Uno"));
}

#[tokio::test]
async fn if_match_with_the_exact_strong_etag_passes() {
    let env = test_env("put-if-match-strong").await;
    make_calendar(&env, "col-work").await;

    let created = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[],
        &ics_event("evt-1@oxidav", "Uno"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let stored_etag = etag(&created);

    let updated = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[("If-Match", &stored_etag)],
        &ics_event("evt-1@oxidav", "Dos"),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::NO_CONTENT);

    let stored = get_calendar_object(&env.pool, "col-work", "evt-1@oxidav")
        .await
        .expect("query")
        .expect("present");
    assert_eq!(stored.summary.as_deref(), Some("Dos"));
}

#[tokio::test]
async fn if_none_match_with_a_weak_validator_matches() {
    let env = test_env("put-if-none-match-weak").await;
    make_calendar(&env, "col-work").await;

    let created = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[],
        &ics_event("evt-1@oxidav", "Uno"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let stored_etag = etag(&created);

    // A weak validator matching the current ETag DOES satisfy If-None-Match
    // (weak comparison), so the PUT is rejected.
    let weak = format!("W/{stored_etag}");
    let response = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[("If-None-Match", &weak)],
        &ics_event("evt-1@oxidav", "Dos"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
}

#[tokio::test]
async fn delete_if_match_with_a_weak_validator_is_412() {
    let env = test_env("delete-if-match-weak").await;
    make_calendar(&env, "col-work").await;

    let created = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[],
        &ics_event("evt-1@oxidav", "Uno"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let stored_etag = etag(&created);

    let weak = format!("W/{stored_etag}");
    let response = dav_with(&env, "DELETE", &resource_uri(), &[("If-Match", &weak)], "").await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);

    // The resource survives.
    let still_there = dav_with(&env, "GET", &resource_uri(), &[], "").await;
    assert_eq!(still_there.status(), StatusCode::OK);
}

#[tokio::test]
async fn put_with_a_body_uid_different_from_the_path_is_a_conflict() {
    let env = test_env("put-uid-mismatch").await;
    make_calendar(&env, "col-work").await;

    // The URL names `evt-1@oxidav` but the body declares another UID.
    let response = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[],
        &ics_event("another@oxidav", "Desincronizado"),
    )
    .await;
    assert!(response.status().is_client_error(), "{}", response.status());
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

// ---------------------------------------------------------------------------
// F07 hardening — m1 / m2 / m5 / m7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn m1_addressbook_reached_as_a_calendar_is_404() {
    let env = test_env("m1-addressbook").await;

    // A CardDAV address book owned by the caller, sitting in the collections
    // table alongside calendars.
    create_collection(
        &env.pool,
        &NewCollection {
            id: "col-addr".to_string(),
            user_id: SUB.to_string(),
            name: "Contactos".to_string(),
            kind: CollectionKind::Addressbook,
            color: "#1677ff".to_string(),
        },
    )
    .await
    .expect("create address book");

    // Addressing it through the calendar tree must not treat it as a calendar;
    // it is reported exactly like a missing collection.
    let propfind = dav(
        &env,
        "PROPFIND",
        &format!("/dav/calendars/{SUB}/col-addr/"),
        Some("0"),
        "",
    )
    .await;
    assert_eq!(propfind.status(), StatusCode::NOT_FOUND);

    let report = dav(
        &env,
        "REPORT",
        &format!("/dav/calendars/{SUB}/col-addr/"),
        Some("1"),
        &calendar_query_body(None),
    )
    .await;
    assert_eq!(report.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn m5_mkcalendar_on_a_collection_path_adopts_the_segment() {
    let env = test_env("m5-mkcalendar-segment").await;

    let uri = format!("/dav/calendars/{SUB}/my-cal/");
    let created = dav(&env, "MKCALENDAR", &uri, None, "").await;
    assert_eq!(created.status(), StatusCode::CREATED);

    // `Location` must echo the requested URL, not a surrogate UUID.
    assert_eq!(location(&created), uri);

    // The new collection is reachable at the very same path.
    let propfind = dav(&env, "PROPFIND", &uri, Some("0"), "").await;
    assert_eq!(propfind.status(), StatusCode::MULTI_STATUS);
    assert!(body_text(propfind).await.contains("<c:calendar/>"));
}

#[tokio::test]
async fn m5_mkcalendar_with_an_invalid_segment_is_400() {
    let env = test_env("m5-mkcalendar-invalid").await;

    // `a%20b` decodes to `a b`, whose space is outside the accepted charset.
    let response = dav(
        &env,
        "MKCALENDAR",
        &format!("/dav/calendars/{SUB}/a%20b/"),
        None,
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn m2_delete_with_if_none_match_star_is_412() {
    let env = test_env("m2-delete-if-none-match").await;
    make_calendar(&env, "col-work").await;

    let created = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[],
        &ics_event("evt-1@oxidav", "Uno"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    // `If-None-Match: *` means "only if it does not exist"; the resource does,
    // so the DELETE is refused.
    let refused = dav_with(
        &env,
        "DELETE",
        &resource_uri(),
        &[("If-None-Match", "*")],
        "",
    )
    .await;
    assert_eq!(refused.status(), StatusCode::PRECONDITION_FAILED);

    // The resource survives.
    let still_there = dav_with(&env, "GET", &resource_uri(), &[], "").await;
    assert_eq!(still_there.status(), StatusCode::OK);
}

#[tokio::test]
async fn m7_get_without_the_ics_suffix_is_404() {
    let env = test_env("m7-get-no-suffix").await;
    make_calendar(&env, "col-work").await;

    let created = dav_with(
        &env,
        "PUT",
        &resource_uri(),
        &[],
        &ics_event("evt-1@oxidav", "Uno"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    // The same resource addressed without `.ics` is not the same identity.
    let no_suffix = dav_with(
        &env,
        "GET",
        &format!("/dav/calendars/{SUB}/col-work/evt-1@oxidav"),
        &[],
        "",
    )
    .await;
    assert_eq!(no_suffix.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Phase B (F09) — REPORT sync-collection (RFC 6578)
// ---------------------------------------------------------------------------

/// The canonical URL of the `col-work` collection.
fn collection_uri() -> String {
    format!("/dav/calendars/{SUB}/col-work/")
}

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
    make_calendar(&env, "col-work").await;
    put_ics(&env, "evt-one", &ics_event("evt-one", "Uno")).await;
    put_ics(&env, "evt-two", &ics_event("evt-two", "Dos")).await;

    let response = report_sync(&env, &collection_uri(), &sync_body(None, None)).await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);

    let xml = body_text(response).await;
    assert!(xml.contains("evt-one.ics"), "{xml}");
    assert!(xml.contains("evt-two.ics"), "{xml}");
    assert!(xml.contains("getetag"), "{xml}");
    assert!(xml.contains("<d:sync-token>"), "{xml}");
    assert!(!sync_token_of(&xml).is_empty(), "{xml}");
}

#[tokio::test]
async fn sync_incremental_reports_created_updated_and_deleted() {
    let env = test_env("sync-incremental").await;
    make_calendar(&env, "col-work").await;
    put_ics(&env, "evt-one", &ics_event("evt-one", "Uno")).await;
    put_ics(&env, "evt-two", &ics_event("evt-two", "Dos")).await;

    let initial = report_sync(&env, &collection_uri(), &sync_body(None, None)).await;
    assert_eq!(initial.status(), StatusCode::MULTI_STATUS);
    let token = sync_token_of(&body_text(initial).await);
    assert!(!token.is_empty());

    // One resource created, one edited, one deleted after the initial sync.
    put_ics(&env, "evt-three", &ics_event("evt-three", "Tres")).await;
    put_ics(&env, "evt-one", &ics_event("evt-one", "Uno editado")).await;
    let deleted = dav_with(&env, "DELETE", &uri_for("evt-two"), &[], "").await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    let response = report_sync(&env, &collection_uri(), &sync_body(Some(&token), None)).await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);

    let xml = body_text(response).await;
    // Added / updated resources are `200` with their `getetag`.
    assert!(xml.contains("evt-three.ics"), "{xml}");
    assert!(xml.contains("evt-one.ics"), "{xml}");
    assert!(xml.contains("getetag"), "{xml}");
    // The deleted resource is a tombstone: the response block whose `href` is
    // exactly the deleted resource is the one carrying the `404`.
    let tombstone = response_block_for(&xml, &uri_for("evt-two"))
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
    make_calendar(&env, "col-work").await;
    put_ics(&env, "evt-one", &ics_event("evt-one", "Uno")).await;

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
    make_calendar(&env, "col-work").await;
    put_ics(&env, "evt-one", &ics_event("evt-one", "Uno")).await;

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
    make_calendar(&env, "col-work").await;

    let response = report_sync(&env, &collection_uri(), &sync_body(None, Some("infinite"))).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sync_with_depth_one_is_400() {
    let env = test_env("sync-depth-one").await;
    make_calendar(&env, "col-work").await;
    put_ics(&env, "evt-one", &ics_event("evt-one", "Uno")).await;

    // RFC 6578 pins `sync-collection` to `Depth: 0`; an explicit `Depth: 1`
    // must not be silently coerced to a collection sweep.
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
    make_calendar(&env, "col-work").await;

    // Ask explicitly for `DAV:` `supported-report-set`.
    let body = prop_body("<d:supported-report-set/>");
    let response = dav(
        &env,
        "PROPFIND",
        &format!("/dav/calendars/{SUB}/col-work/"),
        Some("0"),
        &body,
    )
    .await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(response).await;

    assert!(xml.contains("supported-report-set"), "{xml}");
    assert!(xml.contains("<d:sync-collection/>"), "{xml}");
}

#[tokio::test]
async fn href_percent_encodes_a_uid_needing_escaping() {
    let env = test_env("href-encoding").await;
    make_calendar(&env, "col-work").await;

    // A UID carrying a space: the URL must use its percent-encoded form.
    let uid = "evt con espacio";
    let uri = format!("/dav/calendars/{SUB}/col-work/evt%20con%20espacio.ics");

    let created = dav_with(&env, "PUT", &uri, &[], &ics_event(uid, "Codificado")).await;
    assert_eq!(created.status(), StatusCode::CREATED);

    // GET at the encoded href resolves the stored resource.
    let fetched = dav_with(&env, "GET", &uri, &[], "").await;
    assert_eq!(fetched.status(), StatusCode::OK);

    // A collection PROPFIND emits the percent-encoded href, never the raw UID.
    let list = dav(
        &env,
        "PROPFIND",
        &format!("/dav/calendars/{SUB}/col-work/"),
        Some("1"),
        "",
    )
    .await;
    assert_eq!(list.status(), StatusCode::MULTI_STATUS);
    let xml = body_text(list).await;
    assert!(xml.contains("evt%20con%20espacio.ics"), "{xml}");
    assert!(
        !xml.contains("evt con espacio.ics"),
        "the raw UID must not leak into an href: {xml}"
    );
}
