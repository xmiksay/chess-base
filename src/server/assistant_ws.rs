//! Authenticated WebSocket relay onto the embedded agent engine (#198, step 5).
//!
//! `GET /api/assistant/ws` upgrades to a WebSocket ([`CurrentUser`] gates the
//! upgrade, exactly like [`engine_ws`][super::engine_ws]; 503 before upgrade
//! when the engine never started). One socket serves one authenticated user:
//!
//! - client → server: an envelope carrying either an engine [`InMsg`]
//!   (`{"type":"in","msg":…}`) or a session-CRUD verb (`list`/`new`/`open`/
//!   `delete`/`pong`) — see [`protocol::ClientFrame`].
//! - server → client: the caller's slice of the engine broadcast
//!   (`{"type":"out","ev":…}`) plus CRUD replies — see
//!   [`protocol::ServerFrame`].
//!
//! Both directions are ownership-filtered here: an inbound `InMsg` naming a
//! session the caller does not own is dropped with an error frame *before* it
//! reaches the engine (then `holly.send_from_wire` additionally refuses the
//! privileged variants), and an outbound event is forwarded only when its
//! session belongs to the caller (session-less events: `SessionList` is
//! filtered per-user, `Throttle` is reduced to a bare boolean, the rest are
//! dropped).

mod protocol;

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entanglement_core::{OutEvent, SessionId};
use entanglement_runtime::multi_user::SessionUserRegistry;
use tokio::sync::broadcast::error::RecvError;

use crate::ai::agent::{AgentEngine, SessionError};
use crate::server::{identity::CurrentUser, state::AppState};

use protocol::{history_records, ClientFrame, HistoryRecord, ServerFrame};

/// Liveness probe cadence; the socket is closed after two unanswered pings.
const PING_INTERVAL: Duration = Duration::from_secs(30);
const MAX_MISSED_PONGS: u32 = 2;

/// `GET /api/assistant/ws` — upgrade iff the caller authenticated and the
/// agent engine is running.
pub async fn ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    user: CurrentUser,
) -> Response {
    let Some(engine) = state.agent().cloned() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "agent engine not running (no assistant on this install)",
        )
            .into_response();
    };
    ws.on_upgrade(move |socket| session(socket, engine, user))
}

