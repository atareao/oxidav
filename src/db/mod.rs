//! F03 — Data layer: SQLite pool, migrations and minimal repositories.
//!
//! This module owns the async SQLite pool (`SqlitePool`), the PRAGMAs that tune
//! durability/performance, the versioned migrations that create the DAV schema
//! (tables, indexes and the two external-content FTS5 indexes), and the minimal
//! repositories and search helpers that F06–F10 will build upon.
//!
//! It contains **no** HTTP handlers, authentication or DAV serialisation, and it
//! is not yet wired into `AppState`/`main.rs` (that happens when F04/F06 consume it).

pub mod models;
pub mod search;

use std::path::Path;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::Row;
use sqlx::SqliteConnection;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

pub use models::{
    CalendarObject, CalendarObjectUpdate, CardObject, CardObjectUpdate, ChangeKind, Collection,
    CollectionKind, NewCalendarObject, NewCardObject, NewCollection, ResourceKind, SyncChange,
};

/// Internal alias for the shared connection pool.
pub type Db = SqlitePool;

/// Errors produced by the data layer.
///
/// Wraps the failure modes of [`init_pool`] (directory creation, connection and
/// migration) plus generic query failures from the repositories and search.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// Could not create the data directory for a file-backed database.
    #[error("failed to create data directory: {0}")]
    Io(#[from] std::io::Error),
    /// The database URL could not be parsed into connection options.
    #[error("invalid database URL: {0}")]
    InvalidUrl(String),
    /// Could not open the pool or establish a connection.
    #[error("failed to connect to database: {0}")]
    Connect(#[source] sqlx::Error),
    /// Migrations failed to apply.
    #[error("failed to run migrations: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// A repository or search query failed.
    #[error("database query failed: {0}")]
    Query(#[source] sqlx::Error),
}

impl DbError {
    /// Whether the failure was a SQLite `UNIQUE` constraint violation.
    ///
    /// Callers use this to distinguish an expected identity clash (e.g. a
    /// concurrent `PUT` losing the `(collection_id, uid)` race, or a
    /// `MKCALENDAR` replayed on an existing path) from a genuine internal fault,
    /// without discarding the underlying [`sqlx::Error`] or adding a variant
    /// that would rewrite every existing `DbError::Query` match.
    #[must_use]
    pub fn is_unique_violation(&self) -> bool {
        matches!(
            self,
            Self::Query(err)
                if err
                    .as_database_error()
                    .is_some_and(|db| db.is_unique_violation())
        )
    }
}

/// Embedded, versioned migrations applied by [`init_pool`].
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Open a SQLite pool, apply the performance/durability PRAGMAs and run the
/// migrations.
///
/// The data directory is created when the DSN points to a local file (e.g. the
/// default `sqlite://data/oxidav.db`), so a clean checkout works out of the box.
///
/// # Errors
///
/// Returns [`DbError::Io`] if the data directory cannot be created,
/// [`DbError::InvalidUrl`] if the DSN cannot be parsed, [`DbError::Connect`] if
/// the pool cannot be opened, and [`DbError::Migrate`] if a migration fails.
pub async fn init_pool(database_url: &str) -> Result<SqlitePool, DbError> {
    ensure_data_dir(database_url)?;

    let options = SqliteConnectOptions::from_str(database_url)
        .map_err(|err| DbError::InvalidUrl(err.to_string()))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_millis(5000))
        .pragma("temp_store", "MEMORY")
        .pragma("mmap_size", "268435456");

    let pool = SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .map_err(DbError::Connect)?;

    MIGRATOR.run(&pool).await.map_err(DbError::Migrate)?;

    Ok(pool)
}

/// Create the parent directory of a file-backed SQLite DSN, if any.
///
/// In-memory DSNs (`:memory:`, `file::memory:`) are left untouched.
fn ensure_data_dir(database_url: &str) -> Result<(), DbError> {
    let path = database_url
        .strip_prefix("sqlite://")
        .or_else(|| database_url.strip_prefix("sqlite:"))
        .unwrap_or(database_url);
    let path = path.split('?').next().unwrap_or(path);

    if path.is_empty() || path == ":memory:" || path.starts_with("file::memory:") {
        return Ok(());
    }

    if let Some(parent) = Path::new(path).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    Ok(())
}

/// Current wall-clock time as Unix milliseconds.
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Statement used to begin every **mutating** transaction.
///
/// SQLite starts a deferred transaction (`BEGIN`) that only takes a read
/// snapshot; the write lock is acquired lazily on the first write. If another
/// connection commits between the snapshot and that upgrade, the upgrade fails
/// with `SQLITE_BUSY_SNAPSHOT` (SQLite error 517), which the busy handler does
/// **not** retry — the transaction aborts immediately. That is observable in
/// production, where the best-effort `last_used_at` update (spawned by the DAV
/// Basic-auth extractor) can commit concurrently with a `PUT`'s transaction.
///
/// `BEGIN IMMEDIATE` reserves the write lock up front, so contention happens at
/// `BEGIN`, where the pool's `busy_timeout` does apply: concurrent writers
/// serialise cleanly instead of sporadically failing.
const BEGIN_WRITE: &str = "BEGIN IMMEDIATE";

/// Begin a write transaction with [`BEGIN_WRITE`].
///
/// # Errors
///
/// Returns [`DbError::Query`] if a connection cannot be acquired or the
/// transaction cannot be started.
async fn begin_write(
    pool: &SqlitePool,
) -> Result<sqlx::Transaction<'static, sqlx::Sqlite>, DbError> {
    pool.begin_with(BEGIN_WRITE).await.map_err(DbError::Query)
}

