//! HTTP surface for server-mode auth: `register`, `login`, `logout`. Thin
//! callers of [`AuthService`] that translate JSON ⇄ models and set/clear the
//! browser session cookie. The endpoints are mode-gated: in local mode they
//! answer `400` (there is no login — the single user is the implicit admin).

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::service::{AuthService, AuthServiceError, Authenticated};
use crate::auth::token_from_headers;
use crate::server::config::Mode;
use crate::server::state::AppState;

/// Session lifetime mirrored onto the cookie `Max-Age` (30 days, in seconds).
const SESSION_COOKIE_MAX_AGE: i64 = 30 * 24 * 60 * 60;

/// Auth routes, mounted under the main API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/auth/register", post(register))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .with_state(state)
}

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct UserView {
    id: String,
    is_admin: bool,
}

async fn register(
    State(state): State<AppState>,
    Json(body): Json<Credentials>,
) -> Result<Response, AuthApiError> {
    require_server_mode(&state)?;
    let auth = AuthService::new(state.db.clone());
    let result = auth.register(&body.username, &body.password).await?;
    Ok(session_response(StatusCode::CREATED, result))
}

async fn login(
    State(state): State<AppState>,
    Json(body): Json<Credentials>,
) -> Result<Response, AuthApiError> {
    require_server_mode(&state)?;
    let auth = AuthService::new(state.db.clone());
    let result = auth.login(&body.username, &body.password).await?;
    Ok(session_response(StatusCode::OK, result))
}

async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AuthApiError> {
    require_server_mode(&state)?;
    if let Some(token) = token_from_headers(&headers) {
        AuthService::new(state.db.clone()).logout(&token).await?;
    }
    Ok((
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, clear_cookie())],
    )
        .into_response())
}

/// 200/201 with the issued token in both the JSON body (for Bearer clients) and
/// an `HttpOnly` session cookie (for the browser SPA).
fn session_response(status: StatusCode, auth: Authenticated) -> Response {
    let body = json!({
        "token": auth.token,
        "user": UserView { id: auth.user.id, is_admin: auth.user.is_admin },
    });
    (
        status,
        [(header::SET_COOKIE, session_cookie(&auth.token))],
        Json(body),
    )
        .into_response()
}

fn session_cookie(token: &str) -> String {
    format!("session={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={SESSION_COOKIE_MAX_AGE}")
}

fn clear_cookie() -> String {
    "session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0".to_string()
}

/// Auth endpoints exist only in server mode; reject otherwise.
fn require_server_mode(state: &AppState) -> Result<(), AuthApiError> {
    match state.mode {
        Mode::Server => Ok(()),
        Mode::Local => Err(AuthApiError::Disabled),
    }
}

/// Route-level error: a service failure, or the endpoint being unavailable in
/// local mode. Maps each onto an HTTP status + JSON envelope.
enum AuthApiError {
    Service(AuthServiceError),
    Disabled,
}

impl From<AuthServiceError> for AuthApiError {
    fn from(e: AuthServiceError) -> Self {
        AuthApiError::Service(e)
    }
}

impl IntoResponse for AuthApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AuthApiError::Disabled => (
                StatusCode::BAD_REQUEST,
                "authentication is only available in server mode".to_string(),
            ),
            AuthApiError::Service(e) => {
                let status = match e {
                    AuthServiceError::InvalidInput(_) => StatusCode::BAD_REQUEST,
                    AuthServiceError::UsernameTaken => StatusCode::CONFLICT,
                    AuthServiceError::InvalidCredentials => StatusCode::UNAUTHORIZED,
                    AuthServiceError::Hash | AuthServiceError::Db(_) => {
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                };
                // 5xx details are internal; clients get a generic message.
                let message = match status {
                    StatusCode::INTERNAL_SERVER_ERROR => "internal error".to_string(),
                    _ => e.to_string(),
                };
                (status, message)
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}
