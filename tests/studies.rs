//! Integration tests for the study mutation API (issue #18) over the HTTP layer
//! in server mode: add SAN-validated moves/variations, annotate, promote /
//! reorder / delete variations, and the ownership rules (own vs global-admin vs
//! other-user), exercised end-to-end through real auth tokens.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use chess_base::db::{connect, DbConfig};
use chess_base::server::{build_router, AppState, Mode};

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
