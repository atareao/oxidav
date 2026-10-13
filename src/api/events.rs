//! F11 — REST handlers for calendar events (`/api/v1/collections/{id}/events`).
//!
//! The SPA cannot speak CalDAV, so these endpoints expose the `calendar_objects`
//! of a `calendar` collection as clean JSON DTOs. Every handler is scoped to the
//! authenticated subject ([`AuthUser::sub`]): a collection owned by another user
//! — or a collection of the wrong kind (an address book) — is treated as
//! non-existent (`404`), so no endpoint can leak another user's data (IDOR).
//!
//! # `.ics` composition (single home)
//!
//! F07 CalDAV *parses* `.ics` on `PUT` ([`crate::dav::ics::parse_ics`]) and
//! stores the client-provided payload verbatim. F11 instead **composes** the
//! `.ics` from JSON fields. To avoid a second, drifting iCalendar implementation
//! in the HTTP layer, composition was added next to the parser in the shared
//! CalDAV module ([`crate::dav::ics::build_ics`]): the DAV tree and the REST API
//! now share one home for iCalendar (de)serialisation, and the composed payload
//! is guaranteed to round-trip through [`parse_ics`](crate::dav::ics::parse_ics).
//! UUID minting likewise reuses [`crate::dav::common::new_id`] rather than adding
//! another copy.
//!
//! # Persistence
//!
//! Writes go through the F03 repositories
//! ([`insert_calendar_object`](crate::db::insert_calendar_object) /
//! [`update_calendar_object`](crate::db::update_calendar_object) /
//! [`delete_calendar_object`](crate::db::delete_calendar_object)) and reads
//! through [`list_calendar_objects`](crate::db::list_calendar_objects) /
//! [`get_calendar_object`](crate::db::get_calendar_object); they compute the
//! ETag and bump the collection `sync_token`/`sync_log` exactly like CalDAV, so
//! REST-created events sync to native clients too. No `raw_payload` is ever
//! exposed.
//!
//! # Validation
//!
//! `summary` must be present and non-blank; `dtstart` is required; a `dtend`
//! earlier than `dtstart` is rejected. Blank `description`/`rrule` values are
//! normalised away (so `""` clears an optional field on `PATCH`). Because
//! `RRULE` is emitted verbatim into the stored `.ics`, it is additionally
//! restricted to the RFC 5545 RECUR alphabet (no control characters), which
//! prevents iCalendar content-line injection. Failures use the F06 [`ApiError`]
//! envelope.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;

use crate::auth::oidc::AuthUser;
use crate::dav::common::new_id;
use crate::dav::ics::{EventFields, build_ics};
use crate::db::models::{CalendarObjectUpdate, CollectionKind, NewCalendarObject};
use crate::db::{
    Collection, DbError, clamp_collection_limit, clamp_offset, delete_calendar_object,
    get_calendar_object, get_collection, insert_calendar_object, list_calendar_objects_window,
    update_calendar_object,
};
use crate::router::AppState;

use super::dto::{CreateEventRequest, EventDto, UpdateEventRequest};
use super::error::ApiError;
use super::{parse_json_body, parse_optional_i64};

/// Maximum accepted length of an event summary, in characters.
const MAX_SUMMARY_LEN: usize = 512;

/// Maximum accepted length of an event description, in characters.
const MAX_DESCRIPTION_LEN: usize = 8_192;

/// Maximum accepted length of an `RRULE` value, in characters.
const MAX_RRULE_LEN: usize = 1_024;

/// Maximum accepted length of a client-supplied `uid`, in characters.
const MAX_UID_LEN: usize = 255;

/// Query parameters for `GET /api/v1/collections/{id}/events`.
///
/// `start`/`end` are the optional `[start, end)` window (Unix milliseconds) that
/// filters by **temporal overlap**; `limit`/`offset` paginate. Fields are kept
/// as raw strings so a non-numeric value maps to the uniform `400`
/// [`ApiError::Validation`] envelope instead of axum's plain-text `Query`
/// rejection.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EventListQuery {
    /// Inclusive lower bound of the window, as sent; parsed before use.
    pub start: Option<String>,
    /// Exclusive upper bound of the window, as sent; parsed before use.
    pub end: Option<String>,
    /// Requested page size, as sent; parsed and clamped before use.
    pub limit: Option<String>,
    /// Requested page offset, as sent; parsed and clamped before use.
    pub offset: Option<String>,
}

