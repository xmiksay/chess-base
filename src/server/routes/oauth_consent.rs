//! The OAuth consent screen (ADR-0044, issue #193): `GET /oauth/authorize`
//! parks a pending [`oauth_consent_requests`] row instead of minting a code
//! the instant a user is logged in, and redirects here.
//!
//! `GET /oauth/consent?csrf_token=…` renders a minimal, dependency-free HTML
//! page naming the requesting client and what it would be granted, with a form
//! posting back the same `csrf_token`. `POST /oauth/consent` is the actual CSRF
//! defense: the token is delivered *only* inside that rendered page, so a
//! forged cross-site POST — which under `SameSite=Lax` typically has no valid
//! session cookie at all — additionally cannot supply the one value that makes
//! the request valid, since the attacker's forced navigation of the initial
//! `GET /oauth/authorize` never lets them read the resulting page (same-origin
//! policy). The request row is deleted on the first `POST`, approved or
//! denied, so a replay of that exact POST always fails afterwards.

use axum::{
    extract::{Form, OriginalUri, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use chrono::Utc;
use sea_orm::{ActiveModelTrait, DatabaseConnection, DbErr, EntityTrait, Set};
use serde::Deserialize;

use crate::db::entities::{oauth_clients, oauth_consent_requests, oauth_consents};
use crate::server::state::AppState;

use super::oauth::issue_code_and_redirect;
use super::oauth_shared::{current_user, encode};

/// Mount the consent-screen routes, merged into [`super::oauth::router`].
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/oauth/consent", get(show_consent).post(decide_consent))
        .with_state(state)
}

#[derive(Deserialize)]
struct ConsentQuery {
    csrf_token: String,
}

#[derive(Deserialize)]
struct ConsentDecision {
    csrf_token: String,
    decision: String,
}

/// GET /oauth/consent — render the approve/deny form for a pending request.
async fn show_consent(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(q): Query<ConsentQuery>,
) -> Result<Response, ConsentError> {
    let Some(user) = current_user(&state, &headers).await else {
        let next = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
        return Ok(Redirect::to(&format!("/?next={}", encode(next))).into_response());
    };

    let row = find_live_request(&state.db, &q.csrf_token).await?;
    // Never reveal whether a token exists for someone else.
    if row.user_id != user.id {
        return Err(ConsentError::Forbidden);
    }

    let client = oauth_clients::Entity::find_by_id(row.client_id.clone())
        .one(&state.db)
        .await?
        .ok_or(ConsentError::NotFound)?;

    Ok(Html(render_consent_page(&client.client_name, &q.csrf_token)).into_response())
}

/// POST /oauth/consent — the CSRF-guarded approve/deny decision. Single-use:
/// the request row is deleted the moment it is found, before the decision is
/// even inspected, so a replayed POST (same or different decision) always 400s
/// afterwards.
async fn decide_consent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(body): Form<ConsentDecision>,
) -> Result<Response, ConsentError> {
    let user = current_user(&state, &headers).await;

    let existing = oauth_consent_requests::Entity::find_by_id(body.csrf_token.clone())
        .one(&state.db)
        .await?;
    if existing.is_some() {
        oauth_consent_requests::Entity::delete_by_id(body.csrf_token.clone())
            .exec(&state.db)
            .await?;
    }
    let row = existing.ok_or(ConsentError::NotFound)?;
    if row.expires_at <= Utc::now().naive_utc() {
        return Err(ConsentError::NotFound);
    }
    let user = user.ok_or(ConsentError::Forbidden)?;
    if row.user_id != user.id {
        return Err(ConsentError::Forbidden);
    }

    if body.decision != "approve" {
        // RFC 6749 §4.1.2.1 — deny (or anything not explicitly "approve",
        // which is never treated as an implicit approval) redirects back with
        // `error=access_denied`.
        let sep = if row.redirect_uri.contains('?') {
            '&'
        } else {
            '?'
        };
        let mut target = format!("{}{sep}error=access_denied", row.redirect_uri);
        if let Some(st) = row.state.as_deref().filter(|s| !s.is_empty()) {
            target.push_str(&format!("&state={}", encode(st)));
        }
        return Ok(Redirect::to(&target).into_response());
    }

    remember_consent(&state.db, &row.user_id, &row.client_id).await?;
    issue_code_and_redirect(
        &state.db,
        &row.client_id,
        &row.user_id,
        &row.redirect_uri,
        &row.code_challenge,
        &row.code_challenge_method,
        &row.scope,
        row.state.as_deref(),
    )
    .await
    .map_err(|_| ConsentError::Db)
}

