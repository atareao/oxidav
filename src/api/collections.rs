//! F06 — CRUD handlers for `collections`.
//!
//! Every handler is scoped to the authenticated subject ([`SessionUser::sub`]):
//! a collection owned by another user is treated as non-existent (`404`), which
//! keeps the API free of IDOR leaks. The request bodies are decoded manually
//! from raw bytes so that malformed payloads map to the uniform `400` envelope
//! (the stock `Json` extractor would answer `422` with a plain-text body).
//!
//! Responses use the JSON shapes from [`super::dto`] and the uniform
//! [`super::error::ApiError`] envelope; `raw_payload` / `secret_hash` are never
//! exposed.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use rand::RngCore;
use serde::Deserialize;

use crate::auth::SessionUser;
use crate::db::models::{CollectionKind, NewCollection};
use crate::db::{
    clamp_collection_limit, clamp_offset, count_collections, create_collection, delete_collection,
    get_collection, list_collections, update_collection,
};
use crate::router::AppState;

use super::dto::{
    CollectionDto, CollectionType, CreateCollectionRequest, Page, UpdateCollectionRequest,
};
use super::error::ApiError;
use super::{parse_json_body, parse_optional_i64};

/// Default accent colour stored when the client does not send one.
///
/// Matches the `collections.color` column default in the F03 schema.
const DEFAULT_COLOR: &str = "#1677ff";

/// Maximum accepted length of a collection name, in characters.
const MAX_NAME_LEN: usize = 255;

/// Pagination query parameters for `GET /api/v1/collections`.
///
/// Fields are kept as raw strings and parsed manually so that a non-numeric
/// value (e.g. `?limit=abc`) maps to the uniform `400` [`ApiError::Validation`]
/// envelope instead of axum's plain-text `Query` rejection.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Pagination {
    /// Requested page size, as sent; parsed and clamped before use.
    pub limit: Option<String>,
    /// Requested page offset, as sent; parsed and clamped before use.
    pub offset: Option<String>,
}

impl Pagination {
    /// Parse and clamp the requested page window.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Validation`] when `limit` or `offset` is present but
    /// is not a valid integer.
    fn resolve(&self) -> Result<(i64, i64), ApiError> {
        let limit = parse_optional_i64(self.limit.as_deref(), "limit")?.unwrap_or(0);
        let offset = parse_optional_i64(self.offset.as_deref(), "offset")?.unwrap_or(0);
        Ok((clamp_collection_limit(limit), clamp_offset(offset)))
    }
}

/// `GET /api/v1/collections` — list the caller's collections, paginated.
///
/// # Errors
///
/// Returns [`ApiError::Validation`] when `limit`/`offset` are not integers, and
/// [`ApiError::Internal`] if the database query fails.
pub async fn list(
    State(state): State<AppState>,
    user: SessionUser,
    Query(query): Query<Pagination>,
) -> Result<Json<Page<CollectionDto>>, ApiError> {
    let (limit, offset) = query.resolve()?;

    let total = count_collections(&state.db, &user.sub).await?;
    let rows = list_collections(&state.db, &user.sub, limit, offset).await?;
    let items = rows.iter().map(CollectionDto::from).collect();

    Ok(Json(Page {
        items,
        total,
        limit,
        offset,
    }))
}

/// `POST /api/v1/collections` — create a collection owned by the caller.
///
/// # Errors
///
/// Returns [`ApiError::Validation`] for an empty name, an invalid colour, or a
/// malformed payload, and [`ApiError::Internal`] if the insert fails.
pub async fn create(
    State(state): State<AppState>,
    user: SessionUser,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let request: CreateCollectionRequest = parse_json_body(&body)?;
    let name = validate_name(&request.name)?;
    let color = match request.color {
        Some(value) => validate_color(&value)?,
        None => DEFAULT_COLOR.to_string(),
    };
    let kind = to_kind(request.r#type);

    let new = NewCollection {
        id: new_collection_id(),
        user_id: user.sub,
        name,
        kind,
        color,
    };
    let stored = create_collection(&state.db, &new).await?;

    Ok((StatusCode::CREATED, Json(CollectionDto::from(&stored))))
}

/// `GET /api/v1/collections/{id}` — fetch one of the caller's collections.
///
/// # Errors
///
/// Returns [`ApiError::NotFound`] when the collection does not exist or belongs
/// to another user, and [`ApiError::Internal`] if the query fails.
pub async fn get_one(
    State(state): State<AppState>,
    user: SessionUser,
    Path(id): Path<String>,
) -> Result<Json<CollectionDto>, ApiError> {
    let collection = get_collection(&state.db, &id, &user.sub)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(CollectionDto::from(&collection)))
}

