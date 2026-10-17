//! F11 — REST handlers for calendar events (`/api/v1/collections/{id}/events`).
//!
//! The SPA cannot speak CalDAV, so these endpoints expose the `calendar_objects`
//! of a `calendar` collection as clean JSON DTOs. Every handler is scoped to the
//! authenticated subject ([`SessionUser::sub`]): a collection owned by another user
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
//! normalised away (so `""` clears an optional field on `PATCH`). An optional
//! `tzid` (F18) must be a known IANA zone — resolved and stored in its canonical
//! form; see [`validate_tzid`]. Because
//! `RRULE` is emitted verbatim into the stored `.ics`, it is additionally
//! restricted to the RFC 5545 RECUR alphabet (no control characters), which
//! prevents iCalendar content-line injection. Failures use the F06 [`ApiError`]
//! envelope.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;

use crate::auth::SessionUser;
use crate::dav::common::new_id;
use crate::dav::ics::{
    EventFields, EventProjection, build_ics, project_event, reconcile_ics,
    supported_instant_range_ms,
};
use crate::dav::vtimezone;
use crate::db::models::{CalendarObject, CalendarObjectUpdate, CollectionKind, NewCalendarObject};
use crate::db::{
    Collection, DbError, clamp_collection_limit, clamp_offset, delete_calendar_object,
    get_calendar_object, get_collection, insert_calendar_object, list_calendar_objects_window,
    update_calendar_object,
};
use crate::router::AppState;

use super::dto::{CreateEventRequest, EventDetailDto, EventDto, UpdateEventRequest};
use super::error::ApiError;
use super::{parse_json_body, parse_optional_i64};

/// Maximum accepted length of an event summary, in characters.
const MAX_SUMMARY_LEN: usize = 512;

/// Maximum accepted length of an event description, in characters.
const MAX_DESCRIPTION_LEN: usize = 8_192;

/// Maximum accepted length of an `RRULE` value, in characters.
const MAX_RRULE_LEN: usize = 1_024;

/// Maximum accepted length of an event location (`LOCATION`), in characters.
const MAX_LOCATION_LEN: usize = 1_024;

/// Maximum accepted length of an event link (`URL`), in characters.
const MAX_URL_LEN: usize = 2_048;

/// Maximum accepted length of a client-supplied `uid`, in characters.
const MAX_UID_LEN: usize = 255;

/// Maximum accepted length of a single `CATEGORIES` item, in characters.
const MAX_CATEGORY_LEN: usize = 128;

/// Maximum number of `CATEGORIES` items accepted.
const MAX_CATEGORIES: usize = 64;

/// Maximum number of `ATTACH` URIs accepted.
const MAX_ATTACH: usize = 64;

/// Maximum accepted length of a `DURATION` value, in characters.
const MAX_DURATION_LEN: usize = 64;

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
    user: SessionUser,
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
/// fresh `id` and `uid` are minted unless the client pins a `uid`. An optional
/// `tzid` anchors the event to an IANA zone (both `DTSTART` and `DTEND`), so the
/// `.ics` carries `TZID=` parameters plus a `VTIMEZONE` block.
///
/// # Errors
///
/// [`ApiError::Validation`] for a missing/blank summary, an invalid range, or a
/// malformed payload; [`ApiError::NotFound`] for a missing/foreign/non-calendar
/// collection; [`ApiError::Conflict`] when the (client-supplied) `uid` already
/// exists in the collection; [`ApiError::Internal`] if the insert fails.
pub async fn create(
    State(state): State<AppState>,
    user: SessionUser,
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
    let location = validate_location(request.location.as_deref())?;
    let url = validate_url(request.url.as_deref())?;
    let rrule = validate_rrule(request.rrule.as_deref())?;
    let tzid = validate_tzid(request.tzid.as_deref())?;
    let status = validate_status(request.status.as_deref())?;
    let class_ = validate_class(request.class_.as_deref())?;
    let transp = validate_transp(request.transp.as_deref())?;
    let duration = validate_duration(request.duration.as_deref())?;
    let categories = validate_categories(request.categories.as_deref().unwrap_or_default())?;
    let attach = validate_attach(request.attach.as_deref().unwrap_or_default())?;
    validate_range(request.dtstart, request.dtend)?;

    // `DURATION` and `DTEND` are mutually exclusive (RFC 5545): supplying both is
    // rejected, and a `DURATION` supersedes any (already rejected) `DTEND`.
    if duration.is_some() && request.dtend.is_some() {
        return Err(duration_dtend_exclusive());
    }
    let dtend = if duration.is_some() {
        None
    } else {
        request.dtend
    };

    let collection = calendar_collection(&state, &id, &user.sub).await?;

    let uid = match request.uid.as_deref() {
        Some(raw) => validate_uid(raw)?,
        None => new_id(),
    };

    let fields = EventFields {
        uid: uid.clone(),
        summary: Some(summary.clone()),
        description: description.clone(),
        location: location.clone(),
        url: url.clone(),
        dtstart_ms: request.dtstart,
        dtend_ms: dtend,
        dtstart_tzid: tzid.clone(),
        dtend_tzid: tzid.clone(),
        rrule: rrule.clone(),
        status: status.clone(),
        class_: class_.clone(),
        transp: transp.clone(),
        duration: duration.clone(),
        categories: categories.clone(),
        attach: attach.clone(),
        dtstamp_ms: now_millis(),
    };

    let new = NewCalendarObject {
        id: new_id(),
        collection_id: collection.id.clone(),
        uid,
        summary: Some(summary),
        description,
        location,
        url,
        dtstart: Some(request.dtstart),
        dtend,
        dtstart_tzid: tzid.clone(),
        dtend_tzid: tzid,
        rrule,
        raw_payload: build_ics(&fields),
    };

    let stored = insert_calendar_object(&state.db, &new)
        .await
        .map_err(map_insert_error)?;

    let projection = project_stored(&stored)?;
    Ok((
        StatusCode::CREATED,
        Json(EventDetailDto::new(&stored, projection)),
    ))
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
    user: SessionUser,
    Path((id, uid)): Path<(String, String)>,
) -> Result<Json<EventDetailDto>, ApiError> {
    let collection = calendar_collection(&state, &id, &user.sub).await?;
    let object = get_calendar_object(&state.db, &collection.id, &uid)
        .await?
        .ok_or(ApiError::NotFound)?;
    let projection = project_stored(&object)?;
    Ok(Json(EventDetailDto::new(&object, projection)))
}

