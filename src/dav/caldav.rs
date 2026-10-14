//! F07 — CalDAV handlers: `MKCALENDAR`, `PROPFIND`, `REPORT`, `PUT`, `GET` and
//! `DELETE`.
//!
//! This module implements the CalDAV tree:
//!
//! * `MKCALENDAR` on the calendar-home or on a specific collection path creates
//!   a `calendar` collection owned by the authenticated subject.
//! * `PROPFIND` on the calendar-home lists the caller's collections, on a
//!   collection lists its `.ics` resources, and on a resource returns its
//!   `getetag` / `getcontenttype` / `calendar-data`.
//! * `PUT` on `/{collection}/{uid}.ics` parses the payload, denormalises it into
//!   `calendar_objects` and stores the raw `.ics`; a new `UID` yields `201`, an
//!   update of an existing one `204`, and the response always carries the `ETag`.
//! * `GET` returns the stored `.ics` verbatim with `text/calendar` and its
//!   `ETag`; `DELETE` removes the resource.
//! * `REPORT` with a `calendar-query` body lists the collection's `.ics`
//!   resources whose event span intersects the requested `time-range` (or every
//!   resource when the query carries no `time-range`).
//!
//! `PUT` and `DELETE` honour the HTTP conditionals `If-Match` / `If-None-Match`
//! (`412 Precondition Failed` when unmet). Any other unknown verb is rejected
//! with [`DavError::MethodNotAllowed`].
//!
//! # Collection kind (hardening m1)
//!
//! Every resource/collection handler verifies the addressed collection is a
//! `calendar`. A CardDAV address book reached through the calendar tree is
//! reported as `404 Not Found` — the same answer as a missing collection — so
//! address books can never be silently treated as calendars, and their
//! existence is not leaked (the reasoning mirrors the IDOR note below).
//!
//! # Conditional headers (hardening m2/m3)
//!
//! `If-Match` and `If-None-Match` are evaluated on **both** `PUT` and `DELETE`
//! (m2), with the RFC 7232 strong/weak comparison rules. A header that is
//! present but not a valid (ASCII, non-empty) entity-tag list is a malformed
//! request and is rejected with `400 Bad Request` (m3) rather than skipped
//! silently, so a bad conditional can never let a write through.
//!
//! # Resource identity (hardening m7)
//!
//! The `{uid}` path segment must carry the `.ics` suffix; a request that
//! addresses a resource without it is `404 Not Found`. This keeps the resource
//! identity consistent with the `.ics` `href`s OxiDav emits, instead of folding
//! `foo` and `foo.ics` onto the same stored object.
//!
//! # `MKCALENDAR` `Location` (hardening m5)
//!
//! Issued on a collection path, `MKCALENDAR` adopts the path segment as the new
//! collection id, so the `201 Location` points at the **requested** URL (which
//! is immediately addressable). On the calendar-home a fresh UUID is minted and
//! echoed in the `Location`.
//!
//! # `UID` vs. URL
//!
//! RFC 4791 requires the `UID` of the stored component to match the resource
//! name in the URL. OxiDav enforces this at the application layer: a `PUT` whose
//! body `UID` differs from the `{uid}` path segment is rejected with `409
//! Conflict` (not `400`), because the request is well-formed but clashes with the
//! addressed resource identity.
//!
//! # Authorization
//!
//! The `{username}` path segment must equal the authenticated `sub` claim (the
//! app-token owner resolved by the F05 Basic Auth middleware). A mismatch is a
//! `403 Forbidden` (the resource exists conceptually but is not the caller's),
//! matching PLAN-008; every database read is additionally scoped by the subject,
//! so a foreign collection id is indistinguishable from a missing one (`404`).
//!
//! # `Depth`
//!
//! `PROPFIND` honours the `Depth` header for collections: `0` returns only the
//! addressed resource, `1` returns it plus its members. RFC 4918 says an absent
//! `Depth` defaults to `infinity`; because an unbounded walk is both expensive
//! and unnecessary for a personal server, OxiDav **treats a missing, `infinity`
//! or unrecognised `Depth` as `1`**. The target resource itself is always
//! returned, so a `0`-depth probe and a `1`-depth probe differ only in whether
//! members are appended.
//!
//! # XXE
//!
//! Request bodies are parsed with [`super::xml`], which refuses any DTD /
//! `<!DOCTYPE ...>` and never resolves entities, so no external entity can be
//! smuggled through `PROPFIND` / `MKCALENDAR`.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use percent_encoding::utf8_percent_encode;

