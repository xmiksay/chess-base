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
/// A game whose movetext carries a `(…)` sub-variation on Black's first move.
const WITH_VARIATION: &str =
    "[White \"V\"]\n[Black \"W\"]\n[Result \"*\"]\n\n1. e4 e5 (1... c5 2. Nf3) 2. Nf3 *\n";
/// A game set up from a non-standard position via SetUp/`[FEN]` (a queen endgame).
const SETUP_ENDGAME: &str = "[White \"A\"]\n[Black \"B\"]\n[SetUp \"1\"]\n[FEN \"4k3/8/8/8/8/8/8/3QK3 w - - 0 1\"]\n[Result \"*\"]\n\n1. Qd8+ Kf7 *\n";

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
    // Default sort is newest-first; with no Date tag the last-added game leads.
    assert_eq!(body["games"][0]["white"], "Carlsen");
    assert_eq!(body["total"], 2);
    assert_eq!(body["page"], 0);
}

#[tokio::test]
async fn list_paginates_with_offset() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    // A distinct `[Round]` keeps the rematch's content hash apart (ingest dedup).
    let rematch = format!("[Round \"2\"]\n{SCHOLARS_MATE}");
    let db_id = seed(&db, &alice_id, &[SCHOLARS_MATE, QUEENS_DRAW, &rematch]).await;

    let (_, first) = get(
        &app,
        &format!("/api/games?database_id={db_id}&limit=2"),
        &alice,
    )
    .await;
    assert_eq!(first["games"].as_array().unwrap().len(), 2);
    assert_eq!(first["total"], 3);
    assert_eq!(first["page"], 0);

    let (_, second) = get(
        &app,
        &format!("/api/games?database_id={db_id}&limit=2&page=1"),
        &alice,
    )
    .await;
    assert_eq!(second["games"].as_array().unwrap().len(), 1);
    assert_eq!(second["total"], 3);
    assert_eq!(second["page"], 1);
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

/// The id of the (single) game seeded into `database_id`.
async fn first_game_id(app: &Router, db_id: i32, token: &str) -> i64 {
    let (_, list) = get(app, &format!("/api/games?database_id={db_id}"), token).await;
    list["games"][0]["id"].as_i64().unwrap()
}

#[tokio::test]
async fn tree_returns_linear_tree_for_a_mainline_game() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    let db_id = seed(&db, &alice_id, &[QUEENS_DRAW]).await;
    let game_id = first_game_id(&app, db_id, &alice).await;

    let (status, body) = get(&app, &format!("/api/games/{game_id}/tree"), &alice).await;
    assert_eq!(status, StatusCode::OK);
    // d4 d5 c4 e6 ⇒ root + 4 nodes, each with a single child (a straight line).
    let nodes = body["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 5);
    for node in nodes {
        assert!(node["children"].as_array().unwrap().len() <= 1);
    }
    assert_eq!(nodes[1]["san"], "d4");
}

#[tokio::test]
async fn tree_preserves_a_sub_variation_as_a_second_child() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    let db_id = seed(&db, &alice_id, &[WITH_VARIATION]).await;
    let game_id = first_game_id(&app, db_id, &alice).await;

    let (status, body) = get(&app, &format!("/api/games/{game_id}/tree"), &alice).await;
    assert_eq!(status, StatusCode::OK);
    // The `(1... c5 …)` variation makes e4's node carry two children (e5 + c5),
    // which the chess.js flattener would have dropped.
    let nodes = body["nodes"].as_array().unwrap();
    let branching = nodes
        .iter()
        .find(|n| n["children"].as_array().unwrap().len() == 2)
        .expect("a node branches into mainline + variation");
    assert_eq!(branching["san"], "e4");
}

#[tokio::test]
async fn tree_honours_a_setup_start_position() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    let db_id = seed(&db, &alice_id, &[SETUP_ENDGAME]).await;
    let game_id = first_game_id(&app, db_id, &alice).await;

    let (status, body) = get(&app, &format!("/api/games/{game_id}/tree"), &alice).await;
    // `Qd8+` is illegal from the standard start; a 200 with the right first move
    // proves the game's `start_fen` was threaded into the parser.
    assert_eq!(status, StatusCode::OK);
    let nodes = body["nodes"].as_array().unwrap();
    assert_eq!(nodes[1]["san"], "Qd8+");
    assert_eq!(nodes[2]["san"], "Kf7");
}

#[tokio::test]
async fn tree_hides_a_game_in_another_users_database() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await; // first user → admin
    let (bob, _bob_id) = register(&app, "bob").await;
    let alice_db = seed(&db, &alice_id, &[QUEENS_DRAW]).await;
    let game_id = first_game_id(&app, alice_db, &alice).await;

    let (status, _) = get(&app, &format!("/api/games/{game_id}/tree"), &bob).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// GET `uri` as a download: returns (status, Content-Disposition, text body).
