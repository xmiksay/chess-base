//! Integration tests for the studies HTTP API in server mode. Issue #9 covers the
//! lifecycle CRUD and PGN import/export (create/list/rename/delete, round-trip a
//! study through PGN); issue #18 covers node mutation (add SAN-validated
//! moves/variations, annotate, promote / reorder / delete). Both exercise the
//! ownership rules (own vs global-admin vs other-user) through real auth tokens.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use chess_base::db::{connect, DbConfig};
use chess_base::ingest::ingest_pgn;
use chess_base::server::{build_router, AppState, Mode};
use sea_orm::DatabaseConnection;

async fn server_app() -> Router {
    server_app_with_db().await.0
}

/// Like [`server_app`] but also hands back the DB connection so a test can seed
/// games directly (the merge-games route folds stored games).
async fn server_app_with_db() -> (Router, DatabaseConnection) {
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
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

/// Like [`send`] but for downloads: returns the status, the `Content-Disposition`
/// header (if any) and the raw text body (issue #120 `.pgn` exports).
async fn send_download(app: &Router, req: Request<Body>) -> (StatusCode, Option<String>, String) {
    let resp = app.clone().oneshot(req).await.unwrap();
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

/// Register a user and return their bearer token. The first registered user is
/// admin (auth bootstrap rule).
async fn register(app: &Router, username: &str) -> String {
    let (status, body) = send(
        app,
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

/// Create a database owned by `token`, returning its id.
async fn make_database(app: &Router, token: &str) -> i64 {
    let (status, db) = send(
        app,
        json_req(
            "POST",
            "/api/databases",
            token,
            json!({"name": "Games", "kind": "own"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    db["id"].as_i64().unwrap()
}

#[tokio::test]
async fn mutation_lifecycle_within_a_study() {
    let app = server_app().await;
    let _admin = register(&app, "alice").await; // first user → admin
    let bob = register(&app, "bob").await;
    let db_id = make_database(&app, &bob).await;

    // Create an empty study (just the root node).
    let (status, study) = send(
        &app,
        json_req(
            "POST",
            "/api/studies",
            &bob,
            json!({"database_id": db_id, "name": "Sicilian"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(study["global"], false);
    assert_eq!(study["tree"]["nodes"].as_array().unwrap().len(), 1);
    let study_id = study["id"].as_i64().unwrap();

    // Add e4 from the root, then two replies: c5 (mainline) and e5 (variation).
    let (status, added) = send(
        &app,
        json_req(
            "POST",
            &format!("/api/studies/{study_id}/moves"),
            &bob,
            json!({"from_node_id": 0, "san": "e4"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let e4 = added["new_node_id"].as_u64().unwrap();
    assert_eq!(added["study"]["tree"]["nodes"].as_array().unwrap().len(), 2);

    let (_, c5_res) = send(
        &app,
        json_req(
            "POST",
            &format!("/api/studies/{study_id}/moves"),
            &bob,
            json!({"from_node_id": e4, "san": "c5"}),
        ),
    )
    .await;
    let _c5 = c5_res["new_node_id"].as_u64().unwrap();
    let (_, e5_res) = send(
        &app,
        json_req(
            "POST",
            &format!("/api/studies/{study_id}/moves"),
            &bob,
            json!({"from_node_id": e4, "san": "e5"}),
        ),
    )
    .await;
    let e5 = e5_res["new_node_id"].as_u64().unwrap();

    // An illegal SAN is rejected (validated via position.rs), not stored.
    let (status, body) = send(
        &app,
        json_req(
            "POST",
            &format!("/api/studies/{study_id}/moves"),
            &bob,
            json!({"from_node_id": 0, "san": "e5"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("illegal"));

    // Annotate e4 with a comment and a NAG.
    let (status, view) = send(
        &app,
        json_req(
            "POST",
            &format!("/api/studies/{study_id}/nodes/{e4}/annotate"),
            &bob,
            json!({"comment": "best by test", "nag": 1}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        view["tree"]["nodes"][e4 as usize]["comment"],
        "best by test"
    );
    assert_eq!(view["tree"]["nodes"][e4 as usize]["nags"][0], 1);

    // Promote the e5 variation to the mainline.
    let (status, view) = send(
        &app,
        json_req(
            "POST",
            &format!("/api/studies/{study_id}/nodes/{e5}/promote"),
            &bob,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["tree"]["nodes"][e4 as usize]["children"][0], e5);

    // Reorder it back to second place: c5 leads again.
    let (status, view) = send(
        &app,
        json_req(
            "POST",
            &format!("/api/studies/{study_id}/nodes/{e5}/reorder"),
            &bob,
            json!({"index": 1}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["tree"]["nodes"][e4 as usize]["children"][1], e5);

    // Delete e4 and its subtree: the tree shrinks back to the root.
    let (status, view) = send(
        &app,
        delete_req(&format!("/api/studies/{study_id}/nodes/{e4}"), &bob),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["tree"]["nodes"].as_array().unwrap().len(), 1);

    // Deleting the root is a bad edit, not a server error.
    let (status, _) = send(
        &app,
        delete_req(&format!("/api/studies/{study_id}/nodes/0"), &bob),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn pinning_shapes_persists_across_reload() {
    let app = server_app().await;
    let _admin = register(&app, "alice").await; // first user → admin
    let bob = register(&app, "bob").await;
    let db_id = make_database(&app, &bob).await;

    let (_, study) = send(
        &app,
        json_req(
            "POST",
            "/api/studies",
            &bob,
            json!({"database_id": db_id, "name": "Plans"}),
        ),
    )
    .await;
    let study_id = study["id"].as_i64().unwrap();
    let (_, added) = send(
        &app,
        json_req(
            "POST",
            &format!("/api/studies/{study_id}/moves"),
            &bob,
            json!({"from_node_id": 0, "san": "e4"}),
        ),
    )
    .await;
    let e4 = added["new_node_id"].as_u64().unwrap();

    // Pin a plan: an arrow g1→f3 and a single-square highlight on e4.
    let shapes = json!([
        {"orig": "g1", "dest": "f3", "brush": "green"},
        {"orig": "e4", "brush": "blue"}
    ]);
    let (status, view) = send(
        &app,
        json_req(
            "PUT",
            &format!("/api/studies/{study_id}/nodes/{e4}/shapes"),
            &bob,
            json!({"shapes": shapes}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["tree"]["nodes"][e4 as usize]["shapes"], shapes);

    // Reload the study from scratch: the pinned shapes reappear, and the
    // highlight (no dest) does not carry a `dest` key.
    let (status, reloaded) = send(&app, get_req(&format!("/api/studies/{study_id}"), &bob)).await;
    assert_eq!(status, StatusCode::OK);
    let pinned = &reloaded["tree"]["nodes"][e4 as usize]["shapes"];
    assert_eq!(pinned[0]["orig"], "g1");
    assert_eq!(pinned[0]["dest"], "f3");
    assert_eq!(pinned[1]["orig"], "e4");
    assert!(pinned[1].get("dest").is_none());

    // An empty list clears the pin.
    let (status, view) = send(
        &app,
        json_req(
            "PUT",
            &format!("/api/studies/{study_id}/nodes/{e4}/shapes"),
            &bob,
            json!({"shapes": []}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        view["tree"]["nodes"][e4 as usize]["shapes"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    // Another user cannot pin shapes on Bob's study.
    let mallory = register(&app, "mallory").await;
    let (status, _) = send(
        &app,
        json_req(
            "PUT",
            &format!("/api/studies/{study_id}/nodes/{e4}/shapes"),
            &mallory,
            json!({"shapes": []}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn ownership_is_enforced_across_users_and_globals() {
    let app = server_app().await;
    let admin = register(&app, "alice").await; // first user → admin
    let bob = register(&app, "bob").await;

    // Bob's private study.
    let bob_db = make_database(&app, &bob).await;
    let (_, bob_study) = send(
        &app,
        json_req(
            "POST",
            "/api/studies",
            &bob,
            json!({"database_id": bob_db, "name": "Private"}),
        ),
    )
    .await;
    let bob_study_id = bob_study["id"].as_i64().unwrap();

    // Admin can neither see nor mutate Bob's study.
    let (status, _) = send(
        &app,
        get_req(&format!("/api/studies/{bob_study_id}"), &admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &app,
        json_req(
            "POST",
            &format!("/api/studies/{bob_study_id}/moves"),
            &admin,
            json!({"from_node_id": 0, "san": "e4"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Admin owns a global database and a global study in it.
    let (_, glob_db) = send(
        &app,
        json_req(
            "POST",
            "/api/databases",
            &admin,
            json!({"name": "Masters", "kind": "master", "global": true}),
        ),
    )
    .await;
    let glob_db_id = glob_db["id"].as_i64().unwrap();
    let (status, glob_study) = send(
        &app,
        json_req(
            "POST",
            "/api/studies",
            &admin,
            json!({"database_id": glob_db_id, "name": "Theory", "global": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(glob_study["global"], true);
    let glob_study_id = glob_study["id"].as_i64().unwrap();

    // Bob may read the global study (read scope) but not mutate it (admin-only).
    let (status, _) = send(
        &app,
        get_req(&format!("/api/studies/{glob_study_id}"), &bob),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &app,
        json_req(
            "POST",
            &format!("/api/studies/{glob_study_id}/moves"),
            &bob,
            json!({"from_node_id": 0, "san": "e4"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A non-admin cannot create a global study at all.
    let (status, _) = send(
        &app,
        json_req(
            "POST",
            "/api/studies",
            &bob,
            json!({"database_id": glob_db_id, "name": "Sneaky", "global": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Admin can mutate the global study.
    let (status, _) = send(
        &app,
        json_req(
            "POST",
            &format!("/api/studies/{glob_study_id}/moves"),
            &admin,
            json!({"from_node_id": 0, "san": "e4"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn lifecycle_crud_and_pgn_round_trip() {
    let app = server_app().await;
    let _admin = register(&app, "alice").await; // first user → admin
    let bob = register(&app, "bob").await;
    let db_id = make_database(&app, &bob).await;

    // Import a PGN (mainline + variation + comment) into a new study.
    let pgn = "1. e4 e5 (1... c5 2. Nf3) 2. Nf3 {develops} *";
    let (status, study) = send(
        &app,
        json_req(
            "POST",
            "/api/studies/import",
            &bob,
            json!({"database_id": db_id, "name": "Imported", "pgn": pgn}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let study_id = study["id"].as_i64().unwrap();
    // root + e4, e5, c5, Nf3 (variation), Nf3 (mainline) = 6 nodes.
    assert_eq!(study["tree"]["nodes"].as_array().unwrap().len(), 6);

    // It shows up in the listing.
    let (status, list) = send(&app, get_req("/api/studies", &bob)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);

    // Rename it.
    let (status, renamed) = send(
        &app,
        json_req(
            "PATCH",
            &format!("/api/studies/{study_id}"),
            &bob,
            json!({"name": "Ruy Lopez"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(renamed["name"], "Ruy Lopez");

    // Export it as a `.pgn` download and re-import: the mainline survives the
    // round trip, and the response is an attachment (issue #120), not JSON.
    let (status, disposition, pgn_out) = send_download(
        &app,
        get_req(&format!("/api/studies/{study_id}/export"), &bob),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        disposition.as_deref(),
        Some(&format!("attachment; filename=\"study-{study_id}.pgn\"")[..])
    );
    let (status, reimported) = send(
        &app,
        json_req(
            "POST",
            "/api/studies/import",
            &bob,
            json!({"database_id": db_id, "name": "Reimported", "pgn": pgn_out}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let nodes = reimported["tree"]["nodes"].as_array().unwrap();
    let mainline: Vec<_> = nodes
        .iter()
        .filter_map(|n| n["san"].as_str())
        .filter(|san| ["e4", "e5", "Nf3"].contains(san))
        .collect();
    assert!(mainline.contains(&"e4") && mainline.contains(&"Nf3"));

    // Malformed PGN is a client error, not a 500.
    let (status, _) = send(
        &app,
        json_req(
            "POST",
            "/api/studies/import",
            &bob,
            json!({"database_id": db_id, "name": "Bad", "pgn": "1. e4 e4 *"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Delete it.
    let (status, _) = send(&app, delete_req(&format!("/api/studies/{study_id}"), &bob)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(&app, get_req(&format!("/api/studies/{study_id}"), &bob)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn lichess_export_returns_headered_pgn() {
    let app = server_app().await;
    let bob = register(&app, "bob").await;
    let db_id = make_database(&app, &bob).await;

    let (status, study) = send(
        &app,
        json_req(
            "POST",
            "/api/studies/import",
            &bob,
            json!({"database_id": db_id, "name": "Italian", "pgn": "1. e4 e5 2. Nf3 {develops} *"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let study_id = study["id"].as_i64().unwrap();

    let (status, disposition, pgn_out) = send_download(
        &app,
        get_req(&format!("/api/studies/{study_id}/export/lichess"), &bob),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        disposition.as_deref(),
        Some(&format!("attachment; filename=\"study-{study_id}-lichess.pgn\"")[..])
    );
    assert!(pgn_out.starts_with("[Event \"Italian\"]"));
    assert!(pgn_out.contains("Nf3 {develops}"));

    // The exported chapter re-imports as a valid study.
    let (status, _) = send(
        &app,
        json_req(
            "POST",
            "/api/studies/import",
            &bob,
            json!({"database_id": db_id, "name": "Reimported", "pgn": pgn_out}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn lifecycle_scoping_across_users_and_globals() {
    let app = server_app().await;
    let admin = register(&app, "alice").await; // first user → admin
    let bob = register(&app, "bob").await;
    let bob_db = make_database(&app, &bob).await;

    // Bob's private study, imported from PGN.
    let (_, bob_study) = send(
        &app,
        json_req(
            "POST",
            "/api/studies/import",
            &bob,
            json!({"database_id": bob_db, "name": "Mine", "pgn": "1. d4 d5 *"}),
        ),
    )
    .await;
    let bob_study_id = bob_study["id"].as_i64().unwrap();

    // Admin can neither rename, export nor delete Bob's study (invisible → 404).
    let (status, _) = send(
        &app,
        json_req(
            "PATCH",
            &format!("/api/studies/{bob_study_id}"),
            &admin,
            json!({"name": "Hijack"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &app,
        get_req(&format!("/api/studies/{bob_study_id}/export"), &admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A non-admin cannot import a global study.
    let (status, _) = send(
        &app,
        json_req(
            "POST",
            "/api/studies/import",
            &bob,
            json!({"database_id": bob_db, "name": "Sneaky", "pgn": "1. e4 *", "global": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Bob may read (export) a global study but not rename it.
    let (_, glob_db) = send(
        &app,
        json_req(
            "POST",
            "/api/databases",
            &admin,
            json!({"name": "Masters", "kind": "master", "global": true}),
        ),
    )
    .await;
    let glob_db_id = glob_db["id"].as_i64().unwrap();
    let (status, glob_study) = send(
        &app,
        json_req(
            "POST",
            "/api/studies/import",
            &admin,
            json!({"database_id": glob_db_id, "name": "Theory", "pgn": "1. c4 *", "global": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let glob_study_id = glob_study["id"].as_i64().unwrap();

    let (status, _) = send(
        &app,
        get_req(&format!("/api/studies/{glob_study_id}/export"), &bob),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &app,
        json_req(
            "PATCH",
            &format!("/api/studies/{glob_study_id}"),
            &bob,
            json!({"name": "Bob's"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn unauthenticated_requests_are_rejected() {
    let app = server_app().await;
    let (status, _) = send(
        &app,
        Request::builder()
            .uri("/api/studies")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// `POST /api/studies/generate` is mounted and callable; with no engine wired the
/// orchestrator surfaces a clean `503` (never a panic or a leaked internal error).
/// The full happy path is covered at the service layer with injected fakes
/// (`study_gen::generate` unit tests), since it needs a real engine + LLM.
#[tokio::test]
async fn generate_without_engine_is_service_unavailable() {
    let app = server_app().await;
    let admin = register(&app, "alice").await; // first user → admin
    let db_id = make_database(&app, &admin).await;

    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/studies/generate",
            &admin,
            json!({"database_id": db_id, "name": "From the start"}),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("No engine configured"), "body was: {text}");
}

/// `POST /api/studies/generate-danger-map` is mounted and callable; with no engine
/// wired the orchestrator surfaces a clean `503` (never a panic or a leaked
/// internal). The full happy path is covered at the service layer with injected
/// fakes (`study_gen::danger_generate` tests), since it needs a real engine + LLM.
#[tokio::test]
async fn generate_danger_map_without_engine_is_service_unavailable() {
    let app = server_app().await;
    let admin = register(&app, "alice").await; // first user → admin
    let db_id = make_database(&app, &admin).await;

    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/studies/generate-danger-map",
            &admin,
            json!({"database_id": db_id, "name": "Sicilian traps", "spine_pgn": "1. e4 c5 *"}),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("No engine configured"), "body was: {text}");
}

/// A malformed spine PGN is a client error (`400`) — rejected at the transport
/// before any engine / LLM work. Mounted without an engine, so this also proves
/// PGN validation runs ahead of the engine-presence check.
#[tokio::test]
async fn generate_danger_map_with_bad_pgn_is_bad_request() {
    let app = server_app().await;
    let admin = register(&app, "alice").await;
    let db_id = make_database(&app, &admin).await;

    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/studies/generate-danger-map",
            &admin,
            json!({"database_id": db_id, "name": "Broken", "spine_pgn": "1. e4 e4 *"}),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("invalid spine PGN"), "body was: {text}");
}

/// `POST /api/studies/danger-map` (issue #156) is the engine-only sibling of the
/// LLM generator: it walks a spine for danger and returns the raw `DangerTree`
/// without ever touching a language model. With no engine wired it surfaces a clean
/// `503` (the full happy path needs a real engine, covered at the service layer).
#[tokio::test]
async fn danger_map_walk_without_engine_is_service_unavailable() {
    let app = server_app().await;
    let admin = register(&app, "alice").await; // first user → admin

    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/studies/danger-map",
            &admin,
            json!({"spine_pgn": "1. e4 c5 *"}),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("No engine configured"), "body was: {text}");
}

/// A malformed spine PGN is a client `400` on the engine-only walk too — rejected
/// at the transport before the engine-presence check, with no LLM involved.
#[tokio::test]
async fn danger_map_walk_with_bad_pgn_is_bad_request() {
    let app = server_app().await;
    let admin = register(&app, "alice").await;

    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/studies/danger-map",
            &admin,
            json!({"spine_pgn": "1. e4 e4 *"}),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("invalid spine PGN"), "body was: {text}");
}

/// `POST /api/studies/{id}/analyse` (#162) without an engine configured returns
/// 503 with the operator-facing guidance — the `server_app` wires no engine.
#[tokio::test]
async fn analyse_study_without_an_engine_is_service_unavailable() {
    let app = server_app().await;
    let bob = register(&app, "bob").await;
    let db_id = make_database(&app, &bob).await;

    let (status, study) = send(
        &app,
        json_req(
            "POST",
            "/api/studies",
            &bob,
            json!({"database_id": db_id, "name": "Openings"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let study_id = study["id"].as_i64().unwrap();

    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/studies/{study_id}/analyse"),
            &bob,
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("No engine configured"), "body was: {text}");
}

const STARTPOS: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

/// `POST /api/studies/{id}/merge-danger` (ADR-0032) grafts an engine-walked
/// danger tree into a study as deduped variations: a new reply appears, and
/// merging the same tree again adds nothing.
#[tokio::test]
async fn merge_danger_grafts_variations_and_dedups() {
    let app = server_app().await;
    let bob = register(&app, "bob").await;
    let db_id = make_database(&app, &bob).await;

    // A study with a 1.e4 e5 mainline.
    let (status, study) = send(
        &app,
        json_req(
            "POST",
            "/api/studies/import",
            &bob,
            json!({
                "database_id": db_id,
                "name": "Open Games",
                "pgn": "1. e4 e5 *",
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let study_id = study["id"].as_i64().unwrap();

    // A danger tree at 1.e4 with two replies: e5 (shared with the mainline) and
    // c5 (a new variation to graft). FENs are cosmetic for the graft, which
    // re-derives positions from the study's own start.
    let danger = json!({
        "root": 0,
        "nodes": [
            {"id":0,"parent":null,"fen":STARTPOS,"ply":0,"children":[1]},
            {"id":1,"parent":0,"san":"e4","fen":"rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq e3 0 1","ply":1,"children":[2,3]},
            {"id":2,"parent":1,"san":"e5","fen":"rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq e6 0 2","ply":2,"children":[]},
            {"id":3,"parent":1,"san":"c5","fen":"rnbqkbnr/pp1ppppp/8/2p5/4P3/8/PPPP1PPP/RNBQKBNR w KQkq c6 0 2","ply":2,"children":[]}
        ]
    });

    let (status, merged) = send(
        &app,
        json_req(
            "POST",
            &format!("/api/studies/{study_id}/merge-danger"),
            &bob,
            json!({ "tree": danger }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let nodes = merged["tree"]["nodes"].as_array().unwrap();
    // e4's node now branches into e5 (kept) + c5 (grafted).
    let e4 = nodes
        .iter()
        .find(|n| n["san"] == "e4")
        .expect("the e4 node survives the merge");
    assert_eq!(e4["children"].as_array().unwrap().len(), 2);
    assert!(nodes.iter().any(|n| n["san"] == "c5"));
    let count_after_first = nodes.len();

    // Merging the same tree again follows the existing children — no duplicates.
    let (status, merged2) = send(
        &app,
        json_req(
            "POST",
            &format!("/api/studies/{study_id}/merge-danger"),
            &bob,
            json!({ "tree": danger }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        merged2["tree"]["nodes"].as_array().unwrap().len(),
        count_after_first
    );
}

#[tokio::test]
async fn merge_games_builds_a_frequency_ordered_repertoire_study() {
    let (app, db) = server_app_with_db().await;
    let _admin = register(&app, "alice").await; // first user → admin
    let bob = register(&app, "bob").await;
    let db_id = make_database(&app, &bob).await as i32;

    // Two Bob games open 1.e4, one opens 1.d4.
    for pgn in [
        "[White \"Carlsen, M\"]\n[Black \"Nepo, I\"]\n[Date \"2023.01.01\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Nf3 *\n",
        "[White \"Carlsen, M\"]\n[Black \"So, W\"]\n[Date \"2022.06.01\"]\n[Result \"1-0\"]\n\n1. e4 c5 2. Nf3 *\n",
        "[White \"Carlsen, M\"]\n[Black \"Ding, L\"]\n[Date \"2021.03.01\"]\n[Result \"1/2-1/2\"]\n\n1. d4 d5 *\n",
    ] {
        ingest_pgn(&db, db_id, pgn).await.unwrap();
    }
    let (_, list) = send(
        &app,
        get_req(&format!("/api/games?database_id={db_id}"), &bob),
    )
    .await;
    let ids: Vec<i64> = list["games"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids.len(), 3);

    // Merge all three into a new study → 201 with the merged tree.
    let (status, study) = send(
        &app,
        json_req(
            "POST",
            "/api/studies/merge-games",
            &bob,
            json!({ "game_ids": ids, "name": "Carlsen repertoire" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(study["name"], "Carlsen repertoire");
    assert_eq!(study["origin_game_id"], Value::Null);

    // e4 (2 games) is the mainline over d4 (1); both first moves survive.
    let nodes = study["tree"]["nodes"].as_array().unwrap();
    let root = nodes.iter().find(|n| n["san"].is_null()).unwrap();
    let first_children = root["children"].as_array().unwrap();
    let mainline_san = &nodes[first_children[0].as_u64().unwrap() as usize]["san"];
    assert_eq!(mainline_san, "e4");
    assert_eq!(first_children.len(), 2);

    // An empty request is a clean 400, not a corrupt study.
    let (status, _) = send(
        &app,
        json_req(
            "POST",
            "/api/studies/merge-games",
            &bob,
            json!({ "game_ids": [], "name": "Empty" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// `POST /api/studies/{id}/mark-transpositions` (issue #174) tags a variation
/// that transposes into the mainline via a Zobrist collision — here the classic
/// Queen's-pawn/English reorder (1.d4 d5 2.c4 vs 1.c4 d5 2.d4) — and leaves it
/// untouched on a re-run over an unrelated user's study (ownership) or a
/// nonexistent one (404).
#[tokio::test]
async fn mark_transpositions_tags_a_transposing_line() {
    let app = server_app().await;
    let bob = register(&app, "bob").await;
    let carol = register(&app, "carol").await;
    let db_id = make_database(&app, &bob).await;

    let (status, study) = send(
        &app,
        json_req(
            "POST",
            "/api/studies/import",
            &bob,
            json!({
                "database_id": db_id,
                "name": "Queen's Pawn",
                "pgn": "1. d4 (1. c4 d5 2. d4) 1... d5 2. c4 *",
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let study_id = study["id"].as_i64().unwrap();

    let (status, marked) = send(
        &app,
        json_req(
            "POST",
            &format!("/api/studies/{study_id}/mark-transpositions"),
            &bob,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let nodes = marked["tree"]["nodes"].as_array().unwrap();
    let root = nodes.iter().find(|n| n["san"].is_null()).unwrap();
    let c4_first = root["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| &nodes[id.as_u64().unwrap() as usize])
        .find(|n| n["san"] == "c4")
        .expect("the reversed-order c4 branch survives the import");
    let v_d5 = &nodes[c4_first["children"][0].as_u64().unwrap() as usize];
    let transposed = &nodes[v_d5["children"][0].as_u64().unwrap() as usize];
    assert_eq!(transposed["san"], "d4");
    assert_eq!(
        transposed["comment"],
        "Transposes to the main line after 2.c4"
    );

    // Another user can't touch Bob's study; an unknown id is a clean 404.
    let (status, _) = send(
        &app,
        json_req(
            "POST",
            &format!("/api/studies/{study_id}/mark-transpositions"),
            &carol,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &app,
        json_req(
            "POST",
            "/api/studies/999999/mark-transpositions",
            &bob,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
