//! Integration tests for the assistant WebSocket relay (#198, step 5), driven
//! end-to-end over a real listener (a WS upgrade cannot ride `tower::oneshot`).
//!
//! The fixture seeds one *unresolvable* global provider row: `create`'s
//! provider guard passes (a default `(provider, model)` exists), but the row
//! has an unknown wire and no endpoint, so the session-start model pin fails
//! to resolve (a logged warning) and every turn runs on the engine-default
//! `EchoLlm` — deterministic, no network (the same stance as the step-4
//! engine tests).

use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tower::ServiceExt;

use chess_base::ai::agent::{load_records, AgentEngine, AgentProviderStore};
use chess_base::db::entities::{agent_sessions, llm_providers};
use chess_base::db::{connect, DbConfig};
use chess_base::server::{build_router, AppState, Mode};
use entanglement_runtime::session_store::LogPayload;

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const RECV_TIMEOUT: Duration = Duration::from_secs(10);

/// Boot a full app (engine started) on an ephemeral port.
async fn spawn_app(mode: Mode) -> (SocketAddr, AppState) {
    let db = connect(&DbConfig::in_memory()).await.expect("connect db");
    // The unresolvable default provider row (see the module doc).
    llm_providers::ActiveModel {
        name: Set("stub".into()),
        model: Set("echo-1".into()),
        api_key: Set(String::new()),
        is_default: Set(true),
        created_at: Set(chrono::Utc::now().naive_utc()),
        owner_id: Set(None),
        wire: Set("unresolvable".into()),
        base_url: Set(None),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed provider row");
    let store = AgentProviderStore::new_with_env(db.clone(), None)
        .await
        .expect("provider store");
    let state = AppState {
        db,
        mode,
        engine_service: None,
        provider_store: Some(store),
        agent: Default::default(),
    };
    let engine = AgentEngine::start(state.clone()).await.expect("engine");
    state.agent.set(engine).ok();

    let app = build_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    (addr, state)
}

/// Register a server-mode user through the HTTP API (same DB as the listener)
/// and return their bearer token.
async fn register(state: &AppState, username: &str) -> String {
    let resp = build_router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "username": username, "password": "password123" }).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("register response");
    assert!(resp.status().is_success(), "register {username}");
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let body: Value = serde_json::from_slice(&bytes).expect("json");
    body["token"].as_str().expect("token").to_string()
}

async fn ws_connect(addr: SocketAddr, token: Option<&str>) -> Result<Ws, WsError> {
    let mut request = format!("ws://{addr}/api/assistant/ws")
        .into_client_request()
        .expect("client request");
    if let Some(token) = token {
        request.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().expect("header"),
        );
    }
    connect_async(request).await.map(|(ws, _)| ws)
}

async fn send_frame(ws: &mut Ws, frame: Value) {
    ws.send(Message::Text(frame.to_string().into()))
        .await
        .expect("send frame");
}

/// Next JSON frame, or panic after the timeout.
async fn recv_frame(ws: &mut Ws) -> Value {
    loop {
        let msg = tokio::time::timeout(RECV_TIMEOUT, ws.next())
            .await
            .expect("recv timed out")
            .expect("socket closed")
            .expect("transport error");
        if let Message::Text(text) = msg {
            return serde_json::from_str(&text).expect("json frame");
        }
    }
}

/// Read frames until `pred` matches, returning the match (skips others).
async fn wait_for(ws: &mut Ws, pred: impl Fn(&Value) -> bool) -> Value {
    loop {
        let frame = recv_frame(ws).await;
        if pred(&frame) {
            return frame;
        }
    }
}

/// Read out-frames until the given session's `done`, collecting streamed text.
async fn collect_turn(ws: &mut Ws, root: &str) -> String {
    let mut text = String::new();
    loop {
        let frame = recv_frame(ws).await;
        if frame["type"] != "out" || frame["ev"]["session"] != *root {
            continue;
        }
        match frame["ev"]["kind"].as_str() {
            Some("text_delta") => text.push_str(frame["ev"]["text"].as_str().unwrap_or_default()),
            Some("done") => return text,
            Some("error") => panic!("engine error: {}", frame["ev"]["message"]),
            _ => {}
        }
    }
}

