//! F12 — REST handlers for contacts (`/api/v1/collections/{id}/contacts`).
//!
//! The SPA cannot speak CardDAV, so these endpoints expose the `card_objects`
//! of an `addressbook` collection as clean JSON DTOs. Every handler is scoped to
//! the authenticated subject ([`AuthUser::sub`]): a collection owned by another
//! user — or a collection of the wrong kind (a calendar) — is treated as
//! non-existent (`404`), so no endpoint can leak another user's data (IDOR).
//!
//! # `.vcf` composition (single home)
//!
//! F08 CardDAV *parses* `.vcf` on `PUT` ([`crate::dav::vcf::parse_vcf`]) and
//! stores the client-provided payload verbatim. F12 instead **composes** the
//! `.vcf` from JSON fields. To avoid a second, drifting vCard implementation in
//! the HTTP layer, composition lives next to the parser in the shared CardDAV
//! module ([`crate::dav::vcf::build_vcf`]): the DAV tree and the REST API share
//! one home for vCard (de)serialisation, and the composed payload is guaranteed
//! to round-trip through [`parse_vcf`](crate::dav::vcf::parse_vcf). UUID minting
//! likewise reuses [`crate::dav::common::new_id`] rather than adding another copy.
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
//! `fn` must be present and non-blank. Every free-text field (`fn`, `email`,
//! `phone`, `organization`) is reject-on-control: CR/LF, other control
//! characters and the Unicode line separators U+2028/U+2029 are refused, which
//! prevents vCard content-line/property injection into the composed
//! `raw_payload`. Blank `email`/`phone`/`organization` values are normalised
//! away (so `""` clears an optional field on `PATCH`) and the surviving value is
//! stored trimmed. Failures use the F06 [`ApiError`] envelope, and an invalid
//! value is a `400` — never a `500`.
//!
//! A stored `uid` — which may have been persisted verbatim by CardDAV without
//! passing [`validate_uid`] — is re-validated before it is used to recompose a
//! `.vcf` on `PATCH`; a value that no longer satisfies the invariant is a
//! server-side integrity failure and yields `500` (see [`validate_stored_uid`]).
//! [`build_vcf`] additionally strips line-breaking characters from the `UID`
//! itself, so no route can inject a content line through a stored `uid`.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;

use crate::auth::oidc::AuthUser;
use crate::dav::common::new_id;
use crate::dav::vcf::{CardFields, build_vcf};
use crate::db::models::{CardObjectUpdate, CollectionKind, NewCardObject};
use crate::db::{
    Collection, DbError, clamp_collection_limit, clamp_offset, delete_card_object, get_card_object,
    get_collection, insert_card_object, list_card_objects_window, update_card_object,
};
use crate::router::AppState;

use super::dto::{ContactDto, CreateContactRequest, UpdateContactRequest};
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
    Extension(user): Extension<AuthUser>,
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
/// The `.vcf` is composed from the JSON fields and stored in `raw_payload`; a
/// fresh `id` and `uid` are minted unless the client pins a `uid`.
///
/// # Errors
///
/// [`ApiError::Validation`] for a missing/blank `fn`, a control character in any
/// text field, or a malformed payload; [`ApiError::NotFound`] for a
/// missing/foreign/non-addressbook collection; [`ApiError::Conflict`] when the
/// (client-supplied) `uid` already exists in the collection;
/// [`ApiError::Internal`] if the insert fails.
pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let request: CreateContactRequest = parse_json_body(&body)?;

    let fn_ = validate_required_text(&request.fn_, "fn", MAX_FN_LEN)?;
    let email = normalize_optional(request.email.as_deref(), "email", MAX_EMAIL_LEN)?;
    let phone = normalize_optional(request.phone.as_deref(), "phone", MAX_PHONE_LEN)?;
    let organization = normalize_optional(
        request.organization.as_deref(),
        "organization",
        MAX_ORGANIZATION_LEN,
    )?;

    let collection = addressbook_collection(&state, &id, &user.sub).await?;

    let uid = match request.uid.as_deref() {
        Some(raw) => validate_uid(raw)?,
        None => new_id(),
    };

    let fields = CardFields {
        uid: uid.clone(),
        fn_: fn_.clone(),
        email: email.clone(),
        phone: phone.clone(),
        organization: organization.clone(),
        notes: None,
    };

    let new = NewCardObject {
        id: new_id(),
        collection_id: collection.id.clone(),
        uid,
        fn_: Some(fn_),
        email,
        phone,
        organization,
        notes: None,
        raw_payload: build_vcf(&fields),
    };

    let stored = insert_card_object(&state.db, &new)
        .await
        .map_err(map_insert_error)?;

    Ok((StatusCode::CREATED, Json(ContactDto::from(&stored))))
}