impl EventListQuery {
    /// Parse and clamp the window and page parameters.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Validation`] when any of `start`/`end`/`limit`/
    /// `offset` is present but is not a valid integer, or when the window is
    /// inverted (`end < start`).
    fn resolve(&self) -> Result<(Option<i64>, Option<i64>, i64, i64), ApiError> {
        let start = parse_optional_i64(self.start.as_deref(), "start")?;
        let end = parse_optional_i64(self.end.as_deref(), "end")?;
        if let (Some(start), Some(end)) = (start, end)
            && end < start
        {
            return Err(ApiError::Validation(
                "El parámetro `end` no puede ser anterior a `start`".to_string(),
            ));
        }
        let limit = parse_optional_i64(self.limit.as_deref(), "limit")?.unwrap_or(0);
        let offset = parse_optional_i64(self.offset.as_deref(), "offset")?.unwrap_or(0);
        Ok((
            start,
            end,
            clamp_collection_limit(limit),
            clamp_offset(offset),
        ))
    }
}

/// `GET /api/v1/collections/{id}/events` — list a calendar collection's events.
///
/// Results are ordered by `uid` and may be narrowed to the `[start, end)`
/// window (by overlap) and paginated via `limit`/`offset`; see
/// [`list_calendar_objects_window`] for the overlap semantics.
///
/// # Errors
///
/// [`ApiError::Validation`] for a non-numeric window/page parameter or an
/// inverted window; [`ApiError::NotFound`] when the collection does not exist,
/// belongs to another user, or is not a `calendar`; [`ApiError::Internal`] if a
/// query fails.
pub async fn list(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(query): Query<EventListQuery>,
) -> Result<Json<Vec<EventDto>>, ApiError> {
    let collection = calendar_collection(&state, &id, &user.sub).await?;
    let (start, end, limit, offset) = query.resolve()?;
    let rows =
        list_calendar_objects_window(&state.db, &collection.id, start, end, limit, offset).await?;
    Ok(Json(rows.iter().map(EventDto::from).collect()))
}

/// `POST /api/v1/collections/{id}/events` — create an event.
///
/// The `.ics` is composed from the JSON fields and stored in `raw_payload`; a
/// fresh `id` and `uid` are minted unless the client pins a `uid`.
///
/// # Errors
///
/// [`ApiError::Validation`] for a missing/blank summary, an invalid range, or a
/// malformed payload; [`ApiError::NotFound`] for a missing/foreign/non-calendar
/// collection; [`ApiError::Conflict`] when the (client-supplied) `uid` already
/// exists in the collection; [`ApiError::Internal`] if the insert fails.
pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let request: CreateEventRequest = parse_json_body(&body)?;

    let summary = validate_summary(&request.summary)?;
    let description = normalize_optional(
        request.description.as_deref(),
        "description",
        MAX_DESCRIPTION_LEN,
    )?;
    let rrule = validate_rrule(request.rrule.as_deref())?;
    validate_range(request.dtstart, request.dtend)?;

    let collection = calendar_collection(&state, &id, &user.sub).await?;

    let uid = match request.uid.as_deref() {
        Some(raw) => validate_uid(raw)?,
        None => new_id(),
    };

    let fields = EventFields {
        uid: uid.clone(),
        summary: Some(summary.clone()),
        description: description.clone(),
        dtstart_ms: request.dtstart,
        dtend_ms: request.dtend,
        rrule: rrule.clone(),
        dtstamp_ms: now_millis(),
    };

    let new = NewCalendarObject {
        id: new_id(),
        collection_id: collection.id.clone(),
        uid,
        summary: Some(summary),
        description,
        dtstart: Some(request.dtstart),
        dtend: request.dtend,
        rrule,
        raw_payload: build_ics(&fields),
    };

    let stored = insert_calendar_object(&state.db, &new)
        .await
        .map_err(map_insert_error)?;

    Ok((StatusCode::CREATED, Json(EventDto::from(&stored))))
}

/// `GET /api/v1/collections/{id}/events/{uid}` — fetch one event.
///
/// # Errors
///
/// [`ApiError::NotFound`] when the collection or the event does not exist (or
/// belongs to another user / is not a calendar); [`ApiError::Internal`] if a
/// query fails.
pub async fn get_one(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path((id, uid)): Path<(String, String)>,
) -> Result<Json<EventDto>, ApiError> {
    let collection = calendar_collection(&state, &id, &user.sub).await?;
    let object = get_calendar_object(&state.db, &collection.id, &uid)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(EventDto::from(&object)))
}

