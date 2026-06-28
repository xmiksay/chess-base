//! m0004 — MCP authentication schema (ADR-0016): the `service_tokens` static
//! bearer table (local-mode token + admin-issued server tokens) and the OAuth
//! 2.1 trio `oauth_clients` / `oauth_codes` / `oauth_tokens` backing the
//! authorization-code + refresh-token grants. Schema-builder only, so it runs on
//! SQLite (local) and Postgres (server) alike.

use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0004_oauth"
    }
}

#[derive(DeriveIden)]
enum ServiceTokens {
    Table,
    Token,
    OwnerId,
    IsAdmin,
    Label,
    CreatedAt,
    ExpiresAt,
}

#[derive(DeriveIden)]
enum OauthClients {
    Table,
    ClientId,
    ClientName,
    RedirectUris,
    CreatedAt,
}

#[derive(DeriveIden)]
enum OauthCodes {
    Table,
    Code,
    ClientId,
    UserId,
    RedirectUri,
    CodeChallenge,
    CodeChallengeMethod,
    Scope,
    ExpiresAt,
    Used,
}

#[derive(DeriveIden)]
enum OauthTokens {
    Table,
    AccessToken,
    RefreshToken,
    ClientId,
    UserId,
    Scope,
    CreatedAt,
    ExpiresAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ServiceTokens::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ServiceTokens::Token)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ServiceTokens::OwnerId).string().not_null())
                    .col(
                        ColumnDef::new(ServiceTokens::IsAdmin)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(ServiceTokens::Label).string().not_null())
                    .col(
                        ColumnDef::new(ServiceTokens::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(ColumnDef::new(ServiceTokens::ExpiresAt).timestamp().null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(OauthClients::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OauthClients::ClientId)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(OauthClients::ClientName).string().not_null())
                    // JSON-encoded array of registered redirect URIs.
                    .col(ColumnDef::new(OauthClients::RedirectUris).text().not_null())
                    .col(
                        ColumnDef::new(OauthClients::CreatedAt)
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
                    .table(OauthCodes::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OauthCodes::Code)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(OauthCodes::ClientId).string().not_null())
                    .col(ColumnDef::new(OauthCodes::UserId).string().not_null())
                    .col(ColumnDef::new(OauthCodes::RedirectUri).string().not_null())
                    .col(
                        ColumnDef::new(OauthCodes::CodeChallenge)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthCodes::CodeChallengeMethod)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(OauthCodes::Scope).string().not_null())
                    .col(ColumnDef::new(OauthCodes::ExpiresAt).timestamp().not_null())
                    .col(
                        ColumnDef::new(OauthCodes::Used)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(OauthTokens::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OauthTokens::AccessToken)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(OauthTokens::RefreshToken)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(OauthTokens::ClientId).string().not_null())
                    .col(ColumnDef::new(OauthTokens::UserId).string().not_null())
                    .col(ColumnDef::new(OauthTokens::Scope).string().not_null())
                    .col(
                        ColumnDef::new(OauthTokens::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(OauthTokens::ExpiresAt)
                            .timestamp()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(OauthTokens::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(OauthCodes::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(OauthClients::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ServiceTokens::Table).to_owned())
            .await?;
        Ok(())
    }
}
