//! F08 — CardDAV handlers: `MKCOL`, `PROPFIND`, `GET`, `PUT` and `DELETE`.
//!
//! This module implements the CardDAV tree (RFC 6352):
//!
//! * `MKCOL` on the addressbook-home or on a specific collection path creates an
//!   `addressbook` collection owned by the authenticated subject. Both the
//!   extended body (`<d:mkcol>` with a `resourcetype`) and a body-less request
//!   are accepted and produce the same resource (see [`mkcol_addressbook`]).
//! * `PROPFIND` on the addressbook-home lists the caller's address books, on an
//!   address book lists its `.vcf` resources, and on a resource returns its
//!   `getetag` / `getcontenttype` / `address-data`.
//! * `PUT` on `/{addressbook}/{uid}.vcf` parses the payload, denormalises it into
//!   `card_objects` and stores the raw `.vcf`; a new `UID` yields `201`, an
//!   update of an existing one `204`, and the response always carries the `ETag`.
//! * `GET` returns the stored `.vcf` verbatim with `text/vcard` and its `ETag`;
//!   `DELETE` removes the resource.
//!
//! * `REPORT` on a collection supports `addressbook-query` (a text filter over
//!   the denormalised fields, see [`report`]) and `addressbook-multiget`
//!   (resolution by a list of `<d:href>`), both returning `207 Multi-Status`.
//!
//! `PUT` and `DELETE` honour the HTTP conditionals `If-Match` / `If-None-Match`
//! (`412 Precondition Failed` when unmet), reusing the shared helpers in
//! [`super::common`]. Any other unknown verb is rejected with
//! [`DavError::MethodNotAllowed`].
//!
//! # Collection kind
//!
//! Every resource/collection handler verifies the addressed collection is an
//! `addressbook`. A calendar reached through the card tree is reported as `404
//! Not Found` — the same answer as a missing collection — so calendars can never
//! be silently treated as address books, and their existence is not leaked.
//!
//! # Resource identity
//!
//! The `{uid}` path segment must carry the `.vcf` suffix; a request that
//! addresses a resource without it is `404 Not Found`, keeping the resource
//! identity consistent with the `.vcf` `href`s OxiDav emits.
//!
//! # `UID` vs. URL
//!
//! RFC 6352 requires the `UID` of the stored card to match the resource name in
//! the URL. OxiDav enforces this at the application layer: a `PUT` whose body
//! `UID` differs from the `{uid}` path segment is rejected with `409 Conflict`.
//!
//! # Authorization
//!
//! The `{username}` path segment must equal the authenticated `sub` claim (the
//! app-token owner resolved by the F05 Basic Auth middleware). A mismatch is a
//! `403 Forbidden`; every database read is additionally scoped by the subject,
//! so a foreign collection id is indistinguishable from a missing one (`404`).

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use percent_encoding::{percent_decode_str, utf8_percent_encode};

use crate::auth::oidc::AuthUser;
use crate::db::models::{CardObjectUpdate, CollectionKind, NewCardObject, NewCollection};
use crate::db::{
    CardObject, Collection, DbError, MAX_COLLECTION_LIMIT, ResourceKind, create_collection,
    delete_card_object, get_card_object, get_collection, insert_card_object, list_card_objects,
    list_collections, update_card_object,
};
use crate::router::AppState;

use super::DavError;
use super::common::{
    Depth, MAX_NAME_LEN, PATH_SEGMENT, SYNC_TOKEN_URI_PREFIX, check_conditionals,
    collection_resourcetype, ensure_owner, etag_header, negotiate, new_id, parse_propfind_body,
    report_is_sync_collection, report_sync_collection, requested_depth, sanitize_collection_id,
    supported_report_set, text_prop, validate_sync_depth,
};
use super::vcf;
use super::xml::{
    self, CardDavReport, DavNs, DavProp, DavResponse, ParseAddressbookMultiget,
    ParseAddressbookQuery, ParseMkcalendar, PropValue,
};

/// Accent colour stored for an address book created over DAV (mirrors the F06
/// default and the `collections.color` schema default).
const DEFAULT_COLOR: &str = "#1677ff";

