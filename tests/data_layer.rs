//! F03 — Data layer integration tests (Pasos 2–9).
//!
//! Each test opens its own throwaway file-backed database under
//! `/tmp/opencode` (never `data/oxidav.db`), so tests are isolated and repeatable.

mod common;

use oxidav::db::models::{
    CalendarObjectUpdate, CardObjectUpdate, ChangeKind, CollectionKind, NewCalendarObject,
    NewCardObject, NewCollection, ResourceKind,
};
use oxidav::db::search::{
    MAX_SEARCH_LIMIT, search_contacts, search_contacts_scoped, search_events, search_events_scoped,
};
use oxidav::db::{
    DbError, changes_since, count_collections, create_collection, delete_calendar_object,
    delete_card_object, delete_collection, get_calendar_object, get_card_object, get_collection,
    init_pool, insert_calendar_object, insert_card_object, list_calendar_objects,
    list_card_objects, list_collections, next_sync_token, update_calendar_object,
    update_card_object, update_collection,
};
use sqlx::SqlitePool;

/// Open a fresh templess pool with the schema applied.
async fn fresh_pool(tag: &str) -> SqlitePool {
    init_pool(&common::temp_db_url("data", tag))
        .await
        .expect("init_pool")
}

fn new_collection(id: &str, user: &str, kind: CollectionKind) -> NewCollection {
    NewCollection {
        id: id.to_string(),
        user_id: user.to_string(),
        name: format!("Collection {id}"),
        kind,
        color: "#1677ff".to_string(),
    }
}

fn new_event(id: &str, collection_id: &str, uid: &str, summary: &str) -> NewCalendarObject {
    NewCalendarObject {
        id: id.to_string(),
        collection_id: collection_id.to_string(),
        uid: uid.to_string(),
        summary: Some(summary.to_string()),
        description: Some("descripcion de prueba".to_string()),
        dtstart: Some(1_700_000_000_000),
        dtend: Some(1_700_000_600_000),
        dtstart_tzid: None,
        dtend_tzid: None,
        rrule: None,
        raw_payload: format!("BEGIN:VCALENDAR\nSUMMARY:{summary}\nEND:VCALENDAR"),
    }
}

fn new_card(id: &str, collection_id: &str, uid: &str, fn_: &str) -> NewCardObject {
    NewCardObject {
        id: id.to_string(),
        collection_id: collection_id.to_string(),
        uid: uid.to_string(),
        fn_: Some(fn_.to_string()),
        email: Some("user@example.com".to_string()),
        phone: Some("+34600000000".to_string()),
        organization: Some("OxiDav".to_string()),
        notes: Some("nota de prueba".to_string()),
        raw_payload: format!("BEGIN:VCARD\nFN:{fn_}\nEND:VCARD"),
    }
}

async fn count(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(pool)
        .await
        .expect("count")
}

// ---------------------------------------------------------------------------
// Paso 2 — PRAGMAs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn init_pool_applies_pragmas() {
    let pool = fresh_pool("pragmas").await;

    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await
        .expect("journal_mode");
    assert_eq!(journal_mode, "wal");

    let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
        .fetch_one(&pool)
        .await
        .expect("synchronous");
    assert_eq!(synchronous, 1, "NORMAL == 1");

    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&pool)
        .await
        .expect("foreign_keys");
    assert_eq!(foreign_keys, 1);

    let temp_store: i64 = sqlx::query_scalar("PRAGMA temp_store")
        .fetch_one(&pool)
        .await
        .expect("temp_store");
    assert_eq!(temp_store, 2, "MEMORY == 2");

    let mmap_size: i64 = sqlx::query_scalar("PRAGMA mmap_size")
        .fetch_one(&pool)
        .await
        .expect("mmap_size");
    assert_eq!(mmap_size, 268_435_456);

    let busy_timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
        .fetch_one(&pool)
        .await
        .expect("busy_timeout");
    assert_eq!(busy_timeout, 5000);
}

// ---------------------------------------------------------------------------
// Paso 3 — Migration schema
// ---------------------------------------------------------------------------

#[tokio::test]
async fn migration_creates_tables_and_indexes() {
    let pool = fresh_pool("schema").await;

    let objects: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .expect("sqlite_master");

    for expected in [
        "collections",
        "calendar_objects",
        "card_objects",
        "events_fts",
        "contacts_fts",
        "idx_calendar_objects_collection",
        "idx_calendar_objects_uid",
        "idx_calendar_objects_etag",
        "idx_calendar_objects_dtstart",
        "idx_card_objects_collection",
        "idx_card_objects_uid",
        "idx_card_objects_etag",
        "idx_card_objects_fn",
        "uq_calendar_objects_collection_uid",
        "uq_card_objects_collection_uid",
    ] {
        assert!(
            objects.iter().any(|o| o == expected),
            "missing schema object: {expected}"
        );
    }
}

#[tokio::test]
async fn migration_is_idempotent() {
    let url = common::temp_db_url("data", "idempotent");
    let first = init_pool(&url).await.expect("first init");
    drop(first);
    // Re-running the migrator over the same database must not fail.
    let second = init_pool(&url).await.expect("second init");
    let tables = count(
        &second,
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'collections'",
    )
    .await;
    assert_eq!(tables, 1);
}

// ---------------------------------------------------------------------------
// Paso 4 — Cascade delete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleting_collection_cascades_to_objects() {
    let pool = fresh_pool("cascade").await;
    create_collection(
        &pool,
        &new_collection("c1", "user-a", CollectionKind::Calendar),
    )
    .await
    .expect("collection");
    insert_calendar_object(&pool, &new_event("e1", "c1", "uid-1", "reunion especial"))
        .await
        .expect("event");
    insert_card_object(&pool, &new_card("k1", "c1", "uid-2", "Ada Lovelace"))
        .await
        .expect("card");
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM calendar_objects").await,
        1
    );
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM card_objects").await, 1);

    assert!(
        delete_collection(&pool, "c1", "user-a")
            .await
            .expect("delete collection"),
        "the owner must be able to delete the collection"
    );

    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM calendar_objects").await,
        0
    );
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM card_objects").await, 0);

    // The cascade also fires the DELETE triggers, so the FTS indexes must be
    // purged: no stale tokens / false positives remain.
    assert!(
        search_events(&pool, "especial", 10)
            .await
            .expect("search")
            .is_empty(),
        "events_fts must be cleared after cascading collection delete"
    );
    assert!(
        search_contacts(&pool, "Lovelace", 10)
            .await
            .expect("search")
            .is_empty(),
        "contacts_fts must be cleared after cascading collection delete"
    );
}

