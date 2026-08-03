//! m0010 — OAuth hardening (issue #193, ADR-0044): consent screen + CSRF,
//! refresh-token reuse detection, scoped service tokens.
//!
//! - `service_tokens`: `scope` (`"full"` | `"read_only"` | `"global_read"`,
//!   default `"full"` — matches pre-existing full-access behavior) and `id` (a
//!   non-secret reference id for admin list/revoke — the `token` column stays
//!   the bearer secret/PK and is never re-displayed). Backfilled from `token`
//!   so every existing row gets a stable, already-unique `id`.
//! - `oauth_tokens`: `family_id` (groups every row descended from one code
//!   exchange, across rotations) and `revoked` (rotation now revokes-and-keeps
//!   the old row instead of deleting it, so a replayed refresh token can be
//!   detected). Backfilled `family_id = access_token` so existing rows each
//!   start their own singleton family.
//! - `oauth_consents`: remembers a user's prior approval of a client so
//!   re-authorizing skips the consent screen.
//! - `oauth_consent_requests`: a single-use, 10-minute pending-consent record;
//!   its `csrf_token` PK doubles as the anti-CSRF token bound to this request.
//!
//! Schema-builder only (one ADD COLUMN per `alter_table` call, SQLite-safe),
//! plus raw-SQL backfills, so the same migration runs on SQLite and Postgres.

use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m0010_oauth_hardening"
    }
}

#[derive(DeriveIden)]
enum ServiceTokens {
    Table,
    Id,
    Scope,
}

#[derive(DeriveIden)]
enum OauthTokens {
    Table,
    FamilyId,
    Revoked,
}

#[derive(DeriveIden)]
enum OauthConsents {
    Table,
    UserId,
    ClientId,
    CreatedAt,
}

#[derive(DeriveIden)]
enum OauthConsentRequests {
    Table,
    CsrfToken,
    UserId,
    ClientId,
    RedirectUri,
    CodeChallenge,
    CodeChallengeMethod,
    Scope,
    State,
    ExpiresAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ServiceTokens::Table)
                    .add_column(
                        ColumnDef::new(ServiceTokens::Scope)
                            .string()
                            .not_null()
                            .default("full"),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(ServiceTokens::Table)
                    .add_column(
                        ColumnDef::new(ServiceTokens::Id)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared("UPDATE service_tokens SET id = token WHERE id = ''")
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_service_tokens_id")
                    .table(ServiceTokens::Table)
                    .col(ServiceTokens::Id)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(OauthTokens::Table)
                    .add_column(
                        ColumnDef::new(OauthTokens::FamilyId)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(OauthTokens::Table)
                    .add_column(
                        ColumnDef::new(OauthTokens::Revoked)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "UPDATE oauth_tokens SET family_id = access_token WHERE family_id = ''",
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_oauth_tokens_family")
                    .table(OauthTokens::Table)
                    .col(OauthTokens::FamilyId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(OauthConsents::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(OauthConsents::UserId).string().not_null())
                    .col(ColumnDef::new(OauthConsents::ClientId).string().not_null())
                    .col(
                        ColumnDef::new(OauthConsents::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .primary_key(
                        Index::create()
                            .col(OauthConsents::UserId)
                            .col(OauthConsents::ClientId),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(OauthConsentRequests::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OauthConsentRequests::CsrfToken)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(OauthConsentRequests::UserId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthConsentRequests::ClientId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthConsentRequests::RedirectUri)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthConsentRequests::CodeChallenge)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthConsentRequests::CodeChallengeMethod)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthConsentRequests::Scope)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(OauthConsentRequests::State).string().null())
                    .col(
                        ColumnDef::new(OauthConsentRequests::ExpiresAt)
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
            .drop_table(Table::drop().table(OauthConsentRequests::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(OauthConsents::Table).to_owned())
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_oauth_tokens_family")
                    .table(OauthTokens::Table)
                    .to_owned(),
            )
            .await?;
        for col in [OauthTokens::Revoked, OauthTokens::FamilyId] {
            manager
                .alter_table(
                    Table::alter()
                        .table(OauthTokens::Table)
                        .drop_column(col)
                        .to_owned(),
                )
                .await?;
        }

        manager
            .drop_index(
                Index::drop()
                    .name("idx_service_tokens_id")
                    .table(ServiceTokens::Table)
                    .to_owned(),
            )
            .await?;
        for col in [ServiceTokens::Id, ServiceTokens::Scope] {
            manager
                .alter_table(
                    Table::alter()
                        .table(ServiceTokens::Table)
                        .drop_column(col)
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::db::migrator::Migrator;
    use crate::db::{connect, DbConfig};
    use sea_orm_migration::MigratorTrait;

    /// m0010 must reverse and re-apply cleanly on SQLite — the ALTER-per-
    /// statement / index-before-column pitfalls live here.
    #[tokio::test]
    async fn m0010_reverses_and_reapplies() {
        let conn = connect(&DbConfig::in_memory()).await.unwrap();
        Migrator::down(&conn, Some(1)).await.unwrap();
        Migrator::up(&conn, None).await.unwrap();
    }
}
