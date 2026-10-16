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
//! per-field length caps and a per-group cap ([`MAX_VALUES`]); parameter
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
use crate::dav::vcf::merge::{
    PropertyEdit, escape_text_value, merge_properties, render_list_property,
    render_structured_property, render_structured_property_preescaped, render_text_property,
    render_uri_property,
};
use crate::dav::vcf::{CardFields, build_vcf, is_forbidden_char, parse_vcf, project_card};
use crate::db::models::{CardObjectUpdate, CollectionKind, NewCardObject};
use crate::db::{
    Collection, DbError, clamp_collection_limit, clamp_offset, delete_card_object, get_card_object,
    get_collection, insert_card_object, insert_card_objects_batch, list_card_objects_window,
    update_card_object,
};
use crate::router::AppState;

use super::dto::{
    AddressInput, ContactDetailDto, ContactDto, CreateContactRequest, GenderInput,
    LabelledValueInput, StructuredNameInput, UpdateContactRequest,
};
use super::error::ApiError;
use super::{parse_json_body, parse_optional_i64};

/// Maximum accepted length of a contact's formatted name (`fn`), in characters.
const MAX_FN_LEN: usize = 255;

/// Maximum accepted length of an email address, in characters.
const MAX_EMAIL_LEN: usize = 320;

/// Maximum accepted length of a telephone number, in characters.
const MAX_PHONE_LEN: usize = 64;

/// Maximum accepted length of an organisation name (`ORG`), in characters.
const MAX_ORGANIZATION_LEN: usize = 255;

/// Maximum accepted length of a client-supplied `uid`, in characters.
const MAX_UID_LEN: usize = 255;

/// Maximum number of values accepted in a single multi-value group (so at most
/// this many `TEL`/`EMAIL`/`ADR`/`NOTE`/… lines per request).
const MAX_VALUES: usize = 32;

/// Maximum accepted length, in characters, of a generic free-text value
/// (nickname, title, role, name component, address component, category, `BDAY`,
/// `ANNIVERSARY`…). Chosen well above a realistic value but far below the
/// pathological 1000-character case the contract rejects.
const MAX_TEXT_LEN: usize = 255;

/// Maximum accepted length, in characters, of a single `NOTE`.
const MAX_NOTE_LEN: usize = 4096;

/// Maximum accepted length, in characters, of a single `URL`.
const MAX_URL_LEN: usize = 2048;

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

/// Validate and normalise a required free-text field (e.g. `fn`).
///
/// The value is trimmed, must be non-blank and bounded, and must be free of
/// forbidden characters (see [`is_forbidden_char`]). Rejecting them up front is
/// a security boundary: [`build_vcf`] escapes the free-text fields, but refusing
/// them keeps an injection attempt a `400` rather than a silently sanitised
/// write.
fn validate_required_text(raw: &str, field: &str, max: usize) -> Result<String, ApiError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(ApiError::Validation(format!(
            "El campo `{field}` no puede estar vacío"
        )));
    }
    if value.chars().count() > max {
        return Err(ApiError::Validation(format!(
            "El campo `{field}` no puede superar {max} caracteres"
        )));
    }
    if value.chars().any(is_forbidden_char) {
        return Err(ApiError::Validation(format!(
            "El campo `{field}` contiene caracteres no permitidos"
        )));
    }
    Ok(value.to_string())
}

/// Normalise an optional free-text field: blank values become `None` (so `""`
/// clears the field), the surviving value is stored **trimmed**, over-long
/// values are rejected, and forbidden characters are refused (see
/// [`is_forbidden_char`]). Trimming keeps the persisted value coherent with
/// [`validate_required_text`].
fn normalize_optional(
    raw: Option<&str>,
    field: &str,
    max: usize,
) -> Result<Option<String>, ApiError> {
    match raw {
        None => Ok(None),
        Some(raw) => {
            let value = raw.trim();
            if value.is_empty() {
                return Ok(None);
            }
            if value.chars().count() > max {
                return Err(ApiError::Validation(format!(
                    "El campo `{field}` no puede superar {max} caracteres"
                )));
            }
            if value.chars().any(is_forbidden_char) {
                return Err(ApiError::Validation(format!(
                    "El campo `{field}` contiene caracteres no permitidos"
                )));
            }
            Ok(Some(value.to_string()))
        }
    }
}

