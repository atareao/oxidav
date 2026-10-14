//! Uniform JSON error type for the REST API.
//!
//! Every failure serialises to `{"error": {"code": "...", "message": "..."}}`.
//! Internal failures never leak their detail to the caller: the real error is
//! logged with `tracing` and the response carries a generic message.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::auth::TokenError;
use crate::db::DbError;

/// An error that can be returned from an API handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// `400` — the request payload failed validation.
    Validation(String),
    /// `401` — the request was not authenticated.
    Unauthorized,
    /// `404` — the resource does not exist (or belongs to another user).
    NotFound,
    /// `409` — the request conflicts with the current resource state (e.g. an
    /// event `uid` that already exists in the collection).
    Conflict(String),
    /// `500` — an internal failure whose detail is kept server-side only.
    Internal,
}

impl ApiError {
    /// Stable, machine-readable error code exposed in the JSON envelope.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Validation(_) => "validation",
            Self::Unauthorized => "unauthorized",
            Self::NotFound => "not_found",
            Self::Conflict(_) => "conflict",
            Self::Internal => "internal",
        }
    }

    /// HTTP status code associated with this error.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Validation(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Client-facing message. [`ApiError::Internal`] is always generic so
    /// database or infrastructure details never leak.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Validation(msg) => msg.clone(),
            Self::Unauthorized => "Autenticación requerida".to_string(),
            Self::NotFound => "El recurso solicitado no existe".to_string(),
            Self::Conflict(msg) => msg.clone(),
            Self::Internal => "Error interno del servidor".to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = Json(json!({
            "error": {
                "code": self.code(),
                "message": self.message(),
            }
        }));

        // No `WWW-Authenticate` challenge: F17 authenticates with an HttpOnly
        // session cookie, not `Bearer`, so there is no scheme for a client to
        // negotiate. (DAV keeps its own `Basic` challenge, see `auth::basic`.)
        (status, body).into_response()
    }
}

impl From<DbError> for ApiError {
    fn from(err: DbError) -> Self {
        tracing::error!(error = ?err, "error de base de datos en la API");
        Self::Internal
    }
}

impl From<TokenError> for ApiError {
    /// Map an app-token failure to the public envelope.
    ///
    /// [`TokenError::NotFound`] is the only variant with a client-facing meaning
    /// here (an unknown, foreign or revoked token → `404`). Every other variant
    /// signals a server-side problem (a database failure, or a malformed token
    /// that should never have reached this layer), so it is logged and collapsed
    /// to a generic `500` — its detail is never leaked to the caller.
    fn from(err: TokenError) -> Self {
        match err {
            TokenError::NotFound => Self::NotFound,
            other => {
                tracing::error!(error = ?other, "error de app tokens en la API");
                Self::Internal
            }
        }
    }
}
