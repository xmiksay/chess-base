//! Database layer: backend-agnostic connection + migrations + entities.

pub mod config;
pub mod entities;
mod migrator;

pub use config::{Backend, DbConfig};

use anyhow::Result;
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

/// Connect to the configured database and run pending migrations.
pub async fn connect(cfg: &DbConfig) -> Result<DatabaseConnection> {
    let mut opt = ConnectOptions::new(cfg.url());
    // An in-memory SQLite DB lives only as long as its single connection, so
    // the pool must not open more than one.
    if cfg.is_memory() {
        opt.max_connections(1);
    }
    let conn = Database::connect(opt).await?;
    migrator::Migrator::up(&conn, None).await?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use entities::{databases, events, games, players, position_index, settings, studies};
    use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

    #[tokio::test]
    async fn connects_migrates_and_persists() {
        let conn = connect(&DbConfig::in_memory()).await.unwrap();

        settings::ActiveModel {
            key: Set("theme".to_string()),
            value: Set("dark".to_string()),
        }
        .insert(&conn)
        .await
        .unwrap();

        let got = settings::Entity::find_by_id("theme".to_string())
            .one(&conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.value, "dark");
    }

    #[tokio::test]
    async fn global_database_has_null_owner() {
        let conn = connect(&DbConfig::in_memory()).await.unwrap();

        let model = databases::ActiveModel {
            owner_id: Set(None),
            name: Set("Master DB".to_string()),
            kind: Set("master".to_string()),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();

        assert!(model.owner_id.is_none(), "global database has no owner");
        assert_eq!(model.kind, "master");
    }

    #[tokio::test]
    async fn index_depth_persists_on_databases() {
        let conn = connect(&DbConfig::in_memory()).await.unwrap();

        let model = databases::ActiveModel {
            owner_id: Set(None),
            name: Set("Master DB".to_string()),
            kind: Set("master".to_string()),
            index_depth: Set(databases::default_index_depth("master")),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();

        assert_eq!(model.index_depth, Some(databases::MASTER_INDEX_DEPTH));
    }

    /// End-to-end round trip: a game with header rows, a position-index entry and a
    /// study, exercising the new tables, their FKs and the Zobrist i64 cast.
    #[tokio::test]
    async fn game_position_and_study_round_trip() {
        use crate::position::{zobrist_of_fen, STARTPOS_FEN};
        use shakmaty::CastlingMode;

        let conn = connect(&DbConfig::in_memory()).await.unwrap();

        let db = databases::ActiveModel {
            owner_id: Set(Some("alice".to_string())),
            name: Set("Alice's games".to_string()),
            kind: Set("own".to_string()),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();
        // Own databases index every ply.
        assert_eq!(db.index_depth, None);

        let white = players::ActiveModel {
            name: Set("Carlsen, Magnus".to_string()),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();
        let black = players::ActiveModel {
            name: Set("Nakamura, Hikaru".to_string()),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();
        let event = events::ActiveModel {
            name: Set("Norway Chess".to_string()),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();

        let game = games::ActiveModel {
            database_id: Set(db.id),
            white_player_id: Set(Some(white.id)),
            black_player_id: Set(Some(black.id)),
            event_id: Set(Some(event.id)),
            date: Set(Some("2024.05.27".to_string())),
            result: Set(Some("1-0".to_string())),
            eco: Set(Some("C65".to_string())),
            pgn: Set(Some("1. e4 e5 2. Nf3 Nc6 3. Bb5".to_string())),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();
        // `variant` defaults to "standard" at the DB level when left NotSet.
        let game = games::Entity::find_by_id(game.id)
            .one(&conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(game.variant, "standard");
        assert!(game.start_fen.is_none());

        // High-bit Zobrist values must survive the u64↔i64 reinterpret.
        let zobrist = zobrist_of_fen(STARTPOS_FEN, CastlingMode::Standard).unwrap();
        position_index::ActiveModel {
            zobrist: Set(position_index::to_i64(zobrist)),
            game_id: Set(game.id),
            ply: Set(0),
            r#move: Set("e4".to_string()),
            database_id: Set(db.id),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();

        let hit = position_index::Entity::find()
            .filter(position_index::Column::Zobrist.eq(position_index::to_i64(zobrist)))
            .one(&conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(position_index::from_i64(hit.zobrist), zobrist);
        assert_eq!(hit.r#move, "e4");
        assert_eq!(hit.game_id, game.id);

        let study = studies::ActiveModel {
            database_id: Set(db.id),
            owner_id: Set(Some("alice".to_string())),
            name: Set("Ruy Lopez ideas".to_string()),
            tree_json: Set(r#"{"root":null}"#.to_string()),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();
        // `created_at` is filled by the DB default (current_timestamp).
        let study = studies::Entity::find_by_id(study.id)
            .one(&conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(study.name, "Ruy Lopez ideas");
        assert_eq!(study.tree_json, r#"{"root":null}"#);
    }
}