/// `PATCH /api/v1/collections/{id}` — partially update a collection.
///
/// Only `name` and `color` are applied; omitted fields are left untouched, while
/// the immutable `type` field is rejected explicitly. A body that changes
/// **nothing** (neither `name` nor `color`) is a `400`: it prevents a no-op
/// update from silently refreshing `updated_at`.
///
/// # Errors
///
/// Returns [`ApiError::Validation`] for invalid input, a present `type`, or an
/// empty patch; [`ApiError::NotFound`] when the collection does not exist or
/// belongs to another user; and [`ApiError::Internal`] if the update fails.
pub async fn update(
    State(state): State<AppState>,
    user: SessionUser,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<CollectionDto>, ApiError> {
    let request: UpdateCollectionRequest = parse_json_body(&body)?;

    // A collection never changes kind: reject any attempt to set `type` rather
    // than ignoring it silently.
    if request.r#type.is_some() {
        return Err(ApiError::Validation(
            "El campo `type` no se puede modificar".to_string(),
        ));
    }

    // An empty patch would bump `updated_at` without changing anything; refuse it.
    if request.name.is_none() && request.color.is_none() {
        return Err(ApiError::Validation(
            "Debe indicar al menos un campo a actualizar (`name` o `color`)".to_string(),
        ));
    }

    let name = match request.name {
        Some(value) => Some(validate_name(&value)?),
        None => None,
    };
    let color = match request.color {
        Some(value) => Some(validate_color(&value)?),
        None => None,
    };

    let updated = update_collection(&state.db, &id, &user.sub, name.as_deref(), color.as_deref())
        .await?
        .ok_or(ApiError::NotFound)?;

    Ok(Json(CollectionDto::from(&updated)))
}

/// `DELETE /api/v1/collections/{id}` — delete a collection and cascade its
/// objects.
///
/// Deletion is explicit: an unknown or foreign `id` yields `404` rather than a
/// silent `204`, so the SPA can tell "deleted" from "does not exist".
///
/// # Errors
///
/// Returns [`ApiError::NotFound`] when the collection does not exist or belongs
/// to another user, and [`ApiError::Internal`] if the delete fails.
pub async fn delete_one(
    State(state): State<AppState>,
    user: SessionUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Ownership is enforced inside the statement; no check-then-act window.
    if !delete_collection(&state.db, &id, &user.sub).await? {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Validate and normalise a collection name.
fn validate_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(ApiError::Validation(
            "El nombre no puede estar vacío".to_string(),
        ));
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(ApiError::Validation(format!(
            "El nombre no puede superar {MAX_NAME_LEN} caracteres"
        )));
    }
    Ok(name.to_string())
}

/// Validate an accent colour against `^#[0-9a-fA-F]{6}$`.
fn validate_color(raw: &str) -> Result<String, ApiError> {
    let valid =
        raw.len() == 7 && raw.starts_with('#') && raw[1..].chars().all(|c| c.is_ascii_hexdigit());
    if !valid {
        return Err(ApiError::Validation(
            "El color debe tener el formato #RRGGBB".to_string(),
        ));
    }
    Ok(raw.to_string())
}

/// Map the API enum to the persistence kind.
fn to_kind(kind: CollectionType) -> CollectionKind {
    match kind {
        CollectionType::Calendar => CollectionKind::Calendar,
        CollectionType::Addressbook => CollectionKind::Addressbook,
    }
}

/// Generate a fresh RFC 4122 **version 4** UUID string.
///
/// Uses the CSPRNG already available in the dependency graph (`rand`), with no
/// extra crate. The format is the canonical hyphenated one used by
/// `collections.id`.
fn new_collection_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 1
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_name_rejects_blank() {
        assert!(matches!(validate_name("   "), Err(ApiError::Validation(_))));
    }

    #[test]
    fn validate_name_trims() {
        assert_eq!(
            validate_name("  Personal  ").ok(),
            Some("Personal".to_string())
        );
    }

    #[test]
    fn validate_color_accepts_hex_and_rejects_others() {
        assert_eq!(validate_color("#1677FF").ok(), Some("#1677FF".to_string()));
        assert!(validate_color("1677ff").is_err());
        assert!(validate_color("#1677f").is_err());
        assert!(validate_color("#1677gg").is_err());
    }

    #[test]
    fn generated_id_is_a_v4_uuid() {
        let id = new_collection_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id.chars().nth(14), Some('4'));
        let variant = id.chars().nth(19).unwrap_or(' ');
        assert!(matches!(variant, '8' | '9' | 'a' | 'b'));
    }

    #[test]
    fn parse_json_body_maps_bad_type_to_validation() {
        let bad = br#"{"name":"x","type":"task"}"#;
        let result: Result<CreateCollectionRequest, _> = parse_json_body(bad);
        assert!(matches!(result, Err(ApiError::Validation(_))));
    }

    #[test]
    fn pagination_accepts_absent_and_numeric_values() {
        let empty = Pagination::default();
        assert_eq!(empty.resolve().ok(), Some((20, 0)));

        let paged = Pagination {
            limit: Some("5".to_string()),
            offset: Some("3".to_string()),
        };
        assert_eq!(paged.resolve().ok(), Some((5, 3)));
    }

    #[test]
    fn pagination_rejects_non_numeric_values() {
        let bad_limit = Pagination {
            limit: Some("abc".to_string()),
            offset: None,
        };
        assert!(matches!(bad_limit.resolve(), Err(ApiError::Validation(_))));

        let bad_offset = Pagination {
            limit: None,
            offset: Some("x".to_string()),
        };
        assert!(matches!(bad_offset.resolve(), Err(ApiError::Validation(_))));
    }
}
