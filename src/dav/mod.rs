//! F07 — CalDAV/WebDAV surface mounted under `/dav`.
//!
//! This module owns the DAV router and the shared DAV error type. Phase A wires
//! only the `OPTIONS /dav/` capability probe; the CalDAV handlers
//! (`MKCALENDAR`, `PROPFIND`, `REPORT`, `PUT`, `DELETE`, `GET`) land in phase B
//! on top of the [`xml`] and [`ics`] helpers that already live here.
//!
//! The whole subtree is protected by F05 Basic Auth: [`crate::router::build_router`]
//! wraps the router returned by [`router`] with
//! [`crate::auth::basic::require_basic_auth`], so a request without a valid app
//! token never reaches these handlers.

pub mod caldav;
pub mod carddav;
pub(crate) mod common;
pub mod ics;
pub mod vcf;
pub mod xml;

use axum::Router;
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, options};
use serde_json::json;

use crate::router::AppState;

/// Value advertised in the `DAV` response header: WebDAV class 1 and 2 plus the
/// CalDAV `calendar-access` (RFC 4791 §5.1) and CardDAV `addressbook` (RFC 6352
/// §6.1) compliance classes.
const DAV_HEADER: &str = "1, 2, calendar-access, addressbook";

/// Methods OxiDav answers under `/dav`, advertised via `Allow`.
const ALLOW_HEADER: &str = "OPTIONS, GET, HEAD, PUT, DELETE, PROPFIND, REPORT, MKCALENDAR, MKCOL";

/// Errors returned by DAV handlers.
///
/// Variants map to the HTTP statuses required by the DAV contract (RFC 4918 /
/// RFC 4791). [`DavError::MultiStatus`] is special: it carries an already
/// serialised `207 Multi-Status` body (built with [`xml::multistatus`]) and is
/// returned verbatim rather than as a JSON error envelope, because a `207` is a
/// *successful* response that may legitimately embed per-resource errors.
#[derive(Debug, thiserror::Error)]
pub enum DavError {
    /// `400` — the request body or parameters failed validation.
    #[error("petición DAV inválida: {0}")]
    BadRequest(String),
    /// `403` — the resource exists conceptually but is not the caller's.
    #[error("acceso denegado al recurso DAV")]
    Forbidden,
    /// `404` — the resource does not exist (or is invisible to the caller).
    #[error("recurso DAV no encontrado")]
    NotFound,
    /// `405` — the HTTP verb is not allowed on this resource.
    #[error("método DAV no permitido")]
    MethodNotAllowed,
    /// `409` — the request conflicts with the current resource state (e.g. the
    /// `UID` in the body does not match the one addressed by the URL).
    #[error("conflicto en el recurso DAV: {0}")]
    Conflict(String),
    /// `412` — an `If-Match` / `If-None-Match` precondition was not met.
    #[error("precondición DAV fallida")]
    PreconditionFailed,
    /// `403` — a `sync-collection` `sync-token` was malformed or unknown.
    ///
    /// RFC 6578 §3.2 defines this as the `valid-sync-token` precondition error.
    /// OxiDav answers it with `403 Forbidden` (the token is a pre-state
    /// precondition, not a write conflict) via the shared
    /// [`common::report_sync_collection`](super::common::report_sync_collection).
    #[error("sync-token DAV inválido")]
    SyncTokenInvalid,
    /// `415` — the request `Content-Type` is not supported.
    #[error("tipo de medio no soportado")]
    UnsupportedMediaType,
    /// `500` — an internal failure whose detail is kept server-side only.
    #[error("error interno del servidor DAV")]
    Internal,
    /// `207` — a fully serialised `Multi-Status` document.
    #[error("respuesta multistatus DAV")]
    MultiStatus(String),
}

impl DavError {
    /// HTTP status associated with this error.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::PreconditionFailed => StatusCode::PRECONDITION_FAILED,
            Self::SyncTokenInvalid => StatusCode::FORBIDDEN,
            Self::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            Self::MultiStatus(_) => StatusCode::MULTI_STATUS,
        }
    }

    /// Client-facing message for the JSON envelope.
    ///
    /// [`DavError::BadRequest`] echoes the (already-sanitised) reason supplied by
    /// the handler; every other variant returns a fixed message so internal
    /// detail never leaks.
    #[must_use]
    pub fn client_message(&self) -> String {
        match self {
            Self::BadRequest(reason) => reason.clone(),
            Self::Forbidden => "Acceso denegado".to_string(),
            Self::NotFound => "El recurso DAV no existe".to_string(),
            Self::MethodNotAllowed => "Método DAV no permitido en este recurso".to_string(),
            Self::Conflict(reason) => reason.clone(),
            Self::PreconditionFailed => "La precondición HTTP no se cumple".to_string(),
            Self::SyncTokenInvalid => "El sync-token no es válido".to_string(),
            Self::UnsupportedMediaType => "Tipo de contenido no soportado".to_string(),
            Self::Internal => "Error interno del servidor".to_string(),
            Self::MultiStatus(_) => "Multi-Status".to_string(),
        }
    }
}

