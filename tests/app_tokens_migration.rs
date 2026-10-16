//! F05 — Task 1: the `app_tokens` migration creates the table, its indexes and
//! is idempotent.
//!
//! Each test uses a throwaway file-backed database under `/tmp/opencode` (never
//! `data/oxidav.db`), mirroring the F03 test conventions.

mod common;

use std::str::FromStr;

use oxidav::db::init_pool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Executor, SqlitePool};

/// The migration under test, embedded so the test does not depend on the
/// migrator's version-tracking table.
const MIGRATION: &str = include_str!("../migrations/0002_app_tokens.sql");

/// A raw pool (no migrator) so we can execute the migration file directly.
async fn raw_pool(tag: &str) -> SqlitePool {
    let url = common::temp_db_url("app-tokens", tag);
    let options = SqliteConnectOptions::from_str(&url)
        .expect("valid dsn")
        .create_if_missing(true);
    SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .expect("connect raw pool")
}

async fn column_names(pool: &SqlitePool) -> Vec<String> {
    sqlx::query_scalar("SELECT name FROM pragma_table_info('app_tokens') ORDER BY cid")
        .fetch_all(pool)
        .await
        .expect("pragma_table_info")
}

async fn index_names(pool: &SqlitePool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'app_tokens'",
    )
    .fetch_all(pool)
    .await
    .expect("sqlite_master")
}

#[tokio::test]
async fn migration_creates_table_with_all_columns() {
    let pool = raw_pool("columns").await;
    pool.execute(MIGRATION).await.expect("apply 0002 migration");

    assert_eq!(
        column_names(&pool).await,
        vec![
            "id",
            "user_id",
            "name",
            "secret_hash",
            "created_at",
            "last_used_at",
            "revoked_at",
        ]
    );
}

#[tokio::test]
async fn migration_creates_expected_indexes() {
    let pool = raw_pool("indexes").await;
    pool.execute(MIGRATION).await.expect("apply 0002 migration");

    let indexes = index_names(&pool).await;
    assert!(
        indexes.iter().any(|i| i == "idx_app_tokens_user_id"),
        "missing idx_app_tokens_user_id: {indexes:?}"
    );
    assert!(
        indexes.iter().any(|i| i == "idx_app_tokens_user_active"),
        "missing idx_app_tokens_user_active: {indexes:?}"
    );
}

#[tokio::test]
async fn migration_is_idempotent() {
    let pool = raw_pool("idempotent").await;
    // Applying the same file twice must not fail (`IF NOT EXISTS`).
    pool.execute(MIGRATION).await.expect("first application");
    pool.execute(MIGRATION)
        .await
        .expect("second application must be a no-op");

    let tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'app_tokens'",
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(tables, 1);
}

#[tokio::test]
async fn init_pool_applies_app_tokens_migration() {
    // Through the real migrator (which applies 0001 then 0002), the table exists.
    let pool = init_pool(&common::temp_db_url("app-tokens", "via-migrator"))
        .await
        .expect("init_pool");

    let tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'app_tokens'",
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(tables, 1);
}
