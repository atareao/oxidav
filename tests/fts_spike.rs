//! Paso 1 — Spike FTS5/trigram.
//!
//! Bloqueante: verifica que el SQLite empaquetado trae FTS5 y el tokenizer
//! `trigram`, antes de construir la capa de datos sobre esa base. Se ejecuta en
//! memoria, sin depender de `data/oxidav.db`.

use sqlx::Row;
use sqlx::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;

/// Abre un pool en memoria con una única conexión (una BD `:memory:` por conexión).
async fn memory_pool() -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite should open")
}

#[tokio::test]
async fn fts5_trigram_is_available() {
    let pool = memory_pool().await;

    sqlx::query("CREATE VIRTUAL TABLE fts_probe USING fts5(body, tokenize='trigram')")
        .execute(&pool)
        .await
        .expect("FTS5 with trigram tokenizer must be available in the bundled SQLite");

    for body in ["un ejemplo", "otra cadena", "ejemplos varios"] {
        sqlx::query("INSERT INTO fts_probe(body) VALUES (?)")
            .bind(body)
            .execute(&pool)
            .await
            .expect("insert into fts_probe");
    }

    // `trigram` matches internal substrings of >= 3 characters: "ejem" is an
    // internal substring of both "un ejemplo" and "ejemplos varios".
    let rows =
        sqlx::query("SELECT body, bm25(fts_probe) AS rank FROM fts_probe WHERE fts_probe MATCH ?")
            .bind("ejem")
            .fetch_all(&pool)
            .await
            .expect("MATCH query with bm25 must work");

    assert_eq!(rows.len(), 2, "both 'ejemplo' rows should match 'ejem'");
    for row in &rows {
        let rank: f64 = row.get("rank");
        assert!(rank.is_finite(), "bm25 must return a finite rank");
    }
}
