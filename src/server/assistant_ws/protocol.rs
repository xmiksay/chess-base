//! Wire frames for the assistant WebSocket relay (#198, step 5).
//!
//! The envelope is a small serde-tagged JSON protocol of its own; the engine's
//! [`InMsg`]/[`OutEvent`] ride inside it verbatim (`kind`-tagged, snake_case —
//! entanglement's own wire shape), so the SPA speaks the engine protocol
//! directly and this layer never re-models it.

use entanglement_core::{InMsg, OutEvent};
use entanglement_runtime::session_store::{LogPayload, LogRecord};
use serde::{Deserialize, Serialize};

use crate::ai::agent::AgentSessionSummary;

/// Client → server frames.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum ClientFrame {
    /// Forward an engine [`InMsg`] (ownership-checked, then
    /// `holly.send_from_wire` — which enforces the privileged-variant split).
    In { msg: InMsg },
    /// List the caller's conversations → [`ServerFrame::Sessions`].
    List,
    /// Start a new conversation → [`ServerFrame::Created`]. `provider`/`model`
    /// (both or neither — a lone field is refused with an error frame) pick
    /// the session's starting model (#215, ADR-0046); omitted ⇒ the caller's
    /// default provider row.
    New {
        prompt: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        provider: Option<String>,
        #[serde(default)]
        model: Option<String>,
    },
    /// Open a conversation (resuming it into the engine if needed) →
    /// [`ServerFrame::History`].
    Open { root: String },
    /// Delete a conversation → [`ServerFrame::Deleted`].
    Delete { root: String },
    /// Liveness reply to [`ServerFrame::Ping`].
    Pong,
}

/// Server → client frames.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum ServerFrame {
    /// A live engine [`OutEvent`] the caller is allowed to see.
    Out {
        ev: OutEvent,
    },
    Sessions {
        items: Vec<AgentSessionSummary>,
    },
    Created {
        root: String,
    },
    /// Persisted transcript replay for [`ClientFrame::Open`]: the stored
    /// records in original order — user prompts (`dir: "in"`) interleaved with
    /// engine events (`dir: "out"`).
    History {
        root: String,
        gapped: bool,
        records: Vec<HistoryRecord>,
    },
    Deleted {
        root: String,
    },
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<&'static str>,
        message: String,
    },
    /// The outbound broadcast lagged and `dropped` events were lost.
    Gap {
        dropped: u64,
    },
    /// An LLM endpoint's throttle posture changed. Deliberately carries only
    /// the boolean — never the endpoint name (`OutEvent::Throttle` is
    /// engine-global and would leak another user's provider surface).
    Throttled {
        active: bool,
    },
    Ping,
}

/// One replayed transcript record: `dir` is `"in"` (a user [`InMsg::Prompt`])
/// or `"out"` (an engine [`OutEvent`]); `payload` is the engine type's own JSON.
#[derive(Debug, Serialize)]
pub(crate) struct HistoryRecord {
    pub dir: &'static str,
    pub payload: serde_json::Value,
}

/// Fold persisted [`LogRecord`]s into replay frames, in original `ord` order:
/// every `Out` event, plus only the `In` records that are prompts — control
/// frames (`Approve`/`Stop`/…) carry no transcript content.
pub(crate) fn history_records(records: &[LogRecord]) -> Vec<HistoryRecord> {
    records
        .iter()
        .filter_map(|r| match &r.payload {
            LogPayload::In(msg @ InMsg::Prompt { .. }) => Some(HistoryRecord {
                dir: "in",
                payload: serde_json::to_value(msg).ok()?,
            }),
            LogPayload::Out(ev) => Some(HistoryRecord {
                dir: "out",
                payload: serde_json::to_value(ev).ok()?,
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use entanglement_core::SessionId;
    use serde_json::json;

    fn record(payload: LogPayload) -> LogRecord {
        LogRecord {
            ts: 1,
            session: SessionId::new("u:s"),
            payload,
        }
    }

    #[test]
    fn client_frames_deserialize() {
        let f: ClientFrame =
            serde_json::from_str(r#"{"type":"in","msg":{"kind":"stop","session":"u:s"}}"#)
                .expect("in frame");
        assert!(matches!(
            f,
            ClientFrame::In {
                msg: InMsg::Stop { .. }
            }
        ));

        let f: ClientFrame = serde_json::from_str(r#"{"type":"new","prompt":"hi"}"#).expect("new");
        assert!(matches!(
            f,
            ClientFrame::New {
                name: None,
                provider: None,
                model: None,
                ..
            }
        ));

        let f: ClientFrame = serde_json::from_str(
            r#"{"type":"new","prompt":"hi","provider":"anthropic","model":"claude-x"}"#,
        )
        .expect("new with a model choice");
        assert!(matches!(
            f,
            ClientFrame::New { provider: Some(p), model: Some(m), .. }
                if p == "anthropic" && m == "claude-x"
        ));

        let f: ClientFrame = serde_json::from_str(r#"{"type":"pong"}"#).expect("pong");
        assert!(matches!(f, ClientFrame::Pong));

        // A malformed engine message is a parse error, not a panic.
        assert!(
            serde_json::from_str::<ClientFrame>(r#"{"type":"in","msg":{"kind":"nope"}}"#).is_err()
        );
    }

    #[test]
    fn server_frames_serialize_with_lowercase_type_tags() {
        let v = serde_json::to_value(ServerFrame::Throttled { active: true }).expect("json");
        assert_eq!(v, json!({"type":"throttled","active":true}));

        let v = serde_json::to_value(ServerFrame::Gap { dropped: 7 }).expect("json");
        assert_eq!(v, json!({"type":"gap","dropped":7}));

        let v = serde_json::to_value(ServerFrame::Error {
            code: Some("no_provider"),
            message: "no LLM provider configured".into(),
        })
        .expect("json");
        assert_eq!(v["type"], "error");
        assert_eq!(v["code"], "no_provider");

        let ev = OutEvent::Done {
            session: SessionId::new("u:s"),
            seq: 3,
        };
        let v = serde_json::to_value(ServerFrame::Out { ev }).expect("json");
        assert_eq!(
            v,
            json!({"type":"out","ev":{"kind":"done","session":"u:s","seq":3}})
        );
    }

    #[test]
    fn history_keeps_prompts_and_out_events_only_in_order() {
        let records = vec![
            record(LogPayload::In(InMsg::prompt(
                SessionId::new("u:s"),
                "hello",
            ))),
            record(LogPayload::Out(OutEvent::TextDelta {
                session: SessionId::new("u:s"),
                seq: 1,
                text: "hi".into(),
            })),
            // Control frame: filtered out.
            record(LogPayload::In(InMsg::Stop {
                session: SessionId::new("u:s"),
            })),
            record(LogPayload::Out(OutEvent::Done {
                session: SessionId::new("u:s"),
                seq: 2,
            })),
        ];
        let out = history_records(&records);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].dir, "in");
        assert_eq!(out[0].payload["kind"], "prompt");
        assert_eq!(out[0].payload["content"][0]["text"], "hello");
        assert_eq!(out[1].dir, "out");
        assert_eq!(out[1].payload["kind"], "text_delta");
        assert_eq!(out[2].payload["kind"], "done");
        assert!(
            !out.iter().any(|r| r.payload["kind"] == "stop"),
            "control frames never replay"
        );
    }
}
