//! F12 — REST handlers for contacts (`/api/v1/collections/{id}/contacts`).
//!
//! The SPA cannot speak CardDAV, so these endpoints expose the `card_objects`
//! of an `addressbook` collection as clean JSON DTOs. Every handler is scoped to
//! the authenticated subject ([`SessionUser::sub`]): a collection owned by another
//! user — or a collection of the wrong kind (a calendar) — is treated as
//! non-existent (`404`), so no endpoint can leak another user's data (IDOR).
//!
//! # The write model (F22)
//!
//! `raw_payload` is the **single source of truth**. A write never recomposes the
//! `.vcf` from the denormalised columns; instead it does a **surgical merge**
//! ([`crate::dav::vcf::merge::merge_properties`]): only the lines of the
//! properties the client edited are replaced, and everything else — the
//! properties the server does not model, their groups, ordering, parameters and
//! folding — stays **byte for byte** intact.
//!
//! * `create` composes a **skeleton** (`BEGIN` / `VERSION:4.0` / `UID` / `FN` /
//!   `END`) with [`build_vcf`] and applies the merge of every other field.
//! * `update` applies the merge of only the present fields **over the stored
//!   `raw_payload`**.
//!
//! After the merge the resulting payload is re-parsed ([`parse_vcf`]) and the
//! denormalised columns (`fn`, `email`, `phone`, `organization`, `notes`) are
//! **re-derived** from it, so for every stored row the columns equal
//! `parse_vcf(raw_payload)`. UUID minting reuses [`crate::dav::common::new_id`].
//!
//! Presence semantics (decision 7): a property **absent** from the request is
//! untouched; a property **present** replaces **all** its occurrences; an empty
//! array (`[]`) **deletes** it. The four legacy scalars (`fn`, `email`, `phone`,
//! `organization`) keep their old contract (`""` clears, absent leaves alone); a
//! request carrying both a legacy scalar and its array counterpart is a `400`.
//! "Replacing with nothing" also deletes a **structured**/free-text property
//! modelled by an input object: a totally empty `name` (`{}`, or every one of its
//! five components blank) removes `N`, a totally empty `gender`
//! (`{sex: null, identity: null}` or `{}`) removes `GENDER`, and a blank
//! `birthday`/`anniversary` (`""` after trim) removes `BDAY`/`ANNIVERSARY`. A
//! **partially** filled object is a normal replacement (`fn` is mandatory and is
//! never deletable).
//!
//! # Persistence
//!
//! Writes go through the F03 repositories
//! ([`insert_card_object`](crate::db::insert_card_object) /
//! [`update_card_object`](crate::db::update_card_object) /
//! [`delete_card_object`](crate::db::delete_card_object)) and reads through
//! [`list_card_objects_window`](crate::db::list_card_objects_window) /
//! [`get_card_object`](crate::db::get_card_object); they compute the ETag and
//! bump the collection `sync_token`/`sync_log` exactly like CardDAV, so
//! REST-created contacts sync to native clients too. No `raw_payload` is ever
//! exposed.
//!
//! # Validation
//!
//! `fn` must be present and non-blank. Every free-text field is
//! reject-on-control: CR/LF, other control characters and the Unicode line
//! separators U+2028/U+2029 are refused, which prevents vCard
//! content-line/property injection into `raw_payload`. The new fields also have
//! per-field length caps and a per-group cap (`MAX_VALUES`); parameter
//! validation (`TYPE` alphabet, `PREF` range) is delegated to the `render_*`
//! helpers in [`crate::dav::vcf::merge`] — an invalid parameter makes them return
//! an empty string, which this layer turns into a `400`. Failures use the F06
//! [`ApiError`] envelope, and an invalid value is a `400` — never a `500`.
//!
//! A stored `uid` — which may have been persisted verbatim by CardDAV without
//! passing [`validate_uid`] — is re-validated before a `PATCH`; a value that no
//! longer satisfies the invariant is a server-side integrity failure and yields
//! `500` (see [`validate_stored_uid`]). The `UID` is otherwise immutable: a
//! `PATCH` carrying a `uid` is a `400`.
//!
//! # Server-side import
//!
//! `POST .../contacts/import` takes a raw `text/vcard` body (possibly several
//! concatenated cards) and stores **each card verbatim** — exactly what the
//! CardDAV `PUT` already does — reusing [`parse_vcf`] for validation and
//! [`insert_card_object`] for persistence. The body limit for that single route
//! is raised to [`MAX_IMPORT_BODY_BYTES`].

