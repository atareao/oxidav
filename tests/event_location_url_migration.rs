//! F24 — update-path coverage for `migrations/0005_event_location_url.sql`.
//!
//! Every other test in the suite builds the database from scratch (the migrator
//! runs `0001`–`0005` in order on an empty file), so nothing exercises the
//! *upgrade* path: applying the two `ALTER TABLE ... ADD COLUMN` statements of
//! `0005` on top of a database that already carries the `0001`–`0004` schema, its
//! rows, and the external-content `events_fts` index mirroring them.
//!
//! That is exactly what happens on an existing install, so this file mounts the
//! legacy schema and its data by hand (mirroring `tests/timezone_migration.rs`),
//! then applies `0005` on top and asserts:
//!
//! 1. `calendar_objects` now carries the `location` and `url` columns;
//! 2. the pre-existing row survives with both new columns `NULL` — even though its
//!    `raw_payload` already contains the `LOCATION:`/`URL:` lines — and its
//!    payload is byte-for-byte intact;
//! 3. the external-content FTS search still finds it (the `ALTER TABLE` did not
//!    break the `rowid` linkage / triggers);
//! 4. a new event carrying `location`/`url` round-trips intact through the real
//!    repository helpers.
//!
//! # Why the migration is applied on a pool that is then closed
//!
//! `sqlx-sqlite` caches prepared statements per connection *without* invalidating
//! them on a schema change. When `0005` runs on the same pool that later serves
//! queries, a connection can end up reusing a statement whose cached column
//! metadata predates the `ALTER TABLE` (14 columns) while SQLite returns the new
//! column count (16), which makes `sqlx` panic with
//! `index out of bounds: the len is 14 but the index is 14`. Applying the DDL on
//! a dedicated pool and reconnecting afterwards mirrors what a restart does (fresh
//! connections observe the migrated schema) and keeps the assertions below about
//! the *database*, not about `sqlx`'s statement cache.

mod common;

use std::str::FromStr;

use oxidav::db::search::search_events;
use oxidav::db::{NewCalendarObject, get_calendar_object, insert_calendar_object};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Executor, SqlitePool};

/// Migrations replayed by hand, embedded so the test does not depend on the
/// migrator's version-tracking table.
const INIT: &str = include_str!("../migrations/0001_init.sql");
const APP_TOKENS: &str = include_str!("../migrations/0002_app_tokens.sql");
const SYNC_LOG: &str = include_str!("../migrations/0003_sync_log.sql");
const TIMEZONE: &str = include_str!("../migrations/0004_timezone.sql");
/// The migration under test (the one that adds `location`/`url`).
const LOCATION_URL: &str = include_str!("../migrations/0005_event_location_url.sql");

/// The legacy `.ics`: it already carries `LOCATION` and `URL` (the premise of
/// scenario E14), but on the pre-`0005` schema there is nowhere to denormalise
/// them, so they live only inside `raw_payload`.
const LEGACY_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//oxidav//test//EN\r\n\
                           BEGIN:VEVENT\r\nUID:uid-1\r\nSUMMARY:Weekly planning\r\n\
                           LOCATION:Sala 3\r\nURL:https://ejemplo.org/x\r\n\
                           DTSTART:20240315T100000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

/// A raw pool (no migrator) so the legacy schema can be replayed file by file.
async fn open(url: &str) -> SqlitePool {
    let options = SqliteConnectOptions::from_str(url)
        .expect("valid dsn")
        .create_if_missing(true);
    SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .expect("connect raw pool")
}

/// Replay the pre-`0005` schema exactly as an existing install would have it.
async fn seed_legacy_schema(pool: &SqlitePool) {
    pool.execute(INIT).await.expect("apply 0001_init");
    pool.execute(APP_TOKENS)
        .await
        .expect("apply 0002_app_tokens");
    pool.execute(SYNC_LOG).await.expect("apply 0003_sync_log");
    pool.execute(TIMEZONE).await.expect("apply 0004_timezone");
}

/// Populate the legacy schema using only the *old* `calendar_objects` columns
/// (no `location`/`url` columns exist yet).
async fn seed_legacy_data(pool: &SqlitePool) {
    sqlx::query(
        "INSERT INTO collections
            (id, user_id, name, type, color, sync_token, created_at, updated_at)
         VALUES ('cal1', 'dev', 'Personal', 'calendar', '#1677ff', 1, 0, 0)",
    )
    .execute(pool)
    .await
    .expect("insert collection");

    sqlx::query(
        "INSERT INTO calendar_objects
            (id, collection_id, uid, summary, description, dtstart, dtend, rrule,
             dtstart_tzid, dtend_tzid, etag, raw_payload, created_at, updated_at)
         VALUES ('evt1', 'cal1', 'uid-1', 'Weekly planning', 'Plan the sprint',
                 1000, 2000, NULL, NULL, NULL, '\"etag1\"', ?, 0, 0)",
    )
    .bind(LEGACY_ICS)
    .execute(pool)
    .await
    .expect("insert legacy calendar object");
}

/// Column names of `calendar_objects`, in schema order.
async fn column_names(pool: &SqlitePool) -> Vec<String> {
    sqlx::query_scalar("SELECT name FROM pragma_table_info('calendar_objects') ORDER BY cid")
        .fetch_all(pool)
        .await
        .expect("pragma_table_info")
}