/// Compute an opaque, quoted ETag from the raw payload and modification time.
///
/// Algorithm: 64-bit FNV-1a over the `raw_payload` bytes followed by the
/// little-endian bytes of `updated_at`, rendered as `"<16 hex digits>"`. It is
/// deterministic and cheap.
///
/// The ETag is derived from `(raw_payload, updated_at)` where `updated_at` has
/// **millisecond** resolution. Two writes are therefore guaranteed distinct out
/// of order in normal use, but a re-write of a byte-identical payload within the
/// same millisecond yields the **same** ETag (the inputs are identical). Callers
/// that need strict uniqueness across such a collision must pass a distinct
/// `updated_at`.
fn compute_etag(raw_payload: &str, updated_at: i64) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET;
    for byte in raw_payload.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    for byte in updated_at.to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    format!("\"{hash:016x}\"")
}

/// Advance `collections.sync_token` and `updated_at` within the caller's
/// transaction, so that the object mutation and the token bump commit atomically.
///
/// Takes a `&mut SqliteConnection` (obtained from a transaction via `&mut *tx`)
/// so it participates in the surrounding transaction rather than opening its own.
/// Returns the **new** value of `sync_token`, which the caller threads into
/// [`record_change`] so the log entry and the token never disagree.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure (including a missing
/// collection, since `RETURNING` yields no row).
async fn bump_sync_token(conn: &mut SqliteConnection, collection_id: &str) -> Result<i64, DbError> {
    sqlx::query_scalar(
        "UPDATE collections
            SET sync_token = sync_token + 1, updated_at = ?
          WHERE id = ?
        RETURNING sync_token",
    )
    .bind(now_millis())
    .bind(collection_id)
    .fetch_one(conn)
    .await
    .map_err(DbError::Query)
}

/// Build a [`DbError`] for a stored enum value outside its schema domain.
///
/// The `CHECK` constraints make this unreachable in practice; it exists so the
/// row-mapping code never has to `unwrap` on a defensive branch.
fn corrupt_value_error(kind: &str, value: &str) -> DbError {
    DbError::Query(sqlx::Error::Decode(
        format!("unexpected {kind} value in database: {value}").into(),
    ))
}

// ---------------------------------------------------------------------------
// collections
// ---------------------------------------------------------------------------

/// Insert a new collection and return the stored row.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure (e.g. a constraint clash).
pub async fn create_collection(
    pool: &SqlitePool,
    new: &NewCollection,
) -> Result<Collection, DbError> {
    let now = now_millis();
    sqlx::query_as::<_, Collection>(
        "INSERT INTO collections
            (id, user_id, name, type, color, sync_token, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, 1, ?, ?)
         RETURNING *",
    )
    .bind(&new.id)
    .bind(&new.user_id)
    .bind(&new.name)
    .bind(new.kind.as_str())
    .bind(&new.color)
    .bind(now)
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(DbError::Query)
}

/// Default page size for [`list_collections`] when the caller passes a
/// non-positive `limit`.
pub const DEFAULT_COLLECTION_LIMIT: i64 = 20;

/// Hard ceiling for [`list_collections`]; mirrors the search limit of F03.
pub const MAX_COLLECTION_LIMIT: i64 = 200;