use std::collections::HashSet;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;

use crate::auth::SessionUser;
use crate::dav::common::new_id;
use crate::dav::vcf::merge::{merge_properties, render_text_property};
use crate::dav::vcf::{CardFields, build_vcf, parse_vcf, project_card};
use crate::db::models::{CardObjectUpdate, CollectionKind, NewCardObject};
use crate::db::{
    Collection, DbError, clamp_collection_limit, clamp_offset, delete_card_object, get_card_object,
    get_collection, insert_card_object, insert_card_objects_batch, list_card_objects_window,
    update_card_object,
};
use crate::router::AppState;

use super::dto::{ContactDetailDto, ContactDto, CreateContactRequest, UpdateContactRequest};
use super::error::ApiError;
use super::{parse_json_body, parse_optional_i64};

use self::edits::{WritableFields, build_edits, push_edit};
use self::validate::{MAX_FN_LEN, validate_required_text, validate_stored_uid, validate_uid};

mod edits;
mod validate;

/// Body limit for `POST .../contacts/import`.
///
/// Axum's default (2 MB) is below the multi-card files the SPA imports (the
/// contract exercises a body just over 3 MB). The limit is raised **only** for
/// that one route — never globally — so no other endpoint accepts an oversized
/// body.
pub(crate) const MAX_IMPORT_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Query parameters for `GET /api/v1/collections/{id}/contacts`.
///
/// `limit`/`offset` paginate. Fields are kept as raw strings so a non-numeric
/// value maps to the uniform `400` [`ApiError::Validation`] envelope instead of
/// axum's plain-text `Query` rejection.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ContactListQuery {
    /// Requested page size, as sent; parsed and clamped before use.
    pub limit: Option<String>,
    /// Requested page offset, as sent; parsed and clamped before use.
    pub offset: Option<String>,
}

impl ContactListQuery {
    /// Parse and clamp the page parameters.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Validation`] when `limit`/`offset` is present but is
    /// not a valid integer.
    fn resolve(&self) -> Result<(i64, i64), ApiError> {
        let limit = parse_optional_i64(self.limit.as_deref(), "limit")?.unwrap_or(0);
        let offset = parse_optional_i64(self.offset.as_deref(), "offset")?.unwrap_or(0);
        Ok((clamp_collection_limit(limit), clamp_offset(offset)))
    }
}

/// `GET /api/v1/collections/{id}/contacts` — list an address book's contacts.
///
/// Results are ordered by `uid` and paginated via `limit`/`offset`.
///
/// # Errors
///
/// [`ApiError::Validation`] for a non-numeric page parameter;
/// [`ApiError::NotFound`] when the collection does not exist, belongs to another
/// user, or is not an `addressbook`; [`ApiError::Internal`] if a query fails.
pub async fn list(
    State(state): State<AppState>,
    user: SessionUser,
    Path(id): Path<String>,
    Query(query): Query<ContactListQuery>,
) -> Result<Json<Vec<ContactDto>>, ApiError> {
    let collection = addressbook_collection(&state, &id, &user.sub).await?;
    let (limit, offset) = query.resolve()?;
    let rows = list_card_objects_window(&state.db, &collection.id, limit, offset).await?;
    Ok(Json(rows.iter().map(ContactDto::from).collect()))
}