/// Display name used when neither the `MKCOL` body nor the path segment provides
/// one.
const DEFAULT_ADDRESSBOOK_NAME: &str = "Contactos";

/// Suffix a DAV client appends to a resource UID in the URL.
const VCF_SUFFIX: &str = ".vcf";

/// `Content-Type` returned for contact resources.
const VCARD_CONTENT_TYPE: &str = "text/vcard; charset=utf-8";

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handler for `/dav/addressbooks/{username}/` (the addressbook-home).
///
/// Answers `PROPFIND` (list the caller's address books) and `MKCOL` (create a
/// new address book with a server-minted id). Everything else is
/// [`DavError::MethodNotAllowed`].
///
/// # Errors
///
/// [`DavError::Forbidden`] when `{username}` is not the authenticated subject,
/// [`DavError::BadRequest`] for a malformed `MKCOL` body, and
/// [`DavError::Internal`] if the database fails.
pub async fn home(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(username): Path<String>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, DavError> {
    ensure_owner(&username, &user)?;

    match method.as_str() {
        "PROPFIND" => propfind_home(&state, &username, requested_depth(&headers), &body).await,
        "MKCOL" => mkcol_addressbook(&state, &user, None, &body).await,
        _ => Err(DavError::MethodNotAllowed),
    }
}

/// Handler for `/dav/addressbooks/{username}/{addressbook}/`.
///
/// Answers `PROPFIND` (list the address book's resources), `REPORT`
/// (`addressbook-query` / `addressbook-multiget`, see [`report`]) and `MKCOL`
/// (create the address book at this path). Everything else is
/// [`DavError::MethodNotAllowed`].
///
/// # Errors
///
/// [`DavError::Forbidden`] for a foreign `{username}`, [`DavError::NotFound`]
/// when the collection does not exist or belongs to another user,
/// [`DavError::BadRequest`] for a malformed `MKCOL` / `REPORT` body, and
/// [`DavError::Internal`] if the database fails.
pub async fn collection(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
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
        "MKCOL" => mkcol_addressbook(&state, &user, Some(collection_id), &body).await,
        _ => Err(DavError::MethodNotAllowed),
    }
}

/// Handler for `/dav/addressbooks/{username}/{addressbook}/{uid}.vcf`.
///
/// Dispatches on the HTTP verb: `PROPFIND`, `GET`, `PUT` and `DELETE` are
/// answered; everything else is [`DavError::MethodNotAllowed`].
///
/// # Errors
///
/// [`DavError::Forbidden`] for a foreign `{username}`, [`DavError::NotFound`]
/// when the collection or the resource does not exist (or belongs to another
/// user), [`DavError::BadRequest`] for an unparseable `.vcf` body,
/// [`DavError::Conflict`] when the body `UID` differs from the path `{uid}`,
/// [`DavError::PreconditionFailed`] for an unmet `If-Match` / `If-None-Match`,
/// and [`DavError::Internal`] if the database fails.
pub async fn resource(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path((username, collection_id, filename)): Path<(String, String, String)>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, DavError> {
    ensure_owner(&username, &user)?;

    match method.as_str() {
        "PROPFIND" => propfind_resource(&state, &username, &collection_id, &filename, &body).await,
        "GET" => get_card(&state, &username, &collection_id, &filename).await,
        "PUT" => {
            put_card(
                &state,
                &username,
                &collection_id,
                &filename,
                &headers,
                &body,
            )
            .await
        }
        "DELETE" => delete_card(&state, &username, &collection_id, &filename, &headers).await,
        _ => Err(DavError::MethodNotAllowed),
    }
}

// ---------------------------------------------------------------------------
// PROPFIND
// ---------------------------------------------------------------------------

/// `PROPFIND` on the addressbook-home: the home resource itself plus, at depth
/// `1`, one response per address book owned by the caller.
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

    // Only address books are exposed through the CardDAV home; calendars that
    // share the `collections` table are filtered out here. `list_collections`
    // clamps the page size, and a personal server never exceeds the hard
    // ceiling, so a single page covers every address book.
    let collections = list_collections(&state.db, username, MAX_COLLECTION_LIMIT, 0)
        .await
        .map_err(|_| DavError::Internal)?;
    let addressbooks: Vec<&Collection> = collections
        .iter()
        .filter(|c| c.kind == CollectionKind::Addressbook.as_str())
        .collect();

    let mut responses = vec![negotiate(
        home_href(username),
        home_props(username),
        &request,
    )];
    if depth == Depth::One {
        responses.extend(addressbooks.iter().map(|c| {
            negotiate(
                collection_href(username, &c.id),
                collection_props(c),
                &request,
            )
        }));
    }

    Err(DavError::MultiStatus(xml::multistatus(&responses)))
}

