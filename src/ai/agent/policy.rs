//! Permission policy for the embedded entanglement agent engine (#198, step 3).
//!
//! [`AgentPolicy`] plugs into the runtime tool executor's policy seams
//! ([`PermissionResolver`] + [`GrantStore`]): the mutating MCP tools
//! ([`GATED_TOOLS`]) grade `Ask`, everything else `Allow` ([`base_profile`]),
//! and an `Ask` upgrades to `Allow` when the caller approved the tool earlier —
//! in-memory for a `Session`-scoped approval, or via a persisted `agent_grants`
//! row (m0008) for an `Always` one, scoped to the session's [`UserId`] from the
//! shared [`SessionUserRegistry`]. Per the trait contract, the persisted read
//! side lives in the resolver, so [`GrantStore::is_granted`] is always `false`.
//!
//! [`UserId`]: entanglement_provider::UserId

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use entanglement_core::{ApprovalScope, Permission, PermissionProfile, SessionId};
use entanglement_runtime::multi_user::SessionUserRegistry;
use entanglement_runtime::permission::{permission_arg, permission_workdir};
use entanglement_runtime::policy::{GrantStore, PermissionResolver};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, Set};

use crate::db::entities::agent_grants;

use super::GATED_TOOLS;

/// The engine's base permission profile: every mutating tool asks, every read
/// runs — the same split `requires_approval` encoded for the hand-rolled loop
/// (ADR-0025).
pub fn base_profile() -> PermissionProfile {
    GATED_TOOLS.iter().fold(
        PermissionProfile::new(Permission::Allow),
        |profile, tool| profile.with(*tool, Permission::Ask),
    )
}

/// In-memory session grants: `(tool, arg)` pairs approved for the rest of a
/// session.
type SessionGrants = HashMap<SessionId, HashSet<(String, Option<String>)>>;

/// DB-backed policy for the embedded agent: one instance shared by the
/// executor's resolver and grant-store seams.
pub struct AgentPolicy {
    db: DatabaseConnection,
    users: SessionUserRegistry,
    base: PermissionProfile,
    session_grants: Mutex<SessionGrants>,
}

impl AgentPolicy {
    pub fn new(db: DatabaseConnection, users: SessionUserRegistry) -> Arc<Self> {
        Arc::new(Self {
            db,
            users,
            base: base_profile(),
            session_grants: Mutex::new(HashMap::new()),
        })
    }

    fn session_granted(&self, session: &SessionId, tool: &str, arg: Option<&str>) -> bool {
        self.session_grants
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(session)
            .is_some_and(|set| set.contains(&(tool.to_string(), arg.map(String::from))))
    }

    /// Whether an `Always` grant row exists for this exact `(user, tool, arg)`
    /// — arg comparison is exact, including `NULL`.
    async fn persisted_granted(
        &self,
        user: &str,
        tool: &str,
        arg: Option<&str>,
    ) -> Result<bool, DbErr> {
        let query = agent_grants::Entity::find()
            .filter(agent_grants::Column::UserId.eq(user))
            .filter(agent_grants::Column::Tool.eq(tool));
        let query = match arg {
            Some(a) => query.filter(agent_grants::Column::Arg.eq(a)),
            None => query.filter(agent_grants::Column::Arg.is_null()),
        };
        Ok(query.one(&self.db).await?.is_some())
    }

    /// Idempotently persist an `Always` grant. SQL `NULL` never equals `NULL`,
    /// so the unique `(user_id, tool, arg)` index cannot dedupe a `NULL`-arg
    /// row — the pre-check handles that case; the on-conflict clause covers a
    /// concurrent non-NULL insert.
    async fn persist_always(&self, user: &str, tool: &str, arg: Option<&str>) -> Result<(), DbErr> {
        if self.persisted_granted(user, tool, arg).await? {
            return Ok(());
        }
        let row = agent_grants::ActiveModel {
            user_id: Set(user.to_string()),
            tool: Set(tool.to_string()),
            arg: Set(arg.map(String::from)),
            scope: Set("always".to_string()),
            ..Default::default()
        };
        agent_grants::Entity::insert(row)
            .on_conflict(
                OnConflict::columns([
                    agent_grants::Column::UserId,
                    agent_grants::Column::Tool,
                    agent_grants::Column::Arg,
                ])
                .do_nothing()
                .to_owned(),
            )
            .do_nothing()
            .exec(&self.db)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl PermissionResolver for AgentPolicy {
    async fn resolve(&self, session: &SessionId, tool: &str, input: &str) -> Permission {
        let arg = permission_arg(tool, input);
        let workdir = permission_workdir(tool, input);
        let grade = self
            .base
            .resolve_scoped(tool, arg.as_deref(), workdir.as_deref());
        if grade != Permission::Ask {
            return grade;
        }
        if self.session_granted(session, tool, arg.as_deref()) {
            return Permission::Allow;
        }
        let Some(user) = self.users.user_for(session) else {
            return Permission::Ask;
        };
        match self.persisted_granted(&user.0, tool, arg.as_deref()).await {
            Ok(true) => Permission::Allow,
            Ok(false) => Permission::Ask,
            // Fail-safe: a broken DB must neither silently allow a mutation
            // nor hard-deny the tool — asking the user is always sound.
            Err(err) => {
                tracing::warn!(%session, tool, error = %err, "agent grant lookup failed; asking");
                Permission::Ask
            }
        }
    }
}

#[async_trait]
impl GrantStore for AgentPolicy {
    /// Always `false`: both grant kinds are read back through the resolver
    /// above (the trait's documented multi-tenant posture).
    fn is_granted(&self, _session: &SessionId, _tool: &str, _arg: Option<&str>) -> bool {
        false
    }

    async fn record(
        &self,
        session: &SessionId,
        tool: &str,
        arg: Option<&str>,
        scope: ApprovalScope,
    ) {
        match scope {
            ApprovalScope::Once => {}
            // The MCP tools take no path argument, so a `SessionDir` approval
            // degrades to an exact session grant (mirrors the runtime default).
            ApprovalScope::Session | ApprovalScope::SessionDir => {
                self.session_grants
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .entry(session.clone())
                    .or_default()
                    .insert((tool.to_string(), arg.map(String::from)));
            }
            ApprovalScope::Always => {
                let Some(user) = self.users.user_for(session) else {
                    tracing::warn!(%session, tool, "always-grant from an unregistered session skipped");
                    return;
                };
                if let Err(err) = self.persist_always(&user.0, tool, arg).await {
                    tracing::warn!(%session, tool, error = %err, "persisting an always-grant failed");
                }
            }
        }
    }

    fn forget_session(&self, session: &SessionId) {
        self.session_grants
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(session);
    }
}

#[cfg(test)]
mod tests;
