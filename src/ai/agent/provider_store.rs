//! Per-user LLM provider store for the embedded entanglement engine (#198).
//!
//! Implements entanglement's [`UserProviderStore`] seam over the
//! `llm_providers` table: each user's catalog is their own rows layered over
//! the admin-managed global rows (user wins on a name collision), and the
//! **house** context (global rows, else the `ANTHROPIC_API_KEY` env fallback)
//! serves users with no rows at all. `UserProviderStore::context` is called
//! synchronously on every model resolution, so it is a pure cache read — the
//! cache is pre-warmed in [`AgentProviderStore::new`] and rebuilt by
//! [`AgentProviderStore::invalidate`] after provider CRUD.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use anyhow::Context as _;
use entanglement_provider::{
    Catalog, HttpClient, ModelEntry, ModelResolver, ProviderEntry, UserId, Wire,
};
use entanglement_runtime::multi_user::provider::{
    build_user_model_resolver, UserProviderContext, UserProviderStore,
};
use sea_orm::{DatabaseConnection, EntityTrait};

use crate::db::entities::llm_providers;

/// Cache key (and resolver-side [`UserId`]) for the house context — the
/// fallback provider surface used when a session has no per-user rows. `~` is
/// not a valid account id, so it can never collide with a real user.
pub const HOUSE_USER: &str = "~house";

/// Sentinel `(provider, model)` pin carried by the engine's `build` profile.
/// The session-start model pin is the only per-user binding that lands
/// *before* a `Spawn`'s queued prompt starts the first turn (a `SetModel` sent
/// after `Spawn` queues behind it), so the profile pins this sentinel and
/// [`AgentProviderStore::build_resolver`] maps it to the calling user's
/// default provider row at resolution time. `~` keeps it out of the space of
/// real provider names, like [`HOUSE_USER`].
pub const DEFAULT_PIN: &str = "~default";

/// Requests-per-minute budget applied to every per-user provider entry, so one
/// user cannot starve the shared endpoint pool.
const PER_USER_RPM: u32 = 60;

/// DB-backed [`UserProviderStore`]: async writes rebuild the cache, the sync
/// trait read never touches I/O.
pub struct AgentProviderStore {
    db: DatabaseConnection,
    /// `ANTHROPIC_API_KEY` captured once at construction; injectable for tests.
    env_key: Option<String>,
    cache: RwLock<Caches>,
    /// User id → a **one-shot** `(provider, model)` pin consumed by that
    /// user's next [`DEFAULT_PIN`] resolution — the creation-time model choice
    /// (#215, ADR-0046). Set by `SessionService::create` immediately before
    /// `Spawn`, taken by [`Self::build_resolver`]'s sentinel branch at session
    /// start. Accepted narrow races (ADR-0046): two simultaneous creates by
    /// one user can cross pins, and a `Spawn` whose pin never resolves leaves
    /// an orphan the user's *next* default-pinned session would consume.
    pending_pins: Mutex<HashMap<String, (String, String)>>,
}

/// The pre-warmed per-owner caches, rebuilt together by
/// [`AgentProviderStore::rebuild`].
#[derive(Default)]
struct Caches {
    /// Owner id (or [`HOUSE_USER`]) → that owner's provider context. Every
    /// present user key maps to `Some`; only the house entry can be `None`.
    contexts: HashMap<String, Option<UserProviderContext>>,
    /// Owner id (or [`HOUSE_USER`]) → the `(provider, model)` a fresh session
    /// starts on: the owner's own default row, else the global default, else
    /// the owner's first row, else the first global row; the house entry falls
    /// back to the env-key anthropic default. Absent ⇒ nothing resolvable.
    defaults: HashMap<String, (String, String)>,
}

