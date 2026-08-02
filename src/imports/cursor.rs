//! Persistence of the per-`(database, source, username)` [`SyncCursor`] (issue
//! #95, username-keyed since #197) in the `sync_cursors` table, so an
//! incremental re-sync resumes where the last one stopped instead of starting
//! over — and syncing a different provider username into the same collection
//! gets its own cursor instead of inheriting (and silently corrupting) the
//! wrong one. `save` also records `last_synced_at`/`last_imported`/
//! `last_duplicates` so the UI can show real feedback, and is called on both
//! the success and failure path of a sync (`ImportService::sync`) so a mid-run
//! failure never discards the progress already made.

use chrono::{NaiveDateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, Set,
};

use crate::collectors::SyncCursor;
use crate::db::entities::sync_cursors;

/// Load the saved cursor for `(database_id, source, username)`, or the default
/// (full sync) when none has been persisted yet — including when a cursor
/// exists for the same `(database_id, source)` under a *different* username.
pub(super) async fn load(
    db: &DatabaseConnection,
    database_id: i32,
    source: &str,
    username: &str,
) -> Result<SyncCursor, DbErr> {
    let row = find(db, database_id, source, username).await?;
    Ok(row
        .map(|m| SyncCursor {
            last_month: m.last_month,
            last_game_ms: m.last_game_ms,
        })
        .unwrap_or_default())
}

/// Persist `cursor` and the run's counts for `(database_id, source, username)`,
/// upserting the single row keyed by that triple (the unique index in
/// migration m0009). Returns the `last_synced_at` timestamp it wrote.
pub(super) async fn save(
    db: &DatabaseConnection,
    database_id: i32,
    source: &str,
    username: &str,
    cursor: &SyncCursor,
    imported: usize,
    duplicates: usize,
) -> Result<NaiveDateTime, DbErr> {
    let now = Utc::now().naive_utc();
    match find(db, database_id, source, username).await? {
        Some(existing) => {
            let mut active: sync_cursors::ActiveModel = existing.into();
            active.last_month = Set(cursor.last_month.clone());
            active.last_game_ms = Set(cursor.last_game_ms);
            active.last_synced_at = Set(Some(now));
            active.last_imported = Set(Some(imported as i32));
            active.last_duplicates = Set(Some(duplicates as i32));
            active.update(db).await?;
        }
        None => {
            sync_cursors::ActiveModel {
                database_id: Set(database_id),
                source: Set(source.to_string()),
                username: Set(Some(username.to_string())),
                last_month: Set(cursor.last_month.clone()),
                last_game_ms: Set(cursor.last_game_ms),
                last_synced_at: Set(Some(now)),
                last_imported: Set(Some(imported as i32)),
                last_duplicates: Set(Some(duplicates as i32)),
                ..Default::default()
            }
            .insert(db)
            .await?;
        }
    }
    Ok(now)
}

async fn find(
    db: &DatabaseConnection,
    database_id: i32,
    source: &str,
    username: &str,
) -> Result<Option<sync_cursors::Model>, DbErr> {
    sync_cursors::Entity::find()
        .filter(sync_cursors::Column::DatabaseId.eq(database_id))
        .filter(sync_cursors::Column::Source.eq(source))
        .filter(sync_cursors::Column::Username.eq(username))
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
            load(&conn, id, "lichess", "alice").await.unwrap(),
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
        save(&conn, id, "lichess", "alice", &cursor, 3, 1)
            .await
            .unwrap();
        assert_eq!(load(&conn, id, "lichess", "alice").await.unwrap(), cursor);
    }

    #[tokio::test]
    async fn save_upserts_the_single_row_per_triple() {
        let (conn, id) = db_with_collection().await;
        let first = SyncCursor {
            last_month: Some("2024/01".to_string()),
            ..Default::default()
        };
        let second = SyncCursor {
            last_month: Some("2024/02".to_string()),
            ..Default::default()
        };
        save(&conn, id, "chesscom", "hikaru", &first, 5, 0)
            .await
            .unwrap();
        save(&conn, id, "chesscom", "hikaru", &second, 2, 3)
            .await
            .unwrap();

        assert_eq!(load(&conn, id, "chesscom", "hikaru").await.unwrap(), second);
        // Upsert, not insert: exactly one row for the triple.
        let rows = sync_cursors::Entity::find().all(&conn).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_imported, Some(2));
        assert_eq!(rows[0].last_duplicates, Some(3));
        assert!(rows[0].last_synced_at.is_some());
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
        save(&conn, id, "lichess", "alice", &lichess, 1, 0)
            .await
            .unwrap();
        save(&conn, id, "chesscom", "alice", &chesscom, 1, 0)
            .await
            .unwrap();

        assert_eq!(load(&conn, id, "lichess", "alice").await.unwrap(), lichess);
        assert_eq!(
            load(&conn, id, "chesscom", "alice").await.unwrap(),
            chesscom
        );
    }

    // A different provider username synced into the same database must never
    // reuse another username's cursor (issue #197) — a wrong-cursor resume
    // silently skips that user's earlier games.
    #[tokio::test]
    async fn cursors_are_isolated_per_username() {
        let (conn, id) = db_with_collection().await;
        let alice = SyncCursor {
            last_game_ms: Some(1_000),
            ..Default::default()
        };
        save(&conn, id, "lichess", "alice", &alice, 10, 0)
            .await
            .unwrap();

        // A second username synced into the same collection starts fresh —
        // it does not inherit alice's cursor.
        assert_eq!(
            load(&conn, id, "lichess", "bob").await.unwrap(),
            SyncCursor::default()
        );
        // alice's cursor is untouched by bob's (still-unsaved) sync.
        assert_eq!(load(&conn, id, "lichess", "alice").await.unwrap(), alice);

        let bob = SyncCursor {
            last_game_ms: Some(2_000),
            ..Default::default()
        };
        save(&conn, id, "lichess", "bob", &bob, 4, 0).await.unwrap();

        // Both rows persist independently.
        assert_eq!(load(&conn, id, "lichess", "alice").await.unwrap(), alice);
        assert_eq!(load(&conn, id, "lichess", "bob").await.unwrap(), bob);
        assert_eq!(
            sync_cursors::Entity::find().all(&conn).await.unwrap().len(),
            2
        );
    }
}