#[tokio::test]
async fn delete_collection_is_scoped_to_owner() {
    let pool = fresh_pool("delete-scope").await;
    create_collection(
        &pool,
        &new_collection("c1", "user-a", CollectionKind::Calendar),
    )
    .await
    .expect("collection");

    // A different user cannot delete it (single atomic statement).
    assert!(
        !delete_collection(&pool, "c1", "user-b")
            .await
            .expect("delete attempt"),
        "a foreign user must not be able to delete the collection"
    );
    assert!(
        get_collection(&pool, "c1", "user-a")
            .await
            .expect("get")
            .is_some(),
        "the collection must still exist after a foreign delete attempt"
    );

    // The owner can.
    assert!(
        delete_collection(&pool, "c1", "user-a")
            .await
            .expect("delete"),
        "the owner must be able to delete the collection"
    );
    assert!(
        get_collection(&pool, "c1", "user-a")
            .await
            .expect("get")
            .is_none()
    );
}

// ---------------------------------------------------------------------------
// Paso 5 — INSERT triggers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn insert_trigger_indexes_objects() {
    let pool = fresh_pool("insert-trigger").await;
    create_collection(&pool, &new_collection("c1", "u", CollectionKind::Calendar))
        .await
        .expect("collection");
    create_collection(
        &pool,
        &new_collection("c2", "u", CollectionKind::Addressbook),
    )
    .await
    .expect("collection");

    insert_calendar_object(&pool, &new_event("e1", "c1", "uid-1", "reunion importante"))
        .await
        .expect("event");
    insert_card_object(&pool, &new_card("k1", "c2", "uid-2", "Ada Lovelace"))
        .await
        .expect("card");

    // Internal substring (>= 3 chars) matched by the trigram tokenizer.
    assert_eq!(
        search_events(&pool, "union", 10)
            .await
            .expect("search")
            .len(),
        1
    );
    assert_eq!(
        search_contacts(&pool, "ovel", 10)
            .await
            .expect("search")
            .len(),
        1
    );
}

// ---------------------------------------------------------------------------
// Paso 6 — UPDATE triggers (delete + insert)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_trigger_refreshes_index_without_duplicates() {
    let pool = fresh_pool("update-trigger").await;
    create_collection(&pool, &new_collection("c1", "u", CollectionKind::Calendar))
        .await
        .expect("collection");
    let created =
        insert_calendar_object(&pool, &new_event("e1", "c1", "uid-1", "alpha beta gamma"))
            .await
            .expect("event");

    let updated = update_calendar_object(
        &pool,
        "e1",
        &CalendarObjectUpdate {
            summary: Some("delta epsilon zeta".to_string()),
            description: Some("descripcion de prueba".to_string()),
            dtstart: created.dtstart,
            dtend: created.dtend,
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            raw_payload: "BEGIN:VCALENDAR\nSUMMARY:delta epsilon zeta\nEND:VCALENDAR".to_string(),
        },
    )
    .await
    .expect("update");

    assert_ne!(created.etag, updated.etag, "update must refresh the etag");
    assert_eq!(
        search_events(&pool, "alpha", 10)
            .await
            .expect("search")
            .len(),
        0
    );
    let delta = search_events(&pool, "delta", 10).await.expect("search");
    assert_eq!(delta.len(), 1, "no duplicate rows after update");
    assert_eq!(delta[0].id, "e1");
}

#[tokio::test]
async fn card_update_trigger_refreshes_index() {
    let pool = fresh_pool("card-update-trigger").await;
    create_collection(
        &pool,
        &new_collection("c1", "u", CollectionKind::Addressbook),
    )
    .await
    .expect("collection");
    insert_card_object(&pool, &new_card("k1", "c1", "uid-1", "Grace Hopper"))
        .await
        .expect("card");

    update_card_object(
        &pool,
        "k1",
        &CardObjectUpdate {
            fn_: Some("Alan Turing".to_string()),
            email: Some("alan@example.com".to_string()),
            phone: None,
            organization: None,
            notes: None,
            raw_payload: "BEGIN:VCARD\nFN:Alan Turing\nEND:VCARD".to_string(),
        },
    )
    .await
    .expect("update");

    assert_eq!(
        search_contacts(&pool, "Grace", 10)
            .await
            .expect("search")
            .len(),
        0
    );
    assert_eq!(
        search_contacts(&pool, "Turing", 10)
            .await
            .expect("search")
            .len(),
        1
    );
}

// ---------------------------------------------------------------------------
// Paso 7 — DELETE triggers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_trigger_removes_tokens() {
    let pool = fresh_pool("delete-trigger").await;
    create_collection(&pool, &new_collection("c1", "u", CollectionKind::Calendar))
        .await
        .expect("collection");
    insert_calendar_object(&pool, &new_event("e1", "c1", "uid-1", "palabra única aqui"))
        .await
        .expect("event");
    assert_eq!(
        search_events(&pool, "palabra", 10)
            .await
            .expect("search")
            .len(),
        1
    );

    delete_calendar_object(&pool, "e1").await.expect("delete");

    assert_eq!(
        search_events(&pool, "palabra", 10)
            .await
            .expect("search")
            .len(),
        0,
        "no false positives after delete"
    );
}

#[tokio::test]
async fn card_delete_trigger_removes_tokens() {
    let pool = fresh_pool("card-delete-trigger").await;
    create_collection(
        &pool,
        &new_collection("c1", "u", CollectionKind::Addressbook),
    )
    .await
    .expect("collection");
    insert_card_object(&pool, &new_card("k1", "c1", "uid-1", "Linus Torvalds"))
        .await
        .expect("card");
    delete_card_object(&pool, "k1").await.expect("delete");

    assert_eq!(
        search_contacts(&pool, "Torvalds", 10)
            .await
            .expect("search")
            .len(),
        0
    );
}