/// `PATCH /api/v1/collections/{id}/events/{uid}` — partially update an event.
///
/// Only the supplied fields are applied; the `.ics` is recomposed from the
/// merged values and a fresh ETag is written. A patch with no updatable field is
/// a `400` (it would otherwise silently refresh `updated_at`).
///
/// # Errors
///
/// [`ApiError::Validation`] for invalid input or an empty patch;
/// [`ApiError::NotFound`] when the collection or event does not exist (or
/// belongs to another user / is not a calendar); [`ApiError::Internal`] if the
/// update fails.
pub async fn update(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path((id, uid)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<EventDto>, ApiError> {
    let request: UpdateEventRequest = parse_json_body(&body)?;
    if request.is_empty() {
        return Err(ApiError::Validation(
            "Debe indicar al menos un campo a actualizar".to_string(),
        ));
    }

    let collection = calendar_collection(&state, &id, &user.sub).await?;
    let existing = get_calendar_object(&state.db, &collection.id, &uid)
        .await?
        .ok_or(ApiError::NotFound)?;

    let summary = match &request.summary {
        Some(value) => Some(validate_summary(value)?),
        None => existing.summary.clone(),
    };
    let description = match &request.description {
        Some(value) => {
            normalize_optional(Some(value.as_str()), "description", MAX_DESCRIPTION_LEN)?
        }
        None => existing.description.clone(),
    };
    let rrule = match &request.rrule {
        Some(value) => validate_rrule(Some(value.as_str()))?,
        None => existing.rrule.clone(),
    };
    // An event always carries a DTSTART; fall back to 0 only for a legacy row
    // that lacks the denormalised column.
    let dtstart = request.dtstart.or(existing.dtstart).unwrap_or(0);
    let dtend = request.dtend.or(existing.dtend);
    validate_range(dtstart, dtend)?;

    let fields = EventFields {
        uid: existing.uid.clone(),
        summary: summary.clone(),
        description: description.clone(),
        dtstart_ms: dtstart,
        dtend_ms: dtend,
        rrule: rrule.clone(),
        dtstamp_ms: now_millis(),
    };

    let update = CalendarObjectUpdate {
        summary,
        description,
        dtstart: Some(dtstart),
        dtend,
        rrule,
        raw_payload: build_ics(&fields),
    };

    let stored = update_calendar_object(&state.db, &existing.id, &update).await?;
    Ok(Json(EventDto::from(&stored)))
}

/// `DELETE /api/v1/collections/{id}/events/{uid}` — delete an event.
///
/// Deletion is explicit: an unknown/foreign `uid` yields `404` rather than a
/// silent `204`, so the SPA can distinguish "deleted" from "does not exist".
///
/// # Errors
///
/// [`ApiError::NotFound`] when the collection or event does not exist (or
/// belongs to another user / is not a calendar); [`ApiError::Internal`] if the
/// delete fails.
pub async fn delete_one(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path((id, uid)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let collection = calendar_collection(&state, &id, &user.sub).await?;
    let object = get_calendar_object(&state.db, &collection.id, &uid)
        .await?
        .ok_or(ApiError::NotFound)?;
    delete_calendar_object(&state.db, &object.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Fetch `id` scoped to `user_sub` and prove it is a `calendar`.
///
/// Both failure modes collapse to [`ApiError::NotFound`]: the collection does
/// not exist or belongs to another subject ([`get_collection`] is scoped by
/// `user_id`), or it exists but its kind is not `calendar`. Returning `404` for
/// the wrong kind keeps the answer identical to the missing case, so an address
/// book's existence is never leaked.
///
/// # Errors
///
/// [`ApiError::NotFound`] for a missing/foreign/non-calendar collection, and
/// [`ApiError::Internal`] if the database read fails.
async fn calendar_collection(
    state: &AppState,
    id: &str,
    user_sub: &str,
) -> Result<Collection, ApiError> {
    let collection = get_collection(&state.db, id, user_sub)
        .await?
        .ok_or(ApiError::NotFound)?;
    if collection.kind == CollectionKind::Calendar.as_str() {
        Ok(collection)
    } else {
        Err(ApiError::NotFound)
    }
}

/// Validate and normalise a required event summary.
fn validate_summary(raw: &str) -> Result<String, ApiError> {
    let summary = raw.trim();
    if summary.is_empty() {
        return Err(ApiError::Validation(
            "El título no puede estar vacío".to_string(),
        ));
    }
    if summary.chars().count() > MAX_SUMMARY_LEN {
        return Err(ApiError::Validation(format!(
            "El título no puede superar {MAX_SUMMARY_LEN} caracteres"
        )));
    }
    Ok(summary.to_string())
}

/// Normalise an optional text field: blank values become `None` (so `""` clears
/// the field), and over-long values are rejected.
fn normalize_optional(
    raw: Option<&str>,
    field: &str,
    max: usize,
) -> Result<Option<String>, ApiError> {
    match raw {
        None => Ok(None),
        Some(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            if value.chars().count() > max {
                return Err(ApiError::Validation(format!(
                    "El campo `{field}` no puede superar {max} caracteres"
                )));
            }
            Ok(Some(value.to_string()))
        }
    }
}

/// Validate and normalise an optional `RRULE` value.
///
/// A blank value is normalised away (`None`, so `""` clears the field on
/// `PATCH`). Otherwise the value must be bounded, free of control characters,
/// and restricted to the RFC 5545 RECUR token alphabet `A-Z`, `0-9`, `=`, `;`,
/// `,`, `-`.
///
/// This is a security boundary, not cosmetics: [`build_ics`] emits `RRULE`
/// **verbatim** (its `;`/`=`/`,` are rule separators, so escaping would corrupt
/// the rule), which means an unvalidated value like
/// `FREQ=DAILY\r\nEND:VEVENT\r\nBEGIN:VEVENT…` would inject arbitrary iCalendar
/// content lines into the stored `raw_payload`. Rejecting control characters
/// (including `\r`/`\n`/`\t`) and out-of-alphabet characters closes that
/// injection while keeping genuine rules like `FREQ=WEEKLY;COUNT=3` intact.
fn validate_rrule(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(value) = raw else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Ok(None);
    }
    if value.chars().count() > MAX_RRULE_LEN {
        return Err(ApiError::Validation(format!(
            "El campo `rrule` no puede superar {MAX_RRULE_LEN} caracteres"
        )));
    }
    let allowed = value.chars().all(|ch| {
        ch.is_ascii_uppercase() || ch.is_ascii_digit() || matches!(ch, '=' | ';' | ',' | '-')
    });
    if !allowed {
        return Err(ApiError::Validation(
            "El campo `rrule` contiene caracteres no permitidos".to_string(),
        ));
    }
    Ok(Some(value.to_string()))
}

/// Validate a client-supplied `uid`: non-blank, bounded, and free of control
/// characters or `/` (which would break the resource URL).
fn validate_uid(raw: &str) -> Result<String, ApiError> {
    let uid = raw.trim();
    if uid.is_empty() {
        return Err(ApiError::Validation(
            "El `uid` no puede estar vacío".to_string(),
        ));
    }
    if uid.chars().count() > MAX_UID_LEN {
        return Err(ApiError::Validation(format!(
            "El `uid` no puede superar {MAX_UID_LEN} caracteres"
        )));
    }
    if uid.chars().any(|c| c.is_control() || c == '/') {
        return Err(ApiError::Validation(
            "El `uid` contiene caracteres no permitidos".to_string(),
        ));
    }
    Ok(uid.to_string())
}

/// Reject an end instant earlier than its start instant.
fn validate_range(dtstart: i64, dtend: Option<i64>) -> Result<(), ApiError> {
    if let Some(end) = dtend
        && end < dtstart
    {
        return Err(ApiError::Validation(
            "El fin no puede ser anterior al inicio".to_string(),
        ));
    }
    Ok(())
}

/// Map a failed calendar-object insert to an [`ApiError`].
///
/// A `UNIQUE(collection_id, uid)` clash means the requested `uid` already exists
/// (or a concurrent `POST` won the race): a `409 Conflict`. Any other failure is
/// logged and surfaced as a generic [`ApiError::Internal`].
fn map_insert_error(err: DbError) -> ApiError {
    if err.is_unique_violation() {
        ApiError::Conflict("Ya existe un evento con ese uid en la colección".to_string())
    } else {
        ApiError::from(err)
    }
}

/// Current wall-clock time as Unix milliseconds.
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_summary_rejects_blank_and_trims() {
        assert!(matches!(
            validate_summary("   "),
            Err(ApiError::Validation(_))
        ));
        assert_eq!(
            validate_summary("  Meeting  ").ok(),
            Some("Meeting".to_string())
        );
    }

    #[test]
    fn normalize_optional_drops_blanks() {
        assert_eq!(normalize_optional(None, "description", 10).ok(), Some(None));
        assert_eq!(
            normalize_optional(Some("  "), "description", 10).ok(),
            Some(None)
        );
        assert_eq!(
            normalize_optional(Some("hi"), "description", 10).ok(),
            Some(Some("hi".to_string()))
        );
    }

    #[test]
    fn normalize_optional_rejects_overlong_values() {
        let long = "x".repeat(11);
        assert!(matches!(
            normalize_optional(Some(&long), "description", 10),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn validate_rrule_accepts_grammar_and_normalises_blank() {
        assert_eq!(validate_rrule(None).ok(), Some(None));
        assert_eq!(validate_rrule(Some("  ")).ok(), Some(None));
        assert_eq!(
            validate_rrule(Some("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=3")).ok(),
            Some(Some(
                "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=3".to_string()
            ))
        );
        assert_eq!(
            validate_rrule(Some("FREQ=DAILY;UNTIL=20240101T000000Z")).ok(),
            Some(Some("FREQ=DAILY;UNTIL=20240101T000000Z".to_string()))
        );
    }

    #[test]
    fn validate_rrule_rejects_injection_and_bad_charset() {
        // CRLF injection would smuggle extra lines into the verbatim RRULE.
        assert!(matches!(
            validate_rrule(Some("FREQ=DAILY\r\nEND:VEVENT\r\nBEGIN:VEVENT")),
            Err(ApiError::Validation(_))
        ));
        assert!(matches!(
            validate_rrule(Some("FREQ=DAILY\nX")),
            Err(ApiError::Validation(_))
        ));
        assert!(matches!(
            validate_rrule(Some("FREQ=DAILY\tX")),
            Err(ApiError::Validation(_))
        ));
        assert!(matches!(
            validate_rrule(Some("FREQ=DAILY\u{0007}")),
            Err(ApiError::Validation(_))
        ));
        // Out-of-alphabet punctuation (`:`, `{`, `}`) is rejected.
        assert!(matches!(
            validate_rrule(Some("FREQ=DAILY:{oops}")),
            Err(ApiError::Validation(_))
        ));
        // Lowercase is not part of the RECUR grammar.
        assert!(matches!(
            validate_rrule(Some("freq=daily")),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn validate_rrule_rejects_overlong_values() {
        let long = "A".repeat(MAX_RRULE_LEN + 1);
        assert!(matches!(
            validate_rrule(Some(&long)),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn event_list_query_rejects_bad_or_inverted_windows() {
        let bad = EventListQuery {
            start: Some("nope".to_string()),
            limit: None,
            ..EventListQuery::default()
        };
        assert!(matches!(bad.resolve(), Err(ApiError::Validation(_))));

        let inverted = EventListQuery {
            start: Some("5000".to_string()),
            end: Some("1000".to_string()),
            ..EventListQuery::default()
        };
        assert!(matches!(inverted.resolve(), Err(ApiError::Validation(_))));

        let ok = EventListQuery {
            start: Some("1000".to_string()),
            end: Some("2000".to_string()),
            limit: Some("100000".to_string()),
            offset: Some("-3".to_string()),
        };
        assert_eq!(ok.resolve().ok(), Some((Some(1000), Some(2000), 200, 0)));
    }

    #[test]
    fn validate_range_rejects_inverted_times() {
        assert!(matches!(
            validate_range(100, Some(99)),
            Err(ApiError::Validation(_))
        ));
        assert!(validate_range(100, Some(100)).is_ok());
        assert!(validate_range(100, None).is_ok());
    }

    #[test]
    fn validate_uid_rejects_blank_and_slashes() {
        assert!(matches!(validate_uid("  "), Err(ApiError::Validation(_))));
        assert!(matches!(validate_uid("a/b"), Err(ApiError::Validation(_))));
        assert_eq!(validate_uid(" evt-1 ").ok(), Some("evt-1".to_string()));
    }

    #[test]
    fn empty_patch_is_detected() {
        let empty = UpdateEventRequest {
            summary: None,
            description: None,
            dtstart: None,
            dtend: None,
            rrule: None,
        };
        assert!(empty.is_empty());
        let non_empty = UpdateEventRequest {
            summary: Some("x".to_string()),
            description: None,
            dtstart: None,
            dtend: None,
            rrule: None,
        };
        assert!(!non_empty.is_empty());
    }
}
