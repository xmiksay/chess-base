//! OAuth 2.1 authorization server + resource metadata for the `/mcp` endpoint
//! (ADR-0016). Public-client, PKCE-only: claude.ai self-onboards via dynamic
//! client registration (RFC 7591), runs the authorization-code grant against the
//! logged-in user's session, and refreshes with the refresh-token grant. The
//! resolved access token is what [`authenticate_mcp`] checks on every call.
//!
//! Consent is implicit: a logged-in user hitting `/oauth/authorize` is taken to
//! have approved (single-tenant, self-hosted). An anonymous request is bounced to
//! the SPA login carrying its original URL in `next`.
//!
//! [`authenticate_mcp`]: crate::server::auth::authenticate_mcp

use axum::{
    extract::{Form, OriginalUri, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, Set,
};
use serde::Deserialize;
use serde_json::json;

use crate::db::entities::{oauth_clients, oauth_codes, oauth_tokens};
use crate::server::auth::{base_url, issue_oauth_tokens, new_token, verify_pkce_s256};
use crate::server::config::Mode;
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

/// Authorization-code lifetime — short, single-use.
const CODE_TTL_SECS: i64 = 600;
/// Scope advertised + granted. Authorization is by resource ownership, not by
/// granular OAuth scopes, so a single coarse scope suffices.
const SCOPE: &str = "chess";

/// Mount the well-known metadata + OAuth endpoints.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server_metadata),
        )
        .route("/oauth/register", post(register_client))
        .route("/oauth/authorize", get(authorize))
        .route("/oauth/token", post(token))
        .with_state(state)
}

// --- Discovery metadata --------------------------------------------------

/// RFC 9728 protected-resource metadata: marks `/mcp` as the resource and points
/// at this server as its authorization server.
async fn protected_resource_metadata(headers: HeaderMap) -> Json<serde_json::Value> {
    let base = base_url(&headers);
    Json(json!({
        "resource": format!("{base}/mcp"),
        "authorization_servers": [base],
        "bearer_methods_supported": ["header"],
        "scopes_supported": [SCOPE],
    }))
}

/// RFC 8414 authorization-server metadata: the endpoint URLs and the grants /
/// PKCE method this server supports.
async fn authorization_server_metadata(headers: HeaderMap) -> Json<serde_json::Value> {
    let base = base_url(&headers);
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "registration_endpoint": format!("{base}/oauth/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": [SCOPE],
    }))
}

// --- Dynamic client registration (RFC 7591) ------------------------------

#[derive(Deserialize)]
struct RegisterRequest {
    #[serde(default)]
    redirect_uris: Vec<String>,
    client_name: Option<String>,
}