/// Validate a client-supplied `uid`: non-blank, bounded, and free of forbidden
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
    if uid.chars().any(|c| is_forbidden_char(c) || c == '/') {
        return Err(ApiError::Validation(
            "El `uid` contiene caracteres no permitidos".to_string(),
        ));
    }
    Ok(uid.to_string())
}

/// Re-validate a `uid` read back from the database before it is used to compose
/// a `.vcf`.
///
/// A stored `uid` can originate from CardDAV: [`parse_vcf`] keeps the client's
/// `UID` and the F03 repositories persist it without passing [`validate_uid`].
/// Composing a payload from such a value could inject a content line into
/// `raw_payload` (a `UID:` value must never carry CR/LF). A stored `uid` that no
/// longer satisfies [`validate_uid`] — or that requires trimming, i.e. already
/// differs from what `build_vcf` would emit — is a server-side data-integrity
/// failure, not a client error: the handler answers `500`
/// ([`ApiError::Internal`]) and leaves the corrupt row untouched rather than
/// persisting an injected payload.
///
/// # Errors
///
/// [`ApiError::Internal`] when `stored` is blank, over-long, carries a forbidden
/// character (see [`is_forbidden_char`]) or `/`, or is not already trimmed.
fn validate_stored_uid(stored: &str) -> Result<(), ApiError> {
    match validate_uid(stored) {
        Ok(uid) if uid == stored => Ok(()),
        _ => {
            tracing::error!(
                uid = ?stored,
                "uid almacenado inválido; se rechaza recomponer el .vcf"
            );
            Err(ApiError::Internal)
        }
    }
}

/// A normalised view of the writable fields shared by `POST` and `PATCH`.
///
/// Every field is optional: `None` means "absent" (leave the property alone) and
/// `Some` means "present" (replace — or delete, when a slice is empty). The
/// `fn`/`uid` fields are handled separately because their semantics differ
/// between create (required/optional) and patch (immutable `uid`).
struct WritableFields<'a> {
    email: Option<&'a str>,
    emails: Option<&'a [LabelledValueInput]>,
    phone: Option<&'a str>,
    tels: Option<&'a [LabelledValueInput]>,
    organization: Option<&'a str>,
    organizations: Option<&'a [Vec<String>]>,
    name: Option<&'a StructuredNameInput>,
    nicknames: Option<&'a [String]>,
    birthday: Option<&'a str>,
    anniversary: Option<&'a str>,
    gender: Option<&'a GenderInput>,
    impps: Option<&'a [LabelledValueInput]>,
    addresses: Option<&'a [AddressInput]>,
    geos: Option<&'a [String]>,
    timezones: Option<&'a [String]>,
    titles: Option<&'a [String]>,
    roles: Option<&'a [String]>,
    notes: Option<&'a [String]>,
    categories: Option<&'a [Vec<String>]>,
    urls: Option<&'a [String]>,
}

/// Append a [`PropertyEdit`] for `name` (empty `lines` deletes the property).
fn push_edit(edits: &mut Vec<PropertyEdit>, name: &str, lines: Vec<String>) {
    edits.push(PropertyEdit {
        name: name.to_string(),
        lines,
    });
}

