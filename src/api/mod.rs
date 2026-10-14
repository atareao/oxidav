//! F06 — REST API (`/api/v1`).
//!
//! Builds the JSON API consumed by the React SPA. The router produced here is
//! mounted under `/api/v1` and wrapped by the F17 `session_middleware` in
//! [`crate::router::build_router`], so every endpoint requires a valid session
//! cookie (or runs as the dev user when auth is disabled).
//!
//! * [`dto`] holds the public request/response shapes (never `raw_payload`).
//! * [`error`] holds the uniform [`error::ApiError`] envelope.
//! * [`collections`] holds the CRUD handlers for `/collections`.
//! * [`events`] holds the F11 event handlers for `/collections/{id}/events`.
//! * [`contacts`] holds the F12 contact handlers for `/collections/{id}/contacts`.
//! * [`search`] holds the F10 search handlers for `/search`.
//! * [`app_tokens`] holds the F13 app-token handlers for `/app-tokens`.

pub mod app_tokens;
pub mod collections;
pub mod contacts;
pub mod dto;
pub mod error;
pub mod events;
pub mod search;

use axum::routing::{get, patch};
use axum::{Json, Router};

use crate::auth::SessionUser;
use crate::router::AppState;

use self::dto::MeDto;
use self::error::ApiError;

/// Decode a JSON request body, mapping any (de)serialisation failure to a
/// uniform `400` [`ApiError::Validation`].
///
/// Shared by every API handler so a malformed payload — including an unknown
/// enum discriminant or a missing required field — always answers the F06
/// envelope, rather than axum's plain-text `422` `Json` rejection.
pub(crate) fn parse_json_body<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(body)
        .map_err(|_| ApiError::Validation("Cuerpo de la petición inválido".to_string()))
}

/// Parse an optional query value into an `i64`.
///
/// Query fields are decoded as raw strings and parsed here so a non-numeric
/// value (e.g. `?limit=abc`) maps to the uniform `400` [`ApiError::Validation`]
/// envelope instead of axum's plain-text `Query` rejection. Shared by the
/// collections and events list handlers.
pub(crate) fn parse_optional_i64(raw: Option<&str>, field: &str) -> Result<Option<i64>, ApiError> {
    match raw {
        None => Ok(None),
        Some(value) => value.trim().parse::<i64>().map(Some).map_err(|_| {
            ApiError::Validation(format!("El parámetro `{field}` debe ser un entero"))
        }),
    }
}

/// Build the `/api/v1` router.
///
/// The returned router is intended to be nested under `/api/v1`; the F17
/// `session_middleware` injects the verified [`SessionUser`] into the request
/// extensions that [`me`] reads.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me", get(me))
        .route(
            "/collections",
            get(collections::list).post(collections::create),
        )
        .route(
            "/collections/{id}",
            get(collections::get_one)
                .patch(collections::update)
                .delete(collections::delete_one),
        )
        .route(
            "/collections/{id}/events",
            get(events::list).post(events::create),
        )
        .route(
            "/collections/{id}/events/{uid}",
            get(events::get_one)
                .patch(events::update)
                .delete(events::delete_one),
        )
        .route(
            "/collections/{id}/contacts",
            get(contacts::list).post(contacts::create),
        )
        .route(
            "/collections/{id}/contacts/{uid}",
            get(contacts::get_one)
                .patch(contacts::update)
                .delete(contacts::delete_one),
        )
        .route("/search", get(search::aggregate))
        .route("/search/events", get(search::events))
        .route("/search/contacts", get(search::contacts))
        .route(
            "/app-tokens",
            get(app_tokens::list).post(app_tokens::create),
        )
        .route(
            "/app-tokens/{id}",
            patch(app_tokens::rename).delete(app_tokens::delete_one),
        )
}

/// `GET /api/v1/me` — echo the authenticated subject.
///
/// Demonstrates the authentication middleware end to end: reaching this handler
/// guarantees a valid session (or the dev user when auth is disabled).
async fn me(user: SessionUser) -> Json<MeDto> {
    Json(MeDto { sub: user.sub })
}