/// Poll until `root`'s persisted log contains a `Done` (the sink is async).
async fn wait_for_done_record(db: &sea_orm::DatabaseConnection, root: &str) {
    for _ in 0..250 {
        let records = load_records(db, root).await.expect("load records");
        if records.iter().any(|r| {
            matches!(
                &r.payload,
                LogPayload::Out(entanglement_core::OutEvent::Done { .. })
            )
        }) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("Done never persisted for {root}");
}

#[tokio::test]
async fn server_mode_rejects_an_unauthenticated_upgrade_and_local_mode_connects() {
    let (addr, _state) = spawn_app(Mode::Server).await;
    match ws_connect(addr, None).await {
        Err(WsError::Http(resp)) => assert_eq!(resp.status(), StatusCode::UNAUTHORIZED),
        other => panic!("expected a 401 refusal, got {other:?}"),
    }

    let (addr, _state) = spawn_app(Mode::Local).await;
    let mut ws = ws_connect(addr, None).await.expect("local mode connects");
    send_frame(&mut ws, json!({"type":"list"})).await;
    let frame = wait_for(&mut ws, |f| f["type"] == "sessions").await;
    assert_eq!(frame["items"], json!([]));
}

#[tokio::test]
async fn new_creates_a_session_and_a_prompt_streams_echo_deltas_to_done() {
    let (addr, state) = spawn_app(Mode::Local).await;
    let mut ws = ws_connect(addr, None).await.expect("connect");

    send_frame(
        &mut ws,
        json!({"type":"new","prompt":"hello relay","name":null}),
    )
    .await;
    let created = wait_for(&mut ws, |f| f["type"] == "created").await;
    let root = created["root"].as_str().expect("root").to_string();
    assert!(root.starts_with("local-admin:"), "namespaced id: {root}");

    let row = agent_sessions::Entity::find_by_id(root.clone())
        .one(&state.db)
        .await
        .expect("query")
        .expect("agent_sessions row");
    assert_eq!(row.user_id, "local-admin");

    // The spawn prompt itself runs a first EchoLlm turn.
    let text = collect_turn(&mut ws, &root).await;
    assert!(
        text.contains("hello relay"),
        "echo quotes the prompt: {text}"
    );

    // A follow-up `in` Prompt round-trips the same way.
    send_frame(
        &mut ws,
        json!({"type":"in","msg":{"kind":"prompt","session":root,
               "content":[{"type":"text","text":"second question"}]}}),
    )
    .await;
    let text = collect_turn(&mut ws, &root).await;
    assert!(text.contains("second question"), "echoed: {text}");
}

#[tokio::test]
async fn ownership_is_enforced_inbound_and_outbound() {
    let (addr, state) = spawn_app(Mode::Server).await;
    let alice = register(&state, "alice").await;
    let bob = register(&state, "bob").await;

    let mut ws_a = ws_connect(addr, Some(&alice)).await.expect("alice");
    let mut ws_b = ws_connect(addr, Some(&bob)).await.expect("bob");

    send_frame(
        &mut ws_a,
        json!({"type":"new","prompt":"alice's opening prep"}),
    )
    .await;
    let created = wait_for(&mut ws_a, |f| f["type"] == "created").await;
    let root = created["root"].as_str().expect("root").to_string();

    // Bob prompts Alice's session: refused before the engine sees it.
    send_frame(
        &mut ws_b,
        json!({"type":"in","msg":{"kind":"prompt","session":root,"text":"leak?"}}),
    )
    .await;
    let err = wait_for(&mut ws_b, |f| f["type"] == "error").await;
    assert_eq!(err["message"], "unknown session");

    // Bob opens Alice's session: not found (no existence oracle).
    send_frame(&mut ws_b, json!({"type":"open","root":root})).await;
    let err = wait_for(&mut ws_b, |f| f["type"] == "error").await;
    assert_eq!(err["code"], "not_found");

    // Alice's turns stream to Alice…
    let text = collect_turn(&mut ws_a, &root).await;
    assert!(text.contains("opening prep"));

    // …and Bob's subscription stays silent about them.
    let quiet = tokio::time::timeout(Duration::from_millis(700), async {
        loop {
            let frame = recv_frame(&mut ws_b).await;
            if frame["type"] == "out" {
                return frame;
            }
        }
    })
    .await;
    assert!(
        quiet.is_err(),
        "bob must never receive alice's out events: {quiet:?}"
    );
}

#[tokio::test]
async fn list_returns_only_the_callers_sessions() {
    let (addr, state) = spawn_app(Mode::Server).await;
    let alice = register(&state, "alice").await;
    let bob = register(&state, "bob").await;

    let mut ws_a = ws_connect(addr, Some(&alice)).await.expect("alice");
    send_frame(&mut ws_a, json!({"type":"new","prompt":"mine"})).await;
    let created = wait_for(&mut ws_a, |f| f["type"] == "created").await;
    let root = created["root"].as_str().expect("root").to_string();

    send_frame(&mut ws_a, json!({"type":"list"})).await;
    let frame = wait_for(&mut ws_a, |f| f["type"] == "sessions").await;
    let items = frame["items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["root_session_id"], *root);

    let mut ws_b = ws_connect(addr, Some(&bob)).await.expect("bob");
    send_frame(&mut ws_b, json!({"type":"list"})).await;
    let frame = wait_for(&mut ws_b, |f| f["type"] == "sessions").await;
    assert_eq!(frame["items"], json!([]), "bob sees none of alice's");
}

#[tokio::test]
async fn open_replays_the_in_prompt_and_the_out_done_on_a_fresh_connection() {
    let (addr, state) = spawn_app(Mode::Local).await;
    let mut ws = ws_connect(addr, None).await.expect("connect");

    send_frame(&mut ws, json!({"type":"new","prompt":"remember me"})).await;
    let created = wait_for(&mut ws, |f| f["type"] == "created").await;
    let root = created["root"].as_str().expect("root").to_string();
    collect_turn(&mut ws, &root).await;
    wait_for_done_record(&state.db, &root).await;

    let mut fresh = ws_connect(addr, None).await.expect("fresh connection");
    send_frame(&mut fresh, json!({"type":"open","root":root})).await;
    let history = wait_for(&mut fresh, |f| f["type"] == "history").await;
    assert_eq!(history["root"], *root);
    assert_eq!(history["gapped"], false);

    let records = history["records"].as_array().expect("records");
    let prompt = records
        .iter()
        .find(|r| r["dir"] == "in" && r["payload"]["kind"] == "prompt")
        .expect("the In(Prompt) record replays");
    assert_eq!(prompt["payload"]["content"][0]["text"], "remember me");
    assert!(
        records
            .iter()
            .any(|r| r["dir"] == "out" && r["payload"]["kind"] == "done"),
        "the Out(Done) record replays"
    );
}

#[tokio::test]
async fn a_privileged_in_frame_is_refused_with_an_error_frame() {
    let (addr, _state) = spawn_app(Mode::Local).await;
    let mut ws = ws_connect(addr, None).await.expect("connect");

    // An owned root, so the frame passes the ownership gate and exercises the
    // engine's wire allowlist itself.
    send_frame(&mut ws, json!({"type":"new","prompt":"hi"})).await;
    let created = wait_for(&mut ws, |f| f["type"] == "created").await;
    let root = created["root"].as_str().expect("root").to_string();

    send_frame(
        &mut ws,
        json!({"type":"in","msg":{"kind":"spawn","session":root,"parent":null,
               "predecessor":null,"agent":"build","prompt":"forged","user":null}}),
    )
    .await;
    let err = wait_for(&mut ws, |f| f["type"] == "error").await;
    let message = err["message"].as_str().expect("message");
    assert!(
        message.contains("privileged") && message.contains("spawn"),
        "WireError surfaces: {message}"
    );
}