// ---------------------------------------------------------------------------
// Paso 8 — bm25 ordering and trigram substring matching
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_orders_by_bm25_ascending() {
    let pool = fresh_pool("bm25").await;
    create_collection(&pool, &new_collection("c1", "u", CollectionKind::Calendar))
        .await
        .expect("collection");

    // Row "many" repeats the query term thrice; row "few" once: bm25("many")
    // must be more negative (more relevant) so it sorts first under ASC.
    insert_calendar_object(
        &pool,
        &new_event("e-many", "c1", "uid-many", "test test test"),
    )
    .await
    .expect("event");
    insert_calendar_object(&pool, &new_event("e-few", "c1", "uid-few", "a test b"))
        .await
        .expect("event");

    let results = search_events(&pool, "test", 10).await.expect("search");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].id, "e-many", "most relevant first (bm25 ASC)");
}

#[tokio::test]
async fn short_query_returns_empty() {
    let pool = fresh_pool("short-query").await;
    create_collection(&pool, &new_collection("c1", "u", CollectionKind::Calendar))
        .await
        .expect("collection");
    insert_calendar_object(&pool, &new_event("e1", "c1", "uid-1", "reunion"))
        .await
        .expect("event");

    assert!(
        search_events(&pool, "ab", 10)
            .await
            .expect("search")
            .is_empty()
    );
    assert!(
        search_events(&pool, "  a  ", 10)
            .await
            .expect("search")
            .is_empty()
    );
    assert!(
        search_contacts(&pool, "", 10)
            .await
            .expect("search")
            .is_empty()
    );
}

#[tokio::test]
async fn search_matches_internal_substrings() {
    let pool = fresh_pool("substring").await;
    create_collection(&pool, &new_collection("c1", "u", CollectionKind::Calendar))
        .await
        .expect("collection");
    insert_calendar_object(&pool, &new_event("e1", "c1", "uid-1", "extraordinario"))
        .await
        .expect("event");

    // "ordin" is an internal substring of "extraordinario".
    let found = search_events(&pool, "ordin", 10).await.expect("search");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, "e1");
}

#[tokio::test]
async fn trigram_three_char_boundary() {
    let pool = fresh_pool("trigram-boundary").await;
    create_collection(&pool, &new_collection("c1", "u", CollectionKind::Calendar))
        .await
        .expect("collection");
    // "extraordinario" contains the internal trigram "ord".
    insert_calendar_object(&pool, &new_event("e1", "c1", "uid-1", "extraordinario"))
        .await
        .expect("event");

    // Exactly 3 characters: the minimum the `trigram` tokenizer can index/match.
    let three = search_events(&pool, "ord", 10).await.expect("search");
    assert_eq!(three.len(), 1, "3-char substring must match");
    assert_eq!(three[0].id, "e1");

    // 2 characters: below the tokenizer minimum -> empty, no DB access.
    assert!(
        search_events(&pool, "or", 10)
            .await
            .expect("search")
            .is_empty(),
        "2-char query must not match"
    );
}

#[tokio::test]
async fn search_limit_is_clamped() {
    let pool = fresh_pool("limit-clamp").await;
    create_collection(&pool, &new_collection("c1", "u", CollectionKind::Calendar))
        .await
        .expect("collection");
    for (id, uid) in [("e1", "uid-1"), ("e2", "uid-2"), ("e3", "uid-3")] {
        insert_calendar_object(&pool, &new_event(id, "c1", uid, "reunion importante"))
            .await
            .expect("event");
    }

    // Decision: `limit <= 0` is clamped to 1 (NOT "no limit"). A negative limit
    // must never behave like SQLite's `LIMIT -1` (which means unlimited).
    let zero = search_events(&pool, "reunion", 0).await.expect("search");
    assert_eq!(zero.len(), 1, "limit=0 clamps to 1");
    let negative = search_events(&pool, "reunion", -1).await.expect("search");
    assert_eq!(negative.len(), 1, "limit=-1 must not mean unlimited");

    // A limit above MAX_SEARCH_LIMIT is capped; here 3 rows < MAX, so all 3.
    let huge = search_events(&pool, "reunion", i64::MAX)
        .await
        .expect("search");
    assert!(huge.len() <= MAX_SEARCH_LIMIT as usize);

    // A normal limit still works.
    let normal = search_events(&pool, "reunion", 2).await.expect("search");
    assert_eq!(normal.len(), 2);
}

// ---------------------------------------------------------------------------
// F10 — user-scoped search (anti-IDOR)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_scoped_isolates_users() {
    let pool = fresh_pool("search-scoped").await;

    // Two users, each owning a calendar and an address book.
    for (id, user, kind) in [
        ("cal-a", "user-a", CollectionKind::Calendar),
        ("cal-b", "user-b", CollectionKind::Calendar),
        ("book-a", "user-a", CollectionKind::Addressbook),
        ("book-b", "user-b", CollectionKind::Addressbook),
    ] {
        create_collection(&pool, &new_collection(id, user, kind))
            .await
            .expect("collection");
    }

    // Both calendars hold an event matching "reunion"; both books hold a card
    // matching "Lovelace". Distinct ids let us assert exactly which rows leak.
    insert_calendar_object(
        &pool,
        &new_event("evt-a", "cal-a", "uid-ea", "reunion privada"),
    )
    .await
    .expect("event a");
    insert_calendar_object(
        &pool,
        &new_event("evt-b", "cal-b", "uid-eb", "reunion privada"),
    )
    .await
    .expect("event b");
    insert_card_object(
        &pool,
        &new_card("card-a", "book-a", "uid-ca", "Ada Lovelace"),
    )
    .await
    .expect("card a");
    insert_card_object(
        &pool,
        &new_card("card-b", "book-b", "uid-cb", "Ada Lovelace"),
    )
    .await
    .expect("card b");

    // Baseline: the unscoped F03 helpers see *both* users' objects (the IDOR).
    let all_events = search_events(&pool, "reunion", 10).await.expect("search");
    assert_eq!(all_events.len(), 2, "unscoped helper spans both users");
    let all_cards = search_contacts(&pool, "Lovelace", 10)
        .await
        .expect("search");
    assert_eq!(all_cards.len(), 2, "unscoped helper spans both users");

    // Scoped: user-a sees only user-a's rows, user-b only user-b's.
    let a_events = search_events_scoped(&pool, "user-a", "reunion", 10)
        .await
        .expect("scoped");
    assert_eq!(
        a_events.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec!["evt-a"]
    );
    let b_events = search_events_scoped(&pool, "user-b", "reunion", 10)
        .await
        .expect("scoped");
    assert_eq!(
        b_events.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec!["evt-b"]
    );

    let a_cards = search_contacts_scoped(&pool, "user-a", "Lovelace", 10)
        .await
        .expect("scoped");
    assert_eq!(
        a_cards.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec!["card-a"]
    );
    let b_cards = search_contacts_scoped(&pool, "user-b", "Lovelace", 10)
        .await
        .expect("scoped");
    assert_eq!(
        b_cards.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec!["card-b"]
    );

    // A user with no collections sees nothing, even for a term others match.
    let ghost = search_events_scoped(&pool, "user-c", "reunion", 10)
        .await
        .expect("scoped");
    assert!(ghost.is_empty(), "user-c owns nothing and must see nothing");
    let ghost_cards = search_contacts_scoped(&pool, "user-c", "Lovelace", 10)
        .await
        .expect("scoped");
    assert!(
        ghost_cards.is_empty(),
        "user-c owns nothing and must see nothing"
    );
}