impl AgentProviderStore {
    /// Load all provider rows and pre-warm the per-owner context cache.
    /// Reads `ANTHROPIC_API_KEY` once, here — never in deep code.
    pub async fn new(db: DatabaseConnection) -> anyhow::Result<Arc<Self>> {
        let env_key = std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|k| !k.is_empty());
        Self::new_with_env(db, env_key).await
    }

    /// [`Self::new`] with the env fallback key injected — the hermetic seam
    /// tests use so they never depend on the process environment.
    pub async fn new_with_env(
        db: DatabaseConnection,
        env_key: Option<String>,
    ) -> anyhow::Result<Arc<Self>> {
        let store = Arc::new(Self {
            db,
            env_key,
            cache: RwLock::new(Caches::default()),
            pending_pins: Mutex::new(HashMap::new()),
        });
        store.rebuild().await?;
        Ok(store)
    }

    /// Rebuild the cached context for `owner` after a provider upsert/delete.
    /// Global rows compose into every user's catalog, so the simplest correct
    /// refresh is a full rebuild — provider CRUD is rare (KISS).
    pub async fn invalidate(&self, _owner: Option<&str>) -> anyhow::Result<()> {
        self.rebuild().await
    }

    /// Wrap entanglement's user resolver so a session without a [`UserId`]
    /// resolves against the house context instead of hard-erroring, and so the
    /// [`DEFAULT_PIN`] sentinel resolves to the calling user's default
    /// provider/model (the seam the `build` profile's session-start pin uses).
    pub fn build_resolver(self: &Arc<Self>, http: HttpClient) -> ModelResolver {
        let store: Arc<dyn UserProviderStore> = Arc::clone(self) as _;
        let inner = build_user_model_resolver(store, http, None);
        let this = Arc::clone(self);
        let house = UserId::new(HOUSE_USER);
        Arc::new(move |user, provider, model| {
            let user = user.unwrap_or(&house);
            if provider == DEFAULT_PIN {
                let (provider, model) = this
                    .take_pending_pin(user.0.as_str())
                    .or_else(|| this.default_for(user))
                    .ok_or_else(|| "no LLM provider configured".to_string())?;
                inner(Some(user), &provider, &model)
            } else {
                inner(Some(user), provider, model)
            }
        })
    }

    /// The `(provider, model)` a fresh session for `user` starts on — a pure
    /// cache read (see [`Caches::defaults`]). `None` ⇒ nothing is configured
    /// anywhere, so a session cannot resolve a backend.
    pub fn default_for(&self, user: &UserId) -> Option<(String, String)> {
        let cache = self.cache.read().unwrap_or_else(PoisonError::into_inner);
        cache
            .defaults
            .get(user.0.as_str())
            .or_else(|| cache.defaults.get(HOUSE_USER))
            .cloned()
    }

    /// Park a one-shot `(provider, model)` pin consumed by `user`'s next
    /// [`DEFAULT_PIN`] resolution — the creation-time model choice seam (#215,
    /// ADR-0046). Callers must validate `provider` against the user's catalog
    /// first ([`Self::knows_provider`]): a pin that fails to resolve only
    /// *warns* and leaves the session on the engine-default `EchoLlm` stub.
    pub fn set_pending_pin(&self, user: &str, provider: String, model: String) {
        self.pending_pins
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(user.to_string(), (provider, model));
    }

    fn take_pending_pin(&self, user: &str) -> Option<(String, String)> {
        self.pending_pins
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(user)
    }

    /// Whether `provider` exists in `user`'s composed catalog — a pure cache
    /// read, the guard `SessionService::create` runs before parking a pin.
    pub fn knows_provider(&self, user: &UserId, provider: &str) -> bool {
        self.context(user)
            .is_some_and(|ctx| ctx.catalog.provider(provider).is_some())
    }

    /// Whether *any* provider surface exists (a user row, a global row, or the
    /// env-key house fallback) — the truthful `/api/health` `llm` signal.
    pub fn has_any_context(&self) -> bool {
        self.cache
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contexts
            .values()
            .any(Option::is_some)
    }

    async fn rebuild(&self) -> anyhow::Result<()> {
        let mut rows = llm_providers::Entity::find()
            .all(&self.db)
            .await
            .context("loading llm_providers for the agent provider store")?;
        // `all` carries no ORDER BY; sort so "first row" defaulting is stable.
        rows.sort_by_key(|r| r.id);
        let builtin = Catalog::builtin();

        let mut globals = Vec::new();
        let mut by_owner: HashMap<String, Vec<llm_providers::Model>> = HashMap::new();
        for row in rows {
            match row.owner_id.clone() {
                Some(owner) => by_owner.entry(owner).or_default().push(row),
                None => globals.push(row),
            }
        }

        let mut caches = Caches::default();
        let house_ctx = house_context(&globals, self.env_key.as_deref(), &builtin);
        if let Some(pair) = house_default(&globals, house_ctx.as_ref()) {
            caches.defaults.insert(HOUSE_USER.to_string(), pair);
        }
        caches.contexts.insert(HOUSE_USER.to_string(), house_ctx);
        for (owner, own) in by_owner {
            if let Some(pair) = owner_default(&own, &globals) {
                caches.defaults.insert(owner.clone(), pair);
            }
            caches
                .contexts
                .insert(owner, Some(compose_context(&own, &globals, &builtin)));
        }

        *self.cache.write().unwrap_or_else(PoisonError::into_inner) = caches;
        Ok(())
    }
}

impl UserProviderStore for AgentProviderStore {
    fn context(&self, user: &UserId) -> Option<UserProviderContext> {
        let cache = self.cache.read().unwrap_or_else(PoisonError::into_inner);
        match cache.contexts.get(user.0.as_str()) {
            Some(ctx) => ctx.clone(),
            // A user with no rows of their own still works via the global
            // (house) configuration.
            None => cache.contexts.get(HOUSE_USER).cloned().flatten(),
        }
    }
}

/// One owner's starting `(provider, model)` — mirrors
/// [`crate::ai::providers::ProviderService::resolve_default_for`]'s default-flag
/// precedence, extended with first-row fallbacks so a configured-but-unflagged
/// catalog still yields a usable pin: own default → global default → own first
/// → global first.
fn owner_default(
    own: &[llm_providers::Model],
    globals: &[llm_providers::Model],
) -> Option<(String, String)> {
    own.iter()
        .find(|r| r.is_default)
        .or_else(|| globals.iter().find(|r| r.is_default))
        .or_else(|| own.first())
        .or_else(|| globals.first())
        .map(|r| (r.name.clone(), r.model.clone()))
}

