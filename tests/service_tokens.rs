//! Integration tests for the admin service-token minting HTTP surface
//! (ADR-0044, issue #193): `POST`/`GET`/`DELETE /api/admin/service-tokens`.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use chess_base::db::{connect, DbConfig};
use chess_base::server::{build_router, AppState, Mode};

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

/// Local mode is always the implicit admin, so the create/list/revoke happy
/// path is exercised there without any session plumbing.
#[tokio::test]
async fn admin_mints_lists_and_revokes_a_token() {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let app = build_router(AppState {
        db,
        mode: Mode::Local,
        engine_service: None,
        provider_store: None,
        agent: Default::default(),
    });

    let (status, minted) = send(
        &app,
        post_json(
            "/api/admin/service-tokens",
            json!({ "owner_id": "alice", "label": "ci", "scope": "read_only" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{minted}");
    let token = minted["token"].as_str().unwrap().to_string();
    let id = minted["id"].as_str().unwrap().to_string();
    assert!(!token.is_empty());
    assert_ne!(token, id);

    let (status, listed) = send(
        &app,
        Request::builder()
            .uri("/api/admin/service-tokens")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rows = listed.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], id);
    // The raw secret is never re-displayed after minting.
    assert!(rows[0].get("token").is_none());

    let (status, _body) = send(
        &app,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/admin/service-tokens/{id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, listed) = send(
        &app,
        Request::builder()
            .uri("/api/admin/service-tokens")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed.as_array().unwrap().is_empty());
}

/// A non-admin server-mode user is forbidden on every route.
#[tokio::test]
async fn non_admin_is_forbidden_on_every_route() {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let app = build_router(AppState {
        db,
        mode: Mode::Server,
        engine_service: None,
        provider_store: None,
        agent: Default::default(),
    });

    // First user is admin; register a second, non-admin one.
    send(
        &app,
        post_json(
            "/api/auth/register",
            json!({"username": "admin", "password": "password123"}),
        ),
    )
    .await;
    let (status, reg) = send(
        &app,
        post_json(
            "/api/auth/register",
            json!({"username": "bob", "password": "password123"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let session = reg["token"].as_str().unwrap().to_string();

    let (status, _body) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/admin/service-tokens")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, format!("session={session}"))
            .body(Body::from(
                serde_json::to_vec(&json!({ "owner_id": "bob", "label": "x", "scope": "full" }))
                    .unwrap(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _body) = send(
        &app,
        Request::builder()
            .uri("/api/admin/service-tokens")
            .header(header::COOKIE, format!("session={session}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _body) = send(
        &app,
        Request::builder()
            .method("DELETE")
            .uri("/api/admin/service-tokens/whatever")
            .header(header::COOKIE, format!("session={session}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