// ---------------------------------------------------------------------------
// Paso 9 — Uniqueness (collection_id, uid)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn calendar_uid_is_unique_within_collection() {
    let pool = fresh_pool("unique-calendar").await;
    create_collection(&pool, &new_collection("c1", "u", CollectionKind::Calendar))
        .await
        .expect("collection");
    create_collection(&pool, &new_collection("c2", "u", CollectionKind::Calendar))
        .await
        .expect("collection");

    insert_calendar_object(&pool, &new_event("e1", "c1", "uid-1", "first"))
        .await
        .expect("first insert");

    // Same collection + uid -> constraint violation.
    let clash = insert_calendar_object(&pool, &new_event("e2", "c1", "uid-1", "clash")).await;
    assert!(
        matches!(clash, Err(DbError::Query(_))),
        "expected unique violation"
    );

    // Same uid, different collection -> OK.
    insert_calendar_object(
        &pool,
        &new_event("e3", "c2", "uid-1", "ok-different-collection"),
    )
    .await
    .expect("same uid other collection");
    // Different uid, same collection -> OK.
    insert_calendar_object(&pool, &new_event("e4", "c1", "uid-2", "ok-different-uid"))
        .await
        .expect("different uid same collection");
}

#[tokio::test]
async fn card_uid_is_unique_within_collection() {
    let pool = fresh_pool("unique-card").await;
    create_collection(
        &pool,
        &new_collection("c1", "u", CollectionKind::Addressbook),
    )
    .await
    .expect("collection");
    create_collection(
        &pool,
        &new_collection("c2", "u", CollectionKind::Addressbook),
    )
    .await
    .expect("collection");

    insert_card_object(&pool, &new_card("k1", "c1", "uid-1", "First"))
        .await
        .expect("first insert");

    let clash = insert_card_object(&pool, &new_card("k2", "c1", "uid-1", "Clash")).await;
    assert!(
        matches!(clash, Err(DbError::Query(_))),
        "expected unique violation"
    );

    insert_card_object(&pool, &new_card("k3", "c2", "uid-1", "Other collection"))
        .await
        .expect("same uid other collection");
    insert_card_object(&pool, &new_card("k4", "c1", "uid-2", "Other uid"))
        .await
        .expect("different uid same collection");
}

// ---------------------------------------------------------------------------
// Repo extras — sync_token + listing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn writes_advance_collection_sync_token() {
    let pool = fresh_pool("sync-token").await;
    let created = create_collection(&pool, &new_collection("c1", "u", CollectionKind::Calendar))
        .await
        .expect("collection");
    assert_eq!(created.sync_token, 1);

    insert_calendar_object(&pool, &new_event("e1", "c1", "uid-1", "one"))
        .await
        .expect("insert");
    insert_calendar_object(&pool, &new_event("e2", "c1", "uid-2", "two"))
        .await
        .expect("insert");
    delete_calendar_object(&pool, "e1").await.expect("delete");

    let listed = list_collections(&pool, "u", 50, 0).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].sync_token, 4, "1 initial + 3 writes");

    let other = list_collections(&pool, "nobody", 50, 0)
        .await
        .expect("list");
    assert!(other.is_empty());
}

// ---------------------------------------------------------------------------
// F06 Tarea 0 — collection repository extensions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_collection_is_scoped_by_user() {
    let pool = fresh_pool("get-collection").await;
    create_collection(
        &pool,
        &new_collection("c1", "user-a", CollectionKind::Calendar),
    )
    .await
    .expect("collection");

    let owned = get_collection(&pool, "c1", "user-a").await.expect("get");
    assert_eq!(owned.expect("owned collection").id, "c1");

    let foreign = get_collection(&pool, "c1", "user-b").await.expect("get");
    assert!(foreign.is_none(), "another user's collection is invisible");

    let missing = get_collection(&pool, "nope", "user-a").await.expect("get");
    assert!(missing.is_none(), "unknown id yields None");
}

#[tokio::test]
async fn update_collection_applies_only_provided_fields() {
    let pool = fresh_pool("update-collection").await;
    let created = create_collection(&pool, &new_collection("c1", "u", CollectionKind::Calendar))
        .await
        .expect("collection");

    // Give the clock a millisecond so `updated_at` can observably advance.
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;

    // Only `name` is provided: `color` must stay untouched.
    let renamed = update_collection(&pool, "c1", "u", Some("Renombrada"), None)
        .await
        .expect("update")
        .expect("exists");
    assert_eq!(renamed.name, "Renombrada");
    assert_eq!(renamed.color, created.color, "color must be preserved");
    assert!(
        renamed.updated_at > created.updated_at,
        "update must bump updated_at"
    );

    // Only `color` is provided: `name` from the previous step is preserved.
    let recolored = update_collection(&pool, "c1", "u", None, Some("#ff0000"))
        .await
        .expect("update")
        .expect("exists");
    assert_eq!(recolored.name, "Renombrada", "name must be preserved");
    assert_eq!(recolored.color, "#ff0000");
}

#[tokio::test]
async fn update_collection_is_scoped_by_user() {
    let pool = fresh_pool("update-collection-scoping").await;
    create_collection(
        &pool,
        &new_collection("c1", "owner", CollectionKind::Calendar),
    )
    .await
    .expect("collection");

    let foreign = update_collection(&pool, "c1", "intruder", Some("hacked"), None)
        .await
        .expect("update");
    assert!(foreign.is_none(), "another user cannot update");

    let missing = update_collection(&pool, "nope", "owner", Some("x"), None)
        .await
        .expect("update");
    assert!(missing.is_none(), "unknown id yields None");

    let unchanged = get_collection(&pool, "c1", "owner")
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(
        unchanged.name, "Collection c1",
        "foreign update was a no-op"
    );
}

