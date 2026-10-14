//! F10 — search handlers (`/api/v1/search`).
//!
//! Read-only endpoints over the F03 FTS5 indexes (`trigram` tokenizer +
//! `bm25 ASC`). Every handler is scoped to the authenticated subject
//! ([`SessionUser::sub`]) through the `*_scoped` helpers in [`crate::db::search`],
//! so a search can never surface another user's objects (anti-IDOR).
//!
//! Behaviour carried over verbatim from F03:
//!
//! * A query whose trimmed length is below 3 characters (including empty) is
//!   **not** an error: it returns `200` with an empty list, because the
//!   `trigram` tokenizer cannot match shorter strings.
//! * `limit` is clamped by the data layer to `1..=MAX_SEARCH_LIMIT` (`200`); an
//!   absent `limit` uses [`DEFAULT_SEARCH_LIMIT`].
//!
//! Responses are plain JSON arrays of [`EventSearchResult`] /
//! [`ContactSearchResult`] (never `raw_payload`), and failures use the F06
//! [`ApiError`] envelope.

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;

use crate::auth::SessionUser;
use crate::db::search::{search_contacts_scoped, search_events_scoped};
use crate::router::AppState;

use super::dto::{ContactSearchResult, EventSearchResult, SearchResultsDto};
use super::error::ApiError;

/// Page size applied when the caller omits `limit`.
///
/// Mirrors the collection listing default (`DEFAULT_COLLECTION_LIMIT = 20`); the
/// data layer additionally clamps it into `1..=MAX_SEARCH_LIMIT` (`200`).
pub const DEFAULT_SEARCH_LIMIT: i64 = 20;

/// Query string of the search endpoints.
///
/// `q` is free text; `limit` is kept as a raw string and parsed manually so a
/// non-numeric value (e.g. `?limit=abc`) maps to the uniform `400`
/// [`ApiError::Validation`] envelope instead of axum's plain-text rejection.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SearchQuery {
    /// Free-text query (matched as a substring by the `trigram` tokenizer).
    pub q: Option<String>,
    /// Requested result cap, as sent; parsed and clamped before use.
    pub limit: Option<String>,
}

impl SearchQuery {
    /// The trimmed query text, or `""` when absent.
    fn query(&self) -> &str {
        self.q.as_deref().unwrap_or("")
    }

    /// Resolve the effective limit: the caller's value if parseable, else the
    /// documented default.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Validation`] when `limit` is present but not a valid
    /// integer.
    fn resolve_limit(&self) -> Result<i64, ApiError> {
        match self.limit.as_deref() {
            None => Ok(DEFAULT_SEARCH_LIMIT),
            Some(value) => value.trim().parse::<i64>().map_err(|_| {
                ApiError::Validation("El parámetro `limit` debe ser un entero".to_string())
            }),
        }
    }
}

/// `GET /api/v1/search/events?q=&limit=` — search the caller's calendar objects.
///
/// # Errors
///
/// Returns [`ApiError::Validation`] when `limit` is not an integer, and
/// [`ApiError::Internal`] if the database query fails.
pub async fn events(
    State(state): State<AppState>,
    user: SessionUser,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<EventSearchResult>>, ApiError> {
    let limit = query.resolve_limit()?;
    let rows = search_events_scoped(&state.db, &user.sub, query.query(), limit).await?;
    Ok(Json(rows.iter().map(EventSearchResult::from).collect()))
}

/// `GET /api/v1/search/contacts?q=&limit=` — search the caller's card objects.
///
/// # Errors
///
/// Returns [`ApiError::Validation`] when `limit` is not an integer, and
/// [`ApiError::Internal`] if the database query fails.
pub async fn contacts(
    State(state): State<AppState>,
    user: SessionUser,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<ContactSearchResult>>, ApiError> {
    let limit = query.resolve_limit()?;
    let rows = search_contacts_scoped(&state.db, &user.sub, query.query(), limit).await?;
    Ok(Json(rows.iter().map(ContactSearchResult::from).collect()))
}

/// `GET /api/v1/search?q=&limit=` — aggregate events + contacts in one response.
///
/// Shares the exact validation, scoping, ranking and clamping of the two
/// specific endpoints.
///
/// # Errors
///
/// Returns [`ApiError::Validation`] when `limit` is not an integer, and
/// [`ApiError::Internal`] if either database query fails.
pub async fn aggregate(
    State(state): State<AppState>,
    user: SessionUser,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResultsDto>, ApiError> {
    let limit = query.resolve_limit()?;
    let events = search_events_scoped(&state.db, &user.sub, query.query(), limit).await?;
    let contacts = search_contacts_scoped(&state.db, &user.sub, query.query(), limit).await?;
    Ok(Json(SearchResultsDto {
        events: events.iter().map(EventSearchResult::from).collect(),
        contacts: contacts.iter().map(ContactSearchResult::from).collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_limit_uses_default() {
        let query = SearchQuery::default();
        assert_eq!(query.resolve_limit().ok(), Some(DEFAULT_SEARCH_LIMIT));
    }

    #[test]
    fn numeric_limit_is_parsed() {
        let query = SearchQuery {
            q: Some("hola".to_string()),
            limit: Some(" 42 ".to_string()),
        };
        assert_eq!(query.resolve_limit().ok(), Some(42));
        assert_eq!(query.query(), "hola");
    }

    #[test]
    fn non_numeric_limit_is_a_validation_error() {
        let query = SearchQuery {
            q: None,
            limit: Some("abc".to_string()),
        };
        assert!(matches!(
            query.resolve_limit(),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn absent_query_is_empty_string() {
        assert_eq!(SearchQuery::default().query(), "");
    }
}
