//! A real `.pgn` file download (issue #120): the export routes for games and
//! studies both return their PGN as an attachment — `application/x-chess-pgn`
//! with a `Content-Disposition` header — so the browser saves a file instead of
//! the SPA having to turn a JSON `{pgn}` field into one.

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};

/// Build a `200 OK` PGN attachment response with the given download filename.
pub(crate) fn pgn_attachment(filename: &str, body: String) -> Response {
    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                "application/x-chess-pgn; charset=utf-8".to_string(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body,
    )
        .into_response()
}
