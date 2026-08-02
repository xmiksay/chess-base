//! [`AgentEngine`] — the process-wide embedded entanglement engine (#198, step
//! 4): one `Holly` serving every user's sessions (ids namespaced
//! `{user}:{uuid}`), wired to the MCP tool bridge, the DB permission policy,
//! the `agent_events` persistence sink, the throttle responder and an index
//! subscriber folding engine lifecycle back onto `agent_sessions`.
//!
//! The `build` profile pins [`DEFAULT_PIN`] as its provider/model, which
//! [`AgentProviderStore::build_resolver`] resolves to the calling user's
//! default provider row *at session start, before the spawn prompt's first
//! turn* — the only ordering-safe per-user model binding (see
//! [`sessions`][super::sessions]'s module doc for the full finding).

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};
use std::time::Duration;

use anyhow::{anyhow, Context as _};
use entanglement_core::{
    AgentMode, AgentProfile, EngineConfig, Holly, OutEvent, Permission, PermissionProfile,
    ProfileRegistry, SessionId,
};
use entanglement_provider::HttpClient;
use entanglement_runtime::hooks::Hooks;
use entanglement_runtime::multi_user::SessionUserRegistry;
use entanglement_runtime::persistence::spawn_persistence_subscriber_with_sink;
use entanglement_runtime::policy::{GrantStore, PermissionResolver, SandboxConfig};
use entanglement_runtime::skills::SkillRegistry;
use entanglement_runtime::throttle::spawn_throttle_responder;
use entanglement_runtime::tool_runner;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

use crate::server::state::AppState;

use super::persistence::DbRecordSink;
use super::policy::{base_profile, AgentPolicy};
use super::provider_store::{AgentProviderStore, DEFAULT_PIN};
use super::sessions::SessionService;
use super::tools::bridge_registry;
use super::SYSTEM_PROMPT;

/// The one agent profile chess-base registers — the engine-required `build`.
pub const AGENT_PROFILE: &str = "build";

/// Auto-hibernate a settled, idle session after this long; the id stays
/// resumable from `agent_events` (`SessionService::open`).
const IDLE_TTL: Duration = Duration::from_secs(1800);

/// At most one `last_active` write per root per this interval — session
/// traffic is bursty (a streamed turn is hundreds of events).
const TOUCH_INTERVAL: Duration = Duration::from_secs(30);

/// The study agent's profile: the default `build` shape with the chess-base
/// system prompt and the [`DEFAULT_PIN`] model pin (per-user default binding
/// at session start). No sub-agents: nothing spawnable is registered and
/// `can_spawn` is closed explicitly.
fn build_profile() -> AgentProfile {
    AgentProfile {
        name: AGENT_PROFILE.into(),
        description: "Chess study assistant — analyses positions and builds studies.".into(),
        mode: AgentMode::Primary,
        system_prompt: SYSTEM_PROMPT.into(),
        model: Some(DEFAULT_PIN.into()),
        provider: Some(DEFAULT_PIN.into()),
        permission: PermissionProfile::new(Permission::Allow),
        tools: None,
        disallowed_tools: Vec::new(),
        can_spawn: Some(false),
        spawnable_agents: None,
        sandbox: None,
    }
}

/// The engine's profile registry: the built-ins with `build` replaced by
/// [`build_profile`]. Shared with the session-service tests so they run the
/// exact production profile (whose pin is inert without a `model_resolver`).
pub(crate) fn agent_profiles() -> ProfileRegistry {
    let mut profiles = ProfileRegistry::new();
    profiles.insert(build_profile());
    profiles
}

/// The running engine and its service handles. Built once at server startup
/// (`server::serve`) and stored on [`AppState::agent`].
pub struct AgentEngine {
    pub holly: Holly,
    pub users: SessionUserRegistry,
    pub sessions: SessionService,
    pub providers: Arc<AgentProviderStore>,
    /// Runtime service tasks, in spawn order; aborted in reverse by
    /// [`shutdown`][Self::shutdown].
    tasks: Vec<JoinHandle<()>>,
}