/// Build the [`PropertyEdit`] list for every field present in `fields`.
///
/// Validation happens here (reject, never sanitise): control characters,
/// over-long values, too many values and invalid parameters all yield a `400`
/// before any write. A legacy scalar sent next to its array counterpart is also
/// a `400` (no silent precedence).
fn build_edits(fields: &WritableFields<'_>) -> Result<Vec<PropertyEdit>, ApiError> {
    let mut edits = Vec::new();

    // EMAIL: legacy scalar vs array (mutually exclusive).
    match (fields.email, fields.emails) {
        (Some(_), Some(_)) => return Err(scalar_array_conflict("email", "emails")),
        (Some(raw), None) => {
            let value = normalize_optional(Some(raw), "email", MAX_EMAIL_LEN)?;
            push_edit(
                &mut edits,
                "EMAIL",
                scalar_text_lines("EMAIL", value.as_deref())?,
            );
        }
        (None, Some(values)) => {
            push_edit(
                &mut edits,
                "EMAIL",
                render_labelled("EMAIL", values, false)?,
            );
        }
        (None, None) => {}
    }

    // TEL.
    match (fields.phone, fields.tels) {
        (Some(_), Some(_)) => return Err(scalar_array_conflict("phone", "tels")),
        (Some(raw), None) => {
            let value = normalize_optional(Some(raw), "phone", MAX_PHONE_LEN)?;
            push_edit(
                &mut edits,
                "TEL",
                scalar_text_lines("TEL", value.as_deref())?,
            );
        }
        (None, Some(values)) => {
            push_edit(&mut edits, "TEL", render_labelled("TEL", values, false)?);
        }
        (None, None) => {}
    }

    // ORG: legacy scalar vs array (one entry per line).
    match (fields.organization, fields.organizations) {
        (Some(_), Some(_)) => return Err(scalar_array_conflict("organization", "organizations")),
        (Some(raw), None) => {
            let value = normalize_optional(Some(raw), "organization", MAX_ORGANIZATION_LEN)?;
            let lines = match value {
                Some(value) => vec![structured_line("ORG", &[value.as_str()])?],
                None => Vec::new(),
            };
            push_edit(&mut edits, "ORG", lines);
        }
        (None, Some(rows)) => {
            push_edit(
                &mut edits,
                "ORG",
                render_structured_lines("ORG", rows, MAX_TEXT_LEN)?,
            );
        }
        (None, None) => {}
    }

    if let Some(values) = fields.nicknames {
        push_edit(
            &mut edits,
            "NICKNAME",
            render_text_lines("NICKNAME", values, MAX_TEXT_LEN)?,
        );
    }
    if let Some(name) = fields.name {
        push_edit(&mut edits, "N", vec![render_name(name)?]);
    }
    if let Some(value) = fields.birthday {
        push_edit(
            &mut edits,
            "BDAY",
            vec![text_line("BDAY", value, MAX_TEXT_LEN)?],
        );
    }
    if let Some(value) = fields.anniversary {
        push_edit(
            &mut edits,
            "ANNIVERSARY",
            vec![text_line("ANNIVERSARY", value, MAX_TEXT_LEN)?],
        );
    }
    if let Some(gender) = fields.gender {
        push_edit(&mut edits, "GENDER", vec![render_gender(gender)?]);
    }
    if let Some(values) = fields.impps {
        push_edit(&mut edits, "IMPP", render_labelled("IMPP", values, true)?);
    }
    if let Some(values) = fields.addresses {
        push_edit(&mut edits, "ADR", render_addresses(values)?);
    }
    if let Some(values) = fields.geos {
        push_edit(
            &mut edits,
            "GEO",
            render_uri_lines("GEO", values, MAX_TEXT_LEN)?,
        );
    }
    if let Some(values) = fields.timezones {
        push_edit(
            &mut edits,
            "TZ",
            render_uri_lines("TZ", values, MAX_TEXT_LEN)?,
        );
    }
    if let Some(values) = fields.titles {
        push_edit(
            &mut edits,
            "TITLE",
            render_text_lines("TITLE", values, MAX_TEXT_LEN)?,
        );
    }
    if let Some(values) = fields.roles {
        push_edit(
            &mut edits,
            "ROLE",
            render_text_lines("ROLE", values, MAX_TEXT_LEN)?,
        );
    }
    if let Some(values) = fields.notes {
        push_edit(
            &mut edits,
            "NOTE",
            render_text_lines("NOTE", values, MAX_NOTE_LEN)?,
        );
    }
    if let Some(values) = fields.categories {
        push_edit(
            &mut edits,
            "CATEGORIES",
            render_list_lines("CATEGORIES", values, MAX_TEXT_LEN)?,
        );
    }
    if let Some(values) = fields.urls {
        push_edit(
            &mut edits,
            "URL",
            render_uri_lines("URL", values, MAX_URL_LEN)?,
        );
    }

    Ok(edits)
}