#[tokio::test]
async fn count_collections_is_scoped_by_user() {
    let pool = fresh_pool("count-collections").await;
    for id in ["c1", "c2"] {
        create_collection(
            &pool,
            &new_collection(id, "user-a", CollectionKind::Calendar),
        )
        .await
        .expect("collection");
    }
    create_collection(
        &pool,
        &new_collection("c3", "user-b", CollectionKind::Addressbook),
    )
    .await
    .expect("collection");

    assert_eq!(count_collections(&pool, "user-a").await.expect("count"), 2);
    assert_eq!(count_collections(&pool, "user-b").await.expect("count"), 1);
    assert_eq!(count_collections(&pool, "nobody").await.expect("count"), 0);
}

#[tokio::test]
async fn list_collections_is_scoped_and_paginated() {
    let pool = fresh_pool("list-pagination").await;
    for id in ["c1", "c2", "c3"] {
        create_collection(&pool, &new_collection(id, "u", CollectionKind::Calendar))
            .await
            .expect("collection");
    }
    create_collection(
        &pool,
        &new_collection("x1", "other", CollectionKind::Calendar),
    )
    .await
    .expect("collection");

    // Ordered by name ("Collection c1" < "Collection c2" < ...).
    let first_page = list_collections(&pool, "u", 2, 0).await.expect("list");
    assert_eq!(first_page.len(), 2);
    assert_eq!(first_page[0].id, "c1");
    assert_eq!(first_page[1].id, "c2");

    let second_page = list_collections(&pool, "u", 2, 2).await.expect("list");
    assert_eq!(second_page.len(), 1);
    assert_eq!(second_page[0].id, "c3");

    // Scoping: only the caller's own rows are returned.
    let other = list_collections(&pool, "other", 10, 0).await.expect("list");
    assert_eq!(other.len(), 1);
    assert_eq!(other[0].id, "x1");

    // `limit <= 0` falls back to the default (never SQLite's unbounded -1):
    // with 3 rows the default page still returns all of them.
    let defaulted = list_collections(&pool, "u", 0, 0).await.expect("list");
    assert_eq!(defaulted.len(), 3);
    let negative = list_collections(&pool, "u", -5, 0).await.expect("list");
    assert_eq!(negative.len(), 3);

    // A negative offset is clamped to 0.
    let clamped_offset = list_collections(&pool, "u", 10, -3).await.expect("list");
    assert_eq!(clamped_offset.len(), 3);
}

/// Read `collections.sync_token` directly for atomicity assertions.
async fn sync_token(pool: &SqlitePool, collection_id: &str) -> i64 {
    sqlx::query_scalar("SELECT sync_token FROM collections WHERE id = ?")
        .bind(collection_id)
        .fetch_one(pool)
        .await
        .expect("sync_token")
}

#[tokio::test]
async fn failed_write_does_not_advance_sync_token() {
    let pool = fresh_pool("atomicity").await;
    create_collection(&pool, &new_collection("c1", "u", CollectionKind::Calendar))
        .await
        .expect("collection");
    assert_eq!(sync_token(&pool, "c1").await, 1);

    // A successful write advances the token by exactly one.
    insert_calendar_object(&pool, &new_event("e1", "c1", "uid-1", "one"))
        .await
        .expect("insert");
    assert_eq!(sync_token(&pool, "c1").await, 2);

    // A duplicate (collection_id, uid) violates the UNIQUE index: the INSERT
    // fails, so the mutation + sync_token bump must roll back together and the
    // token must NOT advance.
    let clash = insert_calendar_object(&pool, &new_event("e2", "c1", "uid-1", "clash")).await;
    assert!(
        matches!(clash, Err(DbError::Query(_))),
        "expected unique violation"
    );
    assert_eq!(
        sync_token(&pool, "c1").await,
        2,
        "failed mutation must not advance sync_token"
    );
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM calendar_objects").await,
        1,
        "no partial row persisted"
    );

    // A subsequent valid insert proves the write path still advances the token.
    insert_calendar_object(&pool, &new_event("e3", "c1", "uid-2", "two"))
        .await
        .expect("insert");
    assert_eq!(sync_token(&pool, "c1").await, 3);
}

// ---------------------------------------------------------------------------
// F07 — calendar-object listing/retrieval (scoped by collection)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_calendar_objects_is_scoped_to_collection() {
    let pool = fresh_pool("list-calendar-objects").await;
    create_collection(&pool, &new_collection("c1", "u", CollectionKind::Calendar))
        .await
        .expect("collection");
    create_collection(&pool, &new_collection("c2", "u", CollectionKind::Calendar))
        .await
        .expect("collection");

    // Insert out of `uid` order to prove the deterministic ordering.
    insert_calendar_object(&pool, &new_event("e2", "c1", "uid-b", "second"))
        .await
        .expect("event b");
    insert_calendar_object(&pool, &new_event("e1", "c1", "uid-a", "first"))
        .await
        .expect("event a");
    // A uid identical to one in c1 but in a *different* collection.
    insert_calendar_object(&pool, &new_event("e3", "c2", "uid-a", "other collection"))
        .await
        .expect("event other");

    let listed = list_calendar_objects(&pool, "c1").await.expect("list");
    assert_eq!(listed.len(), 2, "only c1's objects are returned");
    assert_eq!(listed[0].uid, "uid-a", "ordered by uid ascending");
    assert_eq!(listed[1].uid, "uid-b");

    // An unknown collection yields an empty list, not an error.
    assert!(
        list_calendar_objects(&pool, "missing")
            .await
            .expect("list")
            .is_empty()
    );
}