/// `POST /api/v1/collections/{id}/contacts` — create a contact.
///
/// The skeleton (`.vcf` with `UID`/`FN`) is composed with [`build_vcf`] and the
/// remaining fields are merged into it; the payload is then re-parsed to derive
/// the denormalised columns. A fresh `id` and `uid` are minted unless the client
/// pins a `uid`.
///
/// # Errors
///
/// [`ApiError::Validation`] for a missing/blank `fn`, a control character in any
/// text field, an invalid parameter, an over-long value, too many values, or a
/// legacy scalar sent together with its array counterpart; [`ApiError::NotFound`]
/// for a missing/foreign/non-addressbook collection; [`ApiError::Conflict`] when
/// the (client-supplied) `uid` already exists in the collection;
/// [`ApiError::Internal`] if the insert fails.
pub async fn create(
    State(state): State<AppState>,
    user: SessionUser,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let request: CreateContactRequest = parse_json_body(&body)?;

    let fn_ = validate_required_text(&request.fn_, "fn", MAX_FN_LEN)?;
    let edits = build_edits(&WritableFields {
        email: request.email.as_deref(),
        emails: request.emails.as_deref(),
        phone: request.phone.as_deref(),
        tels: request.tels.as_deref(),
        organization: request.organization.as_deref(),
        organizations: request.organizations.as_deref(),
        name: request.name.as_ref(),
        nicknames: request.nicknames.as_deref(),
        birthday: request.birthday.as_deref(),
        anniversary: request.anniversary.as_deref(),
        gender: request.gender.as_ref(),
        impps: request.impps.as_deref(),
        addresses: request.addresses.as_deref(),
        geos: request.geos.as_deref(),
        timezones: request.timezones.as_deref(),
        titles: request.titles.as_deref(),
        roles: request.roles.as_deref(),
        notes: request.notes.as_deref(),
        categories: request.categories.as_deref(),
        urls: request.urls.as_deref(),
    })?;

    let collection = addressbook_collection(&state, &id, &user.sub).await?;

    let requested_uid = match request.uid.as_deref() {
        Some(raw) => validate_uid(raw)?,
        None => new_id(),
    };

    let skeleton = build_vcf(&CardFields {
        uid: requested_uid,
        fn_,
        email: None,
        phone: None,
        organization: None,
        notes: None,
    });
    let raw_payload = merge_properties(&skeleton, &edits).map_err(merge_failure)?;
    let parsed = parse_created(&raw_payload)?;

    // FIX-3: persist the **parsed** `uid`, not the raw one. `vcard4` unescapes
    // the value (`a\,b` -> `a,b`, `a\Nb` -> a real line feed), so the raw uid
    // could diverge from the payload and leave a row that later makes any
    // `PATCH` fail `validate_stored_uid` with a `500`. Re-validate the parsed
    // value so an invalid result is a `400` before anything is written.
    let uid = validate_uid(&parsed.uid)?;

    let new = NewCardObject {
        id: new_id(),
        collection_id: collection.id.clone(),
        uid,
        fn_: Some(parsed.fn_),
        email: parsed.email,
        phone: parsed.phone,
        organization: parsed.organization,
        notes: parsed.notes,
        raw_payload,
    };

    let stored = insert_card_object(&state.db, &new)
        .await
        .map_err(map_insert_error)?;

    Ok((StatusCode::CREATED, Json(ContactDto::from(&stored))))
}

/// `GET /api/v1/collections/{id}/contacts/{uid}` — fetch one contact.
///
/// Returns the **full** projection of the stored vCard
/// ([`ContactDetailDto`]), derived at read time from `raw_payload` (the single
/// source of truth).
///
/// # Errors
///
/// [`ApiError::NotFound`] when the collection or the contact does not exist (or
/// belongs to another user / is not an address book); [`ApiError::Internal`] if a
/// query fails, or if the stored `raw_payload` cannot be parsed (a server-side
/// integrity failure, mirrored by [`validate_stored_uid`]).
pub async fn get_one(
    State(state): State<AppState>,
    user: SessionUser,
    Path((id, uid)): Path<(String, String)>,
) -> Result<Json<ContactDetailDto>, ApiError> {
    let collection = addressbook_collection(&state, &id, &user.sub).await?;
    let object = get_card_object(&state.db, &collection.id, &uid)
        .await?
        .ok_or(ApiError::NotFound)?;
    let projection = project_card(&object.raw_payload).map_err(|err| {
        tracing::error!(
            error = ?err,
            uid = ?object.uid,
            "raw_payload corrupto; no se puede proyectar la vCard"
        );
        ApiError::Internal
    })?;
    Ok(Json(ContactDetailDto::new(&object, projection)))
}