/// Render a legacy scalar as a single line, or delete the property when cleared.
fn scalar_text_lines(name: &str, value: Option<&str>) -> Result<Vec<String>, ApiError> {
    match value {
        Some(value) => Ok(vec![text_line(name, value, MAX_TEXT_LEN)?]),
        None => Ok(Vec::new()),
    }
}

/// Render one text line, rejecting forbidden characters and over-long values.
fn text_line(name: &str, value: &str, max: usize) -> Result<String, ApiError> {
    let value = validate_value(value, name, max)?;
    let line = render_text_property(name, &value, &[], None);
    if line.is_empty() {
        return Err(invalid_parameters(name));
    }
    Ok(line)
}

/// Render one URI line, rejecting forbidden characters and over-long values.
fn uri_line(name: &str, value: &str, max: usize) -> Result<String, ApiError> {
    let value = validate_value(value, name, max)?;
    let line = render_uri_property(name, &value, &[], None);
    if line.is_empty() {
        return Err(invalid_parameters(name));
    }
    Ok(line)
}

/// Render a structured line (`N`, `ADR`, `ORG`), issuing no parameters.
fn structured_line(name: &str, components: &[&str]) -> Result<String, ApiError> {
    let components: Vec<String> = components
        .iter()
        .map(|component| validate_value(component, name, MAX_TEXT_LEN))
        .collect::<Result<_, _>>()?;
    let line = render_structured_property(name, &components, &[], None);
    if line.is_empty() {
        return Err(invalid_parameters(name));
    }
    Ok(line)
}

/// Render a slice of text values (one line each), enforcing the group cap.
fn render_text_lines(name: &str, values: &[String], max: usize) -> Result<Vec<String>, ApiError> {
    enforce_count(name, values.len())?;
    values
        .iter()
        .map(|value| text_line(name, value, max))
        .collect()
}

/// Render a slice of URI values (one line each), enforcing the group cap.
fn render_uri_lines(name: &str, values: &[String], max: usize) -> Result<Vec<String>, ApiError> {
    enforce_count(name, values.len())?;
    values
        .iter()
        .map(|value| uri_line(name, value, max))
        .collect()
}

/// Render a slice of structured rows (`ORG`), enforcing the group cap.
fn render_structured_lines(
    name: &str,
    rows: &[Vec<String>],
    max: usize,
) -> Result<Vec<String>, ApiError> {
    enforce_count(name, rows.len())?;
    rows.iter()
        .map(|row| {
            enforce_count(name, row.len())?;
            let components: Vec<String> = row
                .iter()
                .map(|component| validate_value(component, name, max))
                .collect::<Result<_, _>>()?;
            let line = render_structured_property(name, &components, &[], None);
            if line.is_empty() {
                return Err(invalid_parameters(name));
            }
            Ok(line)
        })
        .collect()
}

/// Render a slice of list rows (`CATEGORIES`), enforcing the group cap.
fn render_list_lines(
    name: &str,
    rows: &[Vec<String>],
    max: usize,
) -> Result<Vec<String>, ApiError> {
    enforce_count(name, rows.len())?;
    rows.iter()
        .map(|row| {
            enforce_count(name, row.len())?;
            let values: Vec<String> = row
                .iter()
                .map(|value| validate_value(value, name, max))
                .collect::<Result<_, _>>()?;
            let line = render_list_property(name, &values, &[], None);
            if line.is_empty() {
                return Err(invalid_parameters(name));
            }
            Ok(line)
        })
        .collect()
}

/// Render a slice of labelled values (`TEL`/`EMAIL`/`IMPP`) with `TYPE`/`PREF`.
fn render_labelled(
    name: &str,
    values: &[LabelledValueInput],
    uri: bool,
) -> Result<Vec<String>, ApiError> {
    enforce_count(name, values.len())?;
    values
        .iter()
        .map(|value| {
            let validated = validate_value(&value.value, name, MAX_TEXT_LEN)?;
            let types = value.types.clone().unwrap_or_default();
            let line = if uri {
                render_uri_property(name, &validated, &types, value.pref)
            } else {
                render_text_property(name, &validated, &types, value.pref)
            };
            if line.is_empty() {
                return Err(invalid_parameters(name));
            }
            Ok(line)
        })
        .collect()
}