/// Drive one socket: interleave client frames, the engine broadcast and the
/// liveness ping until either side closes.
async fn session(mut socket: WebSocket, engine: Arc<AgentEngine>, user: CurrentUser) {
    let mut sub = engine.holly.subscribe();
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.tick().await; // The first tick is immediate; skip it.
    let mut unanswered: u32 = 0;

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if !handle_client_frame(&mut socket, &engine, &user, &mut unanswered, text.as_str()).await {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // transport ping/pong/binary: ignore.
                    Some(Err(_)) => break,
                }
            }
            event = sub.recv() => {
                let frame = match event {
                    Ok(ev) => outbound(&user.id, &engine.users, ev),
                    Err(RecvError::Lagged(dropped)) => Some(ServerFrame::Gap { dropped }),
                    Err(RecvError::Closed) => break,
                };
                if let Some(frame) = frame {
                    if send_frame(&mut socket, &frame).await.is_err() {
                        break;
                    }
                }
            }
            _ = ping.tick() => {
                if unanswered >= MAX_MISSED_PONGS {
                    break;
                }
                unanswered += 1;
                if send_frame(&mut socket, &ServerFrame::Ping).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Handle one decoded client frame. Returns `false` to end the session; every
/// application-level failure stays on the socket as an error frame.
async fn handle_client_frame(
    socket: &mut WebSocket,
    engine: &Arc<AgentEngine>,
    user: &CurrentUser,
    unanswered: &mut u32,
    text: &str,
) -> bool {
    let frame = match serde_json::from_str::<ClientFrame>(text) {
        Ok(frame) => frame,
        Err(e) => {
            return send_error(socket, None, format!("invalid message: {e}")).await;
        }
    };
    match frame {
        ClientFrame::In { msg } => {
            if let Some(session) = msg.session() {
                if !owns_for_send(engine, user, session).await {
                    return send_error(socket, None, "unknown session").await;
                }
            }
            if let Err(e) = engine.holly.send_from_wire(msg).await {
                return send_error(socket, None, e.to_string()).await;
            }
            true
        }
        ClientFrame::List => match engine.sessions.list(user).await {
            Ok(items) => send_ok(socket, &ServerFrame::Sessions { items }).await,
            Err(e) => send_session_error(socket, e).await,
        },
        ClientFrame::New {
            prompt,
            name,
            provider,
            model,
        } => match engine
            .sessions
            .create(user, prompt, name, provider, model)
            .await
        {
            Ok(root) => send_ok(socket, &ServerFrame::Created { root }).await,
            Err(e) => send_session_error(socket, e).await,
        },
        ClientFrame::Open { root } => match open_history(engine, user, &root).await {
            Ok((gapped, records)) => {
                send_ok(
                    socket,
                    &ServerFrame::History {
                        root,
                        gapped,
                        records,
                    },
                )
                .await
            }
            Err(e) => send_session_error(socket, e).await,
        },
        ClientFrame::Delete { root } => match engine.sessions.delete(user, &root).await {
            Ok(()) => send_ok(socket, &ServerFrame::Deleted { root }).await,
            Err(e) => send_session_error(socket, e).await,
        },
        ClientFrame::Pong => {
            *unanswered = 0;
            true
        }
    }
}

/// Open a conversation for replay: `SessionService::open` performs the
/// ownership check and resumes the engine session (its `OutEvent`-only view is
/// discarded), then the full log is re-read so the user's own prompts —
/// persisted as `In` records — interleave into the transcript.
async fn open_history(
    engine: &Arc<AgentEngine>,
    user: &CurrentUser,
    root: &str,
) -> Result<(bool, Vec<HistoryRecord>), SessionError> {
    let opened = engine.sessions.open(user, root).await?;
    let mut records = crate::ai::agent::load_records(engine.sessions.db(), root)
        .await
        .map_err(SessionError::Internal)?;
    crate::ai::agent::sessions::truncate_at_gap(&mut records);
    Ok((opened.gapped, history_records(&records)))
}

/// Whether the caller may address `session` with an inbound frame: the live
/// registry first (it also covers engine-minted child sessions, which inherit
/// the owner), else the persisted `agent_sessions` index (a non-live root the
/// caller is about to prompt awake).
async fn owns_for_send(engine: &Arc<AgentEngine>, user: &CurrentUser, session: &SessionId) -> bool {
    match engine.users.user_for(session) {
        Some(uid) => uid.0 == user.id,
        None => engine.sessions.owns(user, &session.0).await,
    }
}

/// Map one broadcast event onto the caller's outbound frame, or `None` to drop
/// it. Session-scoped events forward only to their owner (registry first, the
/// `{user_id}:` root-id prefix as belt-and-braces for a race right around
/// registration); `SessionList` is filtered to the caller's rows; `Throttle`
/// collapses to a bare boolean; every other session-less event is private to
/// the engine and dropped.
fn outbound(user_id: &str, users: &SessionUserRegistry, ev: OutEvent) -> Option<ServerFrame> {
    if let Some(session) = ev.session() {
        let mine = match users.user_for(session) {
            Some(uid) => uid.0 == user_id,
            None => session.0.starts_with(&format!("{user_id}:")),
        };
        return mine.then_some(ServerFrame::Out { ev });
    }
    match ev {
        OutEvent::SessionList {
            correlation_id,
            sessions,
        } => {
            let sessions = sessions
                .into_iter()
                .filter(|info| info.user.as_ref().is_some_and(|u| u.0 == user_id))
                .collect();
            Some(ServerFrame::Out {
                ev: OutEvent::SessionList {
                    correlation_id,
                    sessions,
                },
            })
        }
        OutEvent::Throttle { throttled, .. } => Some(ServerFrame::Throttled { active: throttled }),
        _ => None,
    }
}

/// Map a [`SessionError`] onto an error frame; only `NoProvider` carries a
/// machine-readable code (the SPA offers the provider settings on it).
async fn send_session_error(socket: &mut WebSocket, err: SessionError) -> bool {
    let code = match &err {
        SessionError::NoProvider => Some("no_provider"),
        SessionError::NotFound => Some("not_found"),
        SessionError::InvalidModelChoice(_) => Some("invalid_model"),
        SessionError::Internal(_) => None,
    };
    // An internal error's chain stays in the log; the client gets the summary.
    if let SessionError::Internal(e) = &err {
        tracing::warn!(error = %format!("{e:#}"), "assistant ws session operation failed");
    }
    send_error(socket, code, err.to_string()).await
}

async fn send_ok(socket: &mut WebSocket, frame: &ServerFrame) -> bool {
    send_frame(socket, frame).await.is_ok()
}

async fn send_error(
    socket: &mut WebSocket,
    code: Option<&'static str>,
    message: impl Into<String>,
) -> bool {
    send_frame(
        socket,
        &ServerFrame::Error {
            code,
            message: message.into(),
        },
    )
    .await
    .is_ok()
}

async fn send_frame(socket: &mut WebSocket, frame: &ServerFrame) -> anyhow::Result<()> {
    let text = serde_json::to_string(frame)?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|e| anyhow::anyhow!(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use entanglement_provider::UserId;

    fn registry_with(session: &str, user: &str) -> SessionUserRegistry {
        let users = SessionUserRegistry::new();
        users.register(SessionId::new(session), UserId::new(user));
        users
    }

    fn delta(session: &str) -> OutEvent {
        OutEvent::TextDelta {
            session: SessionId::new(session),
            seq: 1,
            text: "x".into(),
        }
    }

    #[test]
    fn session_scoped_events_forward_only_to_the_registered_owner() {
        let users = registry_with("alice:1", "alice");
        assert!(matches!(
            outbound("alice", &users, delta("alice:1")),
            Some(ServerFrame::Out { .. })
        ));
        assert!(outbound("bob", &users, delta("alice:1")).is_none());
    }

    #[test]
    fn unregistered_sessions_fall_back_to_the_root_id_prefix() {
        let users = SessionUserRegistry::new();
        assert!(outbound("alice", &users, delta("alice:99")).is_some());
        assert!(outbound("bob", &users, delta("alice:99")).is_none());
        // A child session (uuid id, no prefix) that lost its registration is
        // unattributable — never forwarded.
        assert!(outbound("alice", &users, delta("f00-uuid")).is_none());
    }

    #[test]
    fn session_list_is_filtered_to_the_callers_rows() {
        use entanglement_core::SessionInfo;
        let users = SessionUserRegistry::new();
        let info = |id: &str, user: Option<&str>| SessionInfo {
            session: SessionId::new(id),
            parent: None,
            profile: "build".into(),
            root: true,
            profile_detail: None,
            user: user.map(UserId::new),
        };
        let ev = OutEvent::SessionList {
            correlation_id: "c1".into(),
            sessions: vec![
                info("alice:1", Some("alice")),
                info("bob:1", Some("bob")),
                info("anon", None),
            ],
        };
        let Some(ServerFrame::Out {
            ev: OutEvent::SessionList { sessions, .. },
        }) = outbound("alice", &users, ev)
        else {
            panic!("expected a filtered SessionList");
        };
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session.0, "alice:1");
    }

    #[test]
    fn throttle_collapses_to_a_boolean_and_other_global_events_drop() {
        let users = SessionUserRegistry::new();
        let ev = OutEvent::Throttle {
            endpoint: "secret-endpoint".into(),
            throttled: true,
            in_flight: 1,
            cap: 2,
            retry_in_ms: Some(100),
            pacing_in_ms: None,
            waiters: 0,
            shared_leases: None,
        };
        let frame = outbound("alice", &users, ev).expect("throttle forwards");
        let json = serde_json::to_value(&frame).expect("json");
        assert_eq!(json["type"], "throttled");
        assert_eq!(json["active"], true);
        assert!(
            !json.to_string().contains("secret-endpoint"),
            "endpoint name never leaks"
        );

        let ev = OutEvent::McpChanged {
            name: "srv".into(),
            action: entanglement_core::McpAction::Added,
        };
        assert!(outbound("alice", &users, ev).is_none());
    }
}
