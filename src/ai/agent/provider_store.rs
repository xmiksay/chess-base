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
use std::sync::{Arc, PoisonError, RwLock};

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

/// Requests-per-minute budget applied to every per-user provider entry, so one
/// user cannot starve the shared endpoint pool.
const PER_USER_RPM: u32 = 60;

/// DB-backed [`UserProviderStore`]: async writes rebuild the cache, the sync
/// trait read never touches I/O.
pub struct AgentProviderStore {
    db: DatabaseConnection,
    /// `ANTHROPIC_API_KEY` captured once at construction; injectable for tests.
    env_key: Option<String>,
    /// Owner id (or [`HOUSE_USER`]) → that owner's provider context. Every
    /// present user key maps to `Some`; only the house entry can be `None`.
    cache: RwLock<HashMap<String, Option<UserProviderContext>>>,
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
            cache: RwLock::new(HashMap::new()),
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
    /// resolves against the house context instead of hard-erroring.
    pub fn build_resolver(self: &Arc<Self>, http: HttpClient) -> ModelResolver {
        let store: Arc<dyn UserProviderStore> = Arc::clone(self) as _;
        let inner = build_user_model_resolver(store, http, None);
        let house = UserId::new(HOUSE_USER);
        Arc::new(move |user, provider, model| inner(Some(user.unwrap_or(&house)), provider, model))
    }

    async fn rebuild(&self) -> anyhow::Result<()> {
        let rows = llm_providers::Entity::find()
            .all(&self.db)
            .await
            .context("loading llm_providers for the agent provider store")?;
        let builtin = Catalog::builtin();

        let mut globals = Vec::new();
        let mut by_owner: HashMap<String, Vec<llm_providers::Model>> = HashMap::new();
        for row in rows {
            match row.owner_id.clone() {
                Some(owner) => by_owner.entry(owner).or_default().push(row),
                None => globals.push(row),
            }
        }

        let mut map = HashMap::new();
        map.insert(
            HOUSE_USER.to_string(),
            house_context(&globals, self.env_key.as_deref(), &builtin),
        );
        for (owner, own) in by_owner {
            map.insert(owner, Some(compose_context(&own, &globals, &builtin)));
        }

        *self.cache.write().unwrap_or_else(PoisonError::into_inner) = map;
        Ok(())
    }
}

impl UserProviderStore for AgentProviderStore {
    fn context(&self, user: &UserId) -> Option<UserProviderContext> {
        let cache = self.cache.read().unwrap_or_else(PoisonError::into_inner);
        match cache.get(user.0.as_str()) {
            Some(ctx) => ctx.clone(),
            // A user with no rows of their own still works via the global
            // (house) configuration.
            None => cache.get(HOUSE_USER).cloned().flatten(),
        }
    }
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
