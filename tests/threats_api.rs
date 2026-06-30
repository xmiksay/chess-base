//! Integration tests for the Threats overlay endpoint (issue #123): a clean
//! start returns an empty array, a hanging piece returns its attacker→target
//! arrow, a bad FEN is a 400, and server mode gates on auth.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use chess_base::db::{connect, DbConfig};
use chess_base::server::{build_router, AppState, Mode};

async fn send(app: &Router, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

async fn local_app() -> Router {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    build_router(AppState {
        db,
        mode: Mode::Local,
        engine_service: None,
        llm_provider: None,
    })
}

#[tokio::test]
async fn startpos_has_no_threats() {
    let app = local_app().await;
    let fen = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR%20w%20KQkq%20-%200%201";
    let (status, body) = send(&app, &format!("/api/threats?fen={fen}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn hanging_piece_yields_a_red_arrow() {
    let app = local_app().await;
    // Black pawn d6 attacks the undefended white knight on e5 (white to move).
    let fen = "4k3/8/3p4/4N3/8/8/8/4K3%20w%20-%20-%200%201";
    let (status, body) = send(&app, &format!("/api/threats?fen={fen}")).await;
    assert_eq!(status, StatusCode::OK);
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["orig"], "d6");
    assert_eq!(arr[0]["dest"], "e5");
    assert_eq!(arr[0]["brush"], "threat");
}

#[tokio::test]
async fn invalid_fen_is_a_bad_request() {
    let app = local_app().await;
    let (status, _) = send(&app, "/api/threats?fen=not-a-fen").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn server_mode_requires_auth() {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    let app = build_router(AppState {
        db,
        mode: Mode::Server,
        engine_service: None,
        llm_provider: None,
    });
    let (status, _) = send(&app, "/api/threats?fen=x").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
