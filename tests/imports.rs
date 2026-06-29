//! Integration tests for the game-import HTTP surface in server mode: PGN upload
//! into a target database and the sync trigger's validation/authorization, all
//! exercised end-to-end through real auth tokens. (Provider syncs hit the network
//! and so are not driven here; only their pre-network validation is.)

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use chess_base::db::{connect, DbConfig};
use chess_base::server::{build_router, AppState, Mode};

const TWO_GAMES: &str = "[Event \"Game 1\"]\n[White \"Spassky\"]\n[Black \"Fischer\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n\n[Event \"Game 2\"]\n[White \"Carlsen\"]\n[Black \"Caruana\"]\n[Result \"1/2-1/2\"]\n\n1. d4 d5 2. c4 e6 1/2-1/2\n";

async fn server_app() -> Router {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    build_router(AppState {
        db,
        mode: Mode::Server,
        engine_service: None,
    })
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

/// Register a user and return their bearer token. The first registered user is
/// admin (auth bootstrap rule).
async fn register(app: &Router, username: &str) -> String {
    let (status, body) = send(
        app,
        json_req_anon(
            "POST",
            "/api/auth/register",
            json!({"username": username, "password": "password123"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    body["token"].as_str().unwrap().to_string()
}

fn json_req(method: &str, uri: &str, token: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn json_req_anon(method: &str, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn get_req(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

/// Create a database for `token` and return its id.
async fn create_db(app: &Router, token: &str, body: Value) -> i64 {
    let (status, db) = send(app, json_req("POST", "/api/databases", token, body)).await;
    assert_eq!(status, StatusCode::CREATED);
    db["id"].as_i64().unwrap()
}

#[tokio::test]
async fn pgn_upload_ingests_every_game_into_the_target_database() {
    let app = server_app().await;
    let alice = register(&app, "alice").await;
    let id = create_db(&app, &alice, json!({"name": "Mine", "kind": "own"})).await;

    let (status, body) = send(
        &app,
        json_req(
            "POST",
            "/api/import/pgn",
            &alice,
            json!({"database_id": id, "pgn": TWO_GAMES}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["imported"], 2);

    // The games are now listable from the database.
    let (status, list) = send(
        &app,
        get_req(&format!("/api/games?database_id={id}"), &alice),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["games"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn pgn_upload_rejects_an_empty_body() {
    let app = server_app().await;
    let alice = register(&app, "alice").await;
    let id = create_db(&app, &alice, json!({"name": "Mine", "kind": "own"})).await;

    let (status, body) = send(
        &app,
        json_req(
            "POST",
            "/api/import/pgn",
            &alice,
            json!({"database_id": id, "pgn": "   \n  "}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("empty"));
}

#[tokio::test]
async fn pgn_upload_reports_malformed_input() {
    let app = server_app().await;
    let alice = register(&app, "alice").await;
    let id = create_db(&app, &alice, json!({"name": "Mine", "kind": "own"})).await;

    // An illegal continuation: Black cannot answer 1. e4 with a second e4. Under
    // skip-and-continue (issue #96) a bad game is reported, not fatal — the upload
    // succeeds with zero imported and the game recorded in `errors`.
    let (status, body) = send(
        &app,
        json_req(
            "POST",
            "/api/import/pgn",
            &alice,
            json!({"database_id": id, "pgn": "[Event \"x\"]\n\n1. e4 e4 *"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["imported"], 0);
    assert_eq!(body["skipped"], 1);
    let errors = body["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    // The message is client-safe and indexed, never a raw SQL/anyhow chain.
    assert!(errors[0].as_str().unwrap().starts_with("game 1:"));
}

#[tokio::test]
async fn pgn_upload_skips_a_bad_game_and_keeps_the_rest() {
    let app = server_app().await;
    let alice = register(&app, "alice").await;
    let id = create_db(&app, &alice, json!({"name": "Mine", "kind": "own"})).await;

    // A good game followed by an illegal one: the good game commits, the bad one
    // is skipped — no all-or-nothing abort that would strand the client (issue #96).
    let pgn = "[Event \"Good\"]\n[White \"A\"]\n[Black \"B\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n\n[Event \"Bad\"]\n\n1. e4 e4 *\n";
    let (status, body) = send(
        &app,
        json_req(
            "POST",
            "/api/import/pgn",
            &alice,
            json!({"database_id": id, "pgn": pgn}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["imported"], 1);
    assert_eq!(body["skipped"], 1);

    // Only the good game landed in the database.
    let (status, list) = send(
        &app,
        get_req(&format!("/api/games?database_id={id}"), &alice),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["games"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn cannot_import_into_another_users_database() {
    let app = server_app().await;
    let _alice = register(&app, "alice").await; // first user → admin
    let bob = register(&app, "bob").await;
    let carol = register(&app, "carol").await;

    let bobs = create_db(&app, &bob, json!({"name": "Bob's", "kind": "own"})).await;

    let (status, _) = send(
        &app,
        json_req(
            "POST",
            "/api/import/pgn",
            &carol,
            json!({"database_id": bobs, "pgn": TWO_GAMES}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn sync_rejects_an_unknown_source() {
    let app = server_app().await;
    let alice = register(&app, "alice").await;
    let id = create_db(&app, &alice, json!({"name": "Mine", "kind": "lichess"})).await;

    let (status, body) = send(
        &app,
        json_req(
            "POST",
            "/api/import/sync",
            &alice,
            json!({"database_id": id, "source": "fics", "username": "x"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("fics"));
}

#[tokio::test]
async fn sync_requires_a_username() {
    let app = server_app().await;
    let alice = register(&app, "alice").await;
    let id = create_db(&app, &alice, json!({"name": "Mine", "kind": "lichess"})).await;

    let (status, _) = send(
        &app,
        json_req(
            "POST",
            "/api/import/sync",
            &alice,
            json!({"database_id": id, "source": "lichess", "username": "  "}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sync_forbids_an_unwritable_database_before_any_network() {
    let app = server_app().await;
    let _alice = register(&app, "alice").await;
    let bob = register(&app, "bob").await;
    let carol = register(&app, "carol").await;

    let bobs = create_db(&app, &bob, json!({"name": "Bob's", "kind": "lichess"})).await;

    let (status, _) = send(
        &app,
        json_req(
            "POST",
            "/api/import/sync",
            &carol,
            json!({"database_id": bobs, "source": "lichess", "username": "bob"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn import_requires_authentication() {
    let app = server_app().await;
    let (status, _) = send(
        &app,
        json_req_anon(
            "POST",
            "/api/import/pgn",
            json!({"database_id": 1, "pgn": TWO_GAMES}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
