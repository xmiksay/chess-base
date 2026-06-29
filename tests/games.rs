//! Integration tests for game listing over the HTTP layer in server mode: the
//! keyset-paginated list endpoint, single-game fetch (with PGN), and the scoping
//! rules (own vs global vs other-user), exercised end-to-end through real tokens.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use chess_base::db::entities::databases;
use chess_base::db::{connect, DbConfig};
use chess_base::ingest_pgn;
use chess_base::server::{build_router, AppState, Mode};
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};

const SCHOLARS_MATE: &str =
    "[White \"Spassky\"]\n[Black \"Fischer\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n";
const QUEENS_DRAW: &str =
    "[White \"Carlsen\"]\n[Black \"Caruana\"]\n[Result \"1/2-1/2\"]\n\n1. d4 d5 2. c4 e6 1/2-1/2\n";

async fn app_with_db() -> (Router, DatabaseConnection) {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let app = build_router(AppState {
        db: db.clone(),
        mode: Mode::Server,
        engine_service: None,
        llm_provider: None,
    });
    (app, db)
}

/// Register a user, returning their bearer token and resolved owner id.
async fn register(app: &Router, username: &str) -> (String, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({"username": username, "password": "password123"}))
                        .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    let who = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/whoami")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let who: Value =
        serde_json::from_slice(&who.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let id = who["id"].as_str().unwrap().to_string();
    (token, id)
}

/// Create an own database for `owner` and ingest the given PGNs into it.
async fn seed(db: &DatabaseConnection, owner: &str, pgns: &[&str]) -> i32 {
    let model = databases::ActiveModel {
        owner_id: Set(Some(owner.to_string())),
        name: Set(format!("{owner}'s games")),
        kind: Set("own".to_string()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    for pgn in pgns {
        ingest_pgn(db, model.id, pgn).await.unwrap();
    }
    model.id
}

/// GET `uri` with a bearer token, returning (status, parsed JSON body).
async fn get(app: &Router, uri: &str, token: &str) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes())
        .unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn list_returns_games_in_database() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    let db_id = seed(&db, &alice_id, &[SCHOLARS_MATE, QUEENS_DRAW]).await;

    let (status, body) = get(&app, &format!("/api/games?database_id={db_id}"), &alice).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["games"].as_array().unwrap().len(), 2);
    assert_eq!(body["games"][0]["white"], "Spassky");
    assert_eq!(body["next_cursor"], Value::Null);
}

#[tokio::test]
async fn list_paginates_with_cursor() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    let db_id = seed(&db, &alice_id, &[SCHOLARS_MATE, QUEENS_DRAW, SCHOLARS_MATE]).await;

    let (_, first) = get(
        &app,
        &format!("/api/games?database_id={db_id}&limit=2"),
        &alice,
    )
    .await;
    assert_eq!(first["games"].as_array().unwrap().len(), 2);
    let cursor = first["next_cursor"].as_i64().expect("cursor present");

    let (_, second) = get(
        &app,
        &format!("/api/games?database_id={db_id}&limit=2&after={cursor}"),
        &alice,
    )
    .await;
    assert_eq!(second["games"].as_array().unwrap().len(), 1);
    assert_eq!(second["next_cursor"], Value::Null);
}

#[tokio::test]
async fn get_single_game_returns_pgn() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    let db_id = seed(&db, &alice_id, &[SCHOLARS_MATE]).await;

    let (_, list) = get(&app, &format!("/api/games?database_id={db_id}"), &alice).await;
    let game_id = list["games"][0]["id"].as_i64().unwrap();

    let (status, body) = get(&app, &format!("/api/games/{game_id}"), &alice).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["white"], "Spassky");
    assert_eq!(body["variant"], "standard");
    assert!(body["pgn"].as_str().unwrap().contains("Qxf7#"));
}

#[tokio::test]
async fn list_scope_excludes_other_users_database() {
    let (app, db) = app_with_db().await;
    let (_alice, alice_id) = register(&app, "alice").await; // first user → admin
    let (bob, _bob_id) = register(&app, "bob").await;
    let alice_db = seed(&db, &alice_id, &[SCHOLARS_MATE]).await;

    let (status, _) = get(&app, &format!("/api/games?database_id={alice_db}"), &bob).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_missing_game_is_not_found() {
    let (app, _db) = app_with_db().await;
    let (alice, _alice_id) = register(&app, "alice").await;
    let (status, _) = get(&app, "/api/games/9999", &alice).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unauthenticated_list_is_rejected() {
    let (app, _db) = app_with_db().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/games?database_id=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
