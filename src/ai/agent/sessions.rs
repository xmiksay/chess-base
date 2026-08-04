//! [`SessionService`] — transport-agnostic CRUD + lifecycle over the agent
//! engine's conversations (#198, step 4): `agent_sessions` rows are the index,
//! `agent_events` (via [`persistence`]) the transcript, and `Holly` the live
//! in-memory engine. Routes (step 5) are thin callers.
//!
//! ## Why `create` sends exactly one `Spawn` frame (ordering finding)
//!
//! The supervisor's `Spawn` handler queues the spawn prompt into the new
//! session's command channel *immediately*, so any `SetModel` sent afterwards
//! lands behind it — the first turn would run on the engine-default LLM
//! (`EchoLlm`) before the rebind folds. An empty spawn prompt is no escape:
//! the supervisor wraps it in a `ContentPart::text("")` unconditionally, so it
//! still starts a (garbage) turn. The one per-user binding that lands *before*
//! the queued prompt is the profile **model pin** (#323): `session_loop`
//! resolves `profile.model_pin()` through `EngineConfig::model_resolver` with
//! the Spawn-supplied user *before* draining commands. The engine's `build`
//! profile therefore pins [`DEFAULT_PIN`], which
//! [`AgentProviderStore::build_resolver`] maps to the calling user's default
//! provider row — so `create` sends `Spawn { prompt, user }` and nothing else.
//!
//! [`persistence`]: super::persistence
//! [`DEFAULT_PIN`]: super::provider_store::DEFAULT_PIN

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::anyhow;
use entanglement_core::{Holly, InMsg, OutEvent, SessionId};
use entanglement_provider::UserId;
use entanglement_runtime::multi_user::SessionUserRegistry;
use entanglement_runtime::session_store::{pair_records, LogPayload, LogRecord};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set,
};

use crate::db::entities::{agent_events, agent_sessions};
use crate::server::identity::CurrentUser;

use super::engine::AGENT_PROFILE;
use super::persistence::load_records;
use super::provider_store::AgentProviderStore;

/// Why a session operation failed. Transport-agnostic; the routes map
/// [`NoProvider`](SessionError::NoProvider) to 503 and
/// [`NotFound`](SessionError::NotFound) to 404 — a raw `DbErr` never leaks.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("no LLM provider configured")]
    NoProvider,
    /// A creation-time model choice named a provider absent from the caller's
    /// catalog (#215, ADR-0046). Refused *before* spawning: a pin that fails
    /// to resolve only warns and leaves the session on the `EchoLlm` stub.
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
    #[error("session not found")]
    NotFound,
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

fn db_err(err: DbErr, what: &'static str) -> SessionError {
    SessionError::Internal(anyhow::Error::new(err).context(what))
}

/// One row of the session index, for listings.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentSessionSummary {
    pub root_session_id: String,
    pub name: Option<String>,
    pub agent: String,
    pub created_at: String,
    pub last_active: String,
    /// Whether the engine currently holds this conversation in memory.
    pub live: bool,
}

/// What [`SessionService::open`] returns: the persisted transcript (engine
/// `OutEvent`s, for the client's history replay) plus a loss flag.
#[derive(Debug)]
pub struct OpenResult {
    pub events: Vec<OutEvent>,
    /// The log carried a `Gap` tombstone: records were lost, and a resume
    /// replayed only the intact prefix before it.
    pub gapped: bool,
}

/// Session CRUD + engine lifecycle. Cheap to clone; the live set is shared
/// with the engine's index subscriber, which folds `SessionStarted`/`Ended`/
/// `Hibernated` into it.
#[derive(Clone)]
pub struct SessionService {
    db: DatabaseConnection,
    holly: Holly,
    users: SessionUserRegistry,
    store: Arc<AgentProviderStore>,
    live: Arc<Mutex<HashSet<SessionId>>>,
}

