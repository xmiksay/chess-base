//! Shared helpers between the OAuth token/authorize endpoints ([`super::oauth`])
//! and the consent screen ([`super::oauth_consent`]) — kept in one file so
//! neither duplicates the RFC 6749 error envelope, the percent-encoder, or
//! session resolution (ADR-0016, ADR-0044).

use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use sea_orm::DbErr;
use serde_json::json;

use crate::db::entities::oauth_clients;
use crate::server::config::Mode;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

/// Resolve the logged-in user from the request (server-mode session/Bearer;
/// local mode is always the implicit admin). `None` ⇒ anonymous.
pub(super) async fn current_user(state: &AppState, headers: &HeaderMap) -> Option<CurrentUser> {
    match state.mode {
        Mode::Local => Some(CurrentUser::local_admin()),
        Mode::Server => {
            let token = crate::auth::token_from_headers(headers)?;
            crate::auth::AuthService::new(state.db.clone())
                .authenticate(&token)
                .await
                .ok()
        }
    }
}

/// Whether `uri` is one of the client's registered redirect URIs.
pub(super) fn registered_redirect(client: &oauth_clients::Model, uri: &str) -> bool {
    serde_json::from_str::<Vec<String>>(&client.redirect_uris)
        .map(|uris| uris.iter().any(|u| u == uri))
        .unwrap_or(false)
}

/// Percent-encode a string for use in a URL query component (RFC 3986 unreserved
/// set passes through). Dependency-free; used for `code`/`state`/`next`.
pub(super) fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Build the RFC 6749 token response body.
pub(super) fn token_response(
    access: &str,
    refresh: &str,
    expires_in: i64,
    scope: &str,
) -> Response {
    Json(json!({
        "access_token": access,
        "token_type": "Bearer",
        "expires_in": expires_in,
        "refresh_token": refresh,
        "scope": scope,
    }))
    .into_response()
}

/// An OAuth error response (RFC 6749 §5.2). `bad` ⇒ `400` with the error code;
/// a database failure ⇒ `500` without detail.
pub(super) enum OAuthError {
    Bad(&'static str, String),
    Server,
}

impl OAuthError {
    pub(super) fn bad(error: &'static str, description: impl Into<String>) -> Self {
        OAuthError::Bad(error, description.into())
    }
}

impl From<DbErr> for OAuthError {
    fn from(_: DbErr) -> Self {
        OAuthError::Server
    }
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        match self {
            OAuthError::Bad(error, description) => (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": error, "error_description": description })),
            )
                .into_response(),
            OAuthError::Server => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "server_error" })),
            )
                .into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn encode_passes_unreserved_and_escapes_the_rest() {
        assert_eq!(encode("aZ09-_.~"), "aZ09-_.~");
        assert_eq!(encode("a b/c?"), "a%20b%2Fc%3F");
    }

    #[test]
    fn registered_redirect_matches_only_listed_uris() {
        let client = oauth_clients::Model {
            client_id: "c".into(),
            client_name: "n".into(),
            redirect_uris: r#"["https://a/cb","https://b/cb"]"#.into(),
            created_at: Utc::now().naive_utc(),
        };
        assert!(registered_redirect(&client, "https://a/cb"));
        assert!(!registered_redirect(&client, "https://evil/cb"));
    }
}