async fn get_download(
    app: &Router,
    uri: &str,
    token: &str,
) -> (StatusCode, Option<String>, String) {
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
    let disposition = resp
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        disposition,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

#[tokio::test]
async fn export_downloads_the_stored_pgn_verbatim() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    let db_id = seed(&db, &alice_id, &[SCHOLARS_MATE]).await;

    let (_, list) = get(&app, &format!("/api/games?database_id={db_id}"), &alice).await;
    let game_id = list["games"][0]["id"].as_i64().unwrap();

    let (status, disposition, pgn) =
        get_download(&app, &format!("/api/games/{game_id}/export"), &alice).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        disposition.as_deref(),
        Some(&format!("attachment; filename=\"game-{game_id}.pgn\"")[..])
    );
    // The stored game movetext comes back verbatim.
    assert!(pgn.contains("Qxf7#"));
}

#[tokio::test]
async fn annotated_export_without_engine_is_unavailable() {
    // `app_with_db` configures no engine, so an annotated export reports the
    // operator-configuration gap (503) instead of attempting analysis.
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    let db_id = seed(&db, &alice_id, &[SCHOLARS_MATE]).await;

    let (_, list) = get(&app, &format!("/api/games?database_id={db_id}"), &alice).await;
    let game_id = list["games"][0]["id"].as_i64().unwrap();

    let (status, _, _) = get_download(
        &app,
        &format!("/api/games/{game_id}/export?annotated=true"),
        &alice,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

/// POST `uri` with a JSON body as a download: returns (status, Content-Disposition,
/// text body) — the `POST` counterpart of [`get_download`], for the bulk export
/// endpoint which takes its ids as a body rather than a path segment.
async fn post_download(
    app: &Router,
    uri: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Option<String>, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let disposition = resp
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        disposition,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

#[tokio::test]
async fn export_many_bundles_selected_games_into_one_pgn() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    let db_id = seed(&db, &alice_id, &[SCHOLARS_MATE, QUEENS_DRAW]).await;

    let (_, list) = get(&app, &format!("/api/games?database_id={db_id}"), &alice).await;
    let ids: Vec<i64> = list["games"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["id"].as_i64().unwrap())
        .collect();

    let (status, disposition, pgn) = post_download(
        &app,
        "/api/games/export",
        &alice,
        json!({ "game_ids": ids }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        disposition.as_deref(),
        Some("attachment; filename=\"games-export.pgn\"")
    );
    assert!(pgn.contains("Qxf7#"));
    assert!(pgn.contains("Carlsen"));
    // Games are separated by a blank line, as PGN multi-game files require.
    assert!(pgn.contains("\n\n"));
}

#[tokio::test]
async fn export_many_hides_a_game_in_another_users_database() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await; // first user → admin
    let (bob, _bob_id) = register(&app, "bob").await;
    let alice_db = seed(&db, &alice_id, &[SCHOLARS_MATE]).await;
    let game_id = first_game_id(&app, alice_db, &alice).await;

    let (status, _, _) = post_download(
        &app,
        "/api/games/export",
        &bob,
        json!({ "game_ids": [game_id] }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn export_many_rejects_an_empty_selection() {
    let (app, _db) = app_with_db().await;
    let (alice, _alice_id) = register(&app, "alice").await;

    let (status, _, _) =
        post_download(&app, "/api/games/export", &alice, json!({ "game_ids": [] })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// DELETE `uri` with a bearer token, returning the status.
async fn delete(app: &Router, uri: &str, token: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn owner_deletes_a_game_and_it_no_longer_lists() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    let db_id = seed(&db, &alice_id, &[SCHOLARS_MATE, QUEENS_DRAW]).await;
    let game_id = first_game_id(&app, db_id, &alice).await;

    assert_eq!(
        delete(&app, &format!("/api/games/{game_id}"), &alice).await,
        StatusCode::NO_CONTENT
    );

    // It is gone from the list and from a direct fetch.
    let (_, list) = get(&app, &format!("/api/games?database_id={db_id}"), &alice).await;
    assert_eq!(list["games"].as_array().unwrap().len(), 1);
    let (status, _) = get(&app, &format!("/api/games/{game_id}"), &alice).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn non_owner_cannot_delete_a_game() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await; // first user → admin
    let (bob, _bob_id) = register(&app, "bob").await;
    let alice_db = seed(&db, &alice_id, &[SCHOLARS_MATE]).await;
    let game_id = first_game_id(&app, alice_db, &alice).await;

    // Bob cannot see alice's private database, so the id is hidden as 404.
    assert_eq!(
        delete(&app, &format!("/api/games/{game_id}"), &bob).await,
        StatusCode::NOT_FOUND
    );
    // The game still exists for its owner.
    let (status, _) = get(&app, &format!("/api/games/{game_id}"), &alice).await;
    assert_eq!(status, StatusCode::OK);
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