#[tokio::test]
async fn get_calendar_object_is_scoped_by_collection_and_uid() {
    let pool = fresh_pool("get-calendar-object").await;
    create_collection(&pool, &new_collection("c1", "u", CollectionKind::Calendar))
        .await
        .expect("collection");
    create_collection(&pool, &new_collection("c2", "u", CollectionKind::Calendar))
        .await
        .expect("collection");

    insert_calendar_object(&pool, &new_event("e1", "c1", "uid-1", "first"))
        .await
        .expect("event");

    let found = get_calendar_object(&pool, "c1", "uid-1")
        .await
        .expect("get")
        .expect("must find the object");
    assert_eq!(found.id, "e1");
    assert_eq!(found.collection_id, "c1");

    // Same uid, different collection: invisible to this collection scope.
    assert!(
        get_calendar_object(&pool, "c2", "uid-1")
            .await
            .expect("get")
            .is_none(),
        "uid must not leak across collections"
    );
    // Unknown uid in a known collection.
    assert!(
        get_calendar_object(&pool, "c1", "nope")
            .await
            .expect("get")
            .is_none()
    );
}

// ---------------------------------------------------------------------------
// F08 — card-object listing/retrieval (scoped by collection)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_card_objects_is_scoped_to_collection() {
    let pool = fresh_pool("list-card-objects").await;
    create_collection(
        &pool,
        &new_collection("c1", "u", CollectionKind::Addressbook),
    )
    .await
    .expect("collection");
    create_collection(
        &pool,
        &new_collection("c2", "u", CollectionKind::Addressbook),
    )
    .await
    .expect("collection");

    // Insert out of `uid` order to prove the deterministic ordering.
    insert_card_object(&pool, &new_card("k2", "c1", "uid-b", "Second"))
        .await
        .expect("card b");
    insert_card_object(&pool, &new_card("k1", "c1", "uid-a", "First"))
        .await
        .expect("card a");
    // A uid identical to one in c1 but in a *different* collection.
    insert_card_object(&pool, &new_card("k3", "c2", "uid-a", "Other collection"))
        .await
        .expect("card other");

    let listed = list_card_objects(&pool, "c1").await.expect("list");
    assert_eq!(listed.len(), 2, "only c1's objects are returned");
    assert_eq!(listed[0].uid, "uid-a", "ordered by uid ascending");
    assert_eq!(listed[1].uid, "uid-b");
    assert_eq!(listed[0].fn_.as_deref(), Some("First"));

    // An unknown collection yields an empty list, not an error.
    assert!(
        list_card_objects(&pool, "missing")
            .await
            .expect("list")
            .is_empty()
    );
}

#[tokio::test]
async fn get_card_object_is_scoped_by_collection_and_uid() {
    let pool = fresh_pool("get-card-object").await;
    create_collection(
        &pool,
        &new_collection("c1", "u", CollectionKind::Addressbook),
    )
    .await
    .expect("collection");
    create_collection(
        &pool,
        &new_collection("c2", "u", CollectionKind::Addressbook),
    )
    .await
    .expect("collection");

    insert_card_object(&pool, &new_card("k1", "c1", "uid-1", "Ada Lovelace"))
        .await
        .expect("card");

    let found = get_card_object(&pool, "c1", "uid-1")
        .await
        .expect("get")
        .expect("must find the object");
    assert_eq!(found.id, "k1");
    assert_eq!(found.collection_id, "c1");
    assert_eq!(found.email.as_deref(), Some("user@example.com"));

    // Same uid, different collection: invisible to this collection scope.
    assert!(
        get_card_object(&pool, "c2", "uid-1")
            .await
            .expect("get")
            .is_none(),
        "uid must not leak across collections"
    );
    // Unknown uid in a known collection.
    assert!(
        get_card_object(&pool, "c1", "nope")
            .await
            .expect("get")
            .is_none()
    );
}

// ---------------------------------------------------------------------------
// F09 — sync-collection change log (sync_log, token bump and tombstones)
// ---------------------------------------------------------------------------

/// Raw `(uid, kind, change, token)` rows of a collection's `sync_log`, ordered
/// by the token they were recorded at.
async fn sync_log_rows(
    pool: &SqlitePool,
    collection_id: &str,
) -> Vec<(String, String, String, i64)> {
    sqlx::query_as::<_, (String, String, String, i64)>(
        "SELECT resource_uid, resource_kind, change, sync_token
           FROM sync_log
          WHERE collection_id = ?
          ORDER BY sync_token ASC, id ASC",
    )
    .bind(collection_id)
    .fetch_all(pool)
    .await
    .expect("sync_log rows")
}

#[tokio::test]
async fn migration_creates_sync_log_and_indexes_idempotently() {
    let url = common::temp_db_url("data", "sync-log-schema");
    let pool = init_pool(&url).await.expect("init");
    drop(pool);

    // Re-running the migrator must be a no-op (CREATE ... IF NOT EXISTS).
    let pool = init_pool(&url).await.expect("re-init");

    let table = count(
        &pool,
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'sync_log'",
    )
    .await;
    assert_eq!(table, 1, "sync_log table must exist exactly once");

    for index in [
        "idx_sync_log_collection_token",
        "idx_sync_log_collection_uid",
    ] {
        let present = count(
            &pool,
            &format!(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = '{index}'"
            ),
        )
        .await;
        assert_eq!(present, 1, "missing index: {index}");
    }

    // The incremental sweep index must cover `(collection_id, sync_token)`.
    let info: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_index_info('idx_sync_log_collection_token')")
            .fetch_all(&pool)
            .await
            .expect("index_info");
    assert_eq!(
        info,
        vec!["collection_id".to_string(), "sync_token".to_string()]
    );
}

#[tokio::test]
async fn calendar_writes_record_log_entries_and_advance_token_atomically() {
    let pool = fresh_pool("sync-log-calendar").await;
    create_collection(&pool, &new_collection("c1", "u", CollectionKind::Calendar))
        .await
        .expect("collection");

    assert_eq!(next_sync_token(&pool, "c1").await.expect("token"), 1);

    let created = insert_calendar_object(&pool, &new_event("e1", "c1", "uid-1", "alpha"))
        .await
        .expect("insert");
    assert_eq!(next_sync_token(&pool, "c1").await.expect("token"), 2);

    update_calendar_object(
        &pool,
        "e1",
        &CalendarObjectUpdate {
            summary: Some("beta".to_string()),
            description: Some("descripcion de prueba".to_string()),
            dtstart: created.dtstart,
            dtend: created.dtend,
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            raw_payload: "BEGIN:VCALENDAR\nSUMMARY:beta\nEND:VCALENDAR".to_string(),
        },
    )
    .await
    .expect("update");
    assert_eq!(next_sync_token(&pool, "c1").await.expect("token"), 3);

    delete_calendar_object(&pool, "e1").await.expect("delete");
    assert_eq!(next_sync_token(&pool, "c1").await.expect("token"), 4);

    let rows = sync_log_rows(&pool, "c1").await;
    assert_eq!(
        rows,
        vec![
            (
                "uid-1".to_string(),
                "calendar".to_string(),
                "added".to_string(),
                2
            ),
            (
                "uid-1".to_string(),
                "calendar".to_string(),
                "updated".to_string(),
                3
            ),
            (
                "uid-1".to_string(),
                "calendar".to_string(),
                "deleted".to_string(),
                4
            ),
        ]
    );
}