/// POST /oauth/register — register a public client with its redirect URIs.
async fn register_client(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<Response, OAuthError> {
    if req.redirect_uris.is_empty() {
        return Err(OAuthError::bad(
            "invalid_redirect_uri",
            "redirect_uris is required",
        ));
    }

    let client_id = new_token();
    let client_name = req.client_name.unwrap_or_else(|| "mcp-client".to_string());
    let redirect_uris = serde_json::to_string(&req.redirect_uris)
        .map_err(|_| OAuthError::bad("invalid_redirect_uri", "could not encode redirect_uris"))?;

    oauth_clients::ActiveModel {
        client_id: Set(client_id.clone()),
        client_name: Set(client_name.clone()),
        redirect_uris: Set(redirect_uris),
        created_at: Set(Utc::now().naive_utc()),
    }
    .insert(&state.db)
    .await?;

    let body = json!({
        "client_id": client_id,
        "client_name": client_name,
        "redirect_uris": req.redirect_uris,
        "token_endpoint_auth_method": "none",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
    });
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

// --- Authorization-code grant: authorize --------------------------------

#[derive(Deserialize)]
struct AuthorizeQuery {
    response_type: Option<String>,
    client_id: Option<String>,
    redirect_uri: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    scope: Option<String>,
    state: Option<String>,
}

/// GET /oauth/authorize — validate the request, ensure the user is logged in
/// (else bounce to login), then issue a code and redirect back to the client.
async fn authorize(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(q): Query<AuthorizeQuery>,
) -> Result<Response, OAuthError> {
    if q.response_type.as_deref() != Some("code") {
        return Err(OAuthError::bad(
            "unsupported_response_type",
            "only response_type=code is supported",
        ));
    }
    let method = q.code_challenge_method.as_deref().unwrap_or("S256");
    if method != "S256" {
        return Err(OAuthError::bad(
            "invalid_request",
            "only code_challenge_method=S256 is supported",
        ));
    }
    let challenge = q
        .code_challenge
        .as_deref()
        .filter(|c| !c.is_empty())
        .ok_or_else(|| OAuthError::bad("invalid_request", "code_challenge is required (PKCE)"))?;
    let client_id = q
        .client_id
        .as_deref()
        .ok_or_else(|| OAuthError::bad("invalid_request", "client_id is required"))?;
    let redirect_uri = q
        .redirect_uri
        .as_deref()
        .ok_or_else(|| OAuthError::bad("invalid_request", "redirect_uri is required"))?;

    // Unknown client / unregistered redirect_uri: there is no trusted target, so
    // fail directly rather than redirecting to an attacker-supplied URL.
    let client = oauth_clients::Entity::find_by_id(client_id.to_string())
        .one(&state.db)
        .await?
        .ok_or_else(|| OAuthError::bad("invalid_client", "unknown client_id"))?;
    if !registered_redirect(&client, redirect_uri) {
        return Err(OAuthError::bad(
            "invalid_request",
            "redirect_uri is not registered for this client",
        ));
    }

    // Require a logged-in user; otherwise send them to the SPA login and back.
    let Some(user) = current_user(&state, &headers).await else {
        let next = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
        return Ok(Redirect::to(&format!("/?next={}", encode(next))).into_response());
    };

    let code = new_token();
    oauth_codes::ActiveModel {
        code: Set(code.clone()),
        client_id: Set(client_id.to_string()),
        user_id: Set(user.id),
        redirect_uri: Set(redirect_uri.to_string()),
        code_challenge: Set(challenge.to_string()),
        code_challenge_method: Set(method.to_string()),
        scope: Set(q.scope.clone().unwrap_or_else(|| SCOPE.to_string())),
        expires_at: Set(Utc::now().naive_utc() + chrono::Duration::seconds(CODE_TTL_SECS)),
        used: Set(false),
    }
    .insert(&state.db)
    .await?;

    let sep = if redirect_uri.contains('?') { '&' } else { '?' };
    let mut target = format!("{redirect_uri}{sep}code={}", encode(&code));
    if let Some(st) = q.state.as_deref().filter(|s| !s.is_empty()) {
        target.push_str(&format!("&state={}", encode(st)));
    }
    Ok(Redirect::to(&target).into_response())
}

// --- Token endpoint: code exchange + refresh ----------------------------

#[derive(Deserialize)]
struct TokenRequest {
    grant_type: String,
    // authorization_code
    code: Option<String>,
    redirect_uri: Option<String>,
    code_verifier: Option<String>,
    client_id: Option<String>,
    // refresh_token
    refresh_token: Option<String>,
}

/// POST /oauth/token — `authorization_code` (PKCE verify) or `refresh_token`.
async fn token(
    State(state): State<AppState>,
    Form(req): Form<TokenRequest>,
) -> Result<Response, OAuthError> {
    match req.grant_type.as_str() {
        "authorization_code" => exchange_code(&state.db, req).await,
        "refresh_token" => refresh(&state.db, req).await,
        other => Err(OAuthError::bad(
            "unsupported_grant_type",
            format!("unsupported grant_type: {other}"),
        )),
    }
}

async fn exchange_code(db: &DatabaseConnection, req: TokenRequest) -> Result<Response, OAuthError> {
    let code = req
        .code
        .as_deref()
        .ok_or_else(|| OAuthError::bad("invalid_request", "code is required"))?;
    let verifier = req
        .code_verifier
        .as_deref()
        .ok_or_else(|| OAuthError::bad("invalid_request", "code_verifier is required (PKCE)"))?;

    let row = oauth_codes::Entity::find_by_id(code.to_string())
        .one(db)
        .await?
        .ok_or_else(|| OAuthError::bad("invalid_grant", "unknown authorization code"))?;

    if row.used || row.expires_at <= Utc::now().naive_utc() {
        return Err(OAuthError::bad(
            "invalid_grant",
            "authorization code expired or used",
        ));
    }
    if let Some(rd) = req.redirect_uri.as_deref() {
        if rd != row.redirect_uri {
            return Err(OAuthError::bad("invalid_grant", "redirect_uri mismatch"));
        }
    }
    if let Some(cid) = req.client_id.as_deref() {
        if cid != row.client_id {
            return Err(OAuthError::bad("invalid_grant", "client_id mismatch"));
        }
    }
    if !verify_pkce_s256(verifier, &row.code_challenge) {
        return Err(OAuthError::bad("invalid_grant", "PKCE verification failed"));
    }

    // Burn the code before issuing tokens so it cannot be replayed.
    let client_id = row.client_id.clone();
    let user_id = row.user_id.clone();
    let scope = row.scope.clone();
    let mut active: oauth_codes::ActiveModel = row.into();
    active.used = Set(true);
    active.update(db).await?;

    let (access, refresh, expires_in) =
        issue_oauth_tokens(db, &client_id, &user_id, &scope).await?;
    Ok(token_response(&access, &refresh, expires_in, &scope))
}

async fn refresh(db: &DatabaseConnection, req: TokenRequest) -> Result<Response, OAuthError> {
    let refresh_token = req
        .refresh_token
        .as_deref()
        .ok_or_else(|| OAuthError::bad("invalid_request", "refresh_token is required"))?;

    let row = oauth_tokens::Entity::find()
        .filter(oauth_tokens::Column::RefreshToken.eq(refresh_token))
        .one(db)
        .await?
        .ok_or_else(|| OAuthError::bad("invalid_grant", "unknown refresh token"))?;

    let client_id = row.client_id.clone();
    let user_id = row.user_id.clone();
    let scope = row.scope.clone();
    // Rotate: drop the old pair, mint a fresh one.
    oauth_tokens::Entity::delete_by_id(row.access_token.clone())
        .exec(db)
        .await?;
    let (access, new_refresh, expires_in) =
        issue_oauth_tokens(db, &client_id, &user_id, &scope).await?;
    Ok(token_response(&access, &new_refresh, expires_in, &scope))
}

/// Build the RFC 6749 token response body.
fn token_response(access: &str, refresh: &str, expires_in: i64, scope: &str) -> Response {
    Json(json!({
        "access_token": access,
        "token_type": "Bearer",
        "expires_in": expires_in,
        "refresh_token": refresh,
        "scope": scope,
    }))
    .into_response()
}

// --- Helpers -------------------------------------------------------------

/// Resolve the logged-in user from the request (server-mode session/Bearer;
/// local mode is always the implicit admin). `None` ⇒ anonymous.
async fn current_user(state: &AppState, headers: &HeaderMap) -> Option<CurrentUser> {
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
fn registered_redirect(client: &oauth_clients::Model, uri: &str) -> bool {
    serde_json::from_str::<Vec<String>>(&client.redirect_uris)
        .map(|uris| uris.iter().any(|u| u == uri))
        .unwrap_or(false)
}

/// Percent-encode a string for use in a URL query component (RFC 3986 unreserved
/// set passes through). Dependency-free; used for `code`/`state`/`next`.
fn encode(s: &str) -> String {
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

/// An OAuth error response (RFC 6749 §5.2). `bad` ⇒ `400` with the error code;
/// a database failure ⇒ `500` without detail.
enum OAuthError {
    Bad(&'static str, String),
    Server,
}

impl OAuthError {
    fn bad(error: &'static str, description: impl Into<String>) -> Self {
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