/// Normalise a caller-supplied page `limit`.
///
/// A non-positive value falls back to [`DEFAULT_COLLECTION_LIMIT`] (never
/// SQLite's unbounded `LIMIT -1`) and anything above [`MAX_COLLECTION_LIMIT`]
/// is capped, so the result is always in `1..=MAX_COLLECTION_LIMIT`.
#[must_use]
pub fn clamp_collection_limit(limit: i64) -> i64 {
    if limit <= 0 {
        DEFAULT_COLLECTION_LIMIT
    } else {
        limit.min(MAX_COLLECTION_LIMIT)
    }
}

/// Normalise a caller-supplied page `offset` (negative becomes `0`).
#[must_use]
pub fn clamp_offset(offset: i64) -> i64 {
    offset.max(0)
}

/// List a page of collections owned by `user_id`, ordered by name.
///
/// `limit` and `offset` are clamped (see [`clamp_collection_limit`] and
/// [`clamp_offset`]) before reaching SQLite, so a raw negative `limit` can never
/// act as the unbounded `LIMIT -1`.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn list_collections(
    pool: &SqlitePool,
    user_id: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<Collection>, DbError> {
    sqlx::query_as::<_, Collection>(
        "SELECT * FROM collections
          WHERE user_id = ?
          ORDER BY name ASC
          LIMIT ? OFFSET ?",
    )
    .bind(user_id)
    .bind(clamp_collection_limit(limit))
    .bind(clamp_offset(offset))
    .fetch_all(pool)
    .await
    .map_err(DbError::Query)
}

/// Fetch a single collection by `id`, scoped to its owner.
///
/// Returns `Ok(None)` when no collection with that `id` exists **or** it belongs
/// to another user, so callers cannot distinguish the two cases (IDOR
/// mitigation).
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn get_collection(
    pool: &SqlitePool,
    id: &str,
    user_id: &str,
) -> Result<Option<Collection>, DbError> {
    sqlx::query_as::<_, Collection>("SELECT * FROM collections WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(DbError::Query)
}

/// Partially update a collection owned by `user_id`.
///
/// Only the fields passed as `Some(..)` are modified; `None` leaves the stored
/// value untouched. `updated_at` is always refreshed. Returns `Ok(None)` when
/// the collection does not exist or belongs to another user.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn update_collection(
    pool: &SqlitePool,
    id: &str,
    user_id: &str,
    name: Option<&str>,
    color: Option<&str>,
) -> Result<Option<Collection>, DbError> {
    let now = now_millis();
    sqlx::query_as::<_, Collection>(
        "UPDATE collections
            SET name = COALESCE(?, name),
                color = COALESCE(?, color),
                updated_at = ?
          WHERE id = ? AND user_id = ?
         RETURNING *",
    )
    .bind(name)
    .bind(color)
    .bind(now)
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(DbError::Query)
}

/// Count the collections owned by `user_id`.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn count_collections(pool: &SqlitePool, user_id: &str) -> Result<i64, DbError> {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM collections WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)
}