use crate::auth::SessionUser;
use crate::db::models::{CalendarObjectUpdate, CollectionKind, NewCalendarObject, NewCollection};
use crate::db::{
    CalendarObject, Collection, DbError, MAX_COLLECTION_LIMIT, ResourceKind, create_collection,
    delete_calendar_object, get_calendar_object, get_collection, insert_calendar_object,
    list_calendar_objects, list_collections, update_calendar_object,
};
use crate::router::AppState;

use super::DavError;
use super::common::{
    Depth, MAX_NAME_LEN, PATH_SEGMENT, SYNC_TOKEN_URI_PREFIX, check_conditionals,
    collection_resourcetype, ensure_owner, etag_header, negotiate, new_id, parse_propfind_body,
    report_is_sync_collection, report_sync_collection, requested_depth, sanitize_collection_id,
    supported_report_set, text_prop, validate_sync_depth,
};
use super::ics;
use super::xml::{
    self, DavNs, DavProp, DavResponse, ParseCalendarQuery, ParseMkcalendar, PropValue,
};

/// Accent colour stored for a calendar created over DAV (mirrors the F06
/// default and the `collections.color` schema default).
const DEFAULT_COLOR: &str = "#1677ff";

/// Display name used when neither the `MKCALENDAR` body nor the path segment
/// provides one.
const DEFAULT_CALENDAR_NAME: &str = "Calendario";

/// Suffix a DAV client appends to a resource UID in the URL.
const ICS_SUFFIX: &str = ".ics";

