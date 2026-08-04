//! Integration tests for public sharing (issue #211, ADR-0045) in server mode:
//! the per-object `public` flags on games and studies, the anonymous HTTP read
//! tier behind the `PublicUser` extractor (a request with no credential), and
//! the toggle endpoints' permission chains — exercised end-to-end through real
//! tokens and through credential-less requests.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
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

async fn app_with_db() -> (Router, DatabaseConnection) {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let app = build_router(AppState {
        db: db.clone(),
        mode: Mode::Server,
        engine_service: None,
        provider_store: None,
        agent: Default::default(),
    });
    (app, db)
}

/// Register a user, returning their bearer token and resolved owner id. The
/// first registered user on a deployment is the admin.
async fn register(app: &Router, username: &str) -> (String, String) {
    let resp = request(
        app,
        Method::POST,
        "/api/auth/register",
        None,
        Some(json!({"username": username, "password": "password123"})),
    )
    .await;
    assert_eq!(resp.0, StatusCode::CREATED);
    let token = resp.1["token"].as_str().unwrap().to_string();
    let (_, who) = request(app, Method::GET, "/api/whoami", Some(&token), None).await;
    (token, who["id"].as_str().unwrap().to_string())
}

/// Create a database (owner `None` ⇒ global) and ingest one game; returns
/// (database id, game id).
async fn seed_game(db: &DatabaseConnection, owner: Option<&str>) -> (i32, i32) {
    let model = databases::ActiveModel {
        owner_id: Set(owner.map(str::to_string)),
        name: Set("games".to_string()),
        kind: Set(if owner.is_some() { "own" } else { "master" }.to_string()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let ingested = ingest_pgn(db, model.id, SCHOLARS_MATE)
        .await
        .unwrap()
        .unwrap();
    (model.id, ingested.game_id)
}

/// Fire a request — `token: None` is the anonymous (credential-less) caller —
/// returning (status, parsed JSON body or Null).
async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap())),
        None => builder.body(Body::empty()),
    }
    .unwrap();
    let resp = app.clone().oneshot(request).await.unwrap();
    let status = resp.status();
    let body = serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes())
        .unwrap_or(Value::Null);
    (status, body)
}