/// Delete a collection owned by `user_id`; its objects and change log are
/// removed via `ON DELETE CASCADE`.
///
/// The cascade reaches three tables: `calendar_objects`, `card_objects` and
/// `sync_log` (each declares `REFERENCES collections(id) ON DELETE CASCADE`).
/// Dropping the collection therefore erases its `sync_log` history entirely —
/// there is no orphaned tombstone left behind. Clients holding a `sync-token`
/// for the deleted collection can no longer be served an incremental diff, so
/// they must resynchronise from scratch (a fresh `sync-collection` without a
/// token against the new collection state).
///
/// The `user_id` predicate is part of the statement itself (no prior `SELECT`),
/// so the ownership check and the delete are a single atomic operation. Returns
/// `true` when a row was deleted and `false` when the collection does not exist
/// or belongs to another user, which callers map to `404`.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn delete_collection(
    pool: &SqlitePool,
    id: &str,
    user_id: &str,
) -> Result<bool, DbError> {
    let result = sqlx::query("DELETE FROM collections WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(DbError::Query)?;
    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// calendar_objects
// ---------------------------------------------------------------------------

/// Insert a calendar object, computing its ETag and bumping the collection's
/// `sync_token`.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure (e.g. the `(collection_id,
/// uid)` uniqueness constraint).
pub async fn insert_calendar_object(
    pool: &SqlitePool,
    new: &NewCalendarObject,
) -> Result<CalendarObject, DbError> {
    let now = now_millis();
    let etag = compute_etag(&new.raw_payload, now);

    let mut tx = begin_write(pool).await?;
    let row = sqlx::query_as::<_, CalendarObject>(
        "INSERT INTO calendar_objects
            (id, collection_id, uid, summary, description, dtstart, dtend,
             dtstart_tzid, dtend_tzid, rrule, etag, raw_payload, created_at,
             updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         RETURNING *",
    )
    .bind(&new.id)
    .bind(&new.collection_id)
    .bind(&new.uid)
    .bind(&new.summary)
    .bind(&new.description)
    .bind(new.dtstart)
    .bind(new.dtend)
    .bind(&new.dtstart_tzid)
    .bind(&new.dtend_tzid)
    .bind(&new.rrule)
    .bind(&etag)
    .bind(&new.raw_payload)
    .bind(now)
    .bind(now)
    .fetch_one(&mut *tx)
    .await
    .map_err(DbError::Query)?;

    let token = bump_sync_token(&mut tx, &row.collection_id).await?;
    record_change(
        &mut tx,
        &row.collection_id,
        &row.uid,
        ResourceKind::Calendar,
        ChangeKind::Added,
        token,
    )
    .await?;
    tx.commit().await.map_err(DbError::Query)?;
    Ok(row)
}

/// Update a calendar object's mutable fields, recomputing its ETag and bumping
/// the collection's `sync_token`.
///
/// # Errors
///
/// Returns [`DbError::Query`] if the object does not exist or on any database
/// failure.
pub async fn update_calendar_object(
    pool: &SqlitePool,
    id: &str,
    update: &CalendarObjectUpdate,
) -> Result<CalendarObject, DbError> {
    let now = now_millis();
    let etag = compute_etag(&update.raw_payload, now);

    let mut tx = begin_write(pool).await?;
    let row = sqlx::query_as::<_, CalendarObject>(
        "UPDATE calendar_objects
            SET summary = ?, description = ?, dtstart = ?, dtend = ?,
                dtstart_tzid = ?, dtend_tzid = ?, rrule = ?,
                etag = ?, raw_payload = ?, updated_at = ?
          WHERE id = ?
         RETURNING *",
    )
    .bind(&update.summary)
    .bind(&update.description)
    .bind(update.dtstart)
    .bind(update.dtend)
    .bind(&update.dtstart_tzid)
    .bind(&update.dtend_tzid)
    .bind(&update.rrule)
    .bind(&etag)
    .bind(&update.raw_payload)
    .bind(now)
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .map_err(DbError::Query)?;

    let token = bump_sync_token(&mut tx, &row.collection_id).await?;
    record_change(
        &mut tx,
        &row.collection_id,
        &row.uid,
        ResourceKind::Calendar,
        ChangeKind::Updated,
        token,
    )
    .await?;
    tx.commit().await.map_err(DbError::Query)?;
    Ok(row)
}

/// Delete a calendar object and bump the collection's `sync_token`, atomically.
///
/// If no object with `id` exists this is a **no-op** and returns `Ok(())`
/// (idempotent delete); `sync_token` is left untouched in that case.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn delete_calendar_object(pool: &SqlitePool, id: &str) -> Result<(), DbError> {
    let mut tx = begin_write(pool).await?;
    let deleted =
        sqlx::query("DELETE FROM calendar_objects WHERE id = ? RETURNING collection_id, uid")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DbError::Query)?;

    if let Some(row) = deleted {
        let collection_id: String = row.try_get("collection_id").map_err(DbError::Query)?;
        let uid: String = row.try_get("uid").map_err(DbError::Query)?;
        let token = bump_sync_token(&mut tx, &collection_id).await?;
        record_change(
            &mut tx,
            &collection_id,
            &uid,
            ResourceKind::Calendar,
            ChangeKind::Deleted,
            token,
        )
        .await?;
    }
    tx.commit().await.map_err(DbError::Query)?;
    Ok(())
}

/// List every calendar object of a collection, ordered by `uid`.
///
/// Scoped strictly by `collection_id` (the caller is responsible for proving the
/// collection belongs to the authenticated user beforehand). The deterministic
/// `uid` ordering keeps DAV `PROPFIND` responses stable across calls.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn list_calendar_objects(
    pool: &SqlitePool,
    collection_id: &str,
) -> Result<Vec<CalendarObject>, DbError> {
    sqlx::query_as::<_, CalendarObject>(
        "SELECT * FROM calendar_objects
          WHERE collection_id = ?
          ORDER BY uid ASC",
    )
    .bind(collection_id)
    .fetch_all(pool)
    .await
    .map_err(DbError::Query)
}