/// Look up a pending consent request by its `csrf_token`, rejecting a
/// missing or expired one. Does **not** delete it — only the POST handler's
/// single-use semantics do that; `GET` may be reloaded harmlessly.
async fn find_live_request(
    db: &DatabaseConnection,
    csrf_token: &str,
) -> Result<oauth_consent_requests::Model, ConsentError> {
    let row = oauth_consent_requests::Entity::find_by_id(csrf_token.to_string())
        .one(db)
        .await?
        .ok_or(ConsentError::NotFound)?;
    if row.expires_at <= Utc::now().naive_utc() {
        return Err(ConsentError::NotFound);
    }
    Ok(row)
}

/// Remember this (user, client) approval so a later `GET /oauth/authorize`
/// skips the consent screen (ADR-0044).
async fn remember_consent(
    db: &DatabaseConnection,
    user_id: &str,
    client_id: &str,
) -> Result<(), DbErr> {
    let existing = oauth_consents::Entity::find_by_id((user_id.to_string(), client_id.to_string()))
        .one(db)
        .await?;
    if existing.is_none() {
        oauth_consents::ActiveModel {
            user_id: Set(user_id.to_string()),
            client_id: Set(client_id.to_string()),
            created_at: Set(Utc::now().naive_utc()),
        }
        .insert(db)
        .await?;
    }
    Ok(())
}

/// Render the approve/deny page. `client_name` is attacker-controlled free
/// text from `POST /oauth/register` — this is exactly the page a malicious
/// client's victim lands on, so it is HTML-escaped before embedding.
fn render_consent_page(client_name: &str, csrf_token: &str) -> String {
    format!(
        r#"<!doctype html>
<html>
<head><meta charset="utf-8"><title>Authorize application</title></head>
<body>
<h1>Authorize application</h1>
<p><strong>{name}</strong> is requesting read and write access to your chess
games, databases and studies.</p>
<form method="POST" action="/oauth/consent">
<input type="hidden" name="csrf_token" value="{token}">
<button type="submit" name="decision" value="approve">Approve</button>
<button type="submit" name="decision" value="deny">Deny</button>
</form>
</body>
</html>
"#,
        name = html_escape(client_name),
        token = html_escape(csrf_token),
    )
}

/// Escape `& < > " '` for safe embedding in HTML text/attribute content.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Errors for the browser-facing consent screen: short, human-readable
/// messages rather than the JSON RFC 6749 envelope the token endpoint uses.
enum ConsentError {
    /// Missing, expired or already-consumed consent request. `400`.
    NotFound,
    /// The consent request belongs to someone else, or the poster isn't
    /// logged in at all — deliberately not distinguished, so as not to leak
    /// whether the token exists for someone else (ADR-0044's CSRF defense).
    Forbidden,
    Db,
}

impl From<DbErr> for ConsentError {
    fn from(_: DbErr) -> Self {
        ConsentError::Db
    }
}

impl IntoResponse for ConsentError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ConsentError::NotFound => (
                StatusCode::BAD_REQUEST,
                "unknown or expired consent request",
            ),
            ConsentError::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            ConsentError::Db => (StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
        };
        (status, message).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escape_escapes_all_five_special_chars() {
        assert_eq!(
            html_escape(r#"<a href="x">&'"#),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }

    #[test]
    fn html_escape_passes_plain_text_through() {
        assert_eq!(html_escape("Claude Desktop"), "Claude Desktop");
    }
}
