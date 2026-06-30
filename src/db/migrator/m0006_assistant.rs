//! m0006 — embedded AI study assistant (issue #20, Direction B): the persisted
//! chat sessions + transcript and the admin-managed LLM provider registry.
//!
//! - `assistant_sessions` — one chat conversation, scoped to its `owner_id`
//!   (mirrors `studies.owner_id`: no FK, a plain owner string; `local-admin`
//!   in local mode). Carries the model id the loop drives.
//! - `assistant_messages` — the ordered transcript. Each row stores one
//!   provider-agnostic `ai::llm::Message` serialized as JSON (`content_json`),
//!   with `role` lifted out for cheap filtering/ordering.
//! - `llm_providers` — admin-configured providers (API key server-side only);
//!   the default row builds the [`LlmProvider`] at startup.
//!
//! Schema-builder only, so the same migration runs on SQLite and Postgres.

use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0006_assistant"
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
    Id,
    Name,
    Model,
    ApiKey,
    IsDefault,
    CreatedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
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

        manager
            .create_table(
                Table::create()
                    .table(LlmProviders::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(LlmProviders::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(LlmProviders::Name).string().not_null())
                    .col(ColumnDef::new(LlmProviders::Model).string().not_null())
                    .col(ColumnDef::new(LlmProviders::ApiKey).text().not_null())
                    .col(
                        ColumnDef::new(LlmProviders::IsDefault)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(LlmProviders::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
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
            Index::create()
                .name("idx_llm_providers_name")
                .table(LlmProviders::Table)
                .col(LlmProviders::Name)
                .unique()
                .to_owned(),
        ] {
            manager.create_index(index).await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            Table::drop().table(AssistantMessages::Table).to_owned(),
            Table::drop().table(AssistantSessions::Table).to_owned(),
            Table::drop().table(LlmProviders::Table).to_owned(),
        ] {
            manager.drop_table(table).await?;
        }
        Ok(())
    }
}