/// The house's starting `(provider, model)`: the global default row, else the
/// first global row, else whatever single entry the env-key fallback context
/// synthesized (the builtin anthropic catalog entry).
fn house_default(
    globals: &[llm_providers::Model],
    ctx: Option<&UserProviderContext>,
) -> Option<(String, String)> {
    globals
        .iter()
        .find(|r| r.is_default)
        .or_else(|| globals.first())
        .map(|r| (r.name.clone(), r.model.clone()))
        .or_else(|| {
            ctx.and_then(|c| c.catalog.providers.first())
                .map(|p| (p.name.clone(), p.default_model.clone()))
        })
}

/// The `key_env` label for a row — purely a lookup key inside
/// [`UserProviderContext`], never an actual environment variable.
fn key_label(name: &str) -> String {
    format!("{}_KEY", name.to_uppercase())
}

/// Map a row's `wire` column onto entanglement's [`Wire`]. An unknown wire
/// with an explicit endpoint is assumed OpenAI-compatible (the ecosystem
/// default); without one the row is unusable and skipped with a warning.
fn wire_of(row: &llm_providers::Model) -> Option<Wire> {
    match row.wire.as_str() {
        "anthropic" => Some(Wire::Anthropic),
        "openai" => Some(Wire::Openai),
        "gemini" => Some(Wire::Gemini),
        _ if row.base_url.is_some() => Some(Wire::Openai),
        other => {
            tracing::warn!(
                provider = %row.name,
                wire = %other,
                "unknown wire without a base_url — skipping provider row"
            );
            None
        }
    }
}

/// A single bare model entry for a provider the builtin catalog doesn't know —
/// no metadata, so the engine falls back to its flat context-window cap.
fn minimal_model(id: &str) -> ModelEntry {
    ModelEntry {
        id: id.to_string(),
        display_name: None,
        context_window: None,
        max_output_tokens: None,
        supports_thinking: false,
        thinking_budget_tokens: None,
        supports_temperature: true,
        default_temperature: None,
        default_reasoning_effort: None,
        web_search_tool_version: None,
        concurrency: None,
        pricing: None,
    }
}

fn entry_from_row(row: &llm_providers::Model, builtin: &Catalog) -> Option<ProviderEntry> {
    let wire = wire_of(row)?;
    // A builtin provider of the same name donates its model metadata (pricing,
    // context windows) for free; the row still picks the default model.
    let models = builtin
        .provider(&row.name)
        .map(|p| p.models.clone())
        .filter(|models| !models.is_empty())
        .unwrap_or_else(|| vec![minimal_model(&row.model)]);
    Some(ProviderEntry {
        name: row.name.clone(),
        wire,
        base_url: row.base_url.clone(), // None → the wire's default endpoint
        key_env: Some(key_label(&row.name)),
        rpm: Some(PER_USER_RPM),
        concurrency: None,
        default_model: row.model.clone(),
        models,
        mcp_servers: HashMap::new(),
    })
}

/// One owner's context: their own rows first, then the global rows whose
/// provider name they haven't overridden (user wins on a name collision).
fn compose_context(
    own: &[llm_providers::Model],
    globals: &[llm_providers::Model],
    builtin: &Catalog,
) -> UserProviderContext {
    let own_names: HashSet<&str> = own.iter().map(|r| r.name.as_str()).collect();
    let rows = own.iter().chain(
        globals
            .iter()
            .filter(|g| !own_names.contains(g.name.as_str())),
    );

    let mut providers = Vec::new();
    let mut keys = Vec::new();
    for row in rows {
        let Some(entry) = entry_from_row(row, builtin) else {
            continue;
        };
        if !row.api_key.is_empty() {
            keys.push((key_label(&row.name), row.api_key.clone()));
        }
        providers.push(entry);
    }

    let mut ctx = UserProviderContext::new(Catalog { providers });
    for (label, key) in keys {
        ctx = ctx.with_key(label, key);
    }
    ctx
}

/// The house context: global rows only; with none, fall back to an
/// `ANTHROPIC_API_KEY` env key over the builtin anthropic catalog entry.
/// `None` ⇒ no global configuration at all — resolution fails loudly.
fn house_context(
    globals: &[llm_providers::Model],
    env_key: Option<&str>,
    builtin: &Catalog,
) -> Option<UserProviderContext> {
    let ctx = compose_context(&[], globals, builtin);
    if !ctx.catalog.providers.is_empty() {
        return Some(ctx);
    }
    let key = env_key?;
    let mut entry = builtin.provider("anthropic")?.clone();
    entry.rpm = Some(PER_USER_RPM);
    entry.concurrency = None;
    let label = entry
        .key_env
        .clone()
        .unwrap_or_else(|| "ANTHROPIC_API_KEY".to_string());
    Some(
        UserProviderContext::new(Catalog {
            providers: vec![entry],
        })
        .with_key(label, key),
    )
}

#[cfg(test)]
mod tests;