/// List a page of calendar objects, optionally restricted to those whose time
/// span overlaps `[start, end)`.
///
/// `start`/`end` are Unix epoch milliseconds; either may be `None` to leave that
/// bound open. Overlap is evaluated as `dtstart < end` **and**
/// (`dtend IS NULL` and `dtstart >= start`, or `dtend > start`) so a
/// zero-duration event sitting exactly on `start` is included while a real event
/// ending exactly on `start` is not. An object with no `DTSTART` cannot overlap
/// any window and is therefore excluded when any bound is given; with both
/// bounds absent the filter is not applied at all, so the result is identical to
/// [`list_calendar_objects`] (plus pagination).
///
/// `limit` and `offset` are clamped (see [`clamp_collection_limit`] and
/// [`clamp_offset`]) before reaching SQLite.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn list_calendar_objects_window(
    pool: &SqlitePool,
    collection_id: &str,
    start: Option<i64>,
    end: Option<i64>,
    limit: i64,
    offset: i64,
) -> Result<Vec<CalendarObject>, DbError> {
    let mut sql = String::from("SELECT * FROM calendar_objects WHERE collection_id = ?");
    if start.is_some() || end.is_some() {
        sql.push_str(" AND dtstart IS NOT NULL");
    }
    if end.is_some() {
        sql.push_str(" AND dtstart < ?");
    }
    if start.is_some() {
        sql.push_str(" AND (dtend IS NULL AND dtstart >= ? OR dtend > ?)");
    }
    sql.push_str(" ORDER BY uid ASC LIMIT ? OFFSET ?");

    // Binds must follow the `?` order of the assembled statement exactly.
    let mut query = sqlx::query_as::<_, CalendarObject>(&sql).bind(collection_id);
    if let Some(end) = end {
        query = query.bind(end);
    }
    if let Some(start) = start {
        query = query.bind(start).bind(start);
    }
    query = query
        .bind(clamp_collection_limit(limit))
        .bind(clamp_offset(offset));

    query.fetch_all(pool).await.map_err(DbError::Query)
}

/// Fetch a single calendar object by its collection and `uid`.
///
/// The `(collection_id, uid)` pair is unique in the schema, so at most one row
/// matches. Returns `Ok(None)` when no such object exists. Scoped by
/// `collection_id` so a `uid` that exists in another collection is invisible.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn get_calendar_object(
    pool: &SqlitePool,
    collection_id: &str,
    uid: &str,
) -> Result<Option<CalendarObject>, DbError> {
    sqlx::query_as::<_, CalendarObject>(
        "SELECT * FROM calendar_objects WHERE collection_id = ? AND uid = ?",
    )
    .bind(collection_id)
    .bind(uid)
    .fetch_optional(pool)
    .await
    .map_err(DbError::Query)
}

// ---------------------------------------------------------------------------
// card_objects
// ---------------------------------------------------------------------------