/// `PROPFIND` on an address book: the collection resource itself plus, at depth
/// `1`, one response per `.vcf` object with its `getetag`.
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
    // reported as not found); a non-addressbook collection is rejected too.
    let collection = addressbook_collection(state, collection_id, username).await?;

    let mut responses = vec![negotiate(
        collection_href(username, &collection.id),
        collection_props(&collection),
        &request,
    )];
    if depth == Depth::One {
        let objects = list_card_objects(&state.db, &collection.id)
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

/// `PROPFIND` on a single `.vcf` resource; `404` when it (or its collection) is
/// absent or belongs to another user.
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

    let collection = addressbook_collection(state, collection_id, username).await?;

    // The URL must carry `<uid>.vcf`; the stored UID has no suffix.
    let uid = resource_uid(filename)?;
    let object = get_card_object(&state.db, &collection.id, uid)
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
// REPORT addressbook-query / addressbook-multiget
// ---------------------------------------------------------------------------

/// `REPORT` on an address book (RFC 6352 §8.6 / §8.7).
///
/// The request body is parsed by [`parse_report_body`], which dispatches on the
/// root element:
///
/// * **`addressbook-query`** returns a `207 Multi-Status` with one response per
///   card that satisfies the text filter, each carrying its `getetag` and the
///   raw `address-data`. A request with no `text-match` (or an empty body)
///   selects every card of the collection.
/// * **`addressbook-multiget`** resolves each requested `<d:href>` and returns
///   its `address-data` / `getetag`; an `href` that does not name an existing
///   card of the collection is answered with a per-resource `404` `propstat`.
///
/// # `addressbook-query` matching
///
/// OxiDav applies a scalar, case-insensitive `contains` test: every extracted
/// `text-match` term must be a substring of at least one of the denormalised
/// card fields (`fn`, `email`, `phone`, `organization`, `notes`). The
/// `prop-filter` name, `match-type` and `collation` attributes are not
/// interpreted (advanced filters are out of scope for F08).
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
    let collection = addressbook_collection(state, collection_id, username).await?;

    // `REPORT` dispatches on the body's root element: a `sync-collection`
    // (RFC 6578) is handled by the shared driver, everything else as an
    // `addressbook-query` / `addressbook-multiget`.
    if report_is_sync_collection(body)? {
        // RFC 6578 pins `sync-collection` to `Depth: 0`; reject any other
        // explicit depth instead of silently coercing it.
        validate_sync_depth(headers)?;
        return report_sync_collection(
            &state.db,
            ResourceKind::Card,
            username,
            &collection.id,
            body,
            resource_href,
        )
        .await;
    }

    match parse_report_body(body)? {
        CardDavReport::Query(query) => report_query(state, username, &collection.id, &query).await,
        CardDavReport::Multiget(multiget) => {
            report_multiget(state, username, &collection.id, &multiget).await
        }
    }
}

/// `addressbook-query`: one `200` response per matching card.
async fn report_query(
    state: &AppState,
    username: &str,
    collection_id: &str,
    query: &ParseAddressbookQuery,
) -> Result<Response, DavError> {
    let objects = list_card_objects(&state.db, collection_id)
        .await
        .map_err(|_| DavError::Internal)?;

    let responses: Vec<DavResponse> = objects
        .iter()
        .filter(|object| matches_query(query, object))
        .map(|object| resource_response(username, collection_id, object))
        .collect();

    Err(DavError::MultiStatus(xml::multistatus(&responses)))
}

