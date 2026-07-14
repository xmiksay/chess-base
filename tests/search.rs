//! Integration tests for position search over the HTTP layer: the opening-tree
//! and games-reaching-position endpoints, their NDJSON framing, and that scope
//! is honored end-to-end through real auth tokens (server mode).

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

const STARTPOS_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

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

/// Register a user, returning their bearer token and resolved owner id (the
/// string that lands in `databases.owner_id` — not necessarily the username).
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
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
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

/// Fetch an NDJSON endpoint and return (status, content-type, parsed lines).
async fn ndjson(app: &Router, uri: &str, token: &str) -> (StatusCode, String, Vec<Value>) {
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
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    let lines = text
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str::<Value>(l).expect("each line is valid JSON"))
        .collect();
    (status, content_type, lines)
}

#[tokio::test]
async fn opening_tree_streams_ndjson_move_stats() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    seed(&db, &alice_id, &[SCHOLARS_MATE, QUEENS_DRAW]).await;

    let (status, content_type, lines) = ndjson(
        &app,
        &format!("/api/search/tree?fen={}", urlencoding(STARTPOS_FEN)),
        &alice,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "application/x-ndjson");
    // Two distinct openings: e4 (white win) and d4 (draw).
    assert_eq!(lines.len(), 2);
    let e4 = lines.iter().find(|l| l["san"] == "e4").unwrap();
    assert_eq!(e4["count"], 1);
    assert_eq!(e4["white"], 1);
    let d4 = lines.iter().find(|l| l["san"] == "d4").unwrap();
    assert_eq!(d4["draws"], 1);
}

#[tokio::test]
async fn games_endpoint_returns_only_matching_games() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    seed(&db, &alice_id, &[SCHOLARS_MATE, QUEENS_DRAW]).await;

    // 1. e4 e5 is reached only by the scholar's-mate game.
    let fen = "rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2";
    let (status, content_type, lines) = ndjson(
        &app,
        &format!("/api/search/games?fen={}", urlencoding(fen)),
        &alice,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "application/x-ndjson");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["white"], "Spassky");
    assert_eq!(lines[0]["result"], "1-0");
}

#[tokio::test]
async fn search_scope_excludes_other_users_games() {
    let (app, db) = app_with_db().await;
    let (_alice, alice_id) = register(&app, "alice").await; // first user → admin
    let (bob_token, bob_id) = register(&app, "bob").await;
    seed(&db, &alice_id, &[SCHOLARS_MATE]).await;
    seed(&db, &bob_id, &[QUEENS_DRAW]).await;

    // Bob only sees his own opening (d4), not alice's e4.
    let (status, _, lines) = ndjson(
        &app,
        &format!("/api/search/tree?fen={}", urlencoding(STARTPOS_FEN)),
        &bob_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["san"], "d4");
}