impl AgentEngine {
    /// Build and start the whole stack: provider resolver → `Holly` → tool
    /// executor (with the DB policy) → persistence sink → throttle responder →
    /// index subscriber.
    pub async fn start(app: AppState) -> anyhow::Result<Arc<Self>> {
        let db = app.db.clone();
        let providers = match app.provider_store.clone() {
            Some(store) => store,
            None => AgentProviderStore::new(db.clone()).await?,
        };
        let http = HttpClient::new().context("building the provider HTTP client")?;
        let users = SessionUserRegistry::new();
        let bridged = bridge_registry(&app, &users);
        let specs = bridged.specs();
        let shared = bridged.shared();

        let profiles = agent_profiles();
        let cfg = EngineConfig {
            tool_specs: specs,
            profiles: profiles.clone(),
            model_resolver: Some(providers.build_resolver(http.clone())),
            system_prompt_resolver: Some(Arc::new(|_: &SessionId, _: &AgentProfile| {
                Some(SYSTEM_PROMPT.to_string())
            })),
            idle_ttl: Some(IDLE_TTL),
            ..EngineConfig::default()
        };
        cfg.validate().map_err(|e| anyhow!(e.to_string()))?;
        let holly = Holly::spawn(cfg);

        let policy = AgentPolicy::new(db.clone(), users.clone());
        let resolver: Arc<dyn PermissionResolver> = policy.clone();
        let grants: Arc<dyn GrantStore> = policy;
        let executor = tool_runner::spawn_tool_executor_with_policy(
            &holly,
            shared,
            Arc::new(StdRwLock::new(profiles)),
            Arc::new(StdRwLock::new(Arc::new(SkillRegistry::default()))),
            base_profile(),
            Arc::new(StdMutex::new(HashMap::new())),
            resolver,
            grants,
            Hooks::default(),
            None,
            // The bridged MCP tools never exec anything; bubblewrap is for
            // `bash`/`call`, which chess-base does not register.
            SandboxConfig::none(),
        );

        let (sink, writer) = DbRecordSink::spawn(db.clone(), users.clone());
        let persistence = spawn_persistence_subscriber_with_sink(&holly, sink);
        let throttle = spawn_throttle_responder(&holly, http);

        let sessions = SessionService::new(db, holly.clone(), users.clone(), providers.clone());
        let index = spawn_index_subscriber(&holly, users.clone(), sessions.clone());

        Ok(Arc::new(Self {
            holly,
            users,
            sessions,
            providers,
            tasks: vec![executor, writer, persistence, throttle, index],
        }))
    }

    /// Abort the service tasks, newest first (index/throttle before the
    /// persistence tap, the tap before its writer, the executor last), so no
    /// survivor observes a torn-down dependency. `server::serve` has no
    /// graceful-shutdown seam today (`axum::serve` runs until process exit),
    /// so this is exercised from tests only.
    pub fn shutdown(&self) {
        for task in self.tasks.iter().rev() {
            task.abort();
        }
    }
}

/// Fold the engine's lifecycle stream back onto the service state: user
/// registry entries, the live-session set, and the `agent_sessions` index
/// (`name` from `SessionMetaChanged`, throttled `last_active` from any owned
/// traffic). Lag is logged and skipped — the index is best-effort, the event
/// log itself is owned by the persistence tap.
fn spawn_index_subscriber(
    holly: &Holly,
    users: SessionUserRegistry,
    sessions: SessionService,
) -> JoinHandle<()> {
    let mut sub = holly.subscribe();
    tokio::spawn(async move {
        // session → its root, folded from `SessionStarted` (children resolve
        // through their parent, matching the persistence tap's routing).
        let mut roots: HashMap<SessionId, SessionId> = HashMap::new();
        let mut touched: HashMap<SessionId, tokio::time::Instant> = HashMap::new();
        loop {
            match sub.recv().await {
                Ok(ev) => fold_event(ev, &users, &sessions, &mut roots, &mut touched).await,
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!(dropped = n, "agent index subscriber lagged");
                }
                Err(RecvError::Closed) => break,
            }
        }
    })
}

async fn fold_event(
    ev: OutEvent,
    users: &SessionUserRegistry,
    sessions: &SessionService,
    roots: &mut HashMap<SessionId, SessionId>,
    touched: &mut HashMap<SessionId, tokio::time::Instant>,
) {
    match &ev {
        OutEvent::SessionStarted {
            session,
            parent,
            root,
            user,
            ..
        } => {
            let root_id = if *root {
                session.clone()
            } else {
                parent
                    .as_ref()
                    .and_then(|p| roots.get(p).cloned())
                    .unwrap_or_else(|| session.clone())
            };
            roots.insert(session.clone(), root_id);
            // Children inherit the user; registering them keeps the tool
            // bridge/policy resolvable for any (future) sub-session too.
            if let Some(user) = user {
                users.register(session.clone(), user.clone());
            }
            if *root {
                sessions.mark_live(session.clone());
            }
        }
        OutEvent::SessionEnded { session, .. } | OutEvent::SessionHibernated { session, .. } => {
            users.forget(session);
            if roots.get(session) == Some(session) {
                sessions.mark_gone(session);
            }
            roots.remove(session);
            touched.remove(session);
        }
        OutEvent::SessionMetaChanged {
            session,
            name: Some(name),
            ..
        } if roots.get(session) == Some(session) => {
            if let Err(err) = sessions.rename(&session.0, name).await {
                tracing::warn!(session = %session.0, error = %format!("{err:#}"),
                    "could not store the agent session name");
            }
        }
        _ => {}
    }

    // Any owned, session-scoped event refreshes `last_active`, at most once
    // per `TOUCH_INTERVAL` per root.
    if let Some(session) = ev.session() {
        if let Some(root) = roots.get(session).cloned() {
            let now = tokio::time::Instant::now();
            let due = touched
                .get(&root)
                .is_none_or(|last| now.duration_since(*last) >= TOUCH_INTERVAL);
            if due {
                touched.insert(root.clone(), now);
                if let Err(err) = sessions.touch(&root.0).await {
                    tracing::warn!(session = %root.0, error = %format!("{err:#}"),
                        "could not touch the agent session");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
