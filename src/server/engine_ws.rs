//! WebSocket route streaming live UCI engine analysis to the SPA.
//!
//! `GET /api/engine/analyse` upgrades to a WebSocket. The server spawns the
//! configured engine for the lifetime of the socket and relays a small JSON
//! protocol:
//!
//! - client → server: `{"type":"analyse","fen":…,"limits":{…},"options":{…}}`
//!   and `{"type":"stop"}`. A new `analyse` while a search runs reconfigures and
//!   restarts cleanly (stop → drain → re-`go`).
//! - server → client: `{"type":"ready",…}`, the streamed `info`/`bestmove`
//!   events from [`AnalysisEvent`], and `{"type":"error",…}`.
//!
//! The handler is the one place that interleaves the engine read loop with
//! client control messages; everything chess-specific lives in the pure engine
//! submodules it calls.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::Result;
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;

use crate::engine::{AnalysisEvent, Engine, EngineConfig, Limits};
use crate::server::{identity::CurrentUser, state::AppState};

/// Client → server control messages.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ClientMsg {
    /// Analyse a position. Replaces any in-flight search on the same socket.
    Analyse {
        fen: String,
        #[serde(default)]
        limits: Limits,
        /// `setoption` values applied before the search (e.g. `MultiPV`).
        #[serde(default)]
        options: BTreeMap<String, String>,
    },
    /// Stop the current search; the engine still emits a final `bestmove`.
    Stop,
}

/// Server → client envelope for non-analysis messages. Analysis updates are sent
/// as bare [`AnalysisEvent`]s (`{"type":"info"|"bestmove",…}`).
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ServerMsg {
    /// Sent once the engine is spawned and ready to accept positions.
    Ready {
        name: String,
    },
    Error {
        message: String,
    },
}

/// How long to wait for a search to wind down after `stop` before reconfiguring.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Optional `?engine=<name>` query selecting a specific registered engine; absent
/// ⇒ the registry's resolved default drives the search.
#[derive(Debug, Default, Deserialize)]
pub struct AnalyseParams {
    engine: Option<String>,
}

/// `GET /api/engine/analyse` — upgrade to a WebSocket if an engine resolves.
/// Gated by [`CurrentUser`] so only an authorized caller can spawn a process.
/// The engine is the registry default unless `?engine=<name>` overrides it.
pub async fn analyse(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(params): Query<AnalyseParams>,
    _user: CurrentUser,
) -> Response {
    let registry = state.engines();
    let resolved = match &params.engine {
        Some(name) => registry.get(name).await,
        None => registry.resolve_default().await,
    };
    match resolved {
        Ok(Some(cfg)) => ws.on_upgrade(move |socket| session(socket, cfg)),
        Ok(None) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "no engine configured (add one via /api/engines or --engine)",
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not resolve engine configuration",
        )
            .into_response(),
    }
}

/// Drive one WebSocket: spawn the engine, then relay analysis until the socket
/// closes or the engine dies. The engine is always quit on the way out.
async fn session(mut socket: WebSocket, cfg: EngineConfig) {
    let mut engine = match Engine::spawn(cfg).await {
        Ok(engine) => engine,
        Err(e) => {
            send_error(&mut socket, format!("failed to start engine: {e}")).await;
            return;
        }
    };
    let ready = ServerMsg::Ready {
        name: engine.config().name.clone(),
    };
    if send_json(&mut socket, &ready).await.is_err() {
        let _ = engine.quit().await;
        return;
    }

    let mut analysing = false;
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if !handle_client_msg(&mut socket, &mut engine, &mut analysing, text.as_str()).await {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // ping/pong/binary: ignore.
                    Some(Err(_)) => break, // transport error.
                }
            }
            // Only pull engine output while a search is actually running.
            event = engine.next_event(), if analysing => {
                match event {
                    Ok(Some(event)) => {
                        let terminal = matches!(event, AnalysisEvent::BestMove { .. });
                        if send_json(&mut socket, &event).await.is_err() {
                            break;
                        }
                        if terminal {
                            analysing = false;
                        }
                    }
                    Ok(None) => {
                        send_error(&mut socket, "engine exited unexpectedly").await;
                        break;
                    }
                    Err(e) => {
                        send_error(&mut socket, format!("engine error: {e}")).await;
                        break;
                    }
                }
            }
        }
    }

    let _ = engine.quit().await;
}

/// Handle one decoded client message. Returns `false` to end the session.
async fn handle_client_msg(
    socket: &mut WebSocket,
    engine: &mut Engine,
    analysing: &mut bool,
    text: &str,
) -> bool {
    match serde_json::from_str::<ClientMsg>(text) {
        Ok(ClientMsg::Analyse {
            fen,
            limits,
            options,
        }) => match start_analysis(engine, *analysing, &fen, &limits, &options).await {
            Ok(()) => {
                *analysing = true;
                true
            }
            Err(e) => {
                // A bad FEN / option is recoverable: report it, keep the socket.
                *analysing = false;
                send_error(socket, format!("could not start analysis: {e}")).await;
                true
            }
        },
        Ok(ClientMsg::Stop) => {
            if *analysing {
                let _ = engine.stop().await;
            }
            true
        }
        Err(e) => {
            send_error(socket, format!("invalid message: {e}")).await;
            true
        }
    }
}

/// (Re)start a search: stop and drain any current one, apply options, set the
/// position, and `go`. UCI requires the engine be idle before a new `position`.
async fn start_analysis(
    engine: &mut Engine,
    analysing: bool,
    fen: &str,
    limits: &Limits,
    options: &BTreeMap<String, String>,
) -> Result<()> {
    if analysing {
        engine.stop().await?;
        timeout(DRAIN_TIMEOUT, drain_to_bestmove(engine))
            .await
            .map_err(|_| anyhow::anyhow!("timed out stopping the previous search"))??;
    }
    for (name, value) in options {
        engine.set_option(name, value).await?;
    }
    if !options.is_empty() {
        engine.wait_ready().await?;
    }
    engine.set_position(fen).await?;
    engine.go(limits).await
}

/// Consume engine output up to and including the terminal `bestmove`.
async fn drain_to_bestmove(engine: &mut Engine) -> Result<()> {
    loop {
        match engine.next_event().await? {
            Some(AnalysisEvent::BestMove { .. }) | None => return Ok(()),
            Some(_) => continue,
        }
    }
}

async fn send_json<T: Serialize>(socket: &mut WebSocket, msg: &T) -> Result<()> {
    let text = serde_json::to_string(msg)?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    Ok(())
}

/// Best-effort error frame; ignores send failure since the socket may be gone.
async fn send_error(socket: &mut WebSocket, message: impl Into<String>) {
    let _ = send_json(
        socket,
        &ServerMsg::Error {
            message: message.into(),
        },
    )
    .await;
}