/// `Content-Type` returned for calendar resources.
const CALENDAR_CONTENT_TYPE: &str = "text/calendar; charset=utf-8";

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handler for `/dav/calendars/{username}/` (the calendar-home).
///
/// Answers `PROPFIND` (list the caller's collections) and `MKCALENDAR` (create
/// a new calendar). Everything else is [`DavError::MethodNotAllowed`].
///
/// # Errors
///
/// [`DavError::Forbidden`] when `{username}` is not the authenticated subject,
/// [`DavError::BadRequest`] for a malformed `MKCALENDAR` body, and
/// [`DavError::Internal`] if the database fails.
pub async fn home(
    State(state): State<AppState>,
    user: SessionUser,
    Path(username): Path<String>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, DavError> {
    ensure_owner(&username, &user)?;

    match method.as_str() {
        "PROPFIND" => propfind_home(&state, &username, requested_depth(&headers), &body).await,
        "MKCALENDAR" => mkcalendar(&state, &user, None, &body).await,
        _ => Err(DavError::MethodNotAllowed),
    }
}

/// Handler for `/dav/calendars/{username}/{collection}/`.
///
/// Answers `PROPFIND` (list the collection's resources), `REPORT` (query the
/// collection, see [`report`]) and `MKCALENDAR` (create the calendar at this
/// path). Everything else is [`DavError::MethodNotAllowed`].
///
/// # Errors
///
/// [`DavError::Forbidden`] for a foreign `{username}`, [`DavError::NotFound`]
/// when the collection does not exist or belongs to another user,
/// [`DavError::BadRequest`] for a malformed `MKCALENDAR` / `REPORT` body, and
/// [`DavError::Internal`] if the database fails.
pub async fn collection(
    State(state): State<AppState>,
    user: SessionUser,
    Path((username, collection_id)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, DavError> {
    ensure_owner(&username, &user)?;

    match method.as_str() {
        "PROPFIND" => {
            propfind_collection(
                &state,
                &username,
                &collection_id,
                requested_depth(&headers),
                &body,
            )
            .await
        }
        "REPORT" => report(&state, &username, &collection_id, &headers, &body).await,
        "MKCALENDAR" => mkcalendar(&state, &user, Some(collection_id), &body).await,
        _ => Err(DavError::MethodNotAllowed),
    }
}

/// Handler for `/dav/calendars/{username}/{collection}/{uid}`.
///
/// Dispatches on the HTTP verb: `PROPFIND`, `GET`, `PUT` and `DELETE` are
/// answered; everything else is [`DavError::MethodNotAllowed`].
///
/// # Errors
///
/// [`DavError::Forbidden`] for a foreign `{username}`, [`DavError::NotFound`]
/// when the collection or the resource does not exist (or belongs to another
/// user), [`DavError::BadRequest`] for an unparseable `.ics` body,
/// [`DavError::Conflict`] when the body `UID` differs from the path `{uid}`,
/// [`DavError::PreconditionFailed`] for an unmet `If-Match` / `If-None-Match`,
/// and [`DavError::Internal`] if the database fails.
pub async fn resource(
    State(state): State<AppState>,
    user: SessionUser,
    Path((username, collection_id, filename)): Path<(String, String, String)>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, DavError> {
    ensure_owner(&username, &user)?;

    match method.as_str() {
        "PROPFIND" => propfind_resource(&state, &username, &collection_id, &filename, &body).await,
        "GET" => get_event(&state, &username, &collection_id, &filename).await,
        "PUT" => {
            put_event(
                &state,
                &username,
                &collection_id,
                &filename,
                &headers,
                &body,
            )
            .await
        }
        "DELETE" => delete_event(&state, &username, &collection_id, &filename, &headers).await,
        _ => Err(DavError::MethodNotAllowed),
    }
}

// ---------------------------------------------------------------------------
// PROPFIND
// ---------------------------------------------------------------------------

/// `PROPFIND` on the calendar-home: the home resource itself plus, at depth
/// `1`, one response per calendar collection owned by the caller.
///
/// The request body selects the properties returned (see [`negotiate`]): an
/// empty body means `allprop`.
async fn propfind_home(
    state: &AppState,
    username: &str,
    depth: Depth,
    body: &[u8],
) -> Result<Response, DavError> {
    let request = parse_propfind_body(body)?;

    // `list_collections` clamps the page size; a personal server never exceeds
    // the hard ceiling, so a single page covers every calendar.
    let collections = list_collections(&state.db, username, MAX_COLLECTION_LIMIT, 0)
        .await
        .map_err(|_| DavError::Internal)?;

    let mut responses = vec![negotiate(
        home_href(username),
        home_props(username),
        &request,
    )];
    if depth == Depth::One {
        responses.extend(collections.iter().map(|c| {
            negotiate(
                collection_href(username, &c.id),
                collection_props(c),
                &request,
            )
        }));
    }

    Err(DavError::MultiStatus(xml::multistatus(&responses)))
}

/// `PROPFIND` on a collection: the collection resource itself plus, at depth
/// `1`, one response per `.ics` object with its `getetag`.
///
/// The request body selects the properties returned (see [`negotiate`]).
async fn propfind_collection(
    state: &AppState,
    username: &str,
    collection_id: &str,
    depth: Depth,
    body: &[u8],
) -> Result<Response, DavError> {
    let request = parse_propfind_body(body)?;

    // Ownership is enforced by the `user_id` predicate (a foreign collection is
    // reported as not found); a non-calendar collection is rejected too (m1).
    let collection = calendar_collection(state, collection_id, username).await?;

    let mut responses = vec![negotiate(
        collection_href(username, &collection.id),
        collection_props(&collection),
        &request,
    )];
    if depth == Depth::One {
        let objects = list_calendar_objects(&state.db, &collection.id)
            .await
            .map_err(|_| DavError::Internal)?;
        responses.extend(objects.iter().map(|o| {
            negotiate(
                resource_href(username, &collection.id, &o.uid),
                resource_props(o),
                &request,
            )
        }));
    }

    Err(DavError::MultiStatus(xml::multistatus(&responses)))
}

/// `PROPFIND` on a single `.ics` resource; `404` when it (or its collection)
/// is absent or belongs to another user.
///
/// The request body selects the properties returned (see [`negotiate`]).
async fn propfind_resource(
    state: &AppState,
    username: &str,
    collection_id: &str,
    filename: &str,
    body: &[u8],
) -> Result<Response, DavError> {
    let request = parse_propfind_body(body)?;

    let collection = calendar_collection(state, collection_id, username).await?;

    // The URL must carry `<uid>.ics` (m7); the stored UID has no suffix.
    let uid = resource_uid(filename)?;
    let object = get_calendar_object(&state.db, &collection.id, uid)
        .await
        .map_err(|_| DavError::Internal)?
        .ok_or(DavError::NotFound)?;

    let responses = vec![negotiate(
        resource_href(username, &collection.id, &object.uid),
        resource_props(&object),
        &request,
    )];
    Err(DavError::MultiStatus(xml::multistatus(&responses)))
}

// ---------------------------------------------------------------------------
// REPORT calendar-query
// ---------------------------------------------------------------------------

/// `REPORT calendar-query` on a collection.
///
/// Parses the request body as a CalDAV `calendar-query` (see
/// [`xml::parse_calendar_query`]) and returns a `207 Multi-Status` with one
/// response per matching `.ics` resource, each carrying its `getetag` and the
/// raw `calendar-data`. The body is **not** limited to `time-range`: any
/// recognised `calendar-query` filter is accepted, and an empty body (or one
/// without a `time-range`) selects every resource of the collection.
///
/// # `time-range` matching
///
/// A resource matches when its event span intersects the requested
/// `time-range` (RFC 4791 §9.9). The span is:
///
/// * `[dtstart, dtend)` when the event has a `DTEND` — an ordinary overlap test;
/// * the single instant `dtstart` (an inclusive-left point `[dtstart, dtstart]`)
///   when `DTEND` is absent, because OxiDav stores a zero-duration span for such
///   events; it matches iff `start <= dtstart < end`;
/// * a resource with no denormalised `dtstart` never matches a `time-range`
///   (its temporal position is unknown), but is still returned by a `time-range`
///   -less query.
///
/// `RRULE` is not expanded (documented F07 limitation), so recurring events are
/// matched on their base `DTSTART`/`DTEND` only.
///
/// # Errors
///
/// [`DavError::Forbidden`] for a foreign `{username}`, [`DavError::NotFound`]
/// when the collection does not exist or belongs to another user,
/// [`DavError::BadRequest`] for a malformed query body, a `sync-collection`
/// whose `Depth` is neither absent nor `0` (RFC 6578 §3.2), and
/// [`DavError::Internal`] if the database fails.
async fn report(
    state: &AppState,
    username: &str,
    collection_id: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, DavError> {
    let collection = calendar_collection(state, collection_id, username).await?;

    // `REPORT` dispatches on the body's root element: a `sync-collection`
    // (RFC 6578) is handled by the shared driver, everything else as a
    // `calendar-query`.
    if report_is_sync_collection(body)? {
        // RFC 6578 pins `sync-collection` to `Depth: 0`; reject any other
        // explicit depth instead of silently coercing it.
        validate_sync_depth(headers)?;
        return report_sync_collection(
            &state.db,
            ResourceKind::Calendar,
            username,
            &collection.id,
            body,
            resource_href,
        )
        .await;
    }

    let query = parse_report_body(body)?;

    let objects = list_calendar_objects(&state.db, &collection.id)
        .await
        .map_err(|_| DavError::Internal)?;

    let responses: Vec<DavResponse> = objects
        .iter()
        .filter(|object| matches_query(&query, object))
        .map(|object| resource_response(username, &collection.id, object))
        .collect();

    Err(DavError::MultiStatus(xml::multistatus(&responses)))
}

/// Parse a `REPORT` body as a `calendar-query`.
///
/// An empty body is treated as "no filter" (every resource matches). A non-empty
/// body must be valid UTF-8 XML with no DTD; the parser refuses `<!DOCTYPE ...>`.
fn parse_report_body(body: &[u8]) -> Result<ParseCalendarQuery, DavError> {
    if body.is_empty() {
        return Ok(ParseCalendarQuery::default());
    }
    let text = std::str::from_utf8(body)
        .map_err(|_| DavError::BadRequest("cuerpo no es UTF-8".to_string()))?;
    xml::parse_calendar_query(text)
        .map_err(|_| DavError::BadRequest("cuerpo REPORT inválido".to_string()))
}

/// Whether a stored calendar object satisfies a `calendar-query`.
///
/// A query without a `time-range` matches every object; see [`report`] for the
/// exact temporal-matching rules.
fn matches_query(query: &ParseCalendarQuery, object: &CalendarObject) -> bool {
    let Some(range) = query.time_range else {
        return true;
    };
    let Some(start) = object.dtstart else {
        return false;
    };
    match object.dtend {
        Some(end) => range.matches_event(start, end),
        // No DTEND: a zero-duration instant, matched with an inclusive start.
        None => range.start_ms <= start && start < range.end_ms,
    }
}

// ---------------------------------------------------------------------------
// GET / PUT / DELETE of a calendar resource
// ---------------------------------------------------------------------------

/// `GET` a single `.ics` resource: the stored payload verbatim, its `ETag` and
/// the CalDAV media type. `404` when it (or its collection) is absent or owned
/// by another user.
async fn get_event(
    state: &AppState,
    username: &str,
    collection_id: &str,
    filename: &str,
) -> Result<Response, DavError> {
    let collection = calendar_collection(state, collection_id, username).await?;

    let object = get_calendar_object(&state.db, &collection.id, resource_uid(filename)?)
        .await
        .map_err(|_| DavError::Internal)?
        .ok_or(DavError::NotFound)?;

    let etag = etag_header(&object.etag)?;
    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static(CALENDAR_CONTENT_TYPE),
            ),
            (header::ETAG, etag),
        ],
        object.raw_payload,
    )
        .into_response())
}

/// `PUT` a `.ics` resource: create it (`201`) or update it (`204`).
///
/// The body is parsed with [`ics::parse_ics`]; its `UID` must equal the `{uid}`
/// path segment (else `409 Conflict`). The raw payload is stored verbatim and
/// the denormalised fields (`summary`, `description`, `dtstart`, `dtend`,
/// `rrule`) together with a freshly computed `ETag` are persisted by the F03
/// repositories. The response always carries the new `ETag`.
async fn put_event(
    state: &AppState,
    username: &str,
    collection_id: &str,
    filename: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, DavError> {
    let collection = calendar_collection(state, collection_id, username).await?;

    let raw = std::str::from_utf8(body)
        .map_err(|_| DavError::BadRequest("el cuerpo .ics no es UTF-8".to_string()))?;
    let parsed = ics::parse_ics(raw)
        .map_err(|err| DavError::BadRequest(format!("iCalendar inválido: {err}")))?;

    let route_uid = resource_uid(filename)?;
    if parsed.uid != route_uid {
        return Err(DavError::Conflict(format!(
            "el UID del cuerpo ({}) no coincide con el de la ruta ({route_uid})",
            parsed.uid
        )));
    }

    let existing = get_calendar_object(&state.db, &collection.id, &parsed.uid)
        .await
        .map_err(|_| DavError::Internal)?;

    check_conditionals(
        headers,
        existing.as_ref().map(|object| object.etag.as_str()),
    )?;

    match existing {
        Some(object) => {
            let update = CalendarObjectUpdate {
                summary: parsed.summary,
                description: parsed.description,
                dtstart: Some(parsed.dtstart_ms),
                dtend: parsed.dtend_ms,
                rrule: parsed.rrule,
                raw_payload: raw.to_string(),
            };
            let stored = update_calendar_object(&state.db, &object.id, &update)
                .await
                .map_err(|_| DavError::Internal)?;
            let etag = etag_header(&stored.etag)?;
            Ok((StatusCode::NO_CONTENT, [(header::ETAG, etag)]).into_response())
        }
        None => {
            let new = NewCalendarObject {
                id: new_id(),
                collection_id: collection.id.clone(),
                uid: parsed.uid,
                summary: parsed.summary,
                description: parsed.description,
                dtstart: Some(parsed.dtstart_ms),
                dtend: parsed.dtend_ms,
                rrule: parsed.rrule,
                raw_payload: raw.to_string(),
            };
            let stored = insert_calendar_object(&state.db, &new)
                .await
                .map_err(map_insert_event_error)?;
            let etag = etag_header(&stored.etag)?;
            Ok((StatusCode::CREATED, [(header::ETAG, etag)]).into_response())
        }
    }
}

/// `DELETE` a single `.ics` resource. `404` when it (or its collection) is
/// absent; an unmet `If-Match` / `If-None-Match` yields `412`; success is
/// `204 No Content`.
async fn delete_event(
    state: &AppState,
    username: &str,
    collection_id: &str,
    filename: &str,
    headers: &HeaderMap,
) -> Result<Response, DavError> {
    let collection = calendar_collection(state, collection_id, username).await?;

    let object = get_calendar_object(&state.db, &collection.id, resource_uid(filename)?)
        .await
        .map_err(|_| DavError::Internal)?
        .ok_or(DavError::NotFound)?;

    // Both `If-Match` (strong) and `If-None-Match` (weak) are honoured on
    // `DELETE`, symmetrically with `PUT` (m2/m3).
    check_conditionals(headers, Some(object.etag.as_str()))?;

    delete_calendar_object(&state.db, &object.id)
        .await
        .map_err(|_| DavError::Internal)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Map a failed calendar-object insert to a DAV error.
///
/// A `UNIQUE(collection_id, uid)` clash means a concurrent `PUT` created the
/// resource between our existence check and the insert (TOCTOU, hardening m4):
/// the request conflicts with the current state, so it is a `409 Conflict`
/// rather than a server fault (`500`).
fn map_insert_event_error(err: DbError) -> DavError {
    eprintln!("TEMPDEBUG insert error: {err:?}");
    if err.is_unique_violation() {
        DavError::Conflict("el recurso ya existe en la colección".to_string())
    } else {
        DavError::Internal
    }
}

/// Map a failed collection insert to a DAV error.
///
/// A `UNIQUE` clash on `collections.id` means a `MKCALENDAR` was replayed on an
/// existing path; that is a `409 Conflict`, not a server fault.
fn map_create_collection_error(err: DbError) -> DavError {
    if err.is_unique_violation() {
        DavError::Conflict("ya existe una colección en esa ruta".to_string())
    } else {
        DavError::Internal
    }
}

/// The resource `UID`: the URL filename without its mandatory `.ics` suffix.
///
/// The suffix is required (m7): addressing a resource without it is
/// [`DavError::NotFound`], so `foo` and `foo.ics` can never fold onto the same
/// stored object and the identity stays consistent with the emitted `href`s.
fn resource_uid(filename: &str) -> Result<&str, DavError> {
    filename.strip_suffix(ICS_SUFFIX).ok_or(DavError::NotFound)
}

// ---------------------------------------------------------------------------
// MKCALENDAR
// ---------------------------------------------------------------------------

/// `MKCALENDAR`: create a calendar collection owned by the caller.
///
/// The collection id is the `{collection}` path segment when the verb is issued
/// on a collection path (sanitised, hardening m5); otherwise a fresh UUID is
/// minted. Adopting the requested segment means the `201 Location` points at the
/// **URL the client asked for**, which is immediately addressable, instead of a
/// surrogate UUID the client would have to follow. The display name comes from
/// the `<d:displayname>` in the body, else from the path segment, else a default.
///
/// # Errors
///
/// [`DavError::BadRequest`] for a malformed body or an empty/invalid collection
/// segment, [`DavError::Conflict`] when the addressed collection id already
/// exists, and [`DavError::Internal`] if the database fails.
async fn mkcalendar(
    state: &AppState,
    user: &SessionUser,
    path_segment: Option<String>,
    body: &[u8],
) -> Result<Response, DavError> {
    let parsed = parse_mkcalendar_body(body)?;

    // The path segment (when present) is both the collection id (m5) and the
    // display-name fallback.
    let id = match path_segment.as_deref() {
        Some(segment) => sanitize_collection_id(segment)?,
        None => new_id(),
    };
    let name = resolve_calendar_name(parsed.display_name, path_segment)?;

    let new = NewCollection {
        id,
        user_id: user.sub.clone(),
        name,
        kind: CollectionKind::Calendar,
        color: DEFAULT_COLOR.to_string(),
    };
    let stored = create_collection(&state.db, &new)
        .await
        .map_err(map_create_collection_error)?;

    let location = format!("/dav/calendars/{}/{}/", user.sub, stored.id);
    Ok((StatusCode::CREATED, [(header::LOCATION, location)]).into_response())
}

/// Parse an optional `MKCALENDAR` body.
///
/// An empty body is a valid "create with defaults" request; a non-empty one must
/// be valid UTF-8 XML with no DTD (the parser refuses `<!DOCTYPE ...>`).
fn parse_mkcalendar_body(body: &[u8]) -> Result<ParseMkcalendar, DavError> {
    if body.is_empty() {
        return Ok(ParseMkcalendar::default());
    }
    let text = std::str::from_utf8(body)
        .map_err(|_| DavError::BadRequest("cuerpo no es UTF-8".to_string()))?;
    xml::parse_mkcalendar(text)
        .map_err(|_| DavError::BadRequest("cuerpo MKCALENDAR inválido".to_string()))
}

/// Choose the display name for a new calendar: the body's `displayname` wins,
/// then the collection path segment, then [`DEFAULT_CALENDAR_NAME`].
fn resolve_calendar_name(
    display_name: Option<String>,
    path_segment: Option<String>,
) -> Result<String, DavError> {
    let raw = display_name
        .filter(|name| !name.trim().is_empty())
        .or_else(|| path_segment.filter(|name| !name.trim().is_empty()))
        .unwrap_or_else(|| DEFAULT_CALENDAR_NAME.to_string());
    let name = raw.trim();
    if name.chars().count() > MAX_NAME_LEN {
        return Err(DavError::BadRequest(
            "el nombre del calendario es demasiado largo".to_string(),
        ));
    }
    Ok(name.to_string())
}

// ---------------------------------------------------------------------------
// Responsed building
// ---------------------------------------------------------------------------

/// The canonical href of the calendar-home of `username`.
fn home_href(username: &str) -> String {
    format!("/dav/calendars/{username}/")
}

/// The canonical href of the collection `collection_id` of `username`.
fn collection_href(username: &str, collection_id: &str) -> String {
    format!("/dav/calendars/{username}/{collection_id}/")
}

/// The canonical href of the `.ics` resource named after `uid`.
///
/// The `uid` is percent-encoded as a URL path segment (see [`PATH_SEGMENT`]) so
/// the emitted `href` is a valid URI even when the stored `UID` carries
/// characters such as `#`, `?`, a space or non-ASCII bytes, matching the
/// CardDAV tree.
fn resource_href(username: &str, collection_id: &str, uid: &str) -> String {
    let uid = utf8_percent_encode(uid, PATH_SEGMENT);
    format!("/dav/calendars/{username}/{collection_id}/{uid}{ICS_SUFFIX}")
}

/// The full property set of the calendar-home: a plain DAV collection named
/// after the owner.
fn home_props(username: &str) -> Vec<DavProp> {
    vec![
        collection_resourcetype(),
        text_prop(DavNs::Dav, "displayname", username),
    ]
}

/// The full property set of a calendar collection: `resourcetype`,
/// `displayname`, `getctag`, `sync-token` and `supported-report-set`.
fn collection_props(collection: &Collection) -> Vec<DavProp> {
    vec![
        calendar_resourcetype(),
        text_prop(DavNs::Dav, "displayname", &collection.name),
        text_prop(
            DavNs::Calendarserver,
            "getctag",
            &format!("\"{}\"", collection.sync_token),
        ),
        text_prop(
            DavNs::Dav,
            "sync-token",
            &format!("{SYNC_TOKEN_URI_PREFIX}{}", collection.sync_token),
        ),
        supported_report_set(),
    ]
}

/// The full property set of a calendar object: `getetag`, `getcontenttype` and
/// `calendar-data`.
fn resource_props(object: &CalendarObject) -> Vec<DavProp> {
    vec![
        text_prop(DavNs::Dav, "getetag", &object.etag),
        text_prop(DavNs::Dav, "getcontenttype", CALENDAR_CONTENT_TYPE),
        text_prop(DavNs::CalDav, "calendar-data", &object.raw_payload),
    ]
}

/// A calendar-object response carrying its full property set (used by `REPORT`,
/// which always returns `getetag` and `calendar-data`).
fn resource_response(username: &str, collection_id: &str, object: &CalendarObject) -> DavResponse {
    DavResponse {
        href: resource_href(username, collection_id, &object.uid),
        status: 200,
        props: resource_props(object),
        not_found: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Resource types
// ---------------------------------------------------------------------------

/// `<d:resourcetype><d:collection/><c:calendar/></d:resourcetype>`.
fn calendar_resourcetype() -> DavProp {
    DavProp {
        ns: DavNs::Dav,
        name: "resourcetype".to_string(),
        value: PropValue::Xml("<d:collection/><c:calendar/>".to_string()),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Fetch the collection `collection_id` owned by `user_id` and prove it is a
/// `calendar` (hardening m1).
///
/// Every CalDAV resource/collection handler routes through this helper, so an
/// address book reached through the calendar tree can never be silently treated
/// as a calendar. Both failure modes collapse to [`DavError::NotFound`]:
///
/// * the collection does not exist, or belongs to another subject
///   ([`get_collection`] is scoped by `user_id`, so a foreign id is
///   indistinguishable from a missing one — the same IDOR reasoning documented
///   in the module docs); and
/// * the collection exists but its kind is not `calendar` (e.g. an
///   `addressbook`).
///
/// Returning `404` for the wrong *kind* — rather than `403` / `409` — keeps the
/// answer identical to the missing case, so the calendar tree does not leak the
/// existence of address books.
///
/// # Errors
///
/// [`DavError::NotFound`] for a missing/foreign/non-calendar collection, and
/// [`DavError::Internal`] if the database read fails.
async fn calendar_collection(
    state: &AppState,
    collection_id: &str,
    user_id: &str,
) -> Result<Collection, DavError> {
    let collection = get_collection(&state.db, collection_id, user_id)
        .await
        .map_err(|_| DavError::Internal)?
        .ok_or(DavError::NotFound)?;

    if collection.kind == CollectionKind::Calendar.as_str() {
        Ok(collection)
    } else {
        Err(DavError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::super::common::MAX_ID_LEN;
    use super::*;

    #[test]
    fn depth_defaults_to_one() {
        let headers = HeaderMap::new();
        assert_eq!(requested_depth(&headers), Depth::One);

        let mut zero = HeaderMap::new();
        zero.insert("depth", "0".parse().expect("valid header value"));
        assert_eq!(requested_depth(&zero), Depth::Zero);

        let mut one = HeaderMap::new();
        one.insert("depth", "1".parse().expect("valid header value"));
        assert_eq!(requested_depth(&one), Depth::One);

        let mut infinity = HeaderMap::new();
        infinity.insert("depth", "infinity".parse().expect("valid header value"));
        assert_eq!(requested_depth(&infinity), Depth::One);
    }

    #[test]
    fn resolve_calendar_name_prefers_display_name() {
        assert_eq!(
            resolve_calendar_name(Some("Trabajo".to_string()), Some("seg".to_string())).ok(),
            Some("Trabajo".to_string())
        );
    }

    #[test]
    fn resolve_calendar_name_falls_back_to_segment_then_default() {
        assert_eq!(
            resolve_calendar_name(None, Some("work".to_string())).ok(),
            Some("work".to_string())
        );
        assert_eq!(
            resolve_calendar_name(None, None).ok(),
            Some(DEFAULT_CALENDAR_NAME.to_string())
        );
        // A blank display name is ignored in favour of the segment.
        assert_eq!(
            resolve_calendar_name(Some("   ".to_string()), Some("work".to_string())).ok(),
            Some("work".to_string())
        );
    }

    #[test]
    fn resolve_calendar_name_rejects_overlong_names() {
        let long = "x".repeat(MAX_NAME_LEN + 1);
        assert!(matches!(
            resolve_calendar_name(Some(long), None),
            Err(DavError::BadRequest(_))
        ));
    }

    #[test]
    fn generated_id_is_a_v4_uuid() {
        let id = new_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id.chars().nth(14), Some('4'));
        let variant = id.chars().nth(19).unwrap_or(' ');
        assert!(matches!(variant, '8' | '9' | 'a' | 'b'));
    }

    #[test]
    fn mkcalendar_body_rejects_a_doctype_no_xxe() {
        let xxe = br#"<!DOCTYPE x [<!ENTITY y SYSTEM "file:///etc/passwd">]>
            <c:mkcalendar xmlns:c="urn:ietf:params:xml:ns:caldav"><d:displayname>&y;</d:displayname></c:mkcalendar>"#;
        assert!(matches!(
            parse_mkcalendar_body(xxe),
            Err(DavError::BadRequest(_))
        ));
    }

    #[test]
    fn mkcalendar_body_accepts_an_empty_payload() {
        assert_eq!(
            parse_mkcalendar_body(b"").ok(),
            Some(ParseMkcalendar::default())
        );
    }

    #[test]
    fn sanitize_collection_id_accepts_path_safe_segments() {
        assert_eq!(
            sanitize_collection_id("my-cal").ok(),
            Some("my-cal".to_string())
        );
        assert_eq!(
            sanitize_collection_id("  work_2024  ").ok(),
            Some("work_2024".to_string())
        );
    }

    #[test]
    fn sanitize_collection_id_rejects_empty_and_traversal_segments() {
        for bad in ["", "   ", ".", ".."] {
            assert!(
                matches!(sanitize_collection_id(bad), Err(DavError::BadRequest(_))),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn sanitize_collection_id_rejects_unsafe_characters() {
        for bad in ["a/b", "a b", "a\tb", "caf\u{e9}"] {
            assert!(
                matches!(sanitize_collection_id(bad), Err(DavError::BadRequest(_))),
                "{bad:?} must be rejected"
            );
        }
        let long = "x".repeat(MAX_ID_LEN + 1);
        assert!(matches!(
            sanitize_collection_id(&long),
            Err(DavError::BadRequest(_))
        ));
    }

    #[test]
    fn calendar_resourcetype_marks_a_calendar() {
        let prop = calendar_resourcetype();
        assert_eq!(prop.name, "resourcetype");
        assert_eq!(
            prop.value,
            PropValue::Xml("<d:collection/><c:calendar/>".to_string())
        );
    }
}
