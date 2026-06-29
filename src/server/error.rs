//! Shared HTTP error envelope. Every service's [`IntoResponse`] maps its error
//! variants to a status, then defers here so the JSON shape and the
//! "hide 5xx detail" rule live in exactly one place.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Render `status` + a `{ "error": … }` JSON body. Server (5xx) failures are
/// internal: the caller's `message` is dropped for a generic string so a raw
/// `DbErr` (or any internal detail) never reaches a client; client (4xx) errors
/// surface `message` verbatim.
pub(crate) fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    let body = if status.is_server_error() {
        "internal error".to_string()
    } else {
        message.into()
    };
    (status, Json(json!({ "error": body }))).into_response()
}