/// Insert a card object, computing its ETag and bumping the collection's
/// `sync_token`.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure (e.g. the `(collection_id,
/// uid)` uniqueness constraint).
pub async fn insert_card_object(
    pool: &SqlitePool,
    new: &NewCardObject,
) -> Result<CardObject, DbError> {
    let now = now_millis();
    let etag = compute_etag(&new.raw_payload, now);

    let mut tx = begin_write(pool).await?;
    let row = sqlx::query_as::<_, CardObject>(
        "INSERT INTO card_objects
            (id, collection_id, uid, fn, email, phone, organization, notes,
             etag, raw_payload, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         RETURNING *",
    )
    .bind(&new.id)
    .bind(&new.collection_id)
    .bind(&new.uid)
    .bind(&new.fn_)
    .bind(&new.email)
    .bind(&new.phone)
    .bind(&new.organization)
    .bind(&new.notes)
    .bind(&etag)
    .bind(&new.raw_payload)
    .bind(now)
    .bind(now)
    .fetch_one(&mut *tx)
    .await
    .map_err(DbError::Query)?;

    let token = bump_sync_token(&mut tx, &row.collection_id).await?;
    record_change(
        &mut tx,
        &row.collection_id,
        &row.uid,
        ResourceKind::Card,
        ChangeKind::Added,
        token,
    )
    .await?;
    tx.commit().await.map_err(DbError::Query)?;
    Ok(row)
}

/// Update a card object's mutable fields, recomputing its ETag and bumping the
/// collection's `sync_token`.
///
/// # Errors
///
/// Returns [`DbError::Query`] if the object does not exist or on any database
/// failure.
pub async fn update_card_object(
    pool: &SqlitePool,
    id: &str,
    update: &CardObjectUpdate,
) -> Result<CardObject, DbError> {
    let now = now_millis();
    let etag = compute_etag(&update.raw_payload, now);

    let mut tx = begin_write(pool).await?;
    let row = sqlx::query_as::<_, CardObject>(
        "UPDATE card_objects
            SET fn = ?, email = ?, phone = ?, organization = ?, notes = ?,
                etag = ?, raw_payload = ?, updated_at = ?
          WHERE id = ?
         RETURNING *",
    )
    .bind(&update.fn_)
    .bind(&update.email)
    .bind(&update.phone)
    .bind(&update.organization)
    .bind(&update.notes)
    .bind(&etag)
    .bind(&update.raw_payload)
    .bind(now)
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .map_err(DbError::Query)?;

    let token = bump_sync_token(&mut tx, &row.collection_id).await?;
    record_change(
        &mut tx,
        &row.collection_id,
        &row.uid,
        ResourceKind::Card,
        ChangeKind::Updated,
        token,
    )
    .await?;
    tx.commit().await.map_err(DbError::Query)?;
    Ok(row)
}

/// Delete a card object and bump the collection's `sync_token`, atomically.
///
/// If no object with `id` exists this is a **no-op** and returns `Ok(())`
/// (idempotent delete); `sync_token` is left untouched in that case.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn delete_card_object(pool: &SqlitePool, id: &str) -> Result<(), DbError> {
    let mut tx = begin_write(pool).await?;
    let deleted = sqlx::query("DELETE FROM card_objects WHERE id = ? RETURNING collection_id, uid")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(DbError::Query)?;

    if let Some(row) = deleted {
        let collection_id: String = row.try_get("collection_id").map_err(DbError::Query)?;
        let uid: String = row.try_get("uid").map_err(DbError::Query)?;
        let token = bump_sync_token(&mut tx, &collection_id).await?;
        record_change(
            &mut tx,
            &collection_id,
            &uid,
            ResourceKind::Card,
            ChangeKind::Deleted,
            token,
        )
        .await?;
    }
    tx.commit().await.map_err(DbError::Query)?;
    Ok(())
}

/// List every card object of a collection, ordered by `uid`.
///
/// Scoped strictly by `collection_id` (the caller is responsible for proving the
/// collection belongs to the authenticated user beforehand). The deterministic
/// `uid` ordering keeps DAV `PROPFIND`/`REPORT` responses stable across calls.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn list_card_objects(
    pool: &SqlitePool,
    collection_id: &str,
) -> Result<Vec<CardObject>, DbError> {
    sqlx::query_as::<_, CardObject>(
        "SELECT * FROM card_objects
          WHERE collection_id = ?
          ORDER BY uid ASC",
    )
    .bind(collection_id)
    .fetch_all(pool)
    .await
    .map_err(DbError::Query)
}

/// List a page of card objects of a collection, ordered by `uid`.
///
/// Scoped strictly by `collection_id` (the caller is responsible for proving the
/// collection belongs to the authenticated user beforehand). The deterministic
/// `uid` ordering keeps the REST listing stable across calls. `limit`/`offset`
/// are clamped (see [`clamp_collection_limit`] and [`clamp_offset`]) before
/// reaching SQLite.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn list_card_objects_window(
    pool: &SqlitePool,
    collection_id: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<CardObject>, DbError> {
    sqlx::query_as::<_, CardObject>(
        "SELECT * FROM card_objects
          WHERE collection_id = ?
          ORDER BY uid ASC LIMIT ? OFFSET ?",
    )
    .bind(collection_id)
    .bind(clamp_collection_limit(limit))
    .bind(clamp_offset(offset))
    .fetch_all(pool)
    .await
    .map_err(DbError::Query)
}

/// Fetch a single card object by its collection and `uid`.
///
/// The `(collection_id, uid)` pair is unique in the schema, so at most one row
/// matches. Returns `Ok(None)` when no such object exists. Scoped by
/// `collection_id` so a `uid` that exists in another collection is invisible.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn get_card_object(
    pool: &SqlitePool,
    collection_id: &str,
    uid: &str,
) -> Result<Option<CardObject>, DbError> {
    sqlx::query_as::<_, CardObject>(
        "SELECT * FROM card_objects WHERE collection_id = ? AND uid = ?",
    )
    .bind(collection_id)
    .bind(uid)
    .fetch_optional(pool)
    .await
    .map_err(DbError::Query)
}

// ---------------------------------------------------------------------------
// F09 — sync-collection change log
// ---------------------------------------------------------------------------