impl SessionService {
    pub fn new(
        db: DatabaseConnection,
        holly: Holly,
        users: SessionUserRegistry,
        store: Arc<AgentProviderStore>,
    ) -> Self {
        Self {
            db,
            holly,
            users,
            store,
            live: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// The connection this service reads/writes — shared with callers that
    /// re-read the transcript themselves (the WS relay's history replay).
    pub(crate) fn db(&self) -> &DatabaseConnection {
        &self.db
    }

    /// Whether `user` owns the `root` index row. Fails closed: a missing row,
    /// someone else's row, or a DB error all read as "not owned" — the WS
    /// relay's inbound gate for sessions the live registry doesn't know.
    pub async fn owns(&self, user: &CurrentUser, root: &str) -> bool {
        self.owned_row(user, root).await.is_ok()
    }

    /// The caller's conversations, most recently active first.
    pub async fn list(&self, user: &CurrentUser) -> Result<Vec<AgentSessionSummary>, SessionError> {
        let rows = agent_sessions::Entity::find()
            .filter(agent_sessions::Column::UserId.eq(user.id.clone()))
            .order_by_desc(agent_sessions::Column::LastActive)
            .all(&self.db)
            .await
            .map_err(|e| db_err(e, "listing agent sessions"))?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let live = self.is_live(&SessionId::new(row.root_session_id.clone()));
                AgentSessionSummary {
                    root_session_id: row.root_session_id,
                    name: row.name,
                    agent: row.agent,
                    created_at: row.created_at.to_string(),
                    last_active: row.last_active.to_string(),
                    live,
                }
            })
            .collect())
    }

    /// Start a new conversation on `prompt`: insert the index row, register
    /// the owner, and send the single `Spawn` frame (see the module doc for
    /// why no `SetModel` follows). An explicit `choice` of `(provider, model)`
    /// rides the [`DEFAULT_PIN`] seam as a one-shot pending pin (#215,
    /// ADR-0046) so the first turn already runs on it. Returns the new root
    /// session id.
    pub async fn create(
        &self,
        user: &CurrentUser,
        prompt: String,
        name: Option<String>,
        choice: Option<(String, String)>,
    ) -> Result<String, SessionError> {
        let uid = UserId::new(&user.id);
        match &choice {
            // Validate the chosen provider against the caller's catalog now:
            // pin resolution failure at session start only warns and leaves
            // the session on the EchoLlm stub, so a bad name must be refused
            // loudly before anything is spawned or persisted.
            Some((provider, _)) => {
                if !self.store.knows_provider(&uid, provider) {
                    return Err(SessionError::UnknownProvider(provider.clone()));
                }
            }
            // Fail fast (503) instead of spawning a session whose model pin
            // cannot resolve and would silently run on the EchoLlm stub.
            None => {
                if self.store.default_for(&uid).is_none() {
                    return Err(SessionError::NoProvider);
                }
            }
        }

        let root = format!("{}:{}", user.id, uuid::Uuid::new_v4());
        let now = chrono::Utc::now().naive_utc();
        agent_sessions::ActiveModel {
            root_session_id: Set(root.clone()),
            user_id: Set(user.id.clone()),
            name: Set(name),
            agent: Set(AGENT_PROFILE.to_string()),
            created_at: Set(now),
            last_active: Set(now),
        }
        .insert(&self.db)
        .await
        .map_err(|e| db_err(e, "creating an agent session"))?;

        let session = SessionId::new(root.clone());
        // Register + mark live *before* Spawn so the tool bridge, policy and
        // persistence sink can resolve the owner from the very first event.
        self.users.register(session.clone(), uid.clone());
        self.mark_live(session.clone());
        // Park the one-shot pin as late as possible before Spawn: session
        // start's DEFAULT_PIN resolution consumes it (ADR-0046 documents the
        // orphaned-pin race if Spawn never resolves).
        if let Some((provider, model)) = choice {
            self.store.set_pending_pin(&user.id, provider, model);
        }
        self.holly
            .send(InMsg::Spawn {
                session,
                parent: None,
                predecessor: None,
                agent: AGENT_PROFILE.to_string(),
                prompt,
                user: Some(uid),
            })
            .await
            .map_err(|_| SessionError::Internal(anyhow!("engine inbox closed")))?;
        Ok(root)
    }

    /// Open a conversation: ownership check (the owner only — admin does *not*
    /// bypass, a transcript is private), then return the persisted `OutEvent`
    /// history, resuming the engine session from the log first when it is not
    /// live in this process.
    pub async fn open(&self, user: &CurrentUser, root: &str) -> Result<OpenResult, SessionError> {
        let row = self.owned_row(user, root).await?;
        let session = SessionId::new(root);
        let mut records = load_records(&self.db, root)
            .await
            .map_err(SessionError::Internal)?;
        // A gap means everything after it is unreplayable; resume the intact
        // prefix (mirrors the site embedder's stance) and tell the caller.
        let gapped = truncate_at_gap(&mut records);
        let events: Vec<OutEvent> = records
            .iter()
            .filter_map(|r| match &r.payload {
                LogPayload::Out(ev) => Some(ev.clone()),
                _ => None,
            })
            .collect();

        if self.is_live(&session) {
            return Ok(OpenResult { events, gapped });
        }
        // Pre-register the owner: resume re-emits `SessionStarted` (which the
        // index subscriber also folds), but a tool call raced right after the
        // resume must already resolve its user.
        self.users
            .register(session.clone(), UserId::new(&row.user_id));
        if !records.is_empty() {
            self.holly
                .resume(session.clone(), pair_records(&records))
                .await
                .map_err(|_| SessionError::Internal(anyhow!("engine inbox closed")))?;
            self.mark_live(session);
        }
        // An empty log (nothing persisted yet) has nothing to resume; the next
        // prompt lazily spawns the session fresh.
        Ok(OpenResult { events, gapped })
    }

    /// Delete a conversation: hibernate the live engine session (best-effort,
    /// no wait), then drop the transcript and the index row.
    pub async fn delete(&self, user: &CurrentUser, root: &str) -> Result<(), SessionError> {
        self.owned_row(user, root).await?;
        let session = SessionId::new(root);
        if self.is_live(&session) {
            // Send-and-proceed: eviction is asynchronous, and the rows below
            // are the source of truth the engine can no longer resume from.
            let _ = self.holly.hibernate(session.clone()).await;
        }
        agent_events::Entity::delete_many()
            .filter(agent_events::Column::RootSessionId.eq(root))
            .exec(&self.db)
            .await
            .map_err(|e| db_err(e, "deleting agent events"))?;
        agent_sessions::Entity::delete_by_id(root)
            .exec(&self.db)
            .await
            .map_err(|e| db_err(e, "deleting the agent session row"))?;
        self.users.forget(&session);
        self.mark_gone(&session);
        Ok(())
    }

    /// Refresh `last_active` — called by the engine's index subscriber on
    /// owned session traffic (throttled there). Unknown root ⇒ no-op.
    pub async fn touch(&self, root: &str) -> anyhow::Result<()> {
        agent_sessions::Entity::update_many()
            .col_expr(
                agent_sessions::Column::LastActive,
                Expr::value(chrono::Utc::now().naive_utc()),
            )
            .filter(agent_sessions::Column::RootSessionId.eq(root))
            .exec(&self.db)
            .await
            .map(|_| ())
            .map_err(|e| anyhow::Error::new(e).context("touching an agent session"))
    }

    /// Store the engine-announced display name (`SessionMetaChanged`).
    pub async fn rename(&self, root: &str, name: &str) -> anyhow::Result<()> {
        agent_sessions::Entity::update_many()
            .col_expr(agent_sessions::Column::Name, Expr::value(name))
            .filter(agent_sessions::Column::RootSessionId.eq(root))
            .exec(&self.db)
            .await
            .map(|_| ())
            .map_err(|e| anyhow::Error::new(e).context("renaming an agent session"))
    }

    /// Whether the engine holds `session` in memory (per this process's fold).
    pub fn is_live(&self, session: &SessionId) -> bool {
        self.live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(session)
    }

    pub(crate) fn mark_live(&self, session: SessionId) {
        self.live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(session);
    }

    pub(crate) fn mark_gone(&self, session: &SessionId) {
        self.live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(session);
    }

    /// The `agent_sessions` row iff `user` owns it; `NotFound` otherwise (the
    /// same status for "missing" and "not yours" — no existence oracle).
    async fn owned_row(
        &self,
        user: &CurrentUser,
        root: &str,
    ) -> Result<agent_sessions::Model, SessionError> {
        let row = agent_sessions::Entity::find_by_id(root)
            .one(&self.db)
            .await
            .map_err(|e| db_err(e, "loading an agent session"))?
            .ok_or(SessionError::NotFound)?;
        if row.user_id != user.id {
            return Err(SessionError::NotFound);
        }
        Ok(row)
    }
}

/// Truncate `records` to the prefix strictly before the first `Gap`
/// tombstone. Returns whether anything was cut — replaying *through* a gap
/// would fold an incomplete history into a wrong context. Shared with the WS
/// relay, whose history replay re-reads the log for the `In` prompts.
pub(crate) fn truncate_at_gap(records: &mut Vec<LogRecord>) -> bool {
    match records
        .iter()
        .position(|r| matches!(r.payload, LogPayload::Gap { .. }))
    {
        Some(at) => {
            records.truncate(at);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests;
