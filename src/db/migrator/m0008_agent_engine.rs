//! m0008 — embedded entanglement agent engine (#198), step 1: schema.
//!
//! - Drops the hand-rolled assistant's `assistant_messages` / `assistant_sessions`
//!   (the engine persists an event log instead — no transcript migration).
//! - Extends `llm_providers` for multi-wire, per-user providers: `owner_id`
//!   (NULL ⇒ admin-global), `wire` (protocol, default `anthropic`), `base_url`
//!   (override endpoint). The unique index moves from `name` to
//!   `(owner_id, name)`.
//! - `agent_grants` — persisted per-user tool approvals ("always allow").
//! - `agent_events` — the append-only agent event log, ordered by `ord` within
//!   a `root_session_id`.
//! - `agent_sessions` — one engine conversation per `root_session_id`.
//!
//! Schema-builder only (one ADD COLUMN per `alter_table` for SQLite), so the
//! same migration runs on SQLite and Postgres.

use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0008_agent_engine"
    }
}

#[derive(DeriveIden)]
enum AssistantSessions {
    Table,
    Id,
    OwnerId,
    Title,
    Model,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum AssistantMessages {
    Table,
    Id,
    SessionId,
    Seq,
    Role,
    ContentJson,
    CreatedAt,
}

#[derive(DeriveIden)]
enum LlmProviders {
    Table,
    Name,
    OwnerId,
    Wire,
    BaseUrl,
}

#[derive(DeriveIden)]
enum AgentGrants {
    Table,
    Id,
    UserId,
    Tool,
    Arg,
    Scope,
    CreatedAt,
}

#[derive(DeriveIden)]
enum AgentEvents {
    Table,
    Id,
    RootSessionId,
    UserId,
    Ord,
    Ts,
    SessionId,
    Direction,
    Payload,
}

#[derive(DeriveIden)]
enum AgentSessions {
    Table,
    RootSessionId,
    UserId,
    Name,
    Agent,
    CreatedAt,
    LastActive,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // The old assistant transcript goes away with the hand-rolled loop.
        for table in [
            Table::drop().table(AssistantMessages::Table).to_owned(),
            Table::drop().table(AssistantSessions::Table).to_owned(),
        ] {
            manager.drop_table(table).await?;
        }

        // llm_providers: one ADD COLUMN per alter_table call (SQLite).
        for column in [
            ColumnDef::new(LlmProviders::OwnerId).string().null().take(),
            ColumnDef::new(LlmProviders::Wire)
                .string()
                .not_null()
                .default("anthropic")
                .take(),
            ColumnDef::new(LlmProviders::BaseUrl).string().null().take(),
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(LlmProviders::Table)
                        .add_column(column)
                        .to_owned(),
                )
                .await?;
        }
        manager
            .drop_index(
                Index::drop()
                    .name("idx_llm_providers_name")
                    .table(LlmProviders::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_llm_providers_owner_name")
                    .table(LlmProviders::Table)
                    .col(LlmProviders::OwnerId)
                    .col(LlmProviders::Name)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(AgentGrants::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AgentGrants::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AgentGrants::UserId).string().not_null())
                    .col(ColumnDef::new(AgentGrants::Tool).string().not_null())
                    .col(ColumnDef::new(AgentGrants::Arg).string().null())
                    .col(ColumnDef::new(AgentGrants::Scope).string().not_null())
                    .col(
                        ColumnDef::new(AgentGrants::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(AgentEvents::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AgentEvents::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AgentEvents::RootSessionId)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(AgentEvents::UserId).string().not_null())
                    .col(ColumnDef::new(AgentEvents::Ord).big_integer().not_null())
                    .col(ColumnDef::new(AgentEvents::Ts).big_integer().not_null())
                    .col(ColumnDef::new(AgentEvents::SessionId).string().not_null())
                    .col(ColumnDef::new(AgentEvents::Direction).string().not_null())
                    .col(ColumnDef::new(AgentEvents::Payload).text().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(AgentSessions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AgentSessions::RootSessionId)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AgentSessions::UserId).string().not_null())
                    .col(ColumnDef::new(AgentSessions::Name).string().null())
                    .col(ColumnDef::new(AgentSessions::Agent).string().not_null())
                    .col(
                        ColumnDef::new(AgentSessions::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(AgentSessions::LastActive)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        for index in [
            Index::create()
                .name("idx_agent_grants_user_tool_arg")
                .table(AgentGrants::Table)
                .col(AgentGrants::UserId)
                .col(AgentGrants::Tool)
                .col(AgentGrants::Arg)
                .unique()
                .to_owned(),
            Index::create()
                .name("idx_agent_events_root_ord")
                .table(AgentEvents::Table)
                .col(AgentEvents::RootSessionId)
                .col(AgentEvents::Ord)
                .unique()
                .to_owned(),
            Index::create()
                .name("idx_agent_events_user")
                .table(AgentEvents::Table)
                .col(AgentEvents::UserId)
                .to_owned(),
            Index::create()
                .name("idx_agent_sessions_user_active")
                .table(AgentSessions::Table)
                .col(AgentSessions::UserId)
                .col(AgentSessions::LastActive)
                .to_owned(),
        ] {
            manager.create_index(index).await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            Table::drop().table(AgentSessions::Table).to_owned(),
            Table::drop().table(AgentEvents::Table).to_owned(),
            Table::drop().table(AgentGrants::Table).to_owned(),
        ] {
            manager.drop_table(table).await?;
        }

        // The composite index covers `owner_id`, so it must go before the column.
        manager
            .drop_index(
                Index::drop()
                    .name("idx_llm_providers_owner_name")
                    .table(LlmProviders::Table)
                    .to_owned(),
            )
            .await?;
        for column in [
            LlmProviders::BaseUrl,
            LlmProviders::Wire,
            LlmProviders::OwnerId,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(LlmProviders::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        manager
            .create_index(
                Index::create()
                    .name("idx_llm_providers_name")
                    .table(LlmProviders::Table)
                    .col(LlmProviders::Name)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // Recreate the m0006 assistant tables (empty) so m0006's own down() and
        // any re-up still line up. Definitions copied verbatim from m0006.
        manager
            .create_table(
                Table::create()
                    .table(AssistantSessions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AssistantSessions::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AssistantSessions::OwnerId)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(AssistantSessions::Title).string().not_null())
                    .col(ColumnDef::new(AssistantSessions::Model).string().not_null())
                    .col(
                        ColumnDef::new(AssistantSessions::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(AssistantSessions::UpdatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(AssistantMessages::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AssistantMessages::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AssistantMessages::SessionId)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(AssistantMessages::Seq).integer().not_null())
                    .col(ColumnDef::new(AssistantMessages::Role).string().not_null())
                    .col(
                        ColumnDef::new(AssistantMessages::ContentJson)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AssistantMessages::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_assistant_messages_session")
                            .from(AssistantMessages::Table, AssistantMessages::SessionId)
                            .to(AssistantSessions::Table, AssistantSessions::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        for index in [
            Index::create()
                .name("idx_assistant_sessions_owner")
                .table(AssistantSessions::Table)
                .col(AssistantSessions::OwnerId)
                .to_owned(),
            Index::create()
                .name("idx_assistant_messages_session")
                .table(AssistantMessages::Table)
                .col(AssistantMessages::SessionId)
                .col(AssistantMessages::Seq)
                .to_owned(),
        ] {
            manager.create_index(index).await?;
        }

        Ok(())
    }
}