/// Record a change in `sync_log` **within the caller's transaction**.
///
/// It never opens its own transaction: the caller (a F03 write-path) passes the
/// same `&mut SqliteConnection` it used for the object mutation and the
/// `sync_token` bump, so a failure rolls all three back together and no orphan
/// log entry survives.
///
/// `sync_token` is the value `collections.sync_token` was advanced to by
/// [`bump_sync_token`]; every entry for a collection therefore carries a
/// strictly increasing token.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn record_change(
    conn: &mut SqliteConnection,
    collection_id: &str,
    resource_uid: &str,
    resource_kind: ResourceKind,
    change: ChangeKind,
    sync_token: i64,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO sync_log
            (collection_id, resource_uid, resource_kind, change, sync_token)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(collection_id)
    .bind(resource_uid)
    .bind(resource_kind.as_str())
    .bind(change.as_str())
    .bind(sync_token)
    .execute(conn)
    .await
    .map_err(DbError::Query)?;
    Ok(())
}

/// Current `sync_token` of a collection.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure (including an unknown
/// collection, since the scalar query yields no row).
pub async fn next_sync_token(pool: &SqlitePool, collection_id: &str) -> Result<i64, DbError> {
    sqlx::query_scalar("SELECT sync_token FROM collections WHERE id = ?")
        .bind(collection_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)
}

/// Changes to report for a collection from `since` (RFC 6578 `sync-collection`).
///
/// `kind` scopes the result to a single resource family. Defence in depth: even
/// though a calendar collection never legitimately holds card objects (and vice
/// versa), a `REPORT` for a calendar is never allowed to echo a mis-typed card
/// row, so both branches filter on `resource_kind`.
///
/// * `since = Some(n)` — **incremental** sync: the most recent change per
///   resource whose `sync_token > n`, ordered by `sync_token` ascending.
///   `deleted` entries are tombstones and carry no ETag.
/// * `since = None` — **initial** sync: every live resource of the collection
///   with its current ETag (resources whose last operation was a delete are
///   absent because their object no longer exists).
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn changes_since(
    pool: &SqlitePool,
    collection_id: &str,
    kind: ResourceKind,
    since: Option<i64>,
) -> Result<Vec<SyncChange>, DbError> {
    match since {
        Some(token) => changes_after(pool, collection_id, kind, token).await,
        None => initial_snapshot(pool, collection_id, kind).await,
    }
}

/// Incremental branch of [`changes_since`]: last change per resource after `n`,
/// filtered to `kind` so a calendar report never surfaces card entries.
async fn changes_after(
    pool: &SqlitePool,
    collection_id: &str,
    kind: ResourceKind,
    since: i64,
) -> Result<Vec<SyncChange>, DbError> {
    let rows = sqlx::query(
        "SELECT s.resource_uid  AS resource_uid,
                s.resource_kind AS resource_kind,
                s.change        AS change,
                s.sync_token    AS sync_token,
                CASE s.resource_kind
                    WHEN 'calendar' THEN c.etag
                    WHEN 'card'     THEN k.etag
                END             AS etag
           FROM (
                SELECT resource_uid, resource_kind, change, sync_token,
                       ROW_NUMBER() OVER (
                           PARTITION BY resource_kind, resource_uid
                           ORDER BY sync_token DESC, id DESC
                       ) AS rn
                  FROM sync_log
                 WHERE collection_id = ? AND sync_token > ? AND resource_kind = ?
           ) AS s
           LEFT JOIN calendar_objects c
                  ON s.resource_kind = 'calendar'
                 AND c.collection_id = ?
                 AND c.uid = s.resource_uid
           LEFT JOIN card_objects k
                  ON s.resource_kind = 'card'
                 AND k.collection_id = ?
                 AND k.uid = s.resource_uid
          WHERE s.rn = 1
          ORDER BY s.sync_token ASC",
    )
    .bind(collection_id)
    .bind(since)
    .bind(kind.as_str())
    .bind(collection_id)
    .bind(collection_id)
    .fetch_all(pool)
    .await
    .map_err(DbError::Query)?;

    rows.into_iter().map(sync_change_from_row).collect()
}

