//! Integration tests for the folder hierarchy + game-linked analyses HTTP API
//! (issue #164): folder CRUD with nesting/cycle/cascade, filing a study into a
//! folder, and "save as analysis" from a game with its linked-analyses listing.
//! Exercised end-to-end through real auth tokens.

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

const GAME: &str =
    "[White \"Spassky\"]\n[Black \"Fischer\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Nf3 Nc6 3. Bb5 *\n";

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

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
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

fn get_req(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

fn delete_req(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

/// Register a user, returning (token, owner_id). The first user is admin.
async fn register(app: &Router, username: &str) -> (String, String) {
    let (status, body) = send(
        app,
        json_req(
            "POST",
            "/api/auth/register",
            "",
            json!({"username": username, "password": "password123"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let token = body["token"].as_str().unwrap().to_string();
    let (_, who) = send(app, get_req("/api/whoami", &token)).await;
    (token, who["id"].as_str().unwrap().to_string())
}

/// Create an own database for `owner`, ingest one game, return (database_id, game_id).
async fn seed_game(app: &Router, db: &DatabaseConnection, token: &str, owner: &str) -> (i32, i64) {
    let model = databases::ActiveModel {
        owner_id: Set(Some(owner.to_string())),
        name: Set("Games".to_string()),
        kind: Set("own".to_string()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    ingest_pgn(db, model.id, GAME).await.unwrap();
    let (_, page) = send(
        app,
        get_req(&format!("/api/games?database_id={}", model.id), token),
    )
    .await;
    let game_id = page["games"][0]["id"].as_i64().unwrap();
    (model.id, game_id)
}

#[tokio::test]
async fn folder_crud_nests_renames_and_lists() {
    let (app, _db) = app_with_db().await;
    let (alice, _) = register(&app, "alice").await;

    let (status, openings) = send(
        &app,
        json_req("POST", "/api/folders", &alice, json!({"name": "Openings"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(openings["parent_id"], Value::Null);
    assert_eq!(openings["global"], false);
    let openings_id = openings["id"].as_i64().unwrap();

    // A nested child.
    let (status, child) = send(
        &app,
        json_req(
            "POST",
            "/api/folders",
            &alice,
            json!({"name": "Sicilian", "parent_id": openings_id}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(child["parent_id"], openings_id);

    // Duplicate sibling name → 409.
    let (status, _) = send(
        &app,
        json_req("POST", "/api/folders", &alice, json!({"name": "Openings"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Rename via PATCH.
    let (status, renamed) = send(
        &app,
        json_req(
            "PATCH",
            &format!("/api/folders/{openings_id}"),
            &alice,
            json!({"name": "Repertoire"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(renamed["name"], "Repertoire");

    let (status, list) = send(&app, get_req("/api/folders", &alice)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn move_into_descendant_is_rejected() {
    let (app, _db) = app_with_db().await;
    let (alice, _) = register(&app, "alice").await;

    let (_, a) = send(
        &app,
        json_req("POST", "/api/folders", &alice, json!({"name": "A"})),
    )
    .await;
    let a_id = a["id"].as_i64().unwrap();
    let (_, b) = send(
        &app,
        json_req(
            "POST",
            "/api/folders",
            &alice,
            json!({"name": "B", "parent_id": a_id}),
        ),
    )
    .await;
    let b_id = b["id"].as_i64().unwrap();

    // Move A under B (its own child) → 400.
    let (status, _) = send(
        &app,
        json_req(
            "PATCH",
            &format!("/api/folders/{a_id}"),
            &alice,
            json!({"reparent": true, "parent_id": b_id}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn deleting_a_folder_unfiles_its_studies() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    let (db_id, _game_id) = seed_game(&app, &db, &alice, &alice_id).await;

    let (_, folder) = send(
        &app,
        json_req("POST", "/api/folders", &alice, json!({"name": "Box"})),
    )
    .await;
    let folder_id = folder["id"].as_i64().unwrap();

    // A study filed into the folder.
    let (_, study) = send(
        &app,
        json_req(
            "POST",
            "/api/studies",
            &alice,
            json!({"database_id": db_id, "name": "Line"}),
        ),
    )
    .await;
    let study_id = study["id"].as_i64().unwrap();
    let (status, filed) = send(
        &app,
        json_req(
            "PUT",
            &format!("/api/studies/{study_id}/folder"),
            &alice,
            json!({"folder_id": folder_id}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(filed["folder_id"], folder_id);

    // Delete the folder → 204, and the study comes back unfiled.
    let (status, _) = send(
        &app,
        delete_req(&format!("/api/folders/{folder_id}"), &alice),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, reloaded) = send(&app, get_req(&format!("/api/studies/{study_id}"), &alice)).await;
    assert_eq!(reloaded["folder_id"], Value::Null);
}

#[tokio::test]
async fn save_as_study_links_origin_and_lists_for_the_game() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    let (db_id, game_id) = seed_game(&app, &db, &alice, &alice_id).await;

    let (_, folder) = send(
        &app,
        json_req("POST", "/api/folders", &alice, json!({"name": "Analyses"})),
    )
    .await;
    let folder_id = folder["id"].as_i64().unwrap();

    // Save the game as an analysis (no engine → analyse omitted/false).
    let (status, study) = send(
        &app,
        json_req(
            "POST",
            &format!("/api/games/{game_id}/save-as-study"),
            &alice,
            json!({"name": "From game", "folder_id": folder_id}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(study["origin_game_id"].as_i64().unwrap(), game_id);
    assert_eq!(study["folder_id"], folder_id);
    assert_eq!(study["database_id"].as_i64().unwrap() as i32, db_id);

    // The game lists its linked analysis.
    let (status, linked) = send(
        &app,
        get_req(&format!("/api/games/{game_id}/studies"), &alice),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let linked = linked.as_array().unwrap();
    assert_eq!(linked.len(), 1);
    assert_eq!(linked[0]["name"], "From game");

    // The persisted study carries the game's mainline.
    let study_id = study["id"].as_i64().unwrap();
    let (_, full) = send(&app, get_req(&format!("/api/studies/{study_id}"), &alice)).await;
    let sans: Vec<String> = full["tree"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["san"].as_str().map(str::to_string))
        .collect();
    assert_eq!(sans, vec!["e4", "e5", "Nf3", "Nc6", "Bb5"]);
}

#[tokio::test]
async fn save_as_study_with_analyse_but_no_engine_is_unavailable() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    let (_db_id, game_id) = seed_game(&app, &db, &alice, &alice_id).await;

    let (status, _) = send(
        &app,
        json_req(
            "POST",
            &format!("/api/games/{game_id}/save-as-study"),
            &alice,
            json!({"name": "Engine please", "analyse": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn folders_and_studies_are_scoped_to_their_owner() {
    let (app, db) = app_with_db().await;
    let (_admin, _) = register(&app, "alice").await; // first user → admin
    let (bob, bob_id) = register(&app, "bob").await;
    let (carol, _carol_id) = register(&app, "carol").await;
    let (_db_id, game_id) = seed_game(&app, &db, &bob, &bob_id).await;

    let (_, folder) = send(
        &app,
        json_req("POST", "/api/folders", &bob, json!({"name": "Bob's"})),
    )
    .await;
    let folder_id = folder["id"].as_i64().unwrap();

    // Carol can't see Bob's folder (list empty) and can't delete it (403, the
    // same write-guard the study service uses for another user's resource).
    let (_, carol_list) = send(&app, get_req("/api/folders", &carol)).await;
    assert!(carol_list.as_array().unwrap().is_empty());
    let (status, _) = send(
        &app,
        delete_req(&format!("/api/folders/{folder_id}"), &carol),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Carol can't save Bob's game as an analysis (game not visible → 404).
    let (status, _) = send(
        &app,
        json_req(
            "POST",
            &format!("/api/games/{game_id}/save-as-study"),
            &carol,
            json!({"name": "sneaky"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
