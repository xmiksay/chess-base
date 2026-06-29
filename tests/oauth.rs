//! Integration tests for the MCP OAuth 2.1 surface (ADR-0016): discovery
//! metadata, the full authorization-code + refresh-token dance ending in an
//! authenticated `/mcp` call, and the local-mode service token.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use chess_base::db::{connect, DbConfig};
use chess_base::server::auth::ensure_local_service_token;
use chess_base::server::{build_router, AppState, Mode};

// PKCE S256 test vector (RFC 7636 appendix B) — avoids hashing in the test.
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
const REDIRECT_URI: &str = "https://claude.example/callback";

async fn server_app() -> Router {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    build_router(AppState {
        db,
        mode: Mode::Server,
        engine_service: None,
        llm_provider: None,
    })
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, body)
}

/// Minimal percent-encoder for form/query values.
fn enc(s: &str) -> String {
    let mut out = String::new();
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

#[tokio::test]
async fn well_known_metadata_advertises_endpoints_and_pkce() {
    let app = server_app().await;

    let (status, _h, prm) = send(
        &app,
        Request::builder()
            .uri("/.well-known/oauth-protected-resource")
            .header(header::HOST, "chess.example")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(prm["resource"], "http://chess.example/mcp");
    assert_eq!(prm["authorization_servers"][0], "http://chess.example");

    let (status, _h, asm) = send(
        &app,
        Request::builder()
            .uri("/.well-known/oauth-authorization-server")
            .header(header::HOST, "chess.example")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        asm["authorization_endpoint"],
        "http://chess.example/oauth/authorize"
    );
    assert_eq!(asm["token_endpoint"], "http://chess.example/oauth/token");
    assert_eq!(asm["code_challenge_methods_supported"][0], "S256");
    let grants = asm["grant_types_supported"].as_array().unwrap();
    assert!(grants.iter().any(|g| g == "authorization_code"));
    assert!(grants.iter().any(|g| g == "refresh_token"));
}

#[tokio::test]
async fn full_oauth_dance_then_mcp_and_refresh() {
    let app = server_app().await;

    // A user must exist + be logged in to authorize. The first user is admin.
    let (status, _h, reg) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/auth/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({"username": "alice", "password": "password123"}))
                    .unwrap(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let session = reg["token"].as_str().unwrap().to_string();

    // 1. Dynamic client registration.
    let (status, _h, client) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/oauth/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "client_name": "Claude",
                    "redirect_uris": [REDIRECT_URI]
                }))
                .unwrap(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let client_id = client["client_id"].as_str().unwrap().to_string();

    // 2. Authorize (logged-in via session cookie) → redirect carrying the code.
    let authorize_uri = format!(
        "/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state=xyz",
        enc(&client_id),
        enc(REDIRECT_URI),
        enc(CHALLENGE),
    );
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&authorize_uri)
                .header(header::COOKIE, format!("session={session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status().is_redirection(),
        "authorize should redirect, got {}",
        resp.status()
    );
    let location = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(location.starts_with(REDIRECT_URI), "location: {location}");
    assert!(location.contains("state=xyz"));
    let code = extract_query(&location, "code").expect("code in redirect");

    // 3. Exchange the code (PKCE verifier) for tokens.
    let token_body = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&code_verifier={}&client_id={}",
        enc(&code),
        enc(REDIRECT_URI),
        enc(VERIFIER),
        enc(&client_id),
    );
    let (status, _h, tokens) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/oauth/token")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(token_body))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "token exchange: {tokens}");
    assert_eq!(tokens["token_type"], "Bearer");
    let access = tokens["access_token"].as_str().unwrap().to_string();
    let refresh = tokens["refresh_token"].as_str().unwrap().to_string();

    // 4. The access token authenticates an /mcp call.
    assert_eq!(mcp_tools_list_status(&app, &access).await, StatusCode::OK);

    // 5. Refresh → a fresh access token that also works.
    let refresh_body = format!("grant_type=refresh_token&refresh_token={}", enc(&refresh));
    let (status, _h, refreshed) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/oauth/token")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(refresh_body))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "refresh: {refreshed}");
    let new_access = refreshed["access_token"].as_str().unwrap().to_string();
    assert_ne!(new_access, access);
    assert_eq!(
        mcp_tools_list_status(&app, &new_access).await,
        StatusCode::OK
    );

    // A used authorization code cannot be replayed.
    let (status, _h, replay) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/oauth/token")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(format!(
                "grant_type=authorization_code&code={}&redirect_uri={}&code_verifier={}&client_id={}",
                enc(&code), enc(REDIRECT_URI), enc(VERIFIER), enc(&client_id),
            )))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(replay["error"], "invalid_grant");
}

#[tokio::test]
async fn authorize_rejects_a_bad_pkce_verifier() {
    let app = server_app().await;

    let (_s, _h, reg) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/auth/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({"username": "alice", "password": "password123"}))
                    .unwrap(),
            ))
            .unwrap(),
    )
    .await;
    let session = reg["token"].as_str().unwrap().to_string();

    let (_s, _h, client) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/oauth/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({"redirect_uris": [REDIRECT_URI]})).unwrap(),
            ))
            .unwrap(),
    )
    .await;
    let client_id = client["client_id"].as_str().unwrap().to_string();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256",
                    enc(&client_id), enc(REDIRECT_URI), enc(CHALLENGE),
                ))
                .header(header::COOKIE, format!("session={session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let location = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let code = extract_query(&location, "code").unwrap();

    // Wrong verifier ⇒ invalid_grant.
    let (status, _h, body) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/oauth/token")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(format!(
                "grant_type=authorization_code&code={}&redirect_uri={}&code_verifier=wrong&client_id={}",
                enc(&code), enc(REDIRECT_URI), enc(&client_id),
            )))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_grant");
}

#[tokio::test]
async fn authorize_without_login_bounces_to_login() {
    let app = server_app().await;
    let (_s, _h, client) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/oauth/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({"redirect_uris": [REDIRECT_URI]})).unwrap(),
            ))
            .unwrap(),
    )
    .await;
    let client_id = client["client_id"].as_str().unwrap().to_string();

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256",
                    enc(&client_id), enc(REDIRECT_URI), enc(CHALLENGE),
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_redirection());
    let location = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(location.starts_with("/?next="), "location: {location}");
}

#[tokio::test]
async fn local_service_token_authenticates_mcp() {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let token = ensure_local_service_token(&db).await.unwrap();
    // Reused (not duplicated) across calls.
    assert_eq!(ensure_local_service_token(&db).await.unwrap(), token);

    let app = build_router(AppState {
        db,
        mode: Mode::Local,
        engine_service: None,
        llm_provider: None,
    });
    assert_eq!(mcp_tools_list_status(&app, &token).await, StatusCode::OK);
}

/// POST `tools/list` to `/mcp` with a bearer and return the status.
async fn mcp_tools_list_status(app: &Router, bearer: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "jsonrpc": "2.0", "id": 1, "method": "tools/list"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// Pull a query-param value out of a URL (test helper; values here are simple).
fn extract_query(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}