/// Render the structured `N` from its input.
///
/// RFC 6350 §6.2.2: the components `additional`/`prefix`/`suffix` are
/// **comma-separated lists**, so the comma is a *separator* and must **not** be
/// escaped, while each list item is escaped as text (RFC 6350 §3.4). The
/// family/given components are single values and are escaped whole.
fn render_name(input: &StructuredNameInput) -> Result<String, ApiError> {
    let family = validate_value(&opt_str(&input.family), "name.family", MAX_TEXT_LEN)?;
    let given = validate_value(&opt_str(&input.given), "name.given", MAX_TEXT_LEN)?;
    let components = vec![
        escape_text_value(&family),
        escape_text_value(&given),
        join_name_list(&input.additional, "name.additional")?,
        join_name_list(&input.prefix, "name.prefix")?,
        join_name_list(&input.suffix, "name.suffix")?,
    ];
    let line = render_structured_property_preescaped("N", &components, &[], None);
    if line.is_empty() {
        return Err(invalid_parameters("name"));
    }
    Ok(line)
}

/// Join a comma-separated name sub-list (e.g. `additional`), validating each
/// item.
///
/// Each item is escaped with [`escape_text_value`] and the items are joined with
/// `,` **unescaped**: the comma is the list separator (RFC 6350 §6.2.2), not a
/// value character. Escaping it (as a plain structured component would) would
/// emit `Dr.\,Prof.`, which is not conformant.
fn join_name_list(values: &Option<Vec<String>>, field: &str) -> Result<String, ApiError> {
    let Some(values) = values else {
        return Ok(String::new());
    };
    enforce_count(field, values.len())?;
    let validated: Vec<String> = values
        .iter()
        .map(|value| validate_value(value, field, MAX_TEXT_LEN))
        .collect::<Result<_, _>>()?;
    Ok(validated
        .iter()
        .map(|value| escape_text_value(value))
        .collect::<Vec<_>>()
        .join(","))
}

/// Render `GENDER` (`sex;identity`), rejecting an empty or out-of-range sex.
fn render_gender(input: &GenderInput) -> Result<String, ApiError> {
    let sex = match input.sex.as_deref() {
        Some(sex) => {
            let sex = sex.trim().to_ascii_uppercase();
            if !matches!(sex.as_str(), "M" | "F" | "O" | "N" | "U") {
                return Err(ApiError::Validation(
                    "El sexo de `gender` debe ser M, F, O, N o U".to_string(),
                ));
            }
            sex
        }
        None => String::new(),
    };
    let identity = match input.identity.as_deref() {
        Some(identity) => validate_value(identity, "gender.identity", MAX_TEXT_LEN)?,
        None => String::new(),
    };
    if sex.is_empty() && identity.is_empty() {
        return Err(ApiError::Validation(
            "`gender` no puede estar vacío".to_string(),
        ));
    }
    let mut components: Vec<String> = Vec::new();
    if !sex.is_empty() {
        components.push(sex);
    } else {
        components.push(String::new());
    }
    if !identity.is_empty() {
        components.push(identity);
    }
    let line = render_structured_property("GENDER", &components, &[], None);
    if line.is_empty() {
        return Err(invalid_parameters("gender"));
    }
    Ok(line)
}

/// Render a slice of `ADR` rows, with their `TYPE`/`PREF` and optional `LABEL`.
fn render_addresses(values: &[AddressInput]) -> Result<Vec<String>, ApiError> {
    enforce_count("ADR", values.len())?;
    values
        .iter()
        .map(|input| {
            let components = vec![
                validate_value(&opt_str(&input.po_box), "address.po_box", MAX_TEXT_LEN)?,
                validate_value(&opt_str(&input.extended), "address.extended", MAX_TEXT_LEN)?,
                validate_value(&opt_str(&input.street), "address.street", MAX_TEXT_LEN)?,
                validate_value(&opt_str(&input.locality), "address.locality", MAX_TEXT_LEN)?,
                validate_value(&opt_str(&input.region), "address.region", MAX_TEXT_LEN)?,
                validate_value(
                    &opt_str(&input.postal_code),
                    "address.postal_code",
                    MAX_TEXT_LEN,
                )?,
                validate_value(&opt_str(&input.country), "address.country", MAX_TEXT_LEN)?,
            ];
            let types = input.types.clone().unwrap_or_default();
            let line = render_structured_property("ADR", &components, &types, input.pref);
            if line.is_empty() {
                return Err(invalid_parameters("ADR"));
            }
            match input.label.as_deref() {
                Some(label) => {
                    let label = validate_value(label, "address.label", MAX_TEXT_LEN)?;
                    if label.contains([';', ',', ':', '\\']) {
                        return Err(ApiError::Validation(
                            "El `LABEL` de una dirección no puede contener `;`, `,`, `:` ni `\\`"
                                .to_string(),
                        ));
                    }
                    let (name, rest) = line.split_once(':').unwrap_or((line.as_str(), ""));
                    Ok(format!("{name};LABEL={label}:{rest}"))
                }
                None => Ok(line),
            }
        })
        .collect()
}

