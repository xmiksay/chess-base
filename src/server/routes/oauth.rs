//! OAuth 2.1 authorization server + resource metadata for the `/mcp` endpoint
//! (ADR-0016, hardened in ADR-0044). Public-client, PKCE-only: claude.ai
//! self-onboards via dynamic client registration (RFC 7591, now auth-gated —
//! the registering caller must be logged in), runs the authorization-code
//! grant against the logged-in user's session, and refreshes with the
//! refresh-token grant.
//!
//! Consent is explicit (ADR-0044): a logged-in user hitting `/oauth/authorize`
//! for a client they haven't approved before is routed through a consent
//! screen (`oauth_consent`) rather than a code being minted silently; a
//! remembered approval (`oauth_consents`) skips the screen on later
//! authorizations. An anonymous request is still bounced to the SPA login
//! carrying its original URL in `next`.
//!
//! Refresh tokens rotate on every use and carry a `family_id` shared across
//! rotations; replaying an already-rotated-away refresh token is reuse and
//! revokes the whole family (`refresh`).
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
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set,
};
use serde::Deserialize;
use serde_json::json;

use crate::db::entities::{
    oauth_clients, oauth_codes, oauth_consent_requests, oauth_consents, oauth_tokens,
};
use crate::server::auth::{
    base_url, issue_oauth_tokens, new_token, rotate_oauth_tokens, verify_pkce_s256,
    ABSOLUTE_REFRESH_TTL_DAYS,
};
use crate::server::identity::CurrentUser;
use crate::server::state::AppState;

use super::oauth_consent;
use super::oauth_shared::{current_user, encode, registered_redirect, token_response, OAuthError};

/// Authorization-code lifetime — short, single-use.
const CODE_TTL_SECS: i64 = 600;
/// Pending-consent-request lifetime — short, single-use (ADR-0044).
const CONSENT_REQUEST_TTL_SECS: i64 = 600;
/// Scope advertised + granted. Authorization is by resource ownership, not by
/// granular OAuth scopes, so a single coarse scope suffices.
const SCOPE: &str = "chess";

/// Mount the well-known metadata + OAuth endpoints, plus the consent screen
/// (ADR-0044).
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
        .with_state(state.clone())
        .merge(oauth_consent::router(state))
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
/// Requires an authenticated caller (ADR-0044): an anonymous/unauthenticated
/// request `401`s via the [`CurrentUser`] extractor before a client is ever
/// registered. Local mode still works zero-config, since `CurrentUser` there
/// is always the implicit admin. The registered client itself has no owner
/// column — this is purely an auth gate on the registration endpoint.
async fn register_client(
    State(state): State<AppState>,
    _caller: CurrentUser,
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
/// (else bounce to login), then either issue a code directly (a remembered
/// consent, ADR-0044) or park a pending consent request and send the user to
/// the consent screen.
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

    let scope = q.scope.clone().unwrap_or_else(|| SCOPE.to_string());

    // A prior approval of this exact (user, client) pair skips the consent
    // screen — this is what keeps refresh/re-auth a one-shot approval rather
    // than nagging on every sign-in (ADR-0044).
    let consented = oauth_consents::Entity::find_by_id((user.id.clone(), client_id.to_string()))
        .one(&state.db)
        .await?
        .is_some();
    if consented {
        return issue_code_and_redirect(
            &state.db,
            client_id,
            &user.id,
            redirect_uri,
            challenge,
            method,
            &scope,
            q.state.as_deref(),
        )
        .await;
    }

    // First time this user is asked about this client: park the request and
    // send them to the consent screen. `csrf_token` is only ever delivered
    // inside that rendered page, so it is what makes the screen's POST
    // forgery-proof.
    let csrf_token = new_token();
    oauth_consent_requests::ActiveModel {
        csrf_token: Set(csrf_token.clone()),
        user_id: Set(user.id.clone()),
        client_id: Set(client_id.to_string()),
        redirect_uri: Set(redirect_uri.to_string()),
        code_challenge: Set(challenge.to_string()),
        code_challenge_method: Set(method.to_string()),
        scope: Set(scope),
        state: Set(q.state.clone()),
        expires_at: Set(
            Utc::now().naive_utc() + chrono::Duration::seconds(CONSENT_REQUEST_TTL_SECS)
        ),
    }
    .insert(&state.db)
    .await?;

    Ok(Redirect::to(&format!(
        "/oauth/consent?csrf_token={}",
        encode(&csrf_token)
    ))
    .into_response())
}

