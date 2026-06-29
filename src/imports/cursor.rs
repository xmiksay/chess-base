//! Persistence of the per-`(database, source)` [`SyncCursor`] (issue #95) in the
//! `sync_cursors` table, so an incremental re-sync resumes where the last one
//! stopped instead of starting over (and re-importing everything).

use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, Set,
};

use crate::collectors::SyncCursor;
use crate::db::entities::sync_cursors;

/// Load the saved cursor for `(database_id, source)`, or the default (full sync)
/// when none has been persisted yet.
pub(super) async fn load(
    db: &DatabaseConnection,
    database_id: i32,
    source: &str,
) -> Result<SyncCursor, DbErr> {
    let row = find(db, database_id, source).await?;
    Ok(row
        .map(|m| SyncCursor {
            last_month: m.last_month,
            last_game_ms: m.last_game_ms,
        })
        .unwrap_or_default())
}

/// Persist `cursor` for `(database_id, source)`, upserting the single row keyed by
/// that pair (the unique index in migration m0005).
pub(super) async fn save(
    db: &DatabaseConnection,
    database_id: i32,
    source: &str,
    cursor: &SyncCursor,
) -> Result<(), DbErr> {
    match find(db, database_id, source).await? {
        Some(existing) => {
            let mut active: sync_cursors::ActiveModel = existing.into();
            active.last_month = Set(cursor.last_month.clone());
            active.last_game_ms = Set(cursor.last_game_ms);
            active.update(db).await?;
        }
        None => {
            sync_cursors::ActiveModel {
                database_id: Set(database_id),
                source: Set(source.to_string()),
                last_month: Set(cursor.last_month.clone()),
                last_game_ms: Set(cursor.last_game_ms),
                ..Default::default()
            }
            .insert(db)
            .await?;
        }
    }
    Ok(())
}

async fn find(
    db: &DatabaseConnection,
    database_id: i32,
    source: &str,
) -> Result<Option<sync_cursors::Model>, DbErr> {
    sync_cursors::Entity::find()
        .filter(sync_cursors::Column::DatabaseId.eq(database_id))
        .filter(sync_cursors::Column::Source.eq(source))
        .one(db)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::entities::databases;
    use crate::db::{connect, DbConfig};

    async fn db_with_collection() -> (DatabaseConnection, i32) {
        let conn = connect(&DbConfig::in_memory()).await.unwrap();
        let db = databases::ActiveModel {
            owner_id: Set(Some("alice".to_string())),
            name: Set("Games".to_string()),
            kind: Set("lichess".to_string()),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();
        (conn, db.id)
    }

    #[tokio::test]
    async fn missing_cursor_loads_the_default() {
        let (conn, id) = db_with_collection().await;
        assert_eq!(
            load(&conn, id, "lichess").await.unwrap(),
            SyncCursor::default()
        );
    }

    #[tokio::test]
    async fn save_then_load_round_trips() {
        let (conn, id) = db_with_collection().await;
        let cursor = SyncCursor {
            last_game_ms: Some(1_705_350_705_000),
            ..Default::default()
        };
        save(&conn, id, "lichess", &cursor).await.unwrap();
        assert_eq!(load(&conn, id, "lichess").await.unwrap(), cursor);
    }

    #[tokio::test]
    async fn save_upserts_the_single_row_per_pair() {
        let (conn, id) = db_with_collection().await;
        let first = SyncCursor {
            last_month: Some("2024/01".to_string()),
            ..Default::default()
        };
        let second = SyncCursor {
            last_month: Some("2024/02".to_string()),
            ..Default::default()
        };
        save(&conn, id, "chesscom", &first).await.unwrap();
        save(&conn, id, "chesscom", &second).await.unwrap();

        assert_eq!(load(&conn, id, "chesscom").await.unwrap(), second);
        // Upsert, not insert: exactly one row for the pair.
        let rows = sync_cursors::Entity::find().all(&conn).await.unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn cursors_are_isolated_per_source() {
        let (conn, id) = db_with_collection().await;
        let lichess = SyncCursor {
            last_game_ms: Some(42),
            ..Default::default()
        };
        let chesscom = SyncCursor {
            last_month: Some("2024/03".to_string()),
            ..Default::default()
        };
        save(&conn, id, "lichess", &lichess).await.unwrap();
        save(&conn, id, "chesscom", &chesscom).await.unwrap();

        assert_eq!(load(&conn, id, "lichess").await.unwrap(), lichess);
        assert_eq!(load(&conn, id, "chesscom").await.unwrap(), chesscom);
    }
}