/// `GET /api/v1/collections/{id}/contacts/{uid}` — fetch one contact.
///
/// # Errors
///
/// [`ApiError::NotFound`] when the collection or the contact does not exist (or
/// belongs to another user / is not an address book); [`ApiError::Internal`] if a
/// query fails.
pub async fn get_one(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path((id, uid)): Path<(String, String)>,
) -> Result<Json<ContactDto>, ApiError> {
    let collection = addressbook_collection(&state, &id, &user.sub).await?;
    let object = get_card_object(&state.db, &collection.id, &uid)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(ContactDto::from(&object)))
}

/// `PATCH /api/v1/collections/{id}/contacts/{uid}` — partially update a contact.
///
/// Only the supplied fields are applied; the `.vcf` is recomposed from the merged
/// values and a fresh ETag is written. A patch with no updatable field is a `400`
/// (it would otherwise silently refresh `updated_at`).
///
/// # Errors
///
/// [`ApiError::Validation`] for invalid input (including a control character) or
/// an empty patch; [`ApiError::NotFound`] when the collection or contact does not
/// exist (or belongs to another user / is not an address book);
/// [`ApiError::Internal`] if the update fails, or if the stored `uid` is itself
/// invalid and cannot be safely recomposed (see [`validate_stored_uid`]).
pub async fn update(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path((id, uid)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<ContactDto>, ApiError> {
    let request: UpdateContactRequest = parse_json_body(&body)?;
    if request.is_empty() {
        return Err(ApiError::Validation(
            "Debe indicar al menos un campo a actualizar".to_string(),
        ));
    }

    let collection = addressbook_collection(&state, &id, &user.sub).await?;
    let existing = get_card_object(&state.db, &collection.id, &uid)
        .await?
        .ok_or(ApiError::NotFound)?;

    // The stored `uid` is about to be written back into `raw_payload`; it may
    // have arrived through CardDAV without validation, so re-check it before
    // composing (see [`validate_stored_uid`]).
    validate_stored_uid(&existing.uid)?;

    let fn_ = match &request.fn_ {
        Some(value) => Some(validate_required_text(value, "fn", MAX_FN_LEN)?),
        None => existing.fn_.clone(),
    };
    let email = match &request.email {
        Some(value) => normalize_optional(Some(value.as_str()), "email", MAX_EMAIL_LEN)?,
        None => existing.email.clone(),
    };
    let phone = match &request.phone {
        Some(value) => normalize_optional(Some(value.as_str()), "phone", MAX_PHONE_LEN)?,
        None => existing.phone.clone(),
    };
    let organization = match &request.organization {
        Some(value) => {
            normalize_optional(Some(value.as_str()), "organization", MAX_ORGANIZATION_LEN)?
        }
        None => existing.organization.clone(),
    };

    let fields = CardFields {
        uid: existing.uid.clone(),
        // A contact always carries an `FN` (it is required on create); fall back
        // to an empty string only for a legacy row that lacks the column.
        fn_: fn_.clone().unwrap_or_default(),
        email: email.clone(),
        phone: phone.clone(),
        organization: organization.clone(),
        notes: existing.notes.clone(),
    };

    let update = CardObjectUpdate {
        fn_,
        email,
        phone,
        organization,
        notes: existing.notes.clone(),
        raw_payload: build_vcf(&fields),
    };

    let stored = update_card_object(&state.db, &existing.id, &update).await?;
    Ok(Json(ContactDto::from(&stored)))
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
    Extension(user): Extension<AuthUser>,
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

/// Whether a character is forbidden in any free-text field or `uid`.
///
/// Covers C0/C1 control characters (including `\r`, `\n`, `\t`) and the Unicode
/// line/paragraph separators U+2028/U+2029, which some vCard parsers treat as
/// line breaks. Rejecting them up front is the security boundary against
/// content-line/property injection into the composed `raw_payload`.
fn is_forbidden_char(c: char) -> bool {
    c.is_control() || c == '\u{2028}' || c == '\u{2029}'
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
        let empty = UpdateContactRequest {
            fn_: None,
            email: None,
            phone: None,
            organization: None,
        };
        assert!(empty.is_empty());
        let non_empty = UpdateContactRequest {
            fn_: Some("x".to_string()),
            email: None,
            phone: None,
            organization: None,
        };
        assert!(!non_empty.is_empty());
    }
}