#[tokio::test]
async fn invalid_fen_is_a_bad_request() {
    let (app, _db) = app_with_db().await;
    let (alice, _alice_id) = register(&app, "alice").await;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/search/tree?fen=not-a-fen")
                .header(header::AUTHORIZATION, format!("Bearer {alice}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unauthenticated_search_is_rejected() {
    let (app, _db) = app_with_db().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/search/tree?fen={}",
                    urlencoding(STARTPOS_FEN)
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// A PGN carrying the header roster header search filters on.
fn header_game(
    white: &str,
    black: &str,
    event: &str,
    eco: &str,
    date: &str,
    result: &str,
) -> String {
    format!(
        "[Event \"{event}\"]\n[White \"{white}\"]\n[Black \"{black}\"]\n[ECO \"{eco}\"]\n[Date \"{date}\"]\n[Result \"{result}\"]\n\n1. e4 e5 {result}\n"
    )
}

/// Fetch a JSON endpoint, returning (status, parsed body).
async fn get_json(app: &Router, uri: &str, token: &str) -> (StatusCode, Value) {
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
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn header_search_filters_and_resolves_names() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    seed(
        &db,
        &alice_id,
        &[
            &header_game("Carlsen", "Nepo", "Tata Steel", "B90", "2021.01.16", "1-0"),
            &header_game("Nepo", "Carlsen", "Candidates", "C42", "2021.04.20", "0-1"),
            &header_game(
                "Ding",
                "Nakamura",
                "Tata Steel",
                "B33",
                "2022.01.20",
                "1/2-1/2",
            ),
        ],
    )
    .await;

    // Player on the white side only matches the single Carlsen-white game.
    let (status, body) = get_json(
        &app,
        "/api/search/headers?player=Carlsen&color=white",
        &alice,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["games"].as_array().unwrap().len(), 1);
    assert_eq!(body["games"][0]["white"], "Carlsen");
    assert_eq!(body["games"][0]["black"], "Nepo");
    assert!(body["next_cursor"].is_null());

    // ECO prefix + event combine.
    let (_, body) = get_json(&app, "/api/search/headers?eco=B&event=Tata", &alice).await;
    assert_eq!(body["games"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn header_search_paginates_with_keyset_cursor() {
    let (app, db) = app_with_db().await;
    let (alice, alice_id) = register(&app, "alice").await;
    let games: Vec<String> = (0..5)
        .map(|i| header_game("A", "B", "E", "C00", &format!("2020.01.0{}", i + 1), "1-0"))
        .collect();
    let refs: Vec<&str> = games.iter().map(String::as_str).collect();
    seed(&db, &alice_id, &refs).await;

    let mut seen = Vec::new();
    let mut uri = "/api/search/headers?limit=2".to_string();
    loop {
        let (status, body) = get_json(&app, &uri, &alice).await;
        assert_eq!(status, StatusCode::OK);
        for g in body["games"].as_array().unwrap() {
            seen.push(g["id"].as_i64().unwrap());
        }
        match body["next_cursor"].as_str() {
            Some(c) => uri = format!("/api/search/headers?limit=2&cursor={c}"),
            None => break,
        }
    }
    // Every game exactly once, newest-date first.
    assert_eq!(seen.len(), 5);
    let mut sorted = seen.clone();
    sorted.sort_unstable();
    sorted.reverse();
    assert_eq!(seen, sorted);
}

#[tokio::test]
async fn header_search_scope_excludes_other_users() {
    let (app, db) = app_with_db().await;
    let (_alice, alice_id) = register(&app, "alice").await; // first user → admin
    let (bob_token, bob_id) = register(&app, "bob").await;
    seed(
        &db,
        &alice_id,
        &[&header_game(
            "Carlsen",
            "Nepo",
            "E",
            "C00",
            "2021.01.01",
            "1-0",
        )],
    )
    .await;
    seed(
        &db,
        &bob_id,
        &[&header_game(
            "Ding",
            "Nakamura",
            "E",
            "C00",
            "2021.02.02",
            "0-1",
        )],
    )
    .await;

    // Bob can only see his own game, never alice's private database.
    let (status, body) = get_json(&app, "/api/search/headers?player=Carlsen", &bob_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["games"].as_array().unwrap().len(), 0);

    let (_, body) = get_json(&app, "/api/search/headers", &bob_token).await;
    assert_eq!(body["games"].as_array().unwrap().len(), 1);
    assert_eq!(body["games"][0]["white"], "Ding");
}

#[tokio::test]
async fn header_search_rejects_bad_cursor_and_params() {
    let (app, _db) = app_with_db().await;
    let (alice, _alice_id) = register(&app, "alice").await;
    let (status, _) = get_json(&app, "/api/search/headers?cursor=not-valid", &alice).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = get_json(&app, "/api/search/headers?sort=rating", &alice).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = get_json(
        &app,
        "/api/search/headers?elo_min=2600&elo_max=2500",
        &alice,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// A PGN with explicit ELO tags for the rating filter/sort tests.
fn rated_game(white: &str, white_elo: &str, black: &str, black_elo: &str) -> String {
    format!(
        "[Event \"E\"]\n[White \"{white}\"]\n[Black \"{black}\"]\n[WhiteElo \"{white_elo}\"]\n[BlackElo \"{black_elo}\"]\n[Result \"1-0\"]\n\n1. e4 e5 1-0\n"
    )
}

#[tokio::test]
async fn header_search_filters_by_elo_and_database() {
    let (app, db) = app_with_db().await;
    let (_alice, alice_id) = register(&app, "alice").await; // first user → admin
    let (bob_token, bob_id) = register(&app, "bob").await;
    let alice_db = seed(&db, &alice_id, &[SCHOLARS_MATE]).await;
    let bob_db = seed(
        &db,
        &bob_id,
        &[
            &rated_game("Carlsen", "2850", "Caruana", "2800"),
            &rated_game("Ding", "2450", "Nakamura", "2400"),
            SCHOLARS_MATE, // no ELO tags → excluded once a bound is set
        ],
    )
    .await;

    // ELO band + average-rating sort, pinned to bob's own database.
    let (status, body) = get_json(
        &app,
        &format!("/api/search/headers?database_id={bob_db}&elo_min=2500&sort=elo"),
        &bob_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["games"].as_array().unwrap().len(), 1);
    assert_eq!(body["games"][0]["white"], "Carlsen");
    assert_eq!(body["games"][0]["white_elo"], 2850);

    // Without bounds, `sort=elo` returns everything, unrated last.
    let (_, body) = get_json(
        &app,
        &format!("/api/search/headers?database_id={bob_db}&sort=elo"),
        &bob_token,
    )
    .await;
    let whites: Vec<&str> = body["games"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["white"].as_str().unwrap())
        .collect();
    assert_eq!(whites, ["Carlsen", "Ding", "Spassky"]);

    // Someone else's database id is hidden as not-found.
    let (status, _) = get_json(
        &app,
        &format!("/api/search/headers?database_id={alice_db}"),
        &bob_token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn header_search_requires_auth() {
    let (app, _db) = app_with_db().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/search/headers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Minimal percent-encoding for the FEN query parameter (spaces and `/`).
fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ' ' => "%20".to_string(),
            '/' => "%2F".to_string(),
            other => other.to_string(),
        })
        .collect()
}
