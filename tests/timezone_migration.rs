//! F18 — update-path coverage for `migrations/0004_timezone.sql`.
//!
//! Every other test in the suite builds the database from scratch (the migrator
//! runs `0001`–`0004` in order on an empty file), so nothing ever exercises the
//! *upgrade* path: applying the two `ALTER TABLE ... ADD COLUMN` statements of
//! `0004` on top of a database that already carries the `0001`–`0003` schema, its
//! rows, and the external-content `events_fts` index mirroring them.
//!
//! That is exactly what happens on an existing install, so this file mounts the
//! legacy schema and its data by hand (mirroring `tests/app_tokens_migration.rs`),
//! then applies `0004` on top and asserts:
//!
//! 1. the pre-existing row survives with both TZID columns `NULL`;
//! 2. the external-content FTS search still finds it (the `ALTER TABLE` did not
//!    break the `rowid` linkage / triggers);
//! 3. a new event carrying `dtstart_tzid = 'Europe/Madrid'` round-trips intact.
//!
//! # Why the migration is applied on a pool that is then closed
//!
//! `sqlx-sqlite` caches prepared statements per connection *without* invalidating
//! them on a schema change. When `0004` runs on the same pool that later serves
//! queries, a connection can end up reusing a statement whose cached column
//! metadata predates the `ALTER TABLE` (12 columns) while SQLite returns the new
//! column count (14), which makes `sqlx` panic with
//! `index out of bounds: the len is 12 but the index is 12`. Applying the DDL on
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
/// The migration under test (the one that adds `dtstart_tzid`/`dtend_tzid`).
const TIMEZONE: &str = include_str!("../migrations/0004_timezone.sql");
/// A later additive migration. The repository helpers used below
/// (`insert_calendar_object`, `search_events`) decode the **current**
/// `CalendarObject`, so they need every column the current schema carries — this
/// file replays `0004` and `0005` to reach it while still exercising the `0004`
/// upgrade path.
const LOCATION_URL: &str = include_str!("../migrations/0005_event_location_url.sql");

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

/// Replay the pre-`0004` schema exactly as an existing install would have it.
async fn seed_legacy_schema(pool: &SqlitePool) {
    pool.execute(INIT).await.expect("apply 0001_init");
    pool.execute(APP_TOKENS)
        .await
        .expect("apply 0002_app_tokens");
    pool.execute(SYNC_LOG).await.expect("apply 0003_sync_log");
}

/// Populate the legacy schema using only the *old* `calendar_objects` columns
/// (no TZID columns exist yet).
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
             etag, raw_payload, created_at, updated_at)
         VALUES ('evt1', 'cal1', 'uid-1', 'Weekly planning', 'Plan the sprint',
                 1000, 2000, NULL, '\"etag1\"',
                 'BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n', 0, 0)",
    )
    .execute(pool)
    .await
    .expect("insert legacy calendar object");
}

/// Summaries returned by the `events_fts` index for a literal substring query.
///
/// Queries `MATCH` directly (not through the public helper, which decodes the
/// full row and therefore cannot run before `0004` adds its columns). The `trigram`
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

#[tokio::test]
async fn migration_preserves_preexisting_row_and_keeps_fts_search_working() {
    let url = common::temp_db_url("timezone", "preserve");
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

    // Apply 0004 on top of the *populated* database — the real upgrade path.
    pool.execute(TIMEZONE).await.expect("apply 0004_timezone");
    pool.execute(LOCATION_URL)
        .await
        .expect("apply 0005_event_location_url");
    // Retire the connections that saw the pre-ALTER schema (see module docs).
    pool.close().await;
    let pool = open(&url).await;

    // (1) The pre-existing row survives and both new columns default to NULL.
    let (summary, dtstart_tzid, dtend_tzid): (Option<String>, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT summary, dtstart_tzid, dtend_tzid
               FROM calendar_objects WHERE id = 'evt1'",
        )
        .fetch_one(&pool)
        .await
        .expect("row must survive the ALTER TABLE");
    assert_eq!(summary.as_deref(), Some("Weekly planning"));
    assert_eq!(dtstart_tzid, None, "dtstart_tzid must default to NULL");
    assert_eq!(dtend_tzid, None, "dtend_tzid must default to NULL");

    // (2) The external-content FTS index still finds the row: the ALTER TABLE did
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
    assert_eq!(hits[0].dtstart_tzid, None);
}

#[tokio::test]
async fn migration_allows_inserting_and_reading_back_a_tzid() {
    let url = common::temp_db_url("timezone", "tzid");
    let pool = open(&url).await;
    seed_legacy_schema(&pool).await;
    seed_legacy_data(&pool).await;
    pool.execute(TIMEZONE).await.expect("apply 0004_timezone");
    pool.execute(LOCATION_URL)
        .await
        .expect("apply 0005_event_location_url");
    pool.close().await;
    let pool = open(&url).await;

    // (3) After the migration a new event can carry an IANA TZID and round-trips
    // intact through the real repository helpers.
    let new = NewCalendarObject {
        id: "evt2".to_string(),
        collection_id: "cal1".to_string(),
        uid: "uid-2".to_string(),
        summary: Some("Madrid standup".to_string()),
        description: Some("Daily standup".to_string()),
        dtstart: Some(3_000),
        dtend: Some(3_600),
        dtstart_tzid: Some("Europe/Madrid".to_string()),
        dtend_tzid: Some("Europe/Madrid".to_string()),
        location: None,
        url: None,
        rrule: None,
        raw_payload: "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n".to_string(),
    };
    let inserted = insert_calendar_object(&pool, &new)
        .await
        .expect("insert event with TZID");
    assert_eq!(inserted.dtstart_tzid.as_deref(), Some("Europe/Madrid"));
    assert_eq!(inserted.dtend_tzid.as_deref(), Some("Europe/Madrid"));

    let fetched = get_calendar_object(&pool, "cal1", "uid-2")
        .await
        .expect("get_calendar_object")
        .expect("row must be present");
    assert_eq!(fetched.dtstart_tzid.as_deref(), Some("Europe/Madrid"));
    assert_eq!(fetched.dtend_tzid.as_deref(), Some("Europe/Madrid"));

    // The new row is indexed by the FTS trigger too.
    assert_eq!(
        fts_summaries(&pool, "Madrid").await,
        vec!["Madrid standup"],
        "the FTS trigger must index rows inserted after the migration"
    );
}

#[tokio::test]
async fn alter_table_keeps_external_content_fts_update_trigger_consistent() {
    let url = common::temp_db_url("timezone", "update-trigger");
    let pool = open(&url).await;
    seed_legacy_schema(&pool).await;
    seed_legacy_data(&pool).await;
    pool.execute(TIMEZONE).await.expect("apply 0004_timezone");
    pool.execute(LOCATION_URL)
        .await
        .expect("apply 0005_event_location_url");
    pool.close().await;
    let pool = open(&url).await;

    // UPDATE after the ALTER TABLE: the delete+reinsert trigger must retire the
    // old description tokens and index the new ones.
    sqlx::query(
        "UPDATE calendar_objects SET description = 'Retrospective notes' WHERE id = 'evt1'",
    )
    .execute(&pool)
    .await
    .expect("update after migration");

    // New tokens are searchable...
    assert_eq!(
        fts_summaries(&pool, "Retrospective").await,
        vec!["Weekly planning"]
    );
    // ...and the retired ones are gone (no stale index entries).
    assert!(
        fts_summaries(&pool, "sprint").await.is_empty(),
        "old description tokens must be retired by the UPDATE trigger"
    );
}