/// `PATCH /api/v1/collections/{id}/events/{uid}` — partially update an event.
///
/// Only the supplied fields are applied; the `.ics` is recomposed from the
/// merged values and a fresh ETag is written. A patch with no updatable field is
/// a `400` (it would otherwise silently refresh `updated_at`).
///
/// A supplied `tzid` re-anchors **both** `DTSTART` and `DTEND` to that zone; an
/// absent `tzid` leaves the stored anchors untouched. There is deliberately no
/// way to *clear* the zone through a `PATCH` (an absent field means "do not
/// change", the same MVP limitation as the other optional fields).
///
/// # Errors
///
/// [`ApiError::Validation`] for invalid input or an empty patch;
/// [`ApiError::NotFound`] when the collection or event does not exist (or
/// belongs to another user / is not a calendar); [`ApiError::Internal`] if the
/// update fails.
pub async fn update(
    State(state): State<AppState>,
    user: SessionUser,
    Path((id, uid)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<EventDetailDto>, ApiError> {
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

    // Merge rule (single and uniform, for every modelled field below): when the
    // `PATCH` does not mention a field, its effective value is inherited from the
    // request, then the stored payload, and only then the denormalised column.
    //
    // The stored `.ics` is the source of truth: it is the artefact native clients
    // sync and the very bytes `reconcile_ics` rewrites in place. The columns are a
    // derived index, and an *additive* one — `location`/`url` arrived in migration
    // `0005` and the `*_tzid` columns in `0004`, so a row written under an older
    // schema has them at `NULL` even though its payload already carries the
    // property. Taking the column first would rewrite such a field from stale data,
    // silently mutating something the `PATCH` never mentioned — exactly the silent
    // deletion PLAN-028 exists to remove.
    //
    // As of F25 the payload is read through `project_event` rather than
    // `parse_ics`: the projection carries **everything** the payload says — the
    // column-backed properties *and* the six F25 properties, which have no column
    // at all — so the merge keeps a single source of truth (`petición → payload →
    // columna`).
    //
    // Validation is a frontier for what *enters through the API*: only a value
    // supplied by the request is validated. An inherited value (from the payload or
    // the column) passes through verbatim — revalidating it would `400` *any*
    // `PATCH` on an event imported from a native client with, say, a
    // `URL:mailto:…`/`ftp://…` (a scheme outside our allowlist), a `STATUS:x-custom`
    // or an over-long `LOCATION`.
    let stored = project_event(&existing.raw_payload).ok();

    let summary = match &request.summary {
        Some(value) => Some(validate_summary(value)?),
        None => stored
            .as_ref()
            .and_then(|event| event.summary.clone())
            .or_else(|| existing.summary.clone()),
    };
    let description = match &request.description {
        Some(value) => {
            normalize_optional(Some(value.as_str()), "description", MAX_DESCRIPTION_LEN)?
        }
        None => stored
            .as_ref()
            .and_then(|event| event.description.clone())
            .or_else(|| existing.description.clone()),
    };
    let location = match &request.location {
        Some(value) => validate_location(Some(value))?,
        None => stored
            .as_ref()
            .and_then(|event| event.location.clone())
            .or_else(|| existing.location.clone()),
    };
    let url = match &request.url {
        Some(value) => validate_url(Some(value))?,
        None => stored
            .as_ref()
            .and_then(|event| event.url.clone())
            .or_else(|| existing.url.clone()),
    };
    let rrule = match &request.rrule {
        Some(value) => validate_rrule(Some(value.as_str()))?,
        None => stored
            .as_ref()
            .and_then(|event| event.rrule.clone())
            .or_else(|| existing.rrule.clone()),
    };
    // An event always carries a DTSTART; fall back to 0 only for a legacy row
    // that lacks both a parseable payload and the denormalised column.
    let dtstart = request
        .dtstart
        .or_else(|| stored.as_ref().map(|event| event.dtstart_ms))
        .or(existing.dtstart)
        .unwrap_or(0);
    let dtend = request
        .dtend
        .or_else(|| stored.as_ref().and_then(|event| event.dtend_ms))
        .or(existing.dtend);
    validate_range(dtstart, dtend)?;

    // A `tzid` in the patch replaces **both** anchors with the same canonical
    // zone; an absent `tzid` leaves each anchor untouched (there is no way to
    // clear a zone through a `PATCH`). Preserving the two stored anchors
    // independently keeps a DAV-created event (which may carry distinct
    // `DTSTART`/`DTEND` zones) intact across an unrelated `PATCH`.
    let (dtstart_tzid, dtend_tzid) = match request.tzid.as_deref() {
        Some(raw) => {
            let tzid = validate_tzid(Some(raw))?;
            (tzid.clone(), tzid)
        }
        None => (
            stored
                .as_ref()
                .and_then(|event| event.dtstart_tzid.clone())
                .or_else(|| existing.dtstart_tzid.clone()),
            stored
                .as_ref()
                .and_then(|event| event.dtend_tzid.clone())
                .or_else(|| existing.dtend_tzid.clone()),
        ),
    };

    // The six F25 properties have no column: their effective value is the payload
    // one (`request.X` when supplied, else the projected payload). Only a supplied
    // value is validated; an inherited one passes verbatim.
    let status = match &request.status {
        Some(value) => validate_status(Some(value))?,
        None => stored.as_ref().and_then(|event| event.status.clone()),
    };
    let class_ = match &request.class_ {
        Some(value) => validate_class(Some(value))?,
        None => stored.as_ref().and_then(|event| event.class.clone()),
    };
    let transp = match &request.transp {
        Some(value) => validate_transp(Some(value))?,
        None => stored.as_ref().and_then(|event| event.transp.clone()),
    };
    let categories = match &request.categories {
        Some(values) => validate_categories(values)?,
        None => stored
            .as_ref()
            .map(|event| event.categories.iter().flatten().cloned().collect())
            .unwrap_or_default(),
    };
    let attach = match &request.attach {
        Some(values) => validate_attach(values)?,
        None => stored
            .as_ref()
            .map(|event| event.attach.clone())
            .unwrap_or_default(),
    };

    // `DURATION`/`DTEND` are mutually exclusive (RFC 5545) and the server enforces
    // it, not just the UI:
    //   * a `duration` **with a value** wins and drops `DTEND`;
    //   * a **blank** `duration` clears `DURATION` and leaves `DTEND` untouched;
    //   * an explicit `dtend` wins and drops `DURATION`;
    //   * supplying **both** (a value and an end) is a `400`;
    //   * mentioning neither keeps both exactly as the payload has them (a valid
    //     `.ics` never has both).
    // This is applied **before** `reconcile_ics`.
    let request_duration = validate_duration(request.duration.as_deref())?;
    let dtend_in_request = request.dtend.is_some();
    if request_duration.is_some() && dtend_in_request {
        return Err(duration_dtend_exclusive());
    }
    let inherited_duration = stored.as_ref().and_then(|event| event.duration.clone());
    // Two distinct reasons collapse to the same `(None, dtend)` branch, and the
    // distinction must not be lost:
    //   * `dtend_in_request` — an explicit `dtend` drops `DURATION`;
    //   * `request.duration.is_some()` — a **blank** `duration` clears `DURATION`
    //     and leaves `DTEND` untouched (which is why `dtend` is passed through
    //     unchanged here).
    let (duration, dtend) = if request_duration.is_some() {
        (request_duration, None)
    } else if dtend_in_request || request.duration.is_some() {
        (None, dtend)
    } else {
        (inherited_duration, dtend)
    };

    let fields = EventFields {
        uid: existing.uid.clone(),
        summary: summary.clone(),
        description: description.clone(),
        location: location.clone(),
        url: url.clone(),
        dtstart_ms: dtstart,
        dtend_ms: dtend,
        dtstart_tzid: dtstart_tzid.clone(),
        dtend_tzid: dtend_tzid.clone(),
        rrule: rrule.clone(),
        status,
        class_,
        transp,
        duration,
        categories,
        attach,
        dtstamp_ms: now_millis(),
    };

    // `reconcile_ics` rewrites only the modelled content lines in place, keeping
    // every unmodelled property (`VALARM`, other `ATTENDEE`s, `X-`) and the
    // original folding byte for byte (F24/PLAN-028).
    let raw_payload = match reconcile_ics(&existing.raw_payload, &fields) {
        Ok(payload) => payload,
        // Safety net: a degenerate row without a usable `VEVENT` is recomposed
        // from scratch rather than failing the request. This should never happen
        // — every row is born from an already-parsed `PUT`/`POST`.
        Err(err) => {
            tracing::warn!(error = %err, "reconcile_ics falló; se recompone con build_ics");
            build_ics(&fields)
        }
    };

    let update = CalendarObjectUpdate {
        summary,
        description,
        location,
        url,
        dtstart: Some(dtstart),
        dtend,
        dtstart_tzid,
        dtend_tzid,
        rrule,
        raw_payload,
    };

    let stored = update_calendar_object(&state.db, &existing.id, &update).await?;
    let projection = project_stored(&stored)?;
    Ok(Json(EventDetailDto::new(&stored, projection)))
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
    user: SessionUser,
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

/// Validate and normalise an optional event location (`LOCATION`).
///
/// `LOCATION` is free text: a blank value is normalised away (`None`, so `""`
/// clears the field on `PATCH`) and the value is otherwise kept as sent. It needs
/// no charset restriction — [`build_ics`]/[`reconcile_ics`] escape it per RFC
/// 5545 §3.3.11, so even a newline is emitted as the two characters `\n` and can
/// never split the content line.
fn validate_location(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    normalize_optional(raw, "location", MAX_LOCATION_LEN)
}

/// Validate and normalise an optional event link (`URL`).
///
/// A blank value is normalised away (`None`, so `""` clears the field on
/// `PATCH`). Otherwise the value must be an **absolute URI whose scheme is
/// `http` or `https`** (checked ASCII case-insensitively) followed by `://` and a
/// non-empty remainder.
///
/// This is a security boundary, not cosmetics, on two counts:
///
/// 1. [`build_ics`]/[`reconcile_ics`] emit `URL` **verbatim** (a URI must not be
///    escaped), so a value carrying a control character or whitespace — e.g.
///    `https://x\r\nBEGIN:VEVENT` — would inject an iCalendar content line into
///    the stored `raw_payload`. Those bytes are therefore rejected.
/// 2. The SPA renders the URL as a clickable link, so a `javascript:`/`data:`/
///    `file:` value would be a stored XSS. Only the two `http`/`https` schemes
///    are allowed.
fn validate_url(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(value) = raw else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Ok(None);
    }
    if value.chars().count() > MAX_URL_LEN {
        return Err(ApiError::Validation(format!(
            "El campo `url` no puede superar {MAX_URL_LEN} caracteres"
        )));
    }
    // `URL` is emitted verbatim: reject every control character (`< 0x20`, `0x7F`
    // and the C1 block) and any whitespace, which would otherwise let a value
    // inject a content line.
    if value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(ApiError::Validation(
            "El campo `url` contiene caracteres de control o espacios no permitidos".to_string(),
        ));
    }
    // Scheme allowlist: `http://` or `https://` (ASCII case-insensitive) followed
    // by a non-empty remainder. Done by hand: no `url` crate is a direct
    // dependency.
    let lower = value.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("https://"));
    match rest {
        Some(rest) if !rest.is_empty() => Ok(Some(value.to_string())),
        _ => Err(ApiError::Validation(
            "El campo `url` debe ser una URL absoluta `http://` o `https://`".to_string(),
        )),
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

/// Validate and canonicalise an optional IANA time-zone name (`tzid`).
///
/// `None` means "no anchor" (the event stays in UTC, the pre-F18 behaviour).
/// When a value is supplied it must name a real IANA zone — checked with
/// [`vtimezone::is_valid_tzid`] — otherwise the request is rejected with
/// [`ApiError::Validation`] (`400`). The **canonical** zone name
/// ([`vtimezone::canonical_tzid`]) is returned rather than the string the client
/// sent, so an alias is always persisted under its canonical identifier and the
/// stored `TZID=` parameter can never disagree with the `VTIMEZONE` block.
fn validate_tzid(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(value) = raw else {
        return Ok(None);
    };
    if !vtimezone::is_valid_tzid(value) {
        return Err(ApiError::Validation(format!(
            "La zona horaria `{value}` no es un identificador IANA válido"
        )));
    }
    // `is_valid_tzid` guarantees `parse_tzid` yields a zone.
    let tz = vtimezone::parse_tzid(value).expect("is_valid_tzid implies a parseable zone");
    Ok(Some(vtimezone::canonical_tzid(tz).to_string()))
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

/// Project an event's stored `raw_payload`, mapping a corrupt payload to a `500`.
///
/// The projection is fallible (an `.ics` the parser cannot read); a failure is a
/// server-side integrity problem, so it is logged and surfaced as a generic
/// [`ApiError::Internal`] — never a `200` with invented data. This mirrors the
/// contact detail's handling of a corrupt vCard.
fn project_stored(object: &CalendarObject) -> Result<EventProjection, ApiError> {
    project_event(&object.raw_payload).map_err(|err| {
        tracing::error!(
            error = ?err,
            uid = ?object.uid,
            "raw_payload corrupto; no se puede proyectar el VEVENT"
        );
        ApiError::Internal
    })
}

/// The `400` raised when a request supplies both a `DURATION` and a `DTEND`.
fn duration_dtend_exclusive() -> ApiError {
    ApiError::Validation(
        "`duration` y `dtend` son mutuamente excluyentes; indique solo uno".to_string(),
    )
}

/// Validate and normalise an optional `STATUS` (RFC 5545 §3.8.1.11).
///
/// A blank value is normalised away (`None`, so `""` clears the field on
/// `PATCH`). Otherwise the value must be one of `TENTATIVE`, `CONFIRMED` or
/// `CANCELLED` (ASCII case-insensitive) and is returned in **upper case**.
fn validate_status(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    validate_token(
        raw,
        "status",
        "STATUS",
        &["TENTATIVE", "CONFIRMED", "CANCELLED"],
    )
}

/// Validate and normalise an optional `CLASS` (RFC 5545 §3.8.1.3).
///
/// Only the three standard tokens are accepted through the API; a blank value
/// clears the field and a supplied value is up-cased. (A non-standard token
/// *inherited* from a client is preserved verbatim — see [`update`].)
fn validate_class(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    validate_token(
        raw,
        "class",
        "CLASS",
        &["PUBLIC", "PRIVATE", "CONFIDENTIAL"],
    )
}

/// Validate and normalise an optional `TRANSP` (RFC 5545 §3.8.2.7).
///
/// Only `OPAQUE`/`TRANSPARENT` are accepted; blank clears, supplied is up-cased.
fn validate_transp(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    validate_token(raw, "transp", "TRANSP", &["OPAQUE", "TRANSPARENT"])
}

/// Validate and normalise an optional, single-token property against an
/// upper-case allowlist.
///
/// A blank value clears the field (`None`). A supplied value is compared ASCII
/// case-insensitively against `allowed` and returned in upper case; anything else
/// is a `400`. `field` names the JSON key, `property` the iCalendar property.
fn validate_token(
    raw: Option<&str>,
    field: &str,
    property: &str,
    allowed: &[&str],
) -> Result<Option<String>, ApiError> {
    let Some(value) = raw else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let upper = trimmed.to_ascii_uppercase();
    if allowed.contains(&upper.as_str()) {
        Ok(Some(upper))
    } else {
        Err(ApiError::Validation(format!(
            "El campo `{field}` debe ser uno de los valores de `{property}`: {}",
            allowed.join(", ")
        )))
    }
}

/// Validate and normalise an optional `DURATION` (RFC 5545 §3.3.6 `dur-value`).
///
/// A blank value is normalised away (`None`, so `""` clears the field on
/// `PATCH`). Otherwise the value must be a bounded, well-formed `dur-value`:
/// `[+/-]P` followed by a number of weeks (`nW`), or a number of days (`nD`)
/// optionally followed by a `T` time section (`nH`/`nM`/`nS`), or the time
/// section alone — with **at least one** component (`PT1H`, `P1D`, `P2W`,
/// `P1DT2H`, `PT15M`). The value is returned verbatim.
///
/// This is a security boundary: [`build_ics`]/[`reconcile_ics`] emit `DURATION`
/// **verbatim** (its designator letters are structural), so a value carrying a
/// control character or whitespace could otherwise inject a content line.
fn validate_duration(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(value) = raw else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > MAX_DURATION_LEN {
        return Err(ApiError::Validation(format!(
            "El campo `duration` no puede superar {MAX_DURATION_LEN} caracteres"
        )));
    }
    if !is_valid_duration(trimmed) {
        return Err(ApiError::Validation(
            "El campo `duration` no es una duración RFC 5545 válida (p. ej. `PT1H`, `P1D`, `P2W`)"
                .to_string(),
        ));
    }
    Ok(Some(trimmed.to_string()))
}

/// Whether `value` matches the RFC 5545 `dur-value` grammar.
///
/// Accepts an optional leading `+`/`-`, then `P`, then one of: `nW`; `nD`
/// with an optional `T` time section; or a `T` time section (`nH`, `nM`, `nS`,
/// in that order, at least one). At least one component is required.
fn is_valid_duration(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut i = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    if bytes.get(i) != Some(&b'P') {
        return false;
    }
    i += 1;

    // Weeks stand alone: `nW` and nothing else.
    if scan_number_unit(bytes, &mut i, b'W') {
        return i == bytes.len();
    }

    let mut has_component = scan_number_unit(bytes, &mut i, b'D');

    if bytes.get(i) == Some(&b'T') {
        i += 1;
        let mut last_rank = 0u8;
        let mut time_has = false;
        for (unit, rank) in [(b'H', 1u8), (b'M', 2), (b'S', 3)] {
            if scan_number_unit(bytes, &mut i, unit) {
                // The time units must be contiguous (`H`, `HM`, `HMS`, `M`, `MS`),
                // never a gap such as `H…S` without an `M`.
                if last_rank != 0 && rank != last_rank + 1 {
                    return false;
                }
                last_rank = rank;
                time_has = true;
            }
        }
        if !time_has {
            return false;
        }
        has_component = true;
    }

    has_component && i == bytes.len()
}

/// Consume `1*DIGIT` followed by `unit` from `bytes` at `*i`.
///
/// On success advances `*i` past the unit and returns `true`; otherwise restores
/// `*i` and returns `false`.
fn scan_number_unit(bytes: &[u8], i: &mut usize, unit: u8) -> bool {
    let start = *i;
    while bytes.get(*i).is_some_and(u8::is_ascii_digit) {
        *i += 1;
    }
    if *i == start || bytes.get(*i) != Some(&unit) {
        *i = start;
        return false;
    }
    *i += 1;
    true
}

/// Validate and normalise `CATEGORIES` items.
///
/// Each item is trimmed, blank items are dropped, and control characters are
/// rejected (each item is escaped per RFC 5545 §3.3.11 on emission, so a newline
/// would be written as `\n` — the control-character ban is defence in depth).
/// Per-item and item-count bounds are enforced. An empty (or all-blank) input
/// yields no categories, which clears the property.
fn validate_categories(items: &[String]) -> Result<Vec<String>, ApiError> {
    let mut out = Vec::new();
    for item in items {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().count() > MAX_CATEGORY_LEN {
            return Err(ApiError::Validation(format!(
                "Cada categoría no puede superar {MAX_CATEGORY_LEN} caracteres"
            )));
        }
        if trimmed.chars().any(char::is_control) {
            return Err(ApiError::Validation(
                "Las categorías no pueden contener caracteres de control".to_string(),
            ));
        }
        if out.len() >= MAX_CATEGORIES {
            return Err(ApiError::Validation(format!(
                "No se admiten más de {MAX_CATEGORIES} categorías"
            )));
        }
        out.push(trimmed.to_string());
    }
    Ok(out)
}

/// Validate and normalise `ATTACH` URIs.
///
/// Each item is validated with [`validate_url`] — the same `http`/`https`
/// allowlist and the same ban on spaces/control characters as `URL`, because the
/// SPA renders each attachment as a link and the value is emitted **verbatim**.
/// Blank items are dropped; the count is bounded. An empty (or all-blank) input
/// yields no attachments, which clears the property.
fn validate_attach(items: &[String]) -> Result<Vec<String>, ApiError> {
    let mut out = Vec::new();
    for item in items {
        if let Some(uri) = validate_url(Some(item.as_str()))? {
            if out.len() >= MAX_ATTACH {
                return Err(ApiError::Validation(format!(
                    "No se admiten más de {MAX_ATTACH} adjuntos"
                )));
            }
            out.push(uri);
        }
    }
    Ok(out)
}

/// Reject an inverted range and any instant outside the representable calendar
/// range.
///
/// The instant sanity bound
/// ([`supported_instant_range_ms`](crate::dav::ics::supported_instant_range_ms))
/// is a security boundary, not cosmetics: `dtstart`/`dtend` are arbitrary `i64`
/// milliseconds supplied by the client, and a value such as `i64::MAX` cannot be
/// rendered as an iCalendar date-time nor projected into a zone — it would be
/// handed to [`build_ics`] (and, with a `tzid`, to the [`vtimezone`]
/// composition). Rejecting it here (`400`) keeps that impossible input away from
/// the composition path. The bound is exactly the parser's four-digit-year range,
/// so the REST API and the DAV `PUT` path accept the same instants.
fn validate_range(dtstart: i64, dtend: Option<i64>) -> Result<(), ApiError> {
    let (min_ms, max_ms) = supported_instant_range_ms();
    check_instant(dtstart, "dtstart", min_ms, max_ms)?;
    if let Some(end) = dtend {
        check_instant(end, "dtend", min_ms, max_ms)?;
        if end < dtstart {
            return Err(ApiError::Validation(
                "El fin no puede ser anterior al inicio".to_string(),
            ));
        }
    }
    Ok(())
}

/// Reject an instant outside the half-open supported range `[min_ms, max_ms)`.
fn check_instant(ms: i64, field: &str, min_ms: i64, max_ms: i64) -> Result<(), ApiError> {
    if ms < min_ms || ms >= max_ms {
        return Err(ApiError::Validation(format!(
            "El campo `{field}` está fuera del rango de fechas admitido (años 1 a 9999)"
        )));
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
    fn validate_range_rejects_out_of_range_instants() {
        for ms in [i64::MAX, i64::MIN] {
            assert!(matches!(
                validate_range(ms, None),
                Err(ApiError::Validation(_)),
            ));
            assert!(matches!(
                validate_range(1_700_000_000_000, Some(ms)),
                Err(ApiError::Validation(_)),
            ));
        }
        // A same-day 2024 range stays valid.
        assert!(validate_range(1_700_000_000_000, Some(1_700_000_600_000)).is_ok());
    }

    #[test]
    fn validate_uid_rejects_blank_and_slashes() {
        assert!(matches!(validate_uid("  "), Err(ApiError::Validation(_))));
        assert!(matches!(validate_uid("a/b"), Err(ApiError::Validation(_))));
        assert_eq!(validate_uid(" evt-1 ").ok(), Some("evt-1".to_string()));
    }

    #[test]
    fn empty_patch_is_detected() {
        let empty = UpdateEventRequest::default();
        assert!(empty.is_empty());
        let non_empty = UpdateEventRequest {
            summary: Some("x".to_string()),
            ..UpdateEventRequest::default()
        };
        assert!(!non_empty.is_empty());
        // A patch that only changes the zone is *not* empty (F18): it must be
        // accepted rather than rejected as "no field to update".
        let tzid_only = UpdateEventRequest {
            tzid: Some("Europe/Madrid".to_string()),
            ..UpdateEventRequest::default()
        };
        assert!(!tzid_only.is_empty());
    }

    #[test]
    fn update_is_empty_considers_the_new_fields() {
        // F24: a patch that only touches `location` or `url` is *not* empty — it
        // must be accepted rather than answered with the "no field to update"
        // `400`.
        let location_only = UpdateEventRequest {
            location: Some("Sala 3".to_string()),
            ..UpdateEventRequest::default()
        };
        assert!(!location_only.is_empty());

        let url_only = UpdateEventRequest {
            url: Some("https://ejemplo.org/x".to_string()),
            ..UpdateEventRequest::default()
        };
        assert!(!url_only.is_empty());

        // F25: a patch that only touches `categories` or `attach` is *not* empty
        // either.
        let categories_only = UpdateEventRequest {
            categories: Some(vec!["trabajo".to_string()]),
            ..UpdateEventRequest::default()
        };
        assert!(!categories_only.is_empty());

        let attach_only = UpdateEventRequest {
            attach: Some(vec!["https://ejemplo.org/a".to_string()]),
            ..UpdateEventRequest::default()
        };
        assert!(!attach_only.is_empty());
    }

    #[test]
    fn validate_status_normalises_and_rejects_unknown() {
        assert_eq!(validate_status(None).ok(), Some(None));
        assert_eq!(validate_status(Some("  ")).ok(), Some(None));
        assert_eq!(
            validate_status(Some("confirmed")).ok(),
            Some(Some("CONFIRMED".to_string()))
        );
        assert_eq!(
            validate_status(Some(" tentative ")).ok(),
            Some(Some("TENTATIVE".to_string()))
        );
        assert!(matches!(
            validate_status(Some("MAYBE")),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn validate_class_and_transp_normalise_and_reject_unknown() {
        assert_eq!(
            validate_class(Some("private")).ok(),
            Some(Some("PRIVATE".to_string()))
        );
        assert_eq!(
            validate_class(Some("PUBLIC")).ok(),
            Some(Some("PUBLIC".to_string()))
        );
        assert_eq!(
            validate_class(Some("confidential")).ok(),
            Some(Some("CONFIDENTIAL".to_string()))
        );
        assert!(matches!(
            validate_class(Some("x-custom")),
            Err(ApiError::Validation(_))
        ));

        assert_eq!(
            validate_transp(Some("transparent")).ok(),
            Some(Some("TRANSPARENT".to_string()))
        );
        assert_eq!(
            validate_transp(Some("opaque")).ok(),
            Some(Some("OPAQUE".to_string()))
        );
        assert!(matches!(
            validate_transp(Some("busy")),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn validate_duration_accepts_the_grammar_and_rejects_junk() {
        assert_eq!(validate_duration(None).ok(), Some(None));
        assert_eq!(validate_duration(Some("  ")).ok(), Some(None));
        for value in ["PT1H", "P1D", "P2W", "P1DT2H", "PT15M", "-PT30M", "+P1D"] {
            assert_eq!(
                validate_duration(Some(value)).ok(),
                Some(Some(value.to_string())),
                "must accept {value:?}"
            );
        }
        for value in [
            "1 hora",
            "P",
            "PT",
            "P1",
            "1H",
            "PT1H2",
            "P1W2D",
            "PT1H\r\nX",
        ] {
            assert!(
                matches!(validate_duration(Some(value)), Err(ApiError::Validation(_))),
                "must reject {value:?}"
            );
        }
        let long = format!("PT{}H", "9".repeat(MAX_DURATION_LEN));
        assert!(matches!(
            validate_duration(Some(&long)),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn validate_categories_trims_drops_blanks_and_bounds() {
        assert_eq!(
            validate_categories(&["  trabajo  ".to_string(), String::new()]).ok(),
            Some(vec!["trabajo".to_string()])
        );
        assert_eq!(validate_categories(&[]).ok(), Some(Vec::new()));
        let long = "x".repeat(MAX_CATEGORY_LEN + 1);
        assert!(matches!(
            validate_categories(&[long]),
            Err(ApiError::Validation(_))
        ));
        assert!(matches!(
            validate_categories(&["a\u{0007}".to_string()]),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn validate_attach_reuses_the_url_allowlist() {
        assert_eq!(
            validate_attach(&[
                "https://a.example/x".to_string(),
                String::new(),
                "https://b.example/y".to_string(),
            ])
            .ok(),
            Some(vec![
                "https://a.example/x".to_string(),
                "https://b.example/y".to_string(),
            ])
        );
        assert_eq!(validate_attach(&[]).ok(), Some(Vec::new()));
        for hostile in [
            "javascript:alert(1)",
            "mailto:ada@example.org",
            "https://x y",
        ] {
            assert!(
                matches!(
                    validate_attach(&[hostile.to_string()]),
                    Err(ApiError::Validation(_))
                ),
                "must reject {hostile:?}"
            );
        }
    }

    #[test]
    fn validate_location_normalises_blank_and_bounds_length() {
        assert_eq!(validate_location(None).ok(), Some(None));
        assert_eq!(validate_location(Some("  ")).ok(), Some(None));
        assert_eq!(
            validate_location(Some("Sala 3")).ok(),
            Some(Some("Sala 3".to_string()))
        );
        let long = "x".repeat(MAX_LOCATION_LEN + 1);
        assert!(matches!(
            validate_location(Some(&long)),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn validate_url_allows_only_absolute_http_https_and_rejects_injection() {
        assert_eq!(validate_url(None).ok(), Some(None));
        assert_eq!(validate_url(Some("   ")).ok(), Some(None));
        assert_eq!(
            validate_url(Some("https://ejemplo.org/x")).ok(),
            Some(Some("https://ejemplo.org/x".to_string()))
        );
        // The scheme check is ASCII case-insensitive.
        assert_eq!(
            validate_url(Some("HTTP://ejemplo.org/x")).ok(),
            Some(Some("HTTP://ejemplo.org/x".to_string()))
        );
        for hostile in [
            "javascript:alert(1)",
            "data:text/html,x",
            "file:///etc/passwd",
            "mailto:ada@example.org",
            "https://",
            "https://x\ny",
            "https://x y",
            "https://x\u{0007}",
        ] {
            assert!(
                matches!(validate_url(Some(hostile)), Err(ApiError::Validation(_))),
                "must reject {hostile:?}"
            );
        }
        let long = format!("https://{}", "x".repeat(MAX_URL_LEN));
        assert!(matches!(
            validate_url(Some(&long)),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn validate_tzid_accepts_canonical_names_and_rejects_unknown() {
        assert_eq!(validate_tzid(None).ok(), Some(None));
        assert_eq!(
            validate_tzid(Some("Europe/Madrid")).ok(),
            Some(Some("Europe/Madrid".to_string()))
        );
        // An alias resolves to its canonical identifier.
        assert_eq!(
            validate_tzid(Some("UTC")).ok(),
            Some(Some("UTC".to_string()))
        );
        assert!(matches!(
            validate_tzid(Some("Mars/Olympus")),
            Err(ApiError::Validation(_))
        ));
    }
}
