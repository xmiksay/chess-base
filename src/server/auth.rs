//! MCP authentication (ADR-0016): resolve the caller behind a `/mcp` request and
//! mint the bearer challenge on a miss.
//!
//! A request carries `Authorization: Bearer <token>`. [`authenticate_mcp`] tries
//! an OAuth 2.1 access token first (server-mode, the claude.ai self-onboarding
//! path) then falls back to a static **service token** (the local-mode printed
//! token, or an admin-issued server one). Either resolves to the one
//! [`CurrentUser`] every service scopes against. On any miss the caller gets a
//! `401` with `WWW-Authenticate: Bearer resource_metadata="…"`, the discovery
//! hook OAuth-aware clients follow to the authorization server.

use axum::http::{header, HeaderMap};
use base64::Engine;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, Set,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::db::entities::{oauth_tokens, service_tokens, users};
use crate::server::identity::{CurrentUser, LOCAL_ADMIN_ID};
use crate::server::state::AppState;

/// Label stamped on the auto-seeded local-mode service token, so it is found and
/// reused across restarts instead of multiplying.
pub const LOCAL_TOKEN_LABEL: &str = "local";

/// Access-token lifetime: short, since a refresh token mints a fresh one.
const ACCESS_TOKEN_TTL_SECS: i64 = 3600;

/// A `401` bearer challenge: the `WWW-Authenticate` value pointing an MCP client
/// at the protected-resource metadata so it can discover OAuth.
pub struct BearerChallenge {
    pub www_authenticate: String,
}

/// Resolve the caller behind a `/mcp` request: OAuth access token first, then a
/// static service token. Returns the [`CurrentUser`] to scope every tool to, or a
/// [`BearerChallenge`] to render as `401` on any miss.
pub async fn authenticate_mcp(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<CurrentUser, BearerChallenge> {
    if let Some(token) = crate::auth::token_from_headers(headers) {
        if let Some(user) = oauth_token_user(&state.db, &token).await {
            return Ok(user);
        }
        if let Some(user) = service_token_user(&state.db, &token).await {
            return Ok(user);
        }
    }
    Err(BearerChallenge {
        www_authenticate: format!(
            "Bearer resource_metadata=\"{}\"",
            protected_resource_url(headers)
        ),
    })
}

/// Resolve a live OAuth access token to its user (role read from `users`).
async fn oauth_token_user(db: &DatabaseConnection, token: &str) -> Option<CurrentUser> {
    let row = oauth_tokens::Entity::find_by_id(token.to_string())
        .one(db)
        .await
        .ok()
        .flatten()?;
    if row.expires_at <= Utc::now().naive_utc() {
        return None;
    }
    let user = users::Entity::find_by_id(row.user_id)
        .one(db)
        .await
        .ok()
        .flatten()?;
    Some(CurrentUser {
        id: user.id,
        is_admin: user.is_admin,
    })
}

/// Resolve a static service token to its caller. The role rides on the row, so no
/// `users` lookup is needed (the local-mode token has no `users` row at all).
async fn service_token_user(db: &DatabaseConnection, token: &str) -> Option<CurrentUser> {
    let row = service_tokens::Entity::find_by_id(token.to_string())
        .one(db)
        .await
        .ok()
        .flatten()?;
    if let Some(expires_at) = row.expires_at {
        if expires_at <= Utc::now().naive_utc() {
            return None;
        }
    }
    Some(CurrentUser {
        id: row.owner_id,
        is_admin: row.is_admin,
    })
}

/// Ensure the local-mode service token exists and return it, reusing the seeded
/// row across restarts so the printed `claude mcp add` line stays valid. The
/// token is the implicit `local-admin` with full rights.
pub async fn ensure_local_service_token(db: &DatabaseConnection) -> Result<String, DbErr> {
    if let Some(row) = service_tokens::Entity::find()
        .filter(service_tokens::Column::Label.eq(LOCAL_TOKEN_LABEL))
        .one(db)
        .await?
    {
        return Ok(row.token);
    }
    let token = new_token();
    service_tokens::ActiveModel {
        token: Set(token.clone()),
        owner_id: Set(LOCAL_ADMIN_ID.to_string()),
        is_admin: Set(true),
        label: Set(LOCAL_TOKEN_LABEL.to_string()),
        created_at: Set(Utc::now().naive_utc()),
        expires_at: Set(None),
    }
    .insert(db)
    .await?;
    Ok(token)
}

/// Issue a fresh OAuth access/refresh pair for a user and persist it. Returns the
/// `(access_token, refresh_token, expires_in_secs)`.
pub async fn issue_oauth_tokens(
    db: &DatabaseConnection,
    client_id: &str,
    user_id: &str,
    scope: &str,
) -> Result<(String, String, i64), DbErr> {
    let access_token = new_token();
    let refresh_token = new_token();
    let now = Utc::now().naive_utc();
    oauth_tokens::ActiveModel {
        access_token: Set(access_token.clone()),
        refresh_token: Set(refresh_token.clone()),
        client_id: Set(client_id.to_string()),
        user_id: Set(user_id.to_string()),
        scope: Set(scope.to_string()),
        created_at: Set(now),
        expires_at: Set(now + chrono::Duration::seconds(ACCESS_TOKEN_TTL_SECS)),
    }
    .insert(db)
    .await?;
    Ok((access_token, refresh_token, ACCESS_TOKEN_TTL_SECS))
}

/// A random, unguessable opaque secret — two v4 UUIDs (~244 bits), matching the
/// session-token scheme. Used for service/access/refresh tokens, codes and
/// client ids.
pub fn new_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// PKCE S256 check: `base64url(SHA-256(verifier)) == code_challenge` (no padding).
pub fn verify_pkce_s256(verifier: &str, challenge: &str) -> bool {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest) == challenge
}

/// The externally-reachable base URL inferred from request headers, honouring an
/// ingress's `X-Forwarded-Proto` and falling back to `http`. The OAuth metadata
/// and the bearer challenge are built relative to this.
pub fn base_url(headers: &HeaderMap) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    format!("{scheme}://{host}")
}

/// URL of the protected-resource metadata document (RFC 9728).
pub fn protected_resource_url(headers: &HeaderMap) -> String {
    format!("{}/.well-known/oauth-protected-resource", base_url(headers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn pkce_verifies_a_known_s256_pair() {
        // RFC 7636 appendix B test vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert!(verify_pkce_s256(verifier, challenge));
        assert!(!verify_pkce_s256("wrong", challenge));
    }

    #[test]
    fn base_url_honours_forwarded_proto_and_host() {
        let h = headers(&[("host", "chess.example"), ("x-forwarded-proto", "https")]);
        assert_eq!(base_url(&h), "https://chess.example");
    }

    #[test]
    fn base_url_defaults_to_http_localhost() {
        assert_eq!(base_url(&headers(&[])), "http://localhost");
    }

    #[test]
    fn protected_resource_url_appends_well_known() {
        let h = headers(&[("host", "h:3030")]);
        assert_eq!(
            protected_resource_url(&h),
            "http://h:3030/.well-known/oauth-protected-resource"
        );
    }

    #[test]
    fn new_token_is_unguessably_long_and_unique() {
        let a = new_token();
        let b = new_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 64);
    }
}
