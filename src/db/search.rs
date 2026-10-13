//! FTS5 search helpers with `bm25` ranking.
//!
//! Both indexes use the `trigram` tokenizer, so queries match internal
//! substrings of **at least 3 characters**. Queries shorter than that cannot
//! match by construction and return an empty result without touching the
//! database (a documented, known limitation of the search UX).

use sqlx::SqlitePool;

use crate::db::DbError;
use crate::db::models::{CalendarObject, CardObject};

/// Minimum query length (after trimming) enforced by the `trigram` tokenizer.
const MIN_TRIGRAM_LEN: usize = 3;

/// Escape caller-supplied text so FTS5 treats it as a literal string.
///
/// The right-hand side of `MATCH` is **not** plain text: FTS5 parses it as a
/// query *expression*, where `"`, `(`, `)`, `*`, `:`, `^`, `-` and the bare
/// keywords `AND` / `OR` / `NOT` / `NEAR` are operators. Feeding user input
/// straight through therefore turns a search box into a syntax-error oracle:
/// `q=a"b`, `q=reunion"` (unterminated quote) or `q=foo(` (unbalanced paren)
/// makes SQLite raise an error that surfaces as an HTTP `500`.
///
/// Wrapping the text in a single FTS5 phrase and doubling every embedded double
/// quote (`"` -> `""`, the FTS5 escape for a literal quote inside a phrase)
/// yields an **always syntactically valid** query that matches the literal
/// substring, so no user input can ever make `MATCH` fail.
fn escape_fts5_literal(query: &str) -> String {
    let mut escaped = String::with_capacity(query.len() + 2);
    escaped.push('"');
    for ch in query.chars() {
        if ch == '"' {
            escaped.push('"');
        }
        escaped.push(ch);
    }
    escaped.push('"');
    escaped
}

/// Normalise caller-supplied text into a safe FTS5 `MATCH` operand.
///
/// Returns `None` when the trimmed query is shorter than [`MIN_TRIGRAM_LEN`]
/// (the `trigram` tokenizer cannot match it, so callers answer an empty list
/// without touching the database), otherwise the [escaped literal
/// phrase][escape_fts5_literal]. Every search helper routes its input through
/// this function, so no raw user text ever reaches `MATCH`.
fn match_operand(query: &str) -> Option<String> {
    let trimmed = query.trim();
    if trimmed.chars().count() < MIN_TRIGRAM_LEN {
        return None;
    }
    Some(escape_fts5_literal(trimmed))
}

/// Upper bound on returned rows, to prevent a caller (e.g. F10) from requesting
/// an unbounded result set.
pub const MAX_SEARCH_LIMIT: i64 = 200;

/// Normalise a caller-supplied `limit`.
///
/// In SQLite `LIMIT -1` means *no limit*, so a raw negative value would be a DoS
/// foot-gun. Values `<= 0` are clamped to `1` and values above
/// [`MAX_SEARCH_LIMIT`] are clamped down to it; the result is always in
/// `1..=MAX_SEARCH_LIMIT`.
fn clamp_limit(limit: i64) -> i64 {
    limit.clamp(1, MAX_SEARCH_LIMIT)
}

