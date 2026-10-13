//! F05 — Tasks 2–7: token generation/hashing and the `app_tokens` repository.
//!
//! All tests run against throwaway file-backed databases under `/tmp/opencode`
//! and never touch the network.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use oxidav::auth::app_tokens::{
    TokenError, create_app_token, generate_token, hash_secret, list_app_tokens, parse_token,
    revoke_app_token, spawn_touch_last_used, touch_last_used, verify_app_token,
};
use oxidav::db::init_pool;
use sqlx::SqlitePool;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_db_url(tag: &str) -> String {
    let dir = PathBuf::from("/tmp/opencode/oxidav-tests");
    std::fs::create_dir_all(&dir).expect("create temp test dir");
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("tokens-{tag}-{}-{n}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    format!("sqlite://{}", path.display())
}

async fn fresh_pool(tag: &str) -> SqlitePool {
    init_pool(&temp_db_url(tag)).await.expect("init_pool")
}

async fn stored_hash(pool: &SqlitePool, id: &str) -> String {
    sqlx::query_scalar("SELECT secret_hash FROM app_tokens WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("secret_hash")
}

// ---------------------------------------------------------------------------
// Tasks 2 & 3 — generation and hashing (also covered by unit tests)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn generated_token_round_trips_and_has_high_entropy() {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    let (token, id, secret) = generate_token();
    assert!(token.starts_with("oxidav_"));
    let (parsed_id, parsed_secret) = parse_token(&token).expect("parse");
    assert_eq!(parsed_id, id);
    assert_eq!(parsed_secret, secret);
    assert!(URL_SAFE_NO_PAD.decode(&secret).expect("base64url").len() >= 32);
}

// ---------------------------------------------------------------------------
// Task 4 — persistence and verification
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_app_token_stores_the_hash_not_the_secret() {
    let pool = fresh_pool("store").await;
    let (id, token) = create_app_token(&pool, "user-1", "iPhone")
        .await
        .expect("create");
    let (_, secret) = parse_token(&token).expect("parse");

    let hash = stored_hash(&pool, &id).await;
    assert_ne!(hash, secret, "the secret must never be stored in the clear");
    assert!(!hash.contains(&secret), "hash must not embed the secret");
    assert_eq!(hash, hash_secret(&secret), "hash must be the digest");
    assert_eq!(hash.len(), 64);

    // The full token appears in no persistent column.
    let name: String = sqlx::query_scalar("SELECT name FROM app_tokens WHERE id = ?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .expect("name");
    assert!(!name.contains(&token));
}

#[tokio::test]
async fn verify_app_token_accepts_a_freshly_created_token() {
    let pool = fresh_pool("verify").await;
    let (_id, token) = create_app_token(&pool, "user-42", "DAVx5")
        .await
        .expect("create");

    let user = verify_app_token(&pool, &token).await.expect("verify");
    assert_eq!(user.sub, "user-42");
}

// ---------------------------------------------------------------------------
// Task 5 — revocation and token errors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn verify_app_token_rejects_a_revoked_token() {
    let pool = fresh_pool("revoked").await;
    let (id, token) = create_app_token(&pool, "user-1", "old phone")
        .await
        .expect("create");

    revoke_app_token(&pool, "user-1", &id)
        .await
        .expect("revoke");

    assert!(matches!(
        verify_app_token(&pool, &token).await,
        Err(TokenError::Revoked)
    ));
}

#[tokio::test]
async fn verify_app_token_rejects_unknown_id() {
    let pool = fresh_pool("unknown").await;
    let (_t, _id, secret) = generate_token();
    // Well-formed but never stored.
    let token = format!("oxidav_deadbeefdeadbeefdeadbeefdeadbeef_{secret}");

    assert!(matches!(
        verify_app_token(&pool, &token).await,
        Err(TokenError::NotFound)
    ));
}

#[tokio::test]
async fn verify_app_token_rejects_a_wrong_secret() {
    let pool = fresh_pool("mismatch").await;
    let (id, _token) = create_app_token(&pool, "user-1", "phone")
        .await
        .expect("create");
    let (_, _, other_secret) = generate_token();

    let forged = format!("oxidav_{id}_{other_secret}");
    assert!(matches!(
        verify_app_token(&pool, &forged).await,
        Err(TokenError::SecretMismatch)
    ));
}

#[tokio::test]
async fn verify_app_token_rejects_malformed_tokens() {
    let pool = fresh_pool("malformed").await;
    for bad in ["", "oxidav", "oxidav_only", "not_a_token", "bearer_abc_def"] {
        assert!(
            matches!(
                verify_app_token(&pool, bad).await,
                Err(TokenError::Malformed)
            ),
            "expected Malformed for {bad:?}"
        );
    }
}

#[tokio::test]
async fn revoke_is_scoped_to_the_owning_user() {
    let pool = fresh_pool("scoped").await;
    let (id, token) = create_app_token(&pool, "user-a", "phone")
        .await
        .expect("create");

    // Another user cannot revoke it...
    assert!(matches!(
        revoke_app_token(&pool, "user-b", &id).await,
        Err(TokenError::NotFound)
    ));
    // ...and the token is still valid.
    assert!(verify_app_token(&pool, &token).await.is_ok());

    // The owner can.
    revoke_app_token(&pool, "user-a", &id)
        .await
        .expect("owner revoke");
    assert!(matches!(
        verify_app_token(&pool, &token).await,
        Err(TokenError::Revoked)
    ));
}

#[tokio::test]
async fn revoke_is_idempotent() {
    let pool = fresh_pool("idempotent").await;
    let (id, _token) = create_app_token(&pool, "user-a", "phone")
        .await
        .expect("create");

    revoke_app_token(&pool, "user-a", &id).await.expect("first");
    revoke_app_token(&pool, "user-a", &id)
        .await
        .expect("second revoke is a no-op success");
}

// ---------------------------------------------------------------------------
// Task 6 — touch_last_used
// ---------------------------------------------------------------------------

#[tokio::test]
async fn touch_last_used_stamps_the_token() {
    let pool = fresh_pool("touch").await;
    let (id, _token) = create_app_token(&pool, "user-1", "phone")
        .await
        .expect("create");

    let before: Option<i64> =
        sqlx::query_scalar("SELECT last_used_at FROM app_tokens WHERE id = ?")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .expect("last_used_at");
    assert_eq!(before, None, "a fresh token has never been used");

    touch_last_used(&pool, &id).await.expect("touch");

    let created_at: i64 = sqlx::query_scalar("SELECT created_at FROM app_tokens WHERE id = ?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .expect("created_at");
    let after: Option<i64> = sqlx::query_scalar("SELECT last_used_at FROM app_tokens WHERE id = ?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .expect("last_used_at");
    let after = after.expect("last_used_at is set");
    assert!(after >= created_at, "{after} >= {created_at}");

    // Best-effort: an unknown id is not an error.
    touch_last_used(&pool, "no-such-id")
        .await
        .expect("unknown id must not fail");
}

// ---------------------------------------------------------------------------
// Task 8 — spawn_touch_last_used persists `last_used_at` via a background task
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_touch_last_used_persists_the_timestamp() {
    let pool = fresh_pool("spawn-touch").await;
    let (id, _token) = create_app_token(&pool, "user-1", "phone")
        .await
        .expect("create");

    let before: Option<i64> =
        sqlx::query_scalar("SELECT last_used_at FROM app_tokens WHERE id = ?")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .expect("last_used_at");
    assert_eq!(before, None, "a fresh token has never been used");

    spawn_touch_last_used(pool.clone(), id.clone());

    // The update runs on a background task; poll until it lands.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let after: Option<i64> =
            sqlx::query_scalar("SELECT last_used_at FROM app_tokens WHERE id = ?")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .expect("last_used_at");
        if let Some(after) = after {
            assert!(after > 0, "last_used_at must be a real timestamp: {after}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "background `last_used_at` update did not persist in time"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_app_tokens_returns_the_public_view_per_user() {
    let pool = fresh_pool("list").await;
    let (id_a, _) = create_app_token(&pool, "user-a", "iPhone")
        .await
        .expect("create a1");
    let (id_b, _) = create_app_token(&pool, "user-a", "Thunderbird")
        .await
        .expect("create a2");
    let (id_c, _) = create_app_token(&pool, "user-b", "Other")
        .await
        .expect("create b1");

    let mut ids: Vec<String> = list_app_tokens(&pool, "user-a")
        .await
        .expect("list")
        .into_iter()
        .map(|t| t.id)
        .collect();
    ids.sort();
    let mut expected = vec![id_a, id_b];
    expected.sort();
    assert_eq!(ids, expected, "only user-a's tokens are listed");

    assert_eq!(
        list_app_tokens(&pool, "user-b")
            .await
            .expect("list b")
            .len(),
        1
    );

    // The public view never carries the secret hash (the struct has no field for
    // it) and reports a consistent owner.
    let tokens = list_app_tokens(&pool, "user-a").await.expect("list a");
    assert!(tokens.iter().all(|t| t.user_id == "user-a"));
    assert!(tokens.iter().all(|t| t.revoked_at.is_none()));
    assert!(
        !tokens.iter().any(|t| t.id == id_c),
        "user-a must not see user-b's token"
    );
}