/// Initial branch of [`changes_since`]: live resources of `kind` with their
/// current ETag.
///
/// The table is selected from the `kind` enum (never from caller input), and the
/// emitted `resource_kind` literal is bound rather than interpolated.
async fn initial_snapshot(
    pool: &SqlitePool,
    collection_id: &str,
    kind: ResourceKind,
) -> Result<Vec<SyncChange>, DbError> {
    let token = next_sync_token(pool, collection_id).await?;
    let table = match kind {
        ResourceKind::Calendar => "calendar_objects",
        ResourceKind::Card => "card_objects",
    };
    let rows = sqlx::query(&format!(
        "SELECT uid AS resource_uid, ? AS resource_kind, etag AS etag
           FROM {table}
          WHERE collection_id = ?
          ORDER BY resource_uid ASC",
    ))
    .bind(kind.as_str())
    .bind(collection_id)
    .fetch_all(pool)
    .await
    .map_err(DbError::Query)?;

    rows.into_iter()
        .map(|row| {
            let resource_uid: String = row.try_get("resource_uid").map_err(DbError::Query)?;
            let kind_str: String = row.try_get("resource_kind").map_err(DbError::Query)?;
            let etag: String = row.try_get("etag").map_err(DbError::Query)?;
            let resource_kind = ResourceKind::from_db(&kind_str)
                .ok_or_else(|| corrupt_value_error("resource_kind", &kind_str))?;
            Ok(SyncChange {
                resource_uid,
                resource_kind,
                change: ChangeKind::Added,
                sync_token: token,
                etag: Some(etag),
            })
        })
        .collect()
}

/// Map one `sync_log` join row (incremental branch) to a [`SyncChange`].
fn sync_change_from_row(row: sqlx::sqlite::SqliteRow) -> Result<SyncChange, DbError> {
    let resource_uid: String = row.try_get("resource_uid").map_err(DbError::Query)?;
    let kind_str: String = row.try_get("resource_kind").map_err(DbError::Query)?;
    let change_str: String = row.try_get("change").map_err(DbError::Query)?;
    let sync_token: i64 = row.try_get("sync_token").map_err(DbError::Query)?;
    let etag: Option<String> = row.try_get("etag").map_err(DbError::Query)?;

    let resource_kind = ResourceKind::from_db(&kind_str)
        .ok_or_else(|| corrupt_value_error("resource_kind", &kind_str))?;
    let change = ChangeKind::from_db(&change_str)
        .ok_or_else(|| corrupt_value_error("change", &change_str))?;

    Ok(SyncChange {
        resource_uid,
        resource_kind,
        change,
        sync_token,
        etag,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_etag_is_deterministic() {
        assert_eq!(
            compute_etag("payload", 1_700_000_000_000),
            compute_etag("payload", 1_700_000_000_000),
        );
    }

    #[test]
    fn compute_etag_is_quoted_16_hex_digits() {
        let etag = compute_etag("hello", 42);
        assert!(etag.starts_with('"') && etag.ends_with('"'), "etag: {etag}");
        let inner = &etag[1..etag.len() - 1];
        assert_eq!(inner.len(), 16);
        assert!(inner.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn compute_etag_varies_with_payload_and_time() {
        assert_ne!(compute_etag("alpha", 1), compute_etag("beta", 1));
        assert_ne!(compute_etag("alpha", 1), compute_etag("alpha", 2));
        // Documented collision: byte-identical payload in the same millisecond.
        assert_eq!(compute_etag("alpha", 1), compute_etag("alpha", 1));
    }

    #[test]
    fn ensure_data_dir_skips_in_memory_dsns() {
        assert!(ensure_data_dir(":memory:").is_ok());
        assert!(ensure_data_dir("sqlite::memory:").is_ok());
        assert!(ensure_data_dir("sqlite::memory:?cache=shared").is_ok());
        assert!(ensure_data_dir("sqlite:file::memory:?cache=shared").is_ok());
    }

    #[test]
    fn ensure_data_dir_ignores_bare_filename() {
        // No parent component to create, and empty DSN is a no-op.
        assert!(ensure_data_dir("sqlite://oxidav.db").is_ok());
        assert!(ensure_data_dir("").is_ok());
    }

    #[test]
    fn ensure_data_dir_creates_parent_for_nested_file_dsn() {
        let dir = std::env::temp_dir().join(format!("oxidav-ensure-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let url = format!("sqlite://{}/nested/oxidav.db", dir.display());
        ensure_data_dir(&url).expect("parent directory should be created");
        assert!(dir.join("nested").is_dir());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_data_dir_strips_query_parameters() {
        let dir = std::env::temp_dir().join(format!("oxidav-ensure-q-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let url = format!("sqlite://{}/q/oxidav.db?mode=rwc", dir.display());
        ensure_data_dir(&url).expect("parent directory should be created");
        assert!(dir.join("q").is_dir());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
