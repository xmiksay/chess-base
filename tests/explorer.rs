//! Integration tests for the Lichess Masters explorer (ADR-0053) in server mode,
//! against a fake Lichess explorer bound on a local port — the real one is never
//! contacted. Covers auth gating, the disabled (no token) 503, the bearer token
//! forwarding, query passthrough, caching and the PGN import into a database.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use chess_base::db::{connect, DbConfig};
use chess_base::explorer::MastersClient;
use chess_base::server::{build_router, AppState, Mode};

const START: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

const MASTERS_PGN: &str = "[Event \"Linares\"]\n[Site \"https://lichess.org/abcd1234\"]\n[White \"Kasparov, G.\"]\n[Black \"Karpov, A.\"]\n[Result \"1-0\"]\n\n1. e4 c5 2. Nf3 d6 1-0\n";

#[derive(Clone, Default)]
struct Fake {
    hits: Arc<AtomicUsize>,
    last_fen: Arc<std::sync::Mutex<String>>,
}

fn authorized(headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        == Some("Bearer secret")
}

async fn fake_masters(
    State(fake): State<Fake>,
    headers: HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<axum::Json<Value>, StatusCode> {
    if !authorized(&headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    fake.hits.fetch_add(1, Ordering::SeqCst);
    if let (Some(fen), Ok(mut last)) = (q.get("fen"), fake.last_fen.lock()) {
        last.clone_from(fen);
    }
    // Echo the year window back through the opening name so the test can see it.
    let name = format!(
        "since={} until={}",
        q.get("since").map_or("-", String::as_str),
        q.get("until").map_or("-", String::as_str)
    );
    Ok(axum::Json(json!({
        "white": 3, "draws": 2, "black": 1,
        "moves": [{"uci": "e2e4", "san": "e4", "averageRating": 2600, "white": 3, "draws": 2, "black": 1}],
        "topGames": [{"uci": "e2e4", "id": "abcd1234", "winner": "white",
            "white": {"name": "Kasparov, G.", "rating": 2800},
            "black": {"name": "Karpov, A.", "rating": 2750}, "year": 1990, "month": "1990-10"}],
        "opening": {"eco": "B20", "name": name}
    })))
}

async fn fake_pgn(headers: HeaderMap, Path(id): Path<String>) -> Result<String, StatusCode> {
    if !authorized(&headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    match id.as_str() {
        "abcd1234" => Ok(MASTERS_PGN.to_string()),
        _ => Err(StatusCode::NOT_FOUND),
    }
}

async fn spawn_fake() -> (String, Fake) {
    let fake = Fake::default();
    let app = Router::new()
        .route("/masters", get(fake_masters))
        .route("/masters/pgn/{id}", get(fake_pgn))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), fake)
}

async fn app_with(masters: Option<Arc<MastersClient>>) -> Router {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    build_router(AppState {
        db,
        mode: Mode::Server,
        engine_service: None,
        provider_store: None,
        agent: Default::default(),
        masters,
    })
}

async fn enabled_app() -> (Router, Fake) {
    let (base, fake) = spawn_fake().await;
    let client = MastersClient::new(base, "secret").unwrap();
    (app_with(Some(Arc::new(client))).await, fake)
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

fn req(method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&v).unwrap()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

async fn register(app: &Router, username: &str) -> String {
    let (status, body) = send(
        app,
        req(
            "POST",
            "/api/auth/register",
            None,
            Some(json!({"username": username, "password": "password123"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    body["token"].as_str().unwrap().to_string()
}

fn masters_uri(extra: &str) -> String {
    let fen = START.replace(' ', "%20").replace('/', "%2F");
    format!("/api/explorer/masters?fen={fen}{extra}")
}

#[tokio::test]
async fn anonymous_caller_is_rejected() {
    let (app, fake) = enabled_app().await;
    let (status, _) = send(&app, req("GET", &masters_uri(""), None, None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(fake.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn without_a_token_the_explorer_is_disabled() {
    let app = app_with(None).await;
    let alice = register(&app, "alice").await;
    let (status, _) = send(&app, req("GET", &masters_uri(""), Some(&alice), None)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (_, health) = send(&app, req("GET", "/api/health", None, None)).await;
    assert_eq!(health["masters"], false);
}

#[tokio::test]
async fn report_is_normalised_cached_and_forwards_years() {
    let (app, fake) = enabled_app().await;
    let alice = register(&app, "alice").await;
    let (_, health) = send(&app, req("GET", "/api/health", None, None)).await;
    assert_eq!(health["masters"], true);

    let uri = masters_uri("&since=1980&until=2000");
    let (status, body) = send(&app, req("GET", &uri, Some(&alice), None)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 6);
    assert_eq!(body["moves"][0]["san"], "e4");
    assert_eq!(body["moves"][0]["count"], 6);
    assert_eq!(body["top_games"][0]["id"], "abcd1234");
    assert_eq!(body["opening"]["name"], "since=1980 until=2000");

    // Same query again is served from the cache.
    let (status, _) = send(&app, req("GET", &uri, Some(&alice), None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fake.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn bad_query_is_a_400_without_an_upstream_call() {
    let (app, fake) = enabled_app().await;
    let alice = register(&app, "alice").await;
    let (status, _) = send(
        &app,
        req(
            "GET",
            &masters_uri("&since=2000&until=1990"),
            Some(&alice),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = send(
        &app,
        req(
            "GET",
            "/api/explorer/masters?fen=garbage",
            Some(&alice),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(fake.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn import_ingests_the_game_once_into_a_writable_database() {
    let (app, _) = enabled_app().await;
    let alice = register(&app, "alice").await;
    let (status, db) = send(
        &app,
        req(
            "POST",
            "/api/databases",
            Some(&alice),
            Some(json!({"name": "Mine", "kind": "own"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let body = json!({"lichess_id": "abcd1234", "database_id": db["id"]});

    let (status, first) = send(
        &app,
        req(
            "POST",
            "/api/explorer/masters/import",
            Some(&alice),
            Some(body.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["imported"], 1);

    let (_, again) = send(
        &app,
        req(
            "POST",
            "/api/explorer/masters/import",
            Some(&alice),
            Some(body),
        ),
    )
    .await;
    assert_eq!(again["imported"], 0);
    assert_eq!(again["duplicates"], 1);

    // Another user can't write into alice's database.
    let bob = register(&app, "bob").await;
    let (status, _) = send(
        &app,
        req(
            "POST",
            "/api/explorer/masters/import",
            Some(&bob),
            Some(json!({"lichess_id": "abcd1234", "database_id": db["id"]})),
        ),
    )
    .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "{status}"
    );
}

#[tokio::test]
async fn import_of_an_unknown_game_is_a_404() {
    let (app, _) = enabled_app().await;
    let alice = register(&app, "alice").await;
    let (status, _) = send(
        &app,
        req(
            "POST",
            "/api/explorer/masters/import",
            Some(&alice),
            Some(json!({"lichess_id": "zzzz0000", "database_id": 1})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// The MCP tool resolves a `moves` list server-side (ADR-0056): the upstream
/// sees the replayed FEN, and the reply echoes it back.
#[tokio::test]
async fn mcp_masters_report_accepts_moves() {
    use chess_base::db::entities::service_tokens;
    use sea_orm::{ActiveModelTrait, Set};

    let (base, fake) = spawn_fake().await;
    let client = MastersClient::new(base, "secret").unwrap();
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    service_tokens::ActiveModel {
        token: Set("svc-token".to_string()),
        id: Set("svc".to_string()),
        owner_id: Set("alice".to_string()),
        is_admin: Set(false),
        scope: Set("full".to_string()),
        label: Set("test".to_string()),
        created_at: Set(chrono::Utc::now().naive_utc()),
        expires_at: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
    let app = build_router(AppState {
        db,
        mode: Mode::Server,
        engine_service: None,
        provider_store: None,
        agent: Default::default(),
        masters: Some(Arc::new(client)),
    });

    let call = |args: Value| {
        req(
            "POST",
            "/mcp",
            Some("svc-token"),
            Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": "masters_position_report", "arguments": args}})),
        )
    };
    let (status, body) = send(&app, call(json!({"moves": ["d4", "Nf6", "c4"]}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result = &body["result"];
    assert_ne!(result["isError"], true, "{body}");
    let report: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    let expected = "rnbqkb1r/pppppppp/5n2/8/2PP4/8/PP2PPPP/RNBQKBNR b KQkq";
    assert!(
        report["fen"].as_str().unwrap().starts_with(expected),
        "{report}"
    );
    assert!(fake.last_fen.lock().unwrap().starts_with(expected));

    // An illegal move is named, and Lichess is never asked.
    let hits = fake.hits.load(Ordering::SeqCst);
    let (_, body) = send(&app, call(json!({"moves": ["d4", "Ke3"]}))).await;
    assert_eq!(body["result"]["isError"], true, "{body}");
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("move 2 (`Ke3`)"), "{text}");
    assert_eq!(fake.hits.load(Ordering::SeqCst), hits);
}