#[tokio::test]
async fn card_writes_record_log_entries_and_advance_token_atomically() {
    let pool = fresh_pool("sync-log-card").await;
    create_collection(
        &pool,
        &new_collection("c1", "u", CollectionKind::Addressbook),
    )
    .await
    .expect("collection");

    insert_card_object(&pool, &new_card("k1", "c1", "uid-1", "Grace Hopper"))
        .await
        .expect("insert");
    assert_eq!(next_sync_token(&pool, "c1").await.expect("token"), 2);

    update_card_object(
        &pool,
        "k1",
        &CardObjectUpdate {
            fn_: Some("Alan Turing".to_string()),
            email: Some("alan@example.com".to_string()),
            phone: None,
            organization: None,
            notes: None,
            raw_payload: "BEGIN:VCARD\nFN:Alan Turing\nEND:VCARD".to_string(),
        },
    )
    .await
    .expect("update");
    assert_eq!(next_sync_token(&pool, "c1").await.expect("token"), 3);

    delete_card_object(&pool, "k1").await.expect("delete");
    assert_eq!(next_sync_token(&pool, "c1").await.expect("token"), 4);

    let rows = sync_log_rows(&pool, "c1").await;
    assert_eq!(
        rows,
        vec![
            (
                "uid-1".to_string(),
                "card".to_string(),
                "added".to_string(),
                2
            ),
            (
                "uid-1".to_string(),
                "card".to_string(),
                "updated".to_string(),
                3
            ),
            (
                "uid-1".to_string(),
                "card".to_string(),
                "deleted".to_string(),
                4
            ),
        ]
    );
}

#[tokio::test]
async fn changes_since_returns_only_newer_changes_in_order_with_tombstones() {
    let pool = fresh_pool("sync-changes-since").await;
    create_collection(&pool, &new_collection("c1", "u", CollectionKind::Calendar))
        .await
        .expect("collection");

    let first = insert_calendar_object(&pool, &new_event("e1", "c1", "uid-1", "alpha"))
        .await
        .expect("insert e1"); // token 2
    insert_calendar_object(&pool, &new_event("e2", "c1", "uid-2", "beta"))
        .await
        .expect("insert e2"); // token 3
    update_calendar_object(
        &pool,
        "e1",
        &CalendarObjectUpdate {
            summary: Some("gamma".to_string()),
            description: None,
            dtstart: first.dtstart,
            dtend: first.dtend,
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            raw_payload: "BEGIN:VCALENDAR\nSUMMARY:gamma\nEND:VCALENDAR".to_string(),
        },
    )
    .await
    .expect("update e1"); // token 4
    delete_calendar_object(&pool, "e2")
        .await
        .expect("delete e2"); // token 5

    // Everything from the beginning: last change per resource, ordered by token.
    let all = changes_since(&pool, "c1", ResourceKind::Calendar, Some(0))
        .await
        .expect("changes");
    assert_eq!(all.len(), 2, "one entry per resource, latest change wins");
    // uid-1: added@2, updated@4 -> latest is the update at 4 (still live).
    assert_eq!(all[0].resource_uid, "uid-1");
    assert_eq!(all[0].resource_kind, ResourceKind::Calendar);
    assert_eq!(all[0].change, ChangeKind::Updated);
    assert_eq!(all[0].sync_token, 4);
    assert!(all[0].etag.is_some(), "live resource carries its ETag");
    // uid-2: added@3, deleted@5 -> latest is the tombstone at 5.
    assert_eq!(all[1].resource_uid, "uid-2");
    assert_eq!(all[1].change, ChangeKind::Deleted);
    assert_eq!(all[1].sync_token, 5);
    assert!(all[1].etag.is_none(), "tombstone carries no ETag");

    // Strictly greater than the `since` token: after e2's insert (3) the
    // changes recorded at 4 and 5 qualify, including the delete tombstone.
    let after_three = changes_since(&pool, "c1", ResourceKind::Calendar, Some(3))
        .await
        .expect("changes");
    assert_eq!(after_three.len(), 2);
    assert_eq!(after_three[0].resource_uid, "uid-1");
    assert_eq!(after_three[0].change, ChangeKind::Updated);
    assert_eq!(after_three[0].sync_token, 4);
    assert_eq!(after_three[1].resource_uid, "uid-2");
    assert_eq!(after_three[1].sync_token, 5);
    assert_eq!(after_three[1].change, ChangeKind::Deleted);

    // After the latest token there is nothing new.
    let up_to_date = changes_since(&pool, "c1", ResourceKind::Calendar, Some(5))
        .await
        .expect("changes");
    assert!(up_to_date.is_empty());

    // A `since` in the middle keeps only the strictly newer changes for uid-2.
    let after_four = changes_since(&pool, "c1", ResourceKind::Calendar, Some(4))
        .await
        .expect("changes");
    assert_eq!(after_four.len(), 1);
    assert_eq!(after_four[0].resource_uid, "uid-2");
    assert_eq!(after_four[0].change, ChangeKind::Deleted);
    assert_eq!(after_four[0].sync_token, 5);

    // Initial sync reports only live resources (uid-2 was deleted).
    let initial = changes_since(&pool, "c1", ResourceKind::Calendar, None)
        .await
        .expect("initial");
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].resource_uid, "uid-1");
    assert_eq!(initial[0].change, ChangeKind::Added);
    assert!(initial[0].etag.is_some());
}