impl IntoResponse for DavError {
    fn into_response(self) -> Response {
        // A `207` is a successful response whose body is the multistatus XML;
        // return it verbatim with the CalDAV media type.
        if let Self::MultiStatus(body) = self {
            return (
                StatusCode::MULTI_STATUS,
                [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
                body,
            )
                .into_response();
        }

        // RFC 6578 §3.2: an unrecognised `sync-token` is the `valid-sync-token`
        // precondition. Strict DAV clients expect the `DAV:`-namespaced `error`
        // body, so this variant is serialised as XML (still `403 Forbidden`)
        // instead of the JSON envelope used by the other errors.
        if let Self::SyncTokenInvalid = self {
            tracing::debug!(error = %self, "rechazando petición DAV: valid-sync-token");
            return (
                StatusCode::FORBIDDEN,
                [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
                xml::valid_sync_token_error(),
            )
                .into_response();
        }

        let status = self.status();
        if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = %self, "error en una petición DAV");
        } else {
            tracing::debug!(error = %self, "rechazando petición DAV");
        }

        let body = json!({ "ok": false, "error": self.client_message() });
        (status, axum::Json(body)).into_response()
    }
}

/// Build the `/dav` router.
///
/// Exposes the `OPTIONS /dav/` capability probe plus the CalDAV tree under
/// `/dav/calendars/{username}/…`. The CalDAV routes are registered with
/// [`any`] rather than a method filter because `MKCALENDAR` / `PROPFIND` are
/// WebDAV/CalDAV verbs that axum's [`axum::routing::MethodFilter`] does not know;
/// the handlers therefore receive the raw [`axum::http::Method`] and dispatch on
/// it, returning [`DavError::MethodNotAllowed`] for anything they do not answer.
///
/// The returned router is expected to be nested under `/dav` and wrapped with
/// `require_basic_auth` in [`crate::router::build_router`], so every handler can
/// rely on an [`crate::auth::oidc::AuthUser`] being present in the request
/// extensions.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", options(options_handler))
        .route("/calendars/{username}/", any(caldav::home))
        .route(
            "/calendars/{username}/{collection}/",
            any(caldav::collection),
        )
        .route(
            "/calendars/{username}/{collection}/{uid}",
            any(caldav::resource),
        )
        .route("/addressbooks/{username}/", any(carddav::home))
        .route(
            "/addressbooks/{username}/{collection}/",
            any(carddav::collection),
        )
        .route(
            "/addressbooks/{username}/{collection}/{uid}",
            any(carddav::resource),
        )
}

/// `OPTIONS /dav/` — advertise DAV compliance classes and the allowed methods.
///
/// Reaching this handler already implies a valid Basic app token (the router is
/// wrapped by `require_basic_auth`); it exists so native clients can discover
/// the server capabilities before issuing DAV verbs.
async fn options_handler() -> Response {
    (
        StatusCode::OK,
        [
            (
                HeaderName::from_static("dav"),
                HeaderValue::from_static(DAV_HEADER),
            ),
            (header::ALLOW, HeaderValue::from_static(ALLOW_HEADER)),
        ],
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_match_the_dav_contract() {
        assert_eq!(
            DavError::BadRequest("x".into()).status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(DavError::Forbidden.status(), StatusCode::FORBIDDEN);
        assert_eq!(DavError::NotFound.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            DavError::PreconditionFailed.status(),
            StatusCode::PRECONDITION_FAILED
        );
        assert_eq!(
            DavError::UnsupportedMediaType.status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        assert_eq!(
            DavError::Internal.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            DavError::MultiStatus(String::new()).status(),
            StatusCode::MULTI_STATUS
        );
    }

    #[test]
    fn internal_errors_do_not_leak_detail() {
        // The JSON envelope must never echo the internal cause.
        let message = DavError::Internal.client_message();
        assert!(
            !message.contains("error interno del servidor DAV"),
            "{message}"
        );
        assert_eq!(message, "Error interno del servidor");
    }

    #[test]
    fn multistatus_is_returned_as_xml() {
        let body = "<d:multistatus xmlns:d=\"DAV:\"/>".to_string();
        let response = DavError::MultiStatus(body.clone()).into_response();
        assert_eq!(response.status(), StatusCode::MULTI_STATUS);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(
            content_type.starts_with("application/xml"),
            "{content_type}"
        );
    }

    #[tokio::test]
    async fn sync_token_invalid_is_a_dav_xml_precondition() {
        let response = DavError::SyncTokenInvalid.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            content_type.starts_with("application/xml"),
            "{content_type}"
        );

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let body = String::from_utf8(body.to_vec()).expect("UTF-8 body");
        assert!(body.contains("<d:valid-sync-token/>"), "{body}");
        assert!(body.contains("xmlns:d=\"DAV:\""), "{body}");
    }
}