/// `addressbook-multiget`: one response per requested `href`, `404` when it does
/// not name an existing card of the collection.
async fn report_multiget(
    state: &AppState,
    username: &str,
    collection_id: &str,
    multiget: &ParseAddressbookMultiget,
) -> Result<Response, DavError> {
    let mut responses = Vec::with_capacity(multiget.hrefs.len());
    for href in &multiget.hrefs {
        let object = match uid_from_href(href, username, collection_id) {
            Some(uid) => get_card_object(&state.db, collection_id, &uid)
                .await
                .map_err(|_| DavError::Internal)?,
            None => None,
        };
        match object {
            Some(object) => responses.push(resource_response(username, collection_id, &object)),
            None => responses.push(not_found_response(href)),
        }
    }

    Err(DavError::MultiStatus(xml::multistatus(&responses)))
}

/// Parse a `REPORT` body into a [`CardDavReport`].
///
/// An empty body is treated as an `addressbook-query` with no filter (every card
/// matches). A non-empty body must be valid UTF-8 XML with no DTD; the parser
/// refuses `<!DOCTYPE ...>` and any unsupported root element.
fn parse_report_body(body: &[u8]) -> Result<CardDavReport, DavError> {
    if body.is_empty() {
        return Ok(CardDavReport::Query(ParseAddressbookQuery::default()));
    }
    let text = std::str::from_utf8(body)
        .map_err(|_| DavError::BadRequest("cuerpo no es UTF-8".to_string()))?;
    xml::parse_carddav_report(text)
        .map_err(|_| DavError::BadRequest("cuerpo REPORT inválido".to_string()))
}

/// Whether a stored card satisfies an `addressbook-query`.
///
/// A query with no terms matches every card; otherwise every term must be a
/// case-insensitive substring of at least one denormalised field. An empty term
/// (e.g. `<card:text-match/>`) is ignored rather than rejecting the card.
fn matches_query(query: &ParseAddressbookQuery, object: &CardObject) -> bool {
    query
        .terms
        .iter()
        .all(|term| card_matches_term(object, term))
}

/// Whether a single `text-match` term is contained in any denormalised field.
fn card_matches_term(object: &CardObject, term: &str) -> bool {
    let needle = term.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    [
        object.fn_.as_deref(),
        object.email.as_deref(),
        object.phone.as_deref(),
        object.organization.as_deref(),
        object.notes.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|field| field.to_lowercase().contains(&needle))
}

/// The resource `UID` a multiget `href` addresses within
/// `{username}`/`collection_id`, or `None` when the `href` does not name a
/// `.vcf` resource of that collection.
///
/// The `href` is matched by [`strip_prefix`](str::strip_prefix) against the
/// collection's canonical prefix, so an `href` pointing at another user's tree
/// (or another collection) resolves to `None` and is answered with `404` — the
/// same answer as a missing card, so no existence is leaked. Once the filename
/// segment is isolated it is percent-decoded (a `UID` may carry `@`, `#`, `?`,
/// spaces or non-ASCII), so an `href` such as `ada%40oxidav.vcf` resolves to the
/// stored `UID` `ada@oxidav`.
fn uid_from_href(href: &str, username: &str, collection_id: &str) -> Option<String> {
    // The query string / fragment are cut on the still-encoded `href`, so an
    // encoded `#`/`?` inside the `UID` (`%23`/`%3F`) is preserved.
    let path = href.split(['?', '#']).next().unwrap_or(href);
    let prefix = collection_href(username, collection_id);
    let filename = path.strip_prefix(&prefix)?;
    if filename.is_empty() || filename.contains('/') {
        return None;
    }
    let decoded = percent_decode_str(filename).decode_utf8().ok()?;
    decoded.strip_suffix(VCF_SUFFIX).map(str::to_string)
}