#[tokio::test]
async fn failed_write_rolls_back_the_sync_log_entry() {
    let pool = fresh_pool("sync-log-atomic").await;
    create_collection(&pool, &new_collection("c1", "u", CollectionKind::Calendar))
        .await
        .expect("collection");

    insert_calendar_object(&pool, &new_event("e1", "c1", "uid-1", "alpha"))
        .await
        .expect("insert"); // token 2
    assert_eq!(sync_log_rows(&pool, "c1").await.len(), 1);

    // Same (collection_id, uid) with a different id: UNIQUE violation.
    let err = insert_calendar_object(&pool, &new_event("e2", "c1", "uid-1", "duplicate"))
        .await
        .expect_err("second insert with a duplicate uid must fail");
    assert!(
        err.is_unique_violation(),
        "expected a UNIQUE violation, got {err:?}"
    );

    // The failed write must not leave a log entry nor advance the token.
    assert_eq!(next_sync_token(&pool, "c1").await.expect("token"), 2);
    assert_eq!(sync_log_rows(&pool, "c1").await.len(), 1);
}

#[tokio::test]
async fn failed_card_write_rolls_back_the_sync_log_entry() {
    let pool = fresh_pool("sync-log-card-atomic").await;
    create_collection(
        &pool,
        &new_collection("c1", "u", CollectionKind::Addressbook),
    )
    .await
    .expect("collection");

    insert_card_object(&pool, &new_card("k1", "c1", "uid-1", "Grace Hopper"))
        .await
        .expect("insert"); // token 2

    let err = insert_card_object(&pool, &new_card("k2", "c1", "uid-1", "duplicate"))
        .await
        .expect_err("second insert with a duplicate uid must fail");
    assert!(
        err.is_unique_violation(),
        "expected a UNIQUE violation, got {err:?}"
    );

    assert_eq!(next_sync_token(&pool, "c1").await.expect("token"), 2);
    assert_eq!(sync_log_rows(&pool, "c1").await.len(), 1);
}

#[tokio::test]
async fn next_sync_token_matches_writes_and_idempotent_delete() {
    let pool = fresh_pool("sync-log-token").await;
    create_collection(&pool, &new_collection("c1", "u", CollectionKind::Calendar))
        .await
        .expect("collection");

    // Collections are created with token 1.
    assert_eq!(next_sync_token(&pool, "c1").await.expect("token"), 1);

    insert_calendar_object(&pool, &new_event("e1", "c1", "uid-1", "alpha"))
        .await
        .expect("insert");
    assert_eq!(next_sync_token(&pool, "c1").await.expect("token"), 2);

    // Deleting a non-existent object is a no-op: no bump, no extra entry.
    delete_calendar_object(&pool, "missing")
        .await
        .expect("no-op delete");
    assert_eq!(next_sync_token(&pool, "c1").await.expect("token"), 2);
    assert_eq!(sync_log_rows(&pool, "c1").await.len(), 1);

    // An unknown collection yields an error rather than a bogus token.
    assert!(next_sync_token(&pool, "missing").await.is_err());
}

#[tokio::test]
async fn deleting_a_collection_cascades_to_sync_log() {
    let pool = fresh_pool("sync-log-cascade").await;
    create_collection(
        &pool,
        &new_collection("c1", "user-a", CollectionKind::Calendar),
    )
    .await
    .expect("collection c1");
    create_collection(
        &pool,
        &new_collection("c2", "user-a", CollectionKind::Addressbook),
    )
    .await
    .expect("collection c2");

    // Populate both collections with history so each has sync_log entries.
    insert_calendar_object(&pool, &new_event("e1", "c1", "uid-1", "alpha"))
        .await
        .expect("insert event"); // token 2
    delete_calendar_object(&pool, "e1")
        .await
        .expect("delete event"); // token 3 (tombstone)
    insert_card_object(&pool, &new_card("k1", "c2", "uid-2", "Grace Hopper"))
        .await
        .expect("insert card"); // c2 token 2

    let before = count(&pool, "SELECT COUNT(*) FROM sync_log").await;
    assert_eq!(before, 3, "three log entries before the delete");

    // Deleting c1 must cascade to its objects *and* its sync_log rows...
    assert!(
        delete_collection(&pool, "c1", "user-a")
            .await
            .expect("delete c1"),
        "the owner must be able to delete c1"
    );
    assert_eq!(
        sync_log_rows(&pool, "c1").await.len(),
        0,
        "the deleted collection's sync_log history must be purged"
    );

    // ...without touching the sibling collection's history.
    assert_eq!(
        sync_log_rows(&pool, "c2").await.len(),
        1,
        "an unrelated collection's sync_log must survive"
    );
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM sync_log").await,
        1,
        "only c2's single entry remains — no orphaned garbage"
    );
}

// ---------------------------------------------------------------------------
// F18 — IANA timezone (`dtstart_tzid`/`dtend_tzid`) persistence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn calendar_object_tzid_columns_round_trip_through_sqlite() {
    let pool = fresh_pool("tzid").await;
    create_collection(
        &pool,
        &new_collection("c1", "user-a", CollectionKind::Calendar),
    )
    .await
    .expect("collection");

    // Without a tzid the new columns stay NULL and read back as `None`.
    let plain = insert_calendar_object(&pool, &new_event("e1", "c1", "uid-1", "sin zona"))
        .await
        .expect("insert plain");
    assert_eq!(plain.dtstart_tzid, None);
    assert_eq!(plain.dtend_tzid, None);

    // With a tzid it round-trips byte-for-byte.
    let mut anchored = new_event("e2", "c1", "uid-2", "con zona");
    anchored.dtstart_tzid = Some("Europe/Madrid".to_string());
    anchored.dtend_tzid = Some("Europe/Madrid".to_string());
    let stored = insert_calendar_object(&pool, &anchored)
        .await
        .expect("insert anchored");
    assert_eq!(stored.dtstart_tzid.as_deref(), Some("Europe/Madrid"));
    assert_eq!(stored.dtend_tzid.as_deref(), Some("Europe/Madrid"));

    // Re-read through a fresh query, so this is genuinely the stored column and
    // not the struct we just handed to the insert.
    let reloaded = get_calendar_object(&pool, "c1", "uid-2")
        .await
        .expect("get anchored")
        .expect("anchored row");
    assert_eq!(reloaded.dtstart_tzid.as_deref(), Some("Europe/Madrid"));
    assert_eq!(reloaded.dtend_tzid.as_deref(), Some("Europe/Madrid"));

    let reloaded_plain = get_calendar_object(&pool, "c1", "uid-1")
        .await
        .expect("get plain")
        .expect("plain row");
    assert_eq!(reloaded_plain.dtstart_tzid, None);
    assert_eq!(reloaded_plain.dtend_tzid, None);
}