/// Search calendar objects by substring over `summary` and `description`.
///
/// Results are ordered by `bm25(events_fts) ASC` (in FTS5 the most negative rank
/// is the most relevant, so ascending puts the best match first). A query whose
/// trimmed length is below 3 characters returns an empty vector. The remaining
/// text is escaped into a literal FTS5 phrase (see [`escape_fts5_literal`]), so
/// arbitrary user input — quotes, parentheses, `*`, `:`, `-` or the bare
/// `AND`/`OR`/`NOT`/`NEAR` keywords — can never trigger a syntax error.
///
/// # Warning
///
/// This variant searches **every** user's objects: it applies no ownership
/// predicate. It must **not** be wired to an API handler, which would be an
/// IDOR. HTTP handlers use the user-scoped [`search_events_scoped`].
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn search_events(
    pool: &SqlitePool,
    query: &str,
    limit: i64,
) -> Result<Vec<CalendarObject>, DbError> {
    let Some(match_query) = match_operand(query) else {
        return Ok(Vec::new());
    };

    sqlx::query_as::<_, CalendarObject>(
        "SELECT calendar_objects.*
           FROM calendar_objects
           JOIN events_fts ON events_fts.rowid = calendar_objects.rowid
          WHERE events_fts MATCH ?
          ORDER BY bm25(events_fts) ASC
          LIMIT ?",
    )
    .bind(match_query)
    .bind(clamp_limit(limit))
    .fetch_all(pool)
    .await
    .map_err(DbError::Query)
}

/// Search card objects by substring over `fn`, `email`, `phone`, `organization`
/// and `notes`.
///
/// Results are ordered by `bm25(contacts_fts) ASC`. A query whose trimmed length
/// is below 3 characters returns an empty vector. `limit` is clamped (see
/// [`clamp_limit`]): values `<= 0` become `1`, values above [`MAX_SEARCH_LIMIT`]
/// are capped. The query text is escaped into a literal FTS5 phrase (see
/// [`escape_fts5_literal`]) so no user input can produce a syntax error.
///
/// # Warning
///
/// This variant searches **every** user's objects: it applies no ownership
/// predicate. It must **not** be wired to an API handler, which would be an
/// IDOR. HTTP handlers use the user-scoped [`search_contacts_scoped`].
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn search_contacts(
    pool: &SqlitePool,
    query: &str,
    limit: i64,
) -> Result<Vec<CardObject>, DbError> {
    let Some(match_query) = match_operand(query) else {
        return Ok(Vec::new());
    };

    sqlx::query_as::<_, CardObject>(
        "SELECT card_objects.*
           FROM card_objects
           JOIN contacts_fts ON contacts_fts.rowid = card_objects.rowid
          WHERE contacts_fts MATCH ?
          ORDER BY bm25(contacts_fts) ASC
          LIMIT ?",
    )
    .bind(match_query)
    .bind(clamp_limit(limit))
    .fetch_all(pool)
    .await
    .map_err(DbError::Query)
}

/// User-scoped variant of [`search_events`] (F10 anti-IDOR).
///
/// Identical ranking, query-length rule and `limit` clamping, but the result set
/// is restricted to calendar objects whose owning collection belongs to
/// `user_id`. The ownership predicate is part of the SQL statement
/// (`JOIN collections ... WHERE collections.user_id = ?`), so no object from
/// another user's calendar can ever be returned — the unscoped [`search_events`]
/// would otherwise leak across users.
///
/// A query whose trimmed length is below 3 characters returns an empty vector;
/// otherwise it is escaped into a literal FTS5 phrase (see
/// [`escape_fts5_literal`]) so no user input can produce a syntax error.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn search_events_scoped(
    pool: &SqlitePool,
    user_id: &str,
    query: &str,
    limit: i64,
) -> Result<Vec<CalendarObject>, DbError> {
    let Some(match_query) = match_operand(query) else {
        return Ok(Vec::new());
    };

    sqlx::query_as::<_, CalendarObject>(
        "SELECT calendar_objects.*
           FROM calendar_objects
           JOIN events_fts ON events_fts.rowid = calendar_objects.rowid
           JOIN collections ON collections.id = calendar_objects.collection_id
          WHERE events_fts MATCH ?
            AND collections.user_id = ?
          ORDER BY bm25(events_fts) ASC
          LIMIT ?",
    )
    .bind(match_query)
    .bind(user_id)
    .bind(clamp_limit(limit))
    .fetch_all(pool)
    .await
    .map_err(DbError::Query)
}