/// A per-resource `404` response echoing the requested `href`.
fn not_found_response(href: &str) -> DavResponse {
    DavResponse {
        href: href.to_string(),
        status: 404,
        props: Vec::new(),
        not_found: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// GET / PUT / DELETE of a contact resource
// ---------------------------------------------------------------------------

/// `GET` a single `.vcf` resource: the stored payload verbatim, its `ETag` and
/// the CardDAV media type. `404` when it (or its collection) is absent or owned
/// by another user.
async fn get_card(
    state: &AppState,
    username: &str,
    collection_id: &str,
    filename: &str,
) -> Result<Response, DavError> {
    let collection = addressbook_collection(state, collection_id, username).await?;

    let object = get_card_object(&state.db, &collection.id, resource_uid(filename)?)
        .await
        .map_err(|_| DavError::Internal)?
        .ok_or(DavError::NotFound)?;

    let etag = etag_header(&object.etag)?;
    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static(VCARD_CONTENT_TYPE),
            ),
            (header::ETAG, etag),
        ],
        object.raw_payload,
    )
        .into_response())
}

/// `PUT` a `.vcf` resource: create it (`201`) or update it (`204`).
///
/// The body is parsed with [`vcf::parse_vcf`]; its `UID` must equal the `{uid}`
/// path segment (else `409 Conflict`). The raw payload is stored verbatim and
/// the denormalised fields (`fn`, `email`, `phone`, `organization`, `notes`)
/// together with a freshly computed `ETag` are persisted by the F03
/// repositories. The response always carries the new `ETag`.
async fn put_card(
    state: &AppState,
    username: &str,
    collection_id: &str,
    filename: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, DavError> {
    let collection = addressbook_collection(state, collection_id, username).await?;

    let raw = std::str::from_utf8(body)
        .map_err(|_| DavError::BadRequest("el cuerpo .vcf no es UTF-8".to_string()))?;
    let parsed = vcf::parse_vcf(raw)
        .map_err(|err| DavError::BadRequest(format!("vCard inválida: {err}")))?;

    let route_uid = resource_uid(filename)?;
    if parsed.uid != route_uid {
        return Err(DavError::Conflict(format!(
            "el UID del cuerpo ({}) no coincide con el de la ruta ({route_uid})",
            parsed.uid
        )));
    }

    let existing = get_card_object(&state.db, &collection.id, &parsed.uid)
        .await
        .map_err(|_| DavError::Internal)?;

    check_conditionals(
        headers,
        existing.as_ref().map(|object| object.etag.as_str()),
    )?;

    match existing {
        Some(object) => {
            let update = CardObjectUpdate {
                fn_: Some(parsed.fn_),
                email: parsed.email,
                phone: parsed.phone,
                organization: parsed.organization,
                notes: parsed.notes,
                raw_payload: raw.to_string(),
            };
            let stored = update_card_object(&state.db, &object.id, &update)
                .await
                .map_err(|_| DavError::Internal)?;
            let etag = etag_header(&stored.etag)?;
            Ok((StatusCode::NO_CONTENT, [(header::ETAG, etag)]).into_response())
        }
        None => {
            let new = NewCardObject {
                id: new_id(),
                collection_id: collection.id.clone(),
                uid: parsed.uid,
                fn_: Some(parsed.fn_),
                email: parsed.email,
                phone: parsed.phone,
                organization: parsed.organization,
                notes: parsed.notes,
                raw_payload: raw.to_string(),
            };
            let stored = insert_card_object(&state.db, &new)
                .await
                .map_err(map_insert_card_error)?;
            let etag = etag_header(&stored.etag)?;
            Ok((StatusCode::CREATED, [(header::ETAG, etag)]).into_response())
        }
    }
}

/// `DELETE` a single `.vcf` resource. `404` when it (or its collection) is
/// absent; an unmet `If-Match` / `If-None-Match` yields `412`; success is
/// `204 No Content`.
async fn delete_card(
    state: &AppState,
    username: &str,
    collection_id: &str,
    filename: &str,
    headers: &HeaderMap,
) -> Result<Response, DavError> {
    let collection = addressbook_collection(state, collection_id, username).await?;

    let object = get_card_object(&state.db, &collection.id, resource_uid(filename)?)
        .await
        .map_err(|_| DavError::Internal)?
        .ok_or(DavError::NotFound)?;

    // Both `If-Match` (strong) and `If-None-Match` (weak) are honoured on
    // `DELETE`, symmetrically with `PUT`.
    check_conditionals(headers, Some(object.etag.as_str()))?;

    delete_card_object(&state.db, &object.id)
        .await
        .map_err(|_| DavError::Internal)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Map a failed card-object insert to a DAV error.
///
/// A `UNIQUE(collection_id, uid)` clash means a concurrent `PUT` created the
/// resource between our existence check and the insert (TOCTOU): the request
/// conflicts with the current state, so it is a `409 Conflict` rather than a
/// server fault (`500`).
fn map_insert_card_error(err: DbError) -> DavError {
    if err.is_unique_violation() {
        DavError::Conflict("el recurso ya existe en la colección".to_string())
    } else {
        DavError::Internal
    }
}

/// Map a failed addressbook insert to a DAV error.
///
/// A `UNIQUE` clash on `collections.id` means a `MKCOL` was replayed on an
/// existing path; RFC 6352 reports that as `405 Method Not Allowed`.
fn map_create_addressbook_error(err: DbError) -> DavError {
    if err.is_unique_violation() {
        DavError::MethodNotAllowed
    } else {
        DavError::Internal
    }
}

/// The resource `UID`: the URL filename without its mandatory `.vcf` suffix.
///
/// The suffix is required: addressing a resource without it is
/// [`DavError::NotFound`], so `foo` and `foo.vcf` can never fold onto the same
/// stored object and the identity stays consistent with the emitted `href`s.
fn resource_uid(filename: &str) -> Result<&str, DavError> {
    filename.strip_suffix(VCF_SUFFIX).ok_or(DavError::NotFound)
}

// ---------------------------------------------------------------------------
// MKCOL
// ---------------------------------------------------------------------------

/// `MKCOL`: create an address book collection owned by the caller.
///
/// RFC 6352 §5.1 defines an extended `MKCOL` whose body is a `<d:mkcol>` with a
/// `resourcetype` containing `collection` + `addressbook`. OxiDav **also**
/// accepts a body-less `MKCOL` (a permissive simplified form some clients emit)
/// and produces the very same resource, because the created kind is always
/// `addressbook` regardless of what the body declares. The `displayname`, when
/// the body carries one, is read exactly as for `MKCALENDAR` via the shared,
/// DTD-refusing XML parser.
///
/// The collection id is the `{collection}` path segment when the verb is issued
/// on a collection path (sanitised); otherwise a fresh UUID is minted. Adopting
/// the requested segment means the `201 Location` points at the **URL the client
/// asked for**, which is immediately addressable.
///
/// # Errors
///
/// [`DavError::BadRequest`] for a malformed body or an empty/invalid collection
/// segment, [`DavError::MethodNotAllowed`] when the addressed collection id
/// already exists, and [`DavError::Internal`] if the database fails.
async fn mkcol_addressbook(
    state: &AppState,
    user: &AuthUser,
    path_segment: Option<String>,
    body: &[u8],
) -> Result<Response, DavError> {
    let parsed = parse_mkcol_body(body)?;

    let id = match path_segment.as_deref() {
        Some(segment) => sanitize_collection_id(segment)?,
        None => new_id(),
    };
    let name = resolve_addressbook_name(parsed.display_name, path_segment)?;

    let new = NewCollection {
        id,
        user_id: user.sub.clone(),
        name,
        kind: CollectionKind::Addressbook,
        color: DEFAULT_COLOR.to_string(),
    };
    let stored = create_collection(&state.db, &new)
        .await
        .map_err(map_create_addressbook_error)?;

    let location = format!("/dav/addressbooks/{}/{}/", user.sub, stored.id);
    let location = HeaderValue::from_str(&location).map_err(|_| DavError::Internal)?;
    Ok((StatusCode::CREATED, [(header::LOCATION, location)]).into_response())
}

/// Parse an optional `MKCOL` body.
///
/// An empty body is a valid "create with defaults" request. A non-empty one must
/// be valid UTF-8 XML with no DTD; OxiDav only consumes the `<d:displayname>` of
/// the (extended) `<d:mkcol>` body, reusing the same hardened parser as
/// `MKCALENDAR` (which refuses `<!DOCTYPE ...>`).
fn parse_mkcol_body(body: &[u8]) -> Result<ParseMkcalendar, DavError> {
    if body.is_empty() {
        return Ok(ParseMkcalendar::default());
    }
    let text = std::str::from_utf8(body)
        .map_err(|_| DavError::BadRequest("cuerpo no es UTF-8".to_string()))?;
    xml::parse_mkcalendar(text)
        .map_err(|_| DavError::BadRequest("cuerpo MKCOL inválido".to_string()))
}

/// Choose the display name for a new address book: the body's `displayname`
/// wins, then the collection path segment, then [`DEFAULT_ADDRESSBOOK_NAME`].
fn resolve_addressbook_name(
    display_name: Option<String>,
    path_segment: Option<String>,
) -> Result<String, DavError> {
    let raw = display_name
        .filter(|name| !name.trim().is_empty())
        .or_else(|| path_segment.filter(|name| !name.trim().is_empty()))
        .unwrap_or_else(|| DEFAULT_ADDRESSBOOK_NAME.to_string());
    let name = raw.trim();
    if name.chars().count() > MAX_NAME_LEN {
        return Err(DavError::BadRequest(
            "el nombre de la libreta es demasiado largo".to_string(),
        ));
    }
    Ok(name.to_string())
}

// ---------------------------------------------------------------------------
// Response building
// ---------------------------------------------------------------------------

/// The canonical href of the addressbook-home of `username`.
fn home_href(username: &str) -> String {
    format!("/dav/addressbooks/{username}/")
}

/// The canonical href of the address book `collection_id` of `username`.
fn collection_href(username: &str, collection_id: &str) -> String {
    format!("/dav/addressbooks/{username}/{collection_id}/")
}

/// The canonical href of the `.vcf` resource named after `uid`.
///
/// The `uid` is percent-encoded as a URL path segment (see [`PATH_SEGMENT`]) so
/// the emitted `href` is a valid URI even when the stored `UID` carries
/// characters such as `#`, `?`, a space or non-ASCII bytes.
fn resource_href(username: &str, collection_id: &str, uid: &str) -> String {
    let uid = utf8_percent_encode(uid, PATH_SEGMENT);
    format!("/dav/addressbooks/{username}/{collection_id}/{uid}{VCF_SUFFIX}")
}

/// The full property set of the addressbook-home: a plain DAV collection named
/// after the owner, plus the CardDAV `addressbook-home-set` pointing clients at
/// the address books they own.
fn home_props(username: &str) -> Vec<DavProp> {
    vec![
        collection_resourcetype(),
        text_prop(DavNs::Dav, "displayname", username),
        DavProp {
            ns: DavNs::CardDav,
            name: "addressbook-home-set".to_string(),
            value: PropValue::Xml(format!(
                "<d:href>{}</d:href>",
                xml::escape(&home_href(username))
            )),
        },
    ]
}

/// The full property set of an address book: `resourcetype`, `displayname`,
/// `getctag`, `sync-token` and `supported-report-set`.
fn collection_props(collection: &Collection) -> Vec<DavProp> {
    vec![
        addressbook_resourcetype(),
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

/// The full property set of a card object: `getetag`, `getcontenttype` and
/// `address-data`.
fn resource_props(object: &CardObject) -> Vec<DavProp> {
    vec![
        text_prop(DavNs::Dav, "getetag", &object.etag),
        text_prop(DavNs::Dav, "getcontenttype", VCARD_CONTENT_TYPE),
        text_prop(DavNs::CardDav, "address-data", &object.raw_payload),
    ]
}

/// A card-object response carrying its full property set (used by `REPORT`,
/// which always returns `getetag` and `address-data`).
fn resource_response(username: &str, collection_id: &str, object: &CardObject) -> DavResponse {
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

/// `<d:resourcetype><d:collection/><card:addressbook/></d:resourcetype>`.
fn addressbook_resourcetype() -> DavProp {
    DavProp {
        ns: DavNs::Dav,
        name: "resourcetype".to_string(),
        value: PropValue::Xml("<d:collection/><card:addressbook/>".to_string()),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Fetch the collection `collection_id` owned by `user_id` and prove it is an
/// `addressbook`.
///
/// Every CardDAV resource/collection handler routes through this helper, so a
/// calendar reached through the card tree can never be silently treated as an
/// address book. Both failure modes collapse to [`DavError::NotFound`]:
///
/// * the collection does not exist, or belongs to another subject
///   ([`get_collection`] is scoped by `user_id`, so a foreign id is
///   indistinguishable from a missing one); and
/// * the collection exists but its kind is not `addressbook` (e.g. a
///   `calendar`).
///
/// # Errors
///
/// [`DavError::NotFound`] for a missing/foreign/non-addressbook collection, and
/// [`DavError::Internal`] if the database read fails.
async fn addressbook_collection(
    state: &AppState,
    collection_id: &str,
    user_id: &str,
) -> Result<Collection, DavError> {
    let collection = get_collection(&state.db, collection_id, user_id)
        .await
        .map_err(|_| DavError::Internal)?
        .ok_or(DavError::NotFound)?;

    if collection.kind == CollectionKind::Addressbook.as_str() {
        Ok(collection)
    } else {
        Err(DavError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_addressbook_name_prefers_display_name() {
        assert_eq!(
            resolve_addressbook_name(Some("Amigos".to_string()), Some("friends".to_string())).ok(),
            Some("Amigos".to_string())
        );
    }

    #[test]
    fn resolve_addressbook_name_falls_back_to_segment_then_default() {
        assert_eq!(
            resolve_addressbook_name(None, Some("work".to_string())).ok(),
            Some("work".to_string())
        );
        assert_eq!(
            resolve_addressbook_name(None, None).ok(),
            Some(DEFAULT_ADDRESSBOOK_NAME.to_string())
        );
        assert_eq!(
            resolve_addressbook_name(Some("   ".to_string()), Some("work".to_string())).ok(),
            Some("work".to_string())
        );
    }

    #[test]
    fn resolve_addressbook_name_rejects_overlong_names() {
        let long = "x".repeat(MAX_NAME_LEN + 1);
        assert!(matches!(
            resolve_addressbook_name(Some(long), None),
            Err(DavError::BadRequest(_))
        ));
    }

    #[test]
    fn resource_uid_requires_the_vcf_suffix() {
        assert_eq!(resource_uid("ada.vcf").ok(), Some("ada"));
        assert!(matches!(resource_uid("ada"), Err(DavError::NotFound)));
    }

    #[test]
    fn mkcol_body_accepts_an_empty_payload() {
        assert_eq!(parse_mkcol_body(b"").ok(), Some(ParseMkcalendar::default()));
    }

    #[test]
    fn mkcol_body_rejects_a_doctype_no_xxe() {
        let xxe = br#"<!DOCTYPE x [<!ENTITY y SYSTEM "file:///etc/passwd">]>
            <d:mkcol xmlns:d="DAV:"><d:set><d:prop><d:displayname>&y;</d:displayname></d:prop></d:set></d:mkcol>"#;
        assert!(matches!(
            parse_mkcol_body(xxe),
            Err(DavError::BadRequest(_))
        ));
    }

    #[test]
    fn mkcol_body_reads_the_display_name() {
        let body = br#"<d:mkcol xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
            <d:set><d:prop><d:resourcetype><d:collection/><card:addressbook/></d:resourcetype>
            <d:displayname>Trabajo</d:displayname></d:prop></d:set></d:mkcol>"#;
        assert_eq!(
            parse_mkcol_body(body).ok().and_then(|p| p.display_name),
            Some("Trabajo".to_string())
        );
    }

    #[test]
    fn addressbook_resourcetype_marks_an_addressbook() {
        let prop = addressbook_resourcetype();
        assert_eq!(prop.name, "resourcetype");
        assert_eq!(
            prop.value,
            PropValue::Xml("<d:collection/><card:addressbook/>".to_string())
        );
    }
}
