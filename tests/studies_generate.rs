//! Integration tests for the per-request LLM provider/model choice on the two
//! study-generation routes (issue #214): `POST /api/studies/generate` and
//! `POST /api/studies/generate-danger-map` take an optional `provider` (row
//! name) alongside `model`; a bad choice is the caller's 400 — distinct from
//! the 503s for a missing engine / missing LLM configuration. Split out of
//! `tests/studies.rs`, which is already over the file-size cap.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use sea_orm::{ActiveModelTrait, Set};
use serde_json::{json, Value};
use tower::ServiceExt;

use chess_base::ai::agent::{AgentEngine, AgentProviderStore};
use chess_base::db::entities::llm_providers;
use chess_base::db::{connect, DbConfig};
use chess_base::server::{build_router, AppState, Mode};

/// Server-mode app with no engine, no provider store and no agent — the same
/// bare fixture `tests/studies.rs` uses.
async fn plain_app() -> Router {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    build_router(AppState {
        db,
        mode: Mode::Server,
        engine_service: None,
        provider_store: None,
        agent: Default::default(),
    })
}

/// Server-mode app with a *running agent engine* over a seeded global provider
/// row (mirrors `tests/assistant_ws.rs`), but still no chess engine — so a
/// request that clears LLM-choice validation stops at the engine guard's 503.
async fn app_with_agent() -> Router {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    llm_providers::ActiveModel {
        name: Set("anthropic".into()),
        wire: Set("anthropic".into()),
        model: Set("claude-test".into()),
        base_url: Set(None),
        api_key: Set("test-key".into()),
        is_default: Set(true),
        owner_id: Set(None),
        created_at: Set(chrono::Utc::now().naive_utc()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    let store = AgentProviderStore::new_with_env(db.clone(), None)
        .await
        .unwrap();
    let state = AppState {
        db,
        mode: Mode::Server,
        engine_service: None,
        provider_store: Some(store),
        agent: Default::default(),
    };
    let engine = AgentEngine::start(state.clone()).await.unwrap();
    state.agent.set(engine).ok();
    build_router(state)
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, String) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
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

/// Register a user and return their bearer token (first user ⇒ admin).
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
    serde_json::from_str::<Value>(&body).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn make_database(app: &Router, token: &str) -> i64 {
    let (status, body) = send(
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
    serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_i64()
        .unwrap()
}

/// `provider` without `model` violates the request contract — a 400 before any
/// engine/agent state is probed, so it holds even on this bare fixture.
#[tokio::test]
async fn generate_with_provider_but_no_model_is_bad_request() {
    let app = plain_app().await;
    let admin = register(&app, "alice").await;
    let db_id = make_database(&app, &admin).await;

    let (status, body) = send(
        &app,
        json_req(
            "POST",
            "/api/studies/generate",
            &admin,
            json!({"database_id": db_id, "name": "Pick", "provider": "anthropic"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body was: {body}");
    assert!(body.contains("model"), "body was: {body}");
}

/// An explicit provider name the caller's catalog doesn't know is the caller's
/// 400 (the resolver's message), never the "nothing configured" 503.
#[tokio::test]
async fn generate_with_unknown_provider_is_bad_request() {
    let app = app_with_agent().await;
    let admin = register(&app, "alice").await;
    let db_id = make_database(&app, &admin).await;

    let (status, body) = send(
        &app,
        json_req(
            "POST",
            "/api/studies/generate",
            &admin,
            json!({
                "database_id": db_id,
                "name": "Pick",
                "provider": "no-such-provider",
                "model": "some-model"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body was: {body}");
}

/// A resolvable (provider, model) choice clears LLM validation and proceeds to
/// the next guard — here the missing chess engine's 503 — proving the explicit
/// choice resolves through the same seam the default path uses.
#[tokio::test]
async fn generate_with_known_provider_reaches_the_engine_guard() {
    let app = app_with_agent().await;
    let admin = register(&app, "alice").await;
    let db_id = make_database(&app, &admin).await;

    let (status, body) = send(
        &app,
        json_req(
            "POST",
            "/api/studies/generate",
            &admin,
            json!({
                "database_id": db_id,
                "name": "Pick",
                "provider": "anthropic",
                "model": "claude-test"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "body was: {body}");
    assert!(body.contains("No engine configured"), "body was: {body}");
}

/// The danger-map generator shares the same contract: `provider` without
/// `model` is a 400 (after its own spine-PGN validation, which stays first).
#[tokio::test]
async fn generate_danger_map_with_provider_but_no_model_is_bad_request() {
    let app = plain_app().await;
    let admin = register(&app, "alice").await;
    let db_id = make_database(&app, &admin).await;

    let (status, body) = send(
        &app,
        json_req(
            "POST",
            "/api/studies/generate-danger-map",
            &admin,
            json!({
                "database_id": db_id,
                "name": "Traps",
                "spine_pgn": "1. e4 c5 *",
                "provider": "anthropic"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body was: {body}");
    assert!(body.contains("model"), "body was: {body}");
}

/// The danger-map generator rejects an unknown provider name as a 400 too.
#[tokio::test]
async fn generate_danger_map_with_unknown_provider_is_bad_request() {
    let app = app_with_agent().await;
    let admin = register(&app, "alice").await;
    let db_id = make_database(&app, &admin).await;

    let (status, body) = send(
        &app,
        json_req(
            "POST",
            "/api/studies/generate-danger-map",
            &admin,
            json!({
                "database_id": db_id,
                "name": "Traps",
                "spine_pgn": "1. e4 c5 *",
                "provider": "no-such-provider",
                "model": "some-model"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body was: {body}");
}
