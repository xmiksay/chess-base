//! MCP authentication (ADR-0016, anonymous tier ADR-0043, consent/reuse-
//! detection/scoped-tokens ADR-0044): resolve the caller behind a `/mcp`
//! request and mint the bearer challenge on a miss.
//!
//! A request carries `Authorization: Bearer <token>`. [`authenticate_mcp`] tries
//! an OAuth 2.1 access token first (server-mode, the claude.ai self-onboarding
//! path) then falls back to a static **service token** (the local-mode printed
//! token, or an admin-issued server one, now with a `scope` — full/read-only/
//! global-read, ADR-0044). Either resolves to the one [`CurrentUser`] every
//! service scopes against. A *present but invalid/expired/revoked* credential
//! still gets a `401` with `WWW-Authenticate: Bearer resource_metadata="…"`,
//! the discovery hook OAuth-aware clients follow to the authorization server.
//! In **server mode**, a request with **no** credential at all instead mints
//! [`CurrentUser::anonymous`] (ADR-0043: data reads on global databases only —
//! the dispatch layer enforces the tool allowlist). Local mode is unchanged: no
//! credential still 401s, since the printed local service token is the only way
//! in.
//!
//! Refresh tokens rotate on every use and carry a `family_id`: replaying an
//! already-rotated-away token revokes the whole family (reuse detection), and a
//! family has a hard [`ABSOLUTE_REFRESH_TTL_DAYS`] lifetime regardless of how
//! often it rotates. `/oauth/authorize` no longer mints a code the instant a
//! user is logged in — a first-time client is routed through a consent screen
//! (`oauth_consent`) guarded by a single-use CSRF token; a remembered consent
//! (`oauth_consents`) skips it on subsequent authorizations.

use axum::http::{header, HeaderMap};
use base64::Engine;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, Set,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::db::entities::{oauth_tokens, service_tokens, users};
use crate::server::config::Mode;
use crate::server::identity::{CurrentUser, LOCAL_ADMIN_ID};
use crate::server::state::AppState;

/// Label stamped on the auto-seeded local-mode service token, so it is found and
/// reused across restarts instead of multiplying.
pub const LOCAL_TOKEN_LABEL: &str = "local";

/// Access-token lifetime: short, since a refresh token mints a fresh one.
const ACCESS_TOKEN_TTL_SECS: i64 = 3600;

/// Hard ceiling on a refresh-token family's lifetime (ADR-0044), checked
/// against the family's oldest row's `created_at` regardless of how many times
/// it has rotated since.
pub const ABSOLUTE_REFRESH_TTL_DAYS: i64 = 30;

/// A `401` bearer challenge: the `WWW-Authenticate` value pointing an MCP client
/// at the protected-resource metadata so it can discover OAuth.
pub struct BearerChallenge {
    pub www_authenticate: String,
}

/// Resolve the caller behind a `/mcp` request: OAuth access token first, then a
/// static service token. A credential that fails to resolve is a `401` (still
/// carrying the bearer challenge, ADR-0016). No credential at all mints the
/// anonymous public identity in server mode (ADR-0043) — local mode keeps
/// requiring the printed local service token, so it still `401`s.
pub async fn authenticate_mcp(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<CurrentUser, BearerChallenge> {
    match crate::auth::token_from_headers(headers) {
        Some(token) => {
            if let Some(user) = oauth_token_user(&state.db, &token).await {
                return Ok(user);
            }
            if let Some(user) = service_token_user(&state.db, &token).await {
                return Ok(user);
            }
            Err(challenge(headers))
        }
        None if state.mode == Mode::Server => Ok(CurrentUser::anonymous()),
        None => Err(challenge(headers)),
    }
}

/// Build the bearer challenge for a missing/invalid credential.
fn challenge(headers: &HeaderMap) -> BearerChallenge {
    BearerChallenge {
        www_authenticate: format!(
            "Bearer resource_metadata=\"{}\"",
            protected_resource_url(headers)
        ),
    }
}

/// Resolve a live, non-revoked OAuth access token to its user (role read from
/// `users`). A revoked row (reuse-detection fallout, ADR-0044) is treated the
/// same as an expired one.
async fn oauth_token_user(db: &DatabaseConnection, token: &str) -> Option<CurrentUser> {
    let row = oauth_tokens::Entity::find_by_id(token.to_string())
        .one(db)
        .await
        .ok()
        .flatten()?;
    if row.revoked || row.expires_at <= Utc::now().naive_utc() {
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
        public: false,
        read_only: false,
        global_only: false,
    })
}

