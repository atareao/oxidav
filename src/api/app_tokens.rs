//! F13 — REST handlers for app tokens (`/api/v1/app-tokens`).
//!
//! The SPA manages the revocable tokens that native DAV clients (iOS, DAVx⁵,
//! Thunderbird) present via HTTP Basic Auth. These endpoints expose the token
//! lifecycle as clean JSON; every operation is scoped to the authenticated
//! subject ([`AuthUser::sub`]), so a token owned by another user is treated as
//! non-existent (`404`) rather than access-denied (`403`) — the existence of a
//! foreign token is never leaked (anti-IDOR).
//!
//! # Secret handling
//!
//! The secret is returned **once**, by [`create`], inside the `AppTokenCreated`
//! DTO (`oxidav_<id>_<secret>`). The listing ([`list`]) and renaming ([`rename`])
//! handlers only ever produce [`AppTokenMetadata`], which carries neither the
//! secret nor its `secret_hash` digest. No handler logs the secret.
//!
//! # Persistence
//!
//! Handlers delegate to the F05 repository in [`crate::auth::app_tokens`]:
//! [`create_app_token`], [`list_app_tokens`], [`get_app_token`],
//! [`rename_app_token`] and [`revoke_app_token`]. The hashing/verification logic
//! is never duplicated here.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::auth::oidc::AuthUser;
use crate::auth::{
    create_app_token, get_app_token, list_app_tokens, rename_app_token, revoke_app_token,
};
use crate::router::AppState;

use super::dto::{AppTokenCreated, AppTokenMetadata, CreateAppTokenRequest, RenameAppTokenRequest};
use super::error::ApiError;
use super::parse_json_body;

/// Maximum accepted length of a token label, in characters.
const MAX_TOKEN_NAME_LEN: usize = 255;

/// `GET /api/v1/app-tokens` — list the caller's active app tokens.
///
/// Only **active** (non-revoked) tokens are returned, newest first. Each item is
/// [`AppTokenMetadata`]: `id`, `name`, `created_at` and `last_used_at`. The
/// secret and its digest are never part of the response.
///
/// # Errors
///
/// [`ApiError::Internal`] if the query fails.
pub async fn list(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<Vec<AppTokenMetadata>>, ApiError> {
    let tokens = list_app_tokens(&state.db, &user.sub).await?;
    // Revocation is a logical delete (`revoked_at`): revoked tokens are kept in
    // the table for auditing but are not part of the management view.
    let items = tokens
        .iter()
        .filter(|token| token.revoked_at.is_none())
        .map(AppTokenMetadata::from)
        .collect();
    Ok(Json(items))
}

/// `POST /api/v1/app-tokens` — create an app token for the caller.
///
/// The response is [`AppTokenCreated`]: the **only** place the clear-text secret
/// (`oxidav_<id>_<secret>`) is ever returned. Only the secret's digest is
/// persisted, so it cannot be shown again.
///
/// # Errors
///
/// [`ApiError::Validation`] for a blank or over-long `name`, or a malformed
/// payload; [`ApiError::Internal`] if the token cannot be created.
pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let request: CreateAppTokenRequest = parse_json_body(&body)?;
    let name = validate_name(&request.name)?;

    let (id, token) = create_app_token(&state.db, &user.sub, &name).await?;

    // Re-read the row for its authoritative `created_at`; a missing row right
    // after the insert is a server-side integrity failure.
    let stored = get_app_token(&state.db, &user.sub, &id)
        .await?
        .ok_or_else(|| {
            tracing::error!("app token recién creado no encontrado");
            ApiError::Internal
        })?;

    Ok((
        StatusCode::CREATED,
        Json(AppTokenCreated::new(&stored, token)),
    ))
}

/// `PATCH /api/v1/app-tokens/{id}` — rename one of the caller's tokens.
///
/// # Errors
///
/// [`ApiError::Validation`] for a blank or over-long `name`, or a malformed
/// payload; [`ApiError::NotFound`] when the token does not exist, belongs to
/// another user, or has been revoked; [`ApiError::Internal`] if the update
/// fails.
pub async fn rename(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<AppTokenMetadata>, ApiError> {
    let request: RenameAppTokenRequest = parse_json_body(&body)?;
    let name = validate_name(&request.name)?;

    let updated = rename_app_token(&state.db, &user.sub, &id, &name).await?;
    Ok(Json(AppTokenMetadata::from(&updated)))
}

/// `DELETE /api/v1/app-tokens/{id}` — revoke one of the caller's tokens.
///
/// Revocation is a logical delete (`revoked_at`). A token that does not exist,
/// belongs to another user, or has already been revoked yields `404`, so the
/// operation is not a silent success on unknown ids.
///
/// # Errors
///
/// [`ApiError::NotFound`] when the token does not exist, belongs to another
/// user, or is already revoked; [`ApiError::Internal`] if the update fails.
pub async fn delete_one(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Fetch first so an already-revoked token (the F05 `revoke_app_token` is
    // idempotent) is reported as `404` rather than a repeated `204`. The update
    // below is still scoped by `user_id`, so a foreign id never lands.
    let token = get_app_token(&state.db, &user.sub, &id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if token.revoked_at.is_some() {
        return Err(ApiError::NotFound);
    }

    revoke_app_token(&state.db, &user.sub, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Validate and normalise a token label.
///
/// The label is trimmed, must be non-blank, must not contain control characters
/// (which could smuggle newlines or NULs into logs, headers or UI), and must be
/// bounded in length. Beyond that its content is unrestricted: the value is
/// stored as a bound parameter (no injection surface) and rendered as JSON.
fn validate_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(ApiError::Validation(
            "El nombre no puede estar vacío".to_string(),
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(ApiError::Validation(
            "El nombre no puede contener caracteres de control".to_string(),
        ));
    }
    if name.chars().count() > MAX_TOKEN_NAME_LEN {
        return Err(ApiError::Validation(format!(
            "El nombre no puede superar {MAX_TOKEN_NAME_LEN} caracteres"
        )));
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_name_rejects_blank_and_overlong() {
        assert!(matches!(validate_name("   "), Err(ApiError::Validation(_))));
        let long = "x".repeat(MAX_TOKEN_NAME_LEN + 1);
        assert!(matches!(validate_name(&long), Err(ApiError::Validation(_))));
    }

    #[test]
    fn validate_name_rejects_control_characters() {
        assert!(matches!(
            validate_name("iPho\x00ne"),
            Err(ApiError::Validation(_))
        ));
        assert!(matches!(
            validate_name("line\nbreak"),
            Err(ApiError::Validation(_))
        ));
        assert!(matches!(
            validate_name("tab\tname"),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn validate_name_trims() {
        assert_eq!(validate_name("  iPhone  ").ok(), Some("iPhone".to_string()));
    }
}