/// User-scoped variant of [`search_contacts`] (F10 anti-IDOR).
///
/// Identical ranking, query-length rule and `limit` clamping, but the result set
/// is restricted to card objects whose owning address book belongs to `user_id`
/// via `JOIN collections ... WHERE collections.user_id = ?`.
///
/// A query whose trimmed length is below 3 characters returns an empty vector;
/// otherwise it is escaped into a literal FTS5 phrase (see
/// [`escape_fts5_literal`]) so no user input can produce a syntax error.
///
/// # Errors
///
/// Returns [`DbError::Query`] on any database failure.
pub async fn search_contacts_scoped(
    pool: &SqlitePool,
    user_id: &str,
    query: &str,
    limit: i64,
) -> Result<Vec<CardObject>, DbError> {
    let Some(match_query) = match_operand(query) else {
        return Ok(Vec::new());
    };

    sqlx::query_as::<_, CardObject>(
        "SELECT card_objects.*
           FROM card_objects
           JOIN contacts_fts ON contacts_fts.rowid = card_objects.rowid
           JOIN collections ON collections.id = card_objects.collection_id
          WHERE contacts_fts MATCH ?
            AND collections.user_id = ?
          ORDER BY bm25(contacts_fts) ASC
          LIMIT ?",
    )
    .bind(match_query)
    .bind(user_id)
    .bind(clamp_limit(limit))
    .fetch_all(pool)
    .await
    .map_err(DbError::Query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_fts5_literal_wraps_in_a_phrase() {
        assert_eq!(escape_fts5_literal("reunion"), "\"reunion\"");
    }

    #[test]
    fn escape_fts5_literal_doubles_embedded_quotes() {
        // `"` -> `""`, the FTS5 escape for a literal quote inside a phrase.
        assert_eq!(escape_fts5_literal("a\"b"), "\"a\"\"b\"");
        assert_eq!(escape_fts5_literal("\""), "\"\"\"\"");
    }

    #[test]
    fn escape_fts5_literal_neutralises_operators() {
        // FTS5 operators become inert literal text inside the phrase.
        assert_eq!(escape_fts5_literal("reunion AND"), "\"reunion AND\"");
        assert_eq!(escape_fts5_literal("foo("), "\"foo(\"");
        assert_eq!(escape_fts5_literal("a*b:c^-d"), "\"a*b:c^-d\"");
    }

    #[test]
    fn match_operand_rejects_short_queries() {
        assert_eq!(match_operand(""), None);
        assert_eq!(match_operand("  a  "), None);
        assert_eq!(match_operand("ab"), None);
        assert_eq!(match_operand("\"\""), None);
    }

    #[test]
    fn match_operand_escapes_valid_queries() {
        assert_eq!(
            match_operand("  reunion  "),
            Some("\"reunion\"".to_string())
        );
        // A hostile / malformed query is still a valid literal phrase.
        assert_eq!(match_operand("a\"b"), Some("\"a\"\"b\"".to_string()));
        assert_eq!(
            match_operand("reunion AND"),
            Some("\"reunion AND\"".to_string())
        );
        assert_eq!(match_operand("foo("), Some("\"foo(\"".to_string()));
    }

    #[test]
    fn clamp_limit_never_allows_unbounded() {
        // `limit <= 0` is clamped to 1: SQLite's `LIMIT -1` means "no limit", so
        // a raw value must never reach the query.
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(-1), 1);
        assert_eq!(clamp_limit(i64::MIN), 1);

        // A normal in-range limit is preserved.
        assert_eq!(clamp_limit(1), 1);
        assert_eq!(clamp_limit(42), 42);
        assert_eq!(clamp_limit(MAX_SEARCH_LIMIT), MAX_SEARCH_LIMIT);

        // Anything above the ceiling is capped.
        assert_eq!(clamp_limit(MAX_SEARCH_LIMIT + 1), MAX_SEARCH_LIMIT);
        assert_eq!(clamp_limit(i64::MAX), MAX_SEARCH_LIMIT);
    }
}