/// Summaries returned by the `events_fts` index for a literal substring query.
///
/// Queries `MATCH` directly (not through the public helper, which decodes the
/// full row and therefore cannot run before `0005` adds its columns). The `trigram`
/// tokenizer needs at least three characters; the operand is a quoted phrase so
/// it is always a valid FTS5 expression.
async fn fts_summaries(pool: &SqlitePool, needle: &str) -> Vec<String> {
    let operand = format!("\"{needle}\"");
    sqlx::query_scalar(
        "SELECT c.summary
           FROM calendar_objects c
           JOIN events_fts f ON f.rowid = c.rowid
          WHERE events_fts MATCH ?
          ORDER BY bm25(events_fts) ASC",
    )
    .bind(operand)
    .fetch_all(pool)
    .await
    .expect("fts query")
}

/// Apply `0005` on a dedicated pool, then reconnect (see module docs).
async fn migrate_and_reconnect(url: &str, pool: SqlitePool) -> SqlitePool {
    pool.execute(LOCATION_URL)
        .await
        .expect("apply 0005_event_location_url");
    pool.close().await;
    open(url).await
}

#[tokio::test]
async fn migration_adds_columns_and_preserves_the_legacy_row() {
    let url = common::temp_db_url("location-url", "preserve");
    let pool = open(&url).await;
    seed_legacy_schema(&pool).await;
    seed_legacy_data(&pool).await;

    // Sanity: on the legacy schema the AFTER INSERT trigger already indexed the
    // row into events_fts (needs >= 3 chars — "planning").
    assert_eq!(
        fts_summaries(&pool, "planning").await,
        vec!["Weekly planning"],
        "legacy FTS indexing must work before the migration"
    );

    let pool = migrate_and_reconnect(&url, pool).await;

    // (a) Both new columns exist on `calendar_objects`.
    let columns = column_names(&pool).await;
    assert!(
        columns.iter().any(|c| c == "location"),
        "missing location column: {columns:?}"
    );
    assert!(
        columns.iter().any(|c| c == "url"),
        "missing url column: {columns:?}"
    );

    // (b) The pre-existing row survives; both new columns default to NULL (the
    // migration does not backfill from `raw_payload`) and the payload is intact
    // byte-for-byte, `LOCATION`/`URL` lines included.
    let (summary, location, url_col, raw_payload): (
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    ) = sqlx::query_as(
        "SELECT summary, location, url, raw_payload
           FROM calendar_objects WHERE id = 'evt1'",
    )
    .fetch_one(&pool)
    .await
    .expect("row must survive the ALTER TABLE");
    assert_eq!(summary.as_deref(), Some("Weekly planning"));
    assert_eq!(location, None, "location must default to NULL");
    assert_eq!(url_col, None, "url must default to NULL");
    assert_eq!(
        raw_payload, LEGACY_ICS,
        "the stored .ics must be preserved byte-for-byte"
    );

    // (c) The external-content FTS index still finds the row: the ALTER TABLE did
    // not break the rowid linkage between calendar_objects and events_fts.
    assert_eq!(
        fts_summaries(&pool, "planning").await,
        vec!["Weekly planning"],
        "FTS search must still work after the ALTER TABLE"
    );

    // ... and through the real, public search helper as well.
    let hits = search_events(&pool, "planning", 10)
        .await
        .expect("search_events after migration");
    assert_eq!(hits.len(), 1, "exactly the pre-existing event");
    assert_eq!(hits[0].id, "evt1");
    assert_eq!(hits[0].location, None);
    assert_eq!(hits[0].url, None);
}

#[tokio::test]
async fn migration_allows_inserting_and_reading_back_location_and_url() {
    let url = common::temp_db_url("location-url", "roundtrip");
    let pool = open(&url).await;
    seed_legacy_schema(&pool).await;
    seed_legacy_data(&pool).await;
    let pool = migrate_and_reconnect(&url, pool).await;

    // (d) After the migration a new event can carry `LOCATION`/`URL` and
    // round-trips intact through the real repository helpers.
    let new = NewCalendarObject {
        id: "evt2".to_string(),
        collection_id: "cal1".to_string(),
        uid: "uid-2".to_string(),
        summary: Some("Madrid standup".to_string()),
        description: Some("Daily standup".to_string()),
        location: Some("Sala 3".to_string()),
        url: Some("https://ejemplo.org/x".to_string()),
        dtstart: Some(3_000),
        dtend: Some(3_600),
        dtstart_tzid: None,
        dtend_tzid: None,
        rrule: None,
        raw_payload: "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n".to_string(),
    };
    let inserted = insert_calendar_object(&pool, &new)
        .await
        .expect("insert event with location/url");
    assert_eq!(inserted.location.as_deref(), Some("Sala 3"));
    assert_eq!(inserted.url.as_deref(), Some("https://ejemplo.org/x"));

    let fetched = get_calendar_object(&pool, "cal1", "uid-2")
        .await
        .expect("get_calendar_object")
        .expect("row must be present");
    assert_eq!(fetched.location.as_deref(), Some("Sala 3"));
    assert_eq!(fetched.url.as_deref(), Some("https://ejemplo.org/x"));

    // The new row is indexed by the FTS trigger too.
    assert_eq!(
        fts_summaries(&pool, "Madrid").await,
        vec!["Madrid standup"],
        "the FTS trigger must index rows inserted after the migration"
    );
}