/// Mint an authorization code and build the redirect back to the client. The
/// shared tail of the authorize-with-remembered-consent path and the
/// consent-screen approve path (ADR-0044) — both end up here so the code
/// minting logic lives exactly once.
#[allow(clippy::too_many_arguments)]
pub(super) async fn issue_code_and_redirect(
    db: &DatabaseConnection,
    client_id: &str,
    user_id: &str,
    redirect_uri: &str,
    code_challenge: &str,
    code_challenge_method: &str,
    scope: &str,
    state: Option<&str>,
) -> Result<Response, OAuthError> {
    let code = new_token();
    oauth_codes::ActiveModel {
        code: Set(code.clone()),
        client_id: Set(client_id.to_string()),
        user_id: Set(user_id.to_string()),
        redirect_uri: Set(redirect_uri.to_string()),
        code_challenge: Set(code_challenge.to_string()),
        code_challenge_method: Set(code_challenge_method.to_string()),
        scope: Set(scope.to_string()),
        expires_at: Set(Utc::now().naive_utc() + chrono::Duration::seconds(CODE_TTL_SECS)),
        used: Set(false),
    }
    .insert(db)
    .await?;

    let sep = if redirect_uri.contains('?') { '&' } else { '?' };
    let mut target = format!("{redirect_uri}{sep}code={}", encode(&code));
    if let Some(st) = state.filter(|s| !s.is_empty()) {
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

/// `refresh_token` grant (ADR-0044): rotate on success, but first check for
/// **reuse** (a revoked row's refresh token being presented again — the row
/// only becomes revoked once rotation has already moved past it) and the
/// family's absolute lifetime. Either failure revokes the whole family, so a
/// stolen-and-replayed refresh token can't keep the session alive by racing
/// the legitimate client.
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

    if row.revoked {
        revoke_family(db, &row.family_id).await?;
        return Err(OAuthError::bad(
            "invalid_grant",
            "refresh token reuse detected; the authorization has been revoked",
        ));
    }

    let oldest = oauth_tokens::Entity::find()
        .filter(oauth_tokens::Column::FamilyId.eq(row.family_id.clone()))
        .order_by_asc(oauth_tokens::Column::CreatedAt)
        .one(db)
        .await?;
    if let Some(oldest) = oldest {
        let age = Utc::now().naive_utc() - oldest.created_at;
        if age > chrono::Duration::days(ABSOLUTE_REFRESH_TTL_DAYS) {
            revoke_family(db, &row.family_id).await?;
            return Err(OAuthError::bad("invalid_grant", "refresh token expired"));
        }
    }

    let scope = row.scope.clone();
    let (access, new_refresh, expires_in) = rotate_oauth_tokens(db, &row).await?;
    Ok(token_response(&access, &new_refresh, expires_in, &scope))
}

/// Revoke every row sharing `family_id` — reuse detected, or the family's
/// absolute lifetime elapsed (ADR-0044).
async fn revoke_family(db: &DatabaseConnection, family_id: &str) -> Result<(), DbErr> {
    let rows = oauth_tokens::Entity::find()
        .filter(oauth_tokens::Column::FamilyId.eq(family_id))
        .all(db)
        .await?;
    for row in rows {
        let mut active: oauth_tokens::ActiveModel = row.into();
        active.revoked = Set(true);
        active.update(db).await?;
    }
    Ok(())
}