/// GET `uri` as a download (optionally anonymous): (status, text body).
async fn download(app: &Router, uri: &str, token: Option<&str>) -> (StatusCode, String) {
    let mut builder = Request::builder().uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let resp = app
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Toggle a game's sharing flag as `token`, returning (status, body).
async fn share_game(app: &Router, game_id: i32, token: &str, public: bool) -> (StatusCode, Value) {
    request(
        app,
        Method::PUT,
        &format!("/api/games/{game_id}/public"),
        Some(token),
        Some(json!({ "public": public })),
    )
    .await
}

#[tokio::test]
async fn a_public_game_in_a_private_database_is_readable_anonymously() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    let (_db_id, game_id) = seed_game(&db, Some(&alice_id)).await;

    let (status, body) = share_game(&app, game_id, &alice, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["public"], true);

    // Anonymous deep link: detail, tree and export all serve the shared game.
    let (status, body) = request(
        &app,
        Method::GET,
        &format!("/api/games/{game_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["public"], true);
    assert_eq!(body["white"], "Spassky");

    let (status, tree) = request(
        &app,
        Method::GET,
        &format!("/api/games/{game_id}/tree"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tree["nodes"][1]["san"], "e4");

    let (status, pgn) = download(&app, &format!("/api/games/{game_id}/export"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(pgn.contains("Qxf7#"));
}

#[tokio::test]
async fn a_non_public_game_in_a_private_database_stays_hidden() {
    let (app, db) = app_with_db().await;
    let (_alice, alice_id) = register(&app, "alice").await; // first user → admin
    let (bob, _bob_id) = register(&app, "bob").await;
    let (_db_id, game_id) = seed_game(&db, Some(&alice_id)).await;

    for uri in [
        format!("/api/games/{game_id}"),
        format!("/api/games/{game_id}/tree"),
        format!("/api/games/{game_id}/export"),
    ] {
        let (status, _) = request(&app, Method::GET, &uri, None, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "anonymous {uri}");
        let (status, _) = request(&app, Method::GET, &uri, Some(&bob), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "stranger {uri}");
    }
}

#[tokio::test]
async fn anonymous_writes_are_unauthorized() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    let (_db_id, game_id) = seed_game(&db, Some(&alice_id)).await;
    share_game(&app, game_id, &alice, true).await;

    // Even on a public game, the anonymous tier is read-only.
    let (status, _) = request(
        &app,
        Method::DELETE,
        &format!("/api/games/{game_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = request(
        &app,
        Method::PUT,
        &format!("/api/games/{game_id}/public"),
        None,
        Some(json!({ "public": false })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A public study can be read but never mutated anonymously.
    let study_id = save_analysis(&app, game_id, &alice, "shared").await;
    share_study(&app, study_id, &alice, true).await;
    let (status, _) = request(
        &app,
        Method::PUT,
        &format!("/api/studies/{study_id}/public"),
        None,
        Some(json!({ "public": false })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = request(
        &app,
        Method::POST,
        &format!("/api/studies/{study_id}/moves"),
        None,
        Some(json!({ "from_node_id": 0, "san": "e4" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn anonymous_annotated_export_is_unauthorized() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    let (_db_id, game_id) = seed_game(&db, Some(&alice_id)).await;
    share_game(&app, game_id, &alice, true).await;

    // The annotated export runs the engine — never on the anonymous tier
    // (checked before the missing-engine 503 this fixture would otherwise hit).
    let (status, _) = download(
        &app,
        &format!("/api/games/{game_id}/export?annotated=true"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Create an analysis (study linked to `game_id`) and return its id.
async fn save_analysis(app: &Router, game_id: i32, token: &str, name: &str) -> i32 {
    let (status, body) = request(
        app,
        Method::POST,
        &format!("/api/games/{game_id}/save-as-study"),
        Some(token),
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    body["id"].as_i64().unwrap() as i32
}

/// Toggle a study's sharing flag as `token`, returning (status, body).
async fn share_study(
    app: &Router,
    study_id: i32,
    token: &str,
    public: bool,
) -> (StatusCode, Value) {
    request(
        app,
        Method::PUT,
        &format!("/api/studies/{study_id}/public"),
        Some(token),
        Some(json!({ "public": public })),
    )
    .await
}

#[tokio::test]
async fn linked_studies_show_anonymous_only_the_public_ones() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    let (_db_id, game_id) = seed_game(&db, Some(&alice_id)).await;
    share_game(&app, game_id, &alice, true).await;

    let shared = save_analysis(&app, game_id, &alice, "shared analysis").await;
    let _private = save_analysis(&app, game_id, &alice, "private analysis").await;
    let (status, body) = share_study(&app, shared, &alice, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["public"], true);

    let uri = format!("/api/games/{game_id}/studies");
    let (status, anon) = request(&app, Method::GET, &uri, None, None).await;
    assert_eq!(status, StatusCode::OK);
    let anon = anon.as_array().unwrap();
    assert_eq!(anon.len(), 1);
    assert_eq!(anon[0]["name"], "shared analysis");
    assert_eq!(anon[0]["public"], true);

    let (_, own) = request(&app, Method::GET, &uri, Some(&alice), None).await;
    assert_eq!(own.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn a_public_study_is_readable_anonymously_but_private_and_global_are_not() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await; // first user → admin
    let (db_id, game_id) = seed_game(&db, Some(&alice_id)).await;

    let shared = save_analysis(&app, game_id, &alice, "shared").await;
    let private = save_analysis(&app, game_id, &alice, "private").await;
    share_study(&app, shared, &alice, true).await;
    // A global (owner NULL), non-public study — alice is the admin here.
    let (status, global) = request(
        &app,
        Method::POST,
        "/api/studies",
        Some(&alice),
        Some(json!({ "database_id": db_id, "name": "global", "global": true })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let global_id = global["id"].as_i64().unwrap();

    let (status, body) = request(
        &app,
        Method::GET,
        &format!("/api/studies/{shared}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["public"], true);
    let (status, pgn) = download(&app, &format!("/api/studies/{shared}/export"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(pgn.contains("e4"));

    // Neither the private one nor the global one is on the anonymous tier.
    for id in [private as i64, global_id] {
        let (status, _) =
            request(&app, Method::GET, &format!("/api/studies/{id}"), None, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "study {id}");
        let (status, _) = download(&app, &format!("/api/studies/{id}/export"), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "study {id} export");
    }
}

#[tokio::test]
async fn toggle_permissions_follow_the_write_chain() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await; // first user → admin
    let (bob, bob_id) = register(&app, "bob").await;

    // Bob owns a database: he toggles his own game; a stranger's is hidden.
    let (_bob_db, bob_game) = seed_game(&db, Some(&bob_id)).await;
    let (status, body) = share_game(&app, bob_game, &bob, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["public"], true);
    // Alice's private game: bob can't even see it → 404.
    let (_alice_db, alice_game) = seed_game(&db, Some(&alice_id)).await;
    let (status, _) = share_game(&app, alice_game, &bob, true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A game in a global database: visible to bob, but toggling requires admin.
    let (_global_db, global_game) = seed_game(&db, None).await;
    let (status, _) = share_game(&app, global_game, &bob, true).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = share_game(&app, global_game, &alice, true).await;
    assert_eq!(status, StatusCode::OK);

    // Studies: the owner toggles, a stranger is denied.
    let study = save_analysis(&app, bob_game, &bob, "bob's analysis").await;
    let (status, body) = share_study(&app, study, &bob, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["public"], true);
    let (status, _) = share_study(&app, study, &alice, false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