/// A `None` component becomes an empty string.
fn opt_str(value: &Option<String>) -> String {
    value.clone().unwrap_or_default()
}

/// A `400` for a group holding more than [`MAX_VALUES`] values.
fn enforce_count(field: &str, count: usize) -> Result<(), ApiError> {
    if count > MAX_VALUES {
        return Err(ApiError::Validation(format!(
            "El campo `{field}` no puede tener más de {MAX_VALUES} valores"
        )));
    }
    Ok(())
}

/// Validate a free-text value: bounded and free of forbidden characters.
fn validate_value(raw: &str, field: &str, max: usize) -> Result<String, ApiError> {
    if raw.chars().count() > max {
        return Err(ApiError::Validation(format!(
            "El campo `{field}` no puede superar {max} caracteres"
        )));
    }
    if raw.chars().any(is_forbidden_char) {
        return Err(ApiError::Validation(format!(
            "El campo `{field}` contiene caracteres no permitidos"
        )));
    }
    Ok(raw.to_string())
}

/// A `400` for a field whose `TYPE`/`PREF` parameters are invalid.
fn invalid_parameters(field: &str) -> ApiError {
    ApiError::Validation(format!("Los parámetros del campo `{field}` no son válidos"))
}

/// A `400` for a legacy scalar sent together with its array counterpart.
fn scalar_array_conflict(scalar: &str, array: &str) -> ApiError {
    ApiError::Validation(format!(
        "No puede usar `{scalar}` y `{array}` en la misma petición"
    ))
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
    fn validate_required_text_rejects_blank_and_trims() {
        assert!(matches!(
            validate_required_text("   ", "fn", MAX_FN_LEN),
            Err(ApiError::Validation(_))
        ));
        assert_eq!(
            validate_required_text("  Ada  ", "fn", MAX_FN_LEN).ok(),
            Some("Ada".to_string())
        );
    }

    #[test]
    fn validate_required_text_rejects_control_characters() {
        assert!(matches!(
            validate_required_text("a\r\nb", "fn", MAX_FN_LEN),
            Err(ApiError::Validation(_))
        ));
        assert!(matches!(
            validate_required_text("a\nb", "fn", MAX_FN_LEN),
            Err(ApiError::Validation(_))
        ));
        assert!(matches!(
            validate_required_text("a\tb", "fn", MAX_FN_LEN),
            Err(ApiError::Validation(_))
        ));
        assert!(matches!(
            validate_required_text("a\u{0007}b", "fn", MAX_FN_LEN),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn validate_required_text_rejects_overlong_values() {
        let long = "x".repeat(MAX_FN_LEN + 1);
        assert!(matches!(
            validate_required_text(&long, "fn", MAX_FN_LEN),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn normalize_optional_drops_blanks_and_trims() {
        assert_eq!(normalize_optional(None, "email", 10).ok(), Some(None));
        assert_eq!(normalize_optional(Some("  "), "email", 10).ok(), Some(None));
        assert_eq!(
            normalize_optional(Some("a@b.com"), "email", 20).ok(),
            Some(Some("a@b.com".to_string()))
        );
        // The stored value is trimmed, coherent with `validate_required_text`.
        assert_eq!(
            normalize_optional(Some("  a@b.com "), "email", 20).ok(),
            Some(Some("a@b.com".to_string()))
        );
    }

    #[test]
    fn normalize_optional_rejects_control_and_overlong_values() {
        assert!(matches!(
            normalize_optional(Some("a\r\nb"), "email", 320),
            Err(ApiError::Validation(_))
        ));
        let long = "x".repeat(11);
        assert!(matches!(
            normalize_optional(Some(&long), "email", 10),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn validate_uid_rejects_blank_and_slashes() {
        assert!(matches!(validate_uid("  "), Err(ApiError::Validation(_))));
        assert!(matches!(validate_uid("a/b"), Err(ApiError::Validation(_))));
        assert!(matches!(validate_uid("a\nb"), Err(ApiError::Validation(_))));
        assert_eq!(validate_uid(" card-1 ").ok(), Some("card-1".to_string()));
    }

    #[test]
    fn validators_reject_unicode_line_separators() {
        // U+2028 (line separator) and U+2029 (paragraph separator) are not
        // `char::is_control`, so they need an explicit refusal in every field.
        assert!(matches!(
            validate_required_text("a\u{2028}b", "fn", MAX_FN_LEN),
            Err(ApiError::Validation(_))
        ));
        assert!(matches!(
            normalize_optional(Some("a\u{2029}b"), "email", MAX_EMAIL_LEN),
            Err(ApiError::Validation(_))
        ));
        assert!(matches!(
            validate_uid("a\u{2028}b"),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn validate_stored_uid_accepts_only_a_canonical_uid() {
        assert!(validate_stored_uid("card-1@oxidav").is_ok());
        // A stored uid carrying a real newline (as CardDAV may persist) is a
        // server-side integrity failure, not a client error.
        assert!(matches!(
            validate_stored_uid("evil\nX-INJECTED:1"),
            Err(ApiError::Internal)
        ));
        assert!(matches!(
            validate_stored_uid("a/b"),
            Err(ApiError::Internal)
        ));
        assert!(matches!(
            validate_stored_uid("  spaced  "),
            Err(ApiError::Internal)
        ));
    }

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

    // -----------------------------------------------------------------------
    // FIX-1 — sub-listas del `N` (separador `,` sin escapar)
    // -----------------------------------------------------------------------

    #[test]
    fn render_name_escapes_list_items_but_not_the_separator() {
        let input = StructuredNameInput {
            family: Some("Lovelace".to_string()),
            given: Some("Ada".to_string()),
            additional: Some(vec!["Byron".to_string(), "King".to_string()]),
            prefix: Some(vec!["Dr.".to_string(), "Prof.".to_string()]),
            suffix: Some(vec!["Jr.".to_string(), "PhD".to_string()]),
        };

        assert_eq!(
            render_name(&input).expect("render"),
            "N:Lovelace;Ada;Byron,King;Dr.,Prof.;Jr.,PhD"
        );
    }

    #[test]
    fn render_name_escapes_each_list_item_inside() {
        // Un carácter reservado dentro de un ítem sí se escapa (`;` -> `\;`),
        // pero el separador `,` de la sub-lista no.
        let input = StructuredNameInput {
            prefix: Some(vec!["Dr.;X".to_string(), "Prof.".to_string()]),
            ..StructuredNameInput::default()
        };

        assert_eq!(render_name(&input).expect("render"), "N:;;;Dr.\\;X,Prof.;");
    }

    #[test]
    fn render_name_sub_lists_round_trip_through_the_parser() {
        let input = StructuredNameInput {
            family: Some("Lovelace".to_string()),
            given: Some("Ada".to_string()),
            additional: Some(vec!["Byron".to_string(), "King".to_string()]),
            prefix: Some(vec!["Dr.".to_string(), "Prof.".to_string()]),
            suffix: Some(vec!["PhD".to_string()]),
        };
        let line = render_name(&input).expect("render");
        let payload = format!(
            "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u@oxidav\r\nFN:Ada\r\n{line}\r\nEND:VCARD\r\n"
        );

        let projection = project_card(&payload).expect("project");
        let name = projection.name.expect("name");
        assert_eq!(name.additional, vec!["Byron", "King"]);
        assert_eq!(name.prefix, vec!["Dr.", "Prof."]);
        assert_eq!(name.suffix, vec!["PhD"]);
    }
}