/// `PATCH /api/v1/collections/{id}/contacts/{uid}` — partially update a contact.
///
/// Only the supplied fields are merged (surgically) into the stored
/// `raw_payload`; everything else stays byte for byte intact. The resulting
/// payload is re-parsed to re-derive the denormalised columns and a fresh ETag is
/// written. A patch with no updatable field is a `400` (it would otherwise
/// silently refresh `updated_at`), and so is a patch carrying a `uid` (the
/// `UID` is immutable — it travels in the URL).
///
/// # Errors
///
/// [`ApiError::Validation`] for invalid input (including a control character), a
/// `uid`, or an empty patch; [`ApiError::NotFound`] when the collection or
/// contact does not exist (or belongs to another user / is not an address book);
/// [`ApiError::Internal`] if the update fails, or if the stored `uid` is itself
/// invalid (see [`validate_stored_uid`]).
pub async fn update(
    State(state): State<AppState>,
    user: SessionUser,
    Path((id, uid)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<ContactDto>, ApiError> {
    let request: UpdateContactRequest = parse_json_body(&body)?;
    if request.uid.is_some() {
        return Err(ApiError::Validation(
            "El `uid` no se puede modificar".to_string(),
        ));
    }
    if request.is_empty() {
        return Err(ApiError::Validation(
            "Debe indicar al menos un campo a actualizar".to_string(),
        ));
    }

    let mut edits = Vec::new();
    if let Some(value) = &request.fn_ {
        let fn_ = validate_required_text(value, "fn", MAX_FN_LEN)?;
        push_edit(
            &mut edits,
            "FN",
            vec![render_text_property("FN", &fn_, &[], None)],
        );
    }
    edits.extend(build_edits(&WritableFields {
        email: request.email.as_deref(),
        emails: request.emails.as_deref(),
        phone: request.phone.as_deref(),
        tels: request.tels.as_deref(),
        organization: request.organization.as_deref(),
        organizations: request.organizations.as_deref(),
        name: request.name.as_ref(),
        nicknames: request.nicknames.as_deref(),
        birthday: request.birthday.as_deref(),
        anniversary: request.anniversary.as_deref(),
        gender: request.gender.as_ref(),
        impps: request.impps.as_deref(),
        addresses: request.addresses.as_deref(),
        geos: request.geos.as_deref(),
        timezones: request.timezones.as_deref(),
        titles: request.titles.as_deref(),
        roles: request.roles.as_deref(),
        notes: request.notes.as_deref(),
        categories: request.categories.as_deref(),
        urls: request.urls.as_deref(),
    })?);

    let collection = addressbook_collection(&state, &id, &user.sub).await?;
    let existing = get_card_object(&state.db, &collection.id, &uid)
        .await?
        .ok_or(ApiError::NotFound)?;

    // The stored `uid` is about to be preserved across a rewrite; it may have
    // arrived through CardDAV without validation, so re-check it (see
    // [`validate_stored_uid`]).
    validate_stored_uid(&existing.uid)?;

    let raw_payload = merge_properties(&existing.raw_payload, &edits).map_err(merge_failure)?;
    let parsed = parse_created(&raw_payload)?;

    let update = CardObjectUpdate {
        fn_: Some(parsed.fn_),
        email: parsed.email,
        phone: parsed.phone,
        organization: parsed.organization,
        notes: parsed.notes,
        raw_payload,
    };

    let stored = update_card_object(&state.db, &existing.id, &update).await?;
    Ok(Json(ContactDto::from(&stored)))
}

/// `POST /api/v1/collections/{id}/contacts/import` — import a raw `.vcf` file.
///
/// The body is a `text/vcard` file that may hold **several** concatenated cards.
/// Each card is validated with [`parse_vcf`] and stored **verbatim** (byte for
/// byte, exactly as the CardDAV `PUT` does), reusing [`insert_card_object`] for
/// persistence. This is the same code path CardDAV uses; the HTTP layer only adds
/// the multi-card split and the raised [`MAX_IMPORT_BODY_BYTES`] limit.
///
/// # Errors
///
/// [`ApiError::Validation`] when the body is not UTF-8, holds no usable card, or
/// any card fails to parse; [`ApiError::NotFound`] for a missing/foreign/
/// non-addressbook collection; [`ApiError::Conflict`] when a card's `uid`
/// already exists; [`ApiError::Internal`] if an insert fails.
pub async fn import(
    State(state): State<AppState>,
    user: SessionUser,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let collection = addressbook_collection(&state, &id, &user.sub).await?;

    let raw = std::str::from_utf8(&body)
        .map_err(|_| ApiError::Validation("el cuerpo .vcf no es UTF-8".to_string()))?;

    let cards = split_cards(raw);
    if cards.is_empty() {
        return Err(ApiError::Validation(
            "El cuerpo no contiene ninguna vCard válida".to_string(),
        ));
    }

    // Validate every card up front (parse + uid), so a malformed file or an
    // invalid uid is a `400` before any row is written.
    let mut parsed_cards = Vec::with_capacity(cards.len());
    for card in &cards {
        let parsed = parse_vcf(card).map_err(|_| {
            ApiError::Validation("El cuerpo contiene una vCard inválida".to_string())
        })?;
        // FIX-3: the stored `uid` is the **parsed** (unescaped) value; reject a
        // card whose parsed uid is not a valid stored uid (e.g. a `UID:a\Nb`
        // that unescapes to a real line feed) before anything is written.
        validate_uid(&parsed.uid)?;
        parsed_cards.push((card.to_string(), parsed));
    }

    // FIX-2: pre-validate duplicates — inside the file and against the store —
    // so a clash is a `409` with **zero** rows written. The batch insert below
    // is a single transaction, so the whole import is all-or-nothing.
    let mut seen: HashSet<&str> = HashSet::with_capacity(parsed_cards.len());
    for (_, parsed) in &parsed_cards {
        if !seen.insert(parsed.uid.as_str()) {
            return Err(ApiError::Conflict(
                "El fichero contiene uids duplicados".to_string(),
            ));
        }
    }
    for (_, parsed) in &parsed_cards {
        if get_card_object(&state.db, &collection.id, &parsed.uid)
            .await?
            .is_some()
        {
            return Err(ApiError::Conflict(
                "Ya existe un contacto con ese uid en la colección".to_string(),
            ));
        }
    }

    let new_cards: Vec<NewCardObject> = parsed_cards
        .into_iter()
        .map(|(raw_card, parsed)| NewCardObject {
            id: new_id(),
            collection_id: collection.id.clone(),
            uid: parsed.uid,
            fn_: Some(parsed.fn_),
            email: parsed.email,
            phone: parsed.phone,
            organization: parsed.organization,
            notes: parsed.notes,
            raw_payload: raw_card,
        })
        .collect();

    let stored = insert_card_objects_batch(&state.db, &new_cards)
        .await
        .map_err(map_insert_error)?;

    Ok((
        StatusCode::CREATED,
        Json(stored.iter().map(ContactDto::from).collect::<Vec<_>>()),
    ))
}

/// Split a raw `.vcf` file into its individual card payloads, **verbatim**
/// (byte ranges, folding and line terminators included).
///
/// A card starts at a `BEGIN:VCARD` line and ends at the matching `END:VCARD`
/// line (case-insensitive, mirroring [`crate::dav::vcf`]); a folded continuation
/// line (starting with a space or tab) never opens or closes a card. Text
/// outside any card is ignored, so a header/blank padding does not derail the
/// split.
fn split_cards(raw: &str) -> Vec<&str> {
    let mut cards = Vec::new();
    let mut start: Option<usize> = None;
    let mut offset = 0usize;
    for physical in raw.split_inclusive('\n') {
        let body = physical
            .strip_suffix("\r\n")
            .or_else(|| physical.strip_suffix('\n'))
            .unwrap_or(physical);
        let is_continuation = body.starts_with(' ') || body.starts_with('\t');
        if !is_continuation {
            if start.is_none() && body.trim().eq_ignore_ascii_case("BEGIN:VCARD") {
                start = Some(offset);
            } else if start.is_some() && body.trim().eq_ignore_ascii_case("END:VCARD") {
                let end = offset + physical.len();
                if let Some(begin) = start.take() {
                    cards.push(&raw[begin..end]);
                }
            }
        }
        offset += physical.len();
    }
    cards
}

/// `DELETE /api/v1/collections/{id}/contacts/{uid}` — delete a contact.
///
/// Deletion is explicit: an unknown/foreign `uid` yields `404` rather than a
/// silent `204`, so the SPA can distinguish "deleted" from "does not exist".
///
/// # Errors
///
/// [`ApiError::NotFound`] when the collection or contact does not exist (or
/// belongs to another user / is not an address book); [`ApiError::Internal`] if
/// the delete fails.
pub async fn delete_one(
    State(state): State<AppState>,
    user: SessionUser,
    Path((id, uid)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let collection = addressbook_collection(&state, &id, &user.sub).await?;
    let object = get_card_object(&state.db, &collection.id, &uid)
        .await?
        .ok_or(ApiError::NotFound)?;
    delete_card_object(&state.db, &object.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Fetch `id` scoped to `user_sub` and prove it is an `addressbook`.
///
/// Both failure modes collapse to [`ApiError::NotFound`]: the collection does
/// not exist or belongs to another subject ([`get_collection`] is scoped by
/// `user_id`), or it exists but its kind is not `addressbook`. Returning `404`
/// for the wrong kind keeps the answer identical to the missing case, so a
/// calendar's existence is never leaked.
///
/// # Errors
///
/// [`ApiError::NotFound`] for a missing/foreign/non-addressbook collection, and
/// [`ApiError::Internal`] if the database read fails.
async fn addressbook_collection(
    state: &AppState,
    id: &str,
    user_sub: &str,
) -> Result<Collection, ApiError> {
    let collection = get_collection(&state.db, id, user_sub)
        .await?
        .ok_or(ApiError::NotFound)?;
    if collection.kind == CollectionKind::Addressbook.as_str() {
        Ok(collection)
    } else {
        Err(ApiError::NotFound)
    }
}

/// Map a merge failure to `500`: the skeleton/payload is always a valid card, so
/// a merge failure means server-side corruption, never bad client input.
fn merge_failure(err: crate::dav::vcf::merge::MergeError) -> ApiError {
    tracing::error!(error = ?err, "el merge de propiedades no pudo aplicarse");
    ApiError::Internal
}

/// Re-parse the resulting payload and derive the denormalised columns.
///
/// The payload was just produced by the merge, so a parse failure is a
/// server-side integrity failure (a `500`), not a client error.
fn parse_created(raw_payload: &str) -> Result<crate::dav::vcf::ParsedCard, ApiError> {
    parse_vcf(raw_payload).map_err(|err| {
        tracing::error!(
            error = ?err,
            "el payload compuesto no parsea; no se pueden derivar las columnas"
        );
        ApiError::Internal
    })
}

/// Map a failed card-object insert to an [`ApiError`].
///
/// A `UNIQUE(collection_id, uid)` clash means the requested `uid` already exists
/// (or a concurrent `POST` won the race): a `409 Conflict`. Any other failure is
/// logged and surfaced as a generic [`ApiError::Internal`].
fn map_insert_error(err: DbError) -> ApiError {
    if err.is_unique_violation() {
        ApiError::Conflict("Ya existe un contacto con ese uid en la colección".to_string())
    } else {
        ApiError::from(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_list_query_clamps_page_window() {
        let q = ContactListQuery {
            limit: Some("100000".to_string()),
            offset: Some("-3".to_string()),
        };
        assert_eq!(q.resolve().ok(), Some((200, 0)));

        let bad = ContactListQuery {
            limit: Some("abc".to_string()),
            offset: None,
        };
        assert!(matches!(bad.resolve(), Err(ApiError::Validation(_))));
    }

    #[test]
    fn empty_patch_is_detected() {
        let empty = UpdateContactRequest::default();
        assert!(empty.is_empty());

        let non_empty = UpdateContactRequest {
            fn_: Some("x".to_string()),
            ..UpdateContactRequest::default()
        };
        assert!(!non_empty.is_empty());

        // Every new field counts as "updatable" too.
        let with_note = UpdateContactRequest {
            notes: Some(vec!["n".to_string()]),
            ..UpdateContactRequest::default()
        };
        assert!(!with_note.is_empty());
    }
}