/// Service-token scopes (ADR-0044): a row's `scope` column maps onto
/// [`CurrentUser`]'s `read_only`/`global_only` axes.
pub const SERVICE_SCOPE_FULL: &str = "full";
pub const SERVICE_SCOPE_READ_ONLY: &str = "read_only";
pub const SERVICE_SCOPE_GLOBAL_READ: &str = "global_read";

/// Resolve a static service token to its caller. The role rides on the row, so no
/// `users` lookup is needed (the local-mode token has no `users` row at all).
/// `scope` (ADR-0044) maps onto the `read_only`/`global_only` axes; an
/// unrecognized/legacy value defaults to full access, matching pre-ADR-0044
/// behavior.
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
    let (read_only, global_only) = match row.scope.as_str() {
        SERVICE_SCOPE_READ_ONLY => (true, false),
        SERVICE_SCOPE_GLOBAL_READ => (true, true),
        _ => (false, false),
    };
    Some(CurrentUser {
        id: row.owner_id,
        is_admin: row.is_admin,
        public: false,
        read_only,
        global_only,
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
        id: Set(token.clone()),
        owner_id: Set(LOCAL_ADMIN_ID.to_string()),
        is_admin: Set(true),
        scope: Set(SERVICE_SCOPE_FULL.to_string()),
        label: Set(LOCAL_TOKEN_LABEL.to_string()),
        created_at: Set(Utc::now().naive_utc()),
        expires_at: Set(None),
    }
    .insert(db)
    .await?;
    Ok(token)
}

/// Issue a fresh OAuth access/refresh pair for a user's first grant (code
/// exchange) and persist it. Starts a fresh token *family* (ADR-0044) — later
/// rotations of this pair go through [`rotate_oauth_tokens`] instead, which
/// keeps the family id and revokes-in-place rather than deleting. Returns the
/// `(access_token, refresh_token, expires_in_secs)`.
pub async fn issue_oauth_tokens(
    db: &DatabaseConnection,
    client_id: &str,
    user_id: &str,
    scope: &str,
) -> Result<(String, String, i64), DbErr> {
    let access_token = new_token();
    let refresh_token = new_token();
    let family_id = new_token();
    let now = Utc::now().naive_utc();
    oauth_tokens::ActiveModel {
        access_token: Set(access_token.clone()),
        refresh_token: Set(refresh_token.clone()),
        client_id: Set(client_id.to_string()),
        user_id: Set(user_id.to_string()),
        scope: Set(scope.to_string()),
        family_id: Set(family_id),
        revoked: Set(false),
        created_at: Set(now),
        expires_at: Set(now + chrono::Duration::seconds(ACCESS_TOKEN_TTL_SECS)),
    }
    .insert(db)
    .await?;
    Ok((access_token, refresh_token, ACCESS_TOKEN_TTL_SECS))
}

/// Rotate a refresh grant (ADR-0044): mark `old` revoked **in place** — never
/// delete it, since the revoked row is exactly what makes a later replay of
/// `old`'s refresh token detectable as reuse — then insert a fresh pair
/// reusing `old`'s `family_id`/`client_id`/`user_id`/`scope`. Returns the new
/// `(access_token, refresh_token, expires_in_secs)`.
pub async fn rotate_oauth_tokens(
    db: &DatabaseConnection,
    old: &oauth_tokens::Model,
) -> Result<(String, String, i64), DbErr> {
    let mut active: oauth_tokens::ActiveModel = old.clone().into();
    active.revoked = Set(true);
    active.update(db).await?;

    let access_token = new_token();
    let refresh_token = new_token();
    let now = Utc::now().naive_utc();
    oauth_tokens::ActiveModel {
        access_token: Set(access_token.clone()),
        refresh_token: Set(refresh_token.clone()),
        client_id: Set(old.client_id.clone()),
        user_id: Set(old.user_id.clone()),
        scope: Set(old.scope.clone()),
        family_id: Set(old.family_id.clone()),
        revoked: Set(false),
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
