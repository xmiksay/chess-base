//! DB-backed [`RecordSink`] for the embedded entanglement engine (#198, step
//! 4): the persistence tap (`spawn_persistence_subscriber_with_sink`) hands
//! every finished [`LogRecord`] to [`DbRecordSink::append`], which must never
//! block — it `try_send`s onto a bounded channel drained by a dedicated writer
//! task that appends `agent_events` rows (m0008), strictly `ord`-ordered per
//! root session. [`load_records`] is the read side an open/resume uses.
//!
//! Loss handling: a full channel makes `append` return `Err` (the tap logs
//! it); the tap itself writes [`LogPayload::Gap`] tombstones when the outbound
//! broadcast lags, and those flow through this sink like any record — so
//! `integrity_gap` over [`load_records`] output still detects loss.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use entanglement_core::SessionId;
use entanglement_runtime::multi_user::SessionUserRegistry;
use entanglement_runtime::persistence::RecordSink;
use entanglement_runtime::session_store::{LogPayload, LogRecord};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::db::entities::agent_events;

/// Backlog cap on the writer channel. `append` sheds (with an `Err`) once the
/// DB writer is this far behind rather than blocking the persistence tap.
const SINK_CHANNEL_CAPACITY: usize = 4096;

/// Owner recorded when neither the session registry nor the id's
/// `{user}:{uuid}` prefix names one (should not happen for engine-minted ids).
const UNKNOWN_USER: &str = "unknown";

/// A [`RecordSink`] appending to `agent_events` behind a bounded channel and a
/// dedicated writer task, so the sync `append` never awaits a DB round-trip.
pub struct DbRecordSink {
    tx: mpsc::Sender<(SessionId, LogRecord)>,
}

impl DbRecordSink {
    /// Spawn the writer task and return the sink plus the task handle. The
    /// writer exits once every sink clone is dropped (the channel closes).
    pub fn spawn(
        db: DatabaseConnection,
        users: SessionUserRegistry,
    ) -> (Arc<Self>, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel::<(SessionId, LogRecord)>(SINK_CHANNEL_CAPACITY);
        let writer = tokio::spawn(writer_loop(db, users, rx));
        (Arc::new(Self { tx }), writer)
    }
}

impl RecordSink for DbRecordSink {
    fn append(&self, root: &SessionId, record: &LogRecord) -> anyhow::Result<()> {
        // Sync by contract: a blocked sink starves the tap's broadcast receiver
        // and manufactures the very lag-gaps it exists to record.
        self.tx
            .try_send((root.clone(), record.clone()))
            .map_err(|_| anyhow::anyhow!("agent_events sink backlog full, dropping record"))
    }
}

/// The writer task: resolves the owning user, mints the next per-root `ord`
/// (seeded from `MAX(ord)` on the first write for a root after startup) and
/// inserts the row. DB errors are logged and the loop continues — persistence
/// must never take the engine down.
async fn writer_loop(
    db: DatabaseConnection,
    users: SessionUserRegistry,
    mut rx: mpsc::Receiver<(SessionId, LogRecord)>,
) {
    let mut next_ord: HashMap<String, i64> = HashMap::new();
    while let Some((root, record)) = rx.recv().await {
        let ord = match next_ord.get(&root.0) {
            Some(n) => *n,
            None => match seed_ord(&db, &root.0).await {
                Ok(n) => n,
                Err(err) => {
                    tracing::error!(root = %root.0, error = %format!("{err:#}"),
                        "could not seed agent_events ord; dropping record");
                    continue;
                }
            },
        };
        let user_id = user_of(&users, &root);
        let row = agent_events::ActiveModel {
            root_session_id: Set(root.0.clone()),
            user_id: Set(user_id),
            ord: Set(ord),
            ts: Set(record.ts as i64),
            session_id: Set(record.session.0.clone()),
            direction: Set(direction_of(&record.payload).to_string()),
            payload: Set(match serde_json::to_string(&record.payload) {
                Ok(json) => json,
                Err(err) => {
                    tracing::error!(root = %root.0, error = %err, "unserializable agent event");
                    continue;
                }
            }),
            ..Default::default()
        };
        match row.insert(&db).await {
            Ok(_) => {
                next_ord.insert(root.0.clone(), ord + 1);
            }
            Err(err) => {
                tracing::error!(root = %root.0, ord, error = %err,
                    "failed to persist agent event");
            }
        }
    }
}

/// The next free `ord` for `root`: `MAX(ord) + 1`, or `0` for a fresh log.
async fn seed_ord(db: &DatabaseConnection, root: &str) -> anyhow::Result<i64> {
    let last: Option<i64> = agent_events::Entity::find()
        .filter(agent_events::Column::RootSessionId.eq(root))
        .order_by_desc(agent_events::Column::Ord)
        .limit(1)
        .all(db)
        .await
        .context("reading MAX(ord) for agent_events")?
        .first()
        .map(|row| row.ord);
    Ok(last.map_or(0, |n| n + 1))
}

/// Resolve the root's owning user: the session registry first, else the
/// `{user}:{uuid}` id convention, else [`UNKNOWN_USER`].
fn user_of(users: &SessionUserRegistry, root: &SessionId) -> String {
    if let Some(user) = users.user_for(root) {
        return user.0;
    }
    root.0
        .split_once(':')
        .map(|(user, _)| user.to_string())
        .unwrap_or_else(|| UNKNOWN_USER.to_string())
}

/// The `direction` column value for a payload — matches [`LogPayload`]'s own
/// serde tag (`in`/`out`/`gap`) so the column stays queryable without parsing
/// the payload JSON.
fn direction_of(payload: &LogPayload) -> &'static str {
    match payload {
        LogPayload::In(_) => "in",
        LogPayload::Out(_) => "out",
        LogPayload::Gap { .. } => "gap",
    }
}

/// Load `root`'s full log from `agent_events`, `ord`-ordered. A row whose
/// payload no longer deserializes (an engine-version bump) is skipped with a
/// warning rather than failing the whole read.
pub async fn load_records(db: &DatabaseConnection, root: &str) -> anyhow::Result<Vec<LogRecord>> {
    let rows = agent_events::Entity::find()
        .filter(agent_events::Column::RootSessionId.eq(root))
        .order_by_asc(agent_events::Column::Ord)
        .all(db)
        .await
        .context("loading agent_events")?;
    let mut records = Vec::with_capacity(rows.len());
    for row in rows {
        match serde_json::from_str::<LogPayload>(&row.payload) {
            Ok(payload) => records.push(LogRecord {
                ts: row.ts.max(0) as u64,
                session: SessionId::new(row.session_id),
                payload,
            }),
            Err(err) => {
                tracing::warn!(root, ord = row.ord, error = %err,
                    "skipping undeserializable agent event");
            }
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests;
