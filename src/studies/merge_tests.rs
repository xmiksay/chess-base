//! Service-level tests for the games → repertoire merge (issue #170) over an
//! in-memory SQLite DB: frequency ordering, branch-point stats, grafting into an
//! existing study, visibility gating and the empty/no-name guards.

use super::*;
use crate::db::entities::{databases, games};
use crate::db::{connect, DbConfig};
use crate::ingest::ingest_pgn;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};

fn user(id: &str) -> CurrentUser {
    CurrentUser {
        id: id.to_string(),
        is_admin: false,
    }
}

/// Ingest one full-header game into `database_id` (resolving players the
/// production way) and return its id.
async fn seed_game(
    db: &DatabaseConnection,
    database_id: i32,
    white: &str,
    black: &str,
    date: &str,
    result: &str,
    moves: &str,
) -> i32 {
    let pgn = format!(
        "[White \"{white}\"]\n[Black \"{black}\"]\n[Date \"{date}\"]\n[Result \"{result}\"]\n\n{moves}\n"
    );
    ingest_pgn(db, database_id, &pgn)
        .await
        .unwrap()
        .unwrap()
        .game_id
}

/// Fresh DB with one owned games database; returns the service and that db's id.
async fn setup() -> (StudyService, i32) {
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
    (StudyService::new(conn), db.id)
}

#[tokio::test]
async fn merges_games_into_a_new_study_ordered_by_frequency() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");

    // Two games play 1.e4, one plays 1.d4 → e4 is the most-common first move.
    let g1 = seed_game(
        &svc.db,
        db_id,
        "Carlsen, M",
        "Nepo, I",
        "2023.01.01",
        "1-0",
        "1. e4 e5 2. Nf3 *",
    )
    .await;
    let g2 = seed_game(
        &svc.db,
        db_id,
        "Carlsen, M",
        "So, W",
        "2022.06.01",
        "1-0",
        "1. e4 c5 2. Nf3 *",
    )
    .await;
    let g3 = seed_game(
        &svc.db,
        db_id,
        "Carlsen, M",
        "Ding, L",
        "2021.03.01",
        "1/2-1/2",
        "1. d4 d5 *",
    )
    .await;

    let study = svc
        .merge_games(
            &alice,
            &[g1, g2, g3],
            None,
            Some("Carlsen repertoire".into()),
            None,
        )
        .await
        .unwrap();
    assert_eq!(study.database_id, db_id);
    assert_eq!(study.owner_id.as_deref(), Some("alice"));
    assert_eq!(study.origin_game_id, None);

    let tree: MoveTree = serde_json::from_str(&study.tree_json).unwrap();
    // e4 (2 games) beats d4 (1) for the mainline; both survive as siblings.
    assert_eq!(tree.mainline(), vec!["e4", "e5", "Nf3"]);
    let first_moves: Vec<_> = tree.nodes[tree.root]
        .children
        .iter()
        .map(|&c| tree.nodes[c].san.clone().unwrap())
        .collect();
    assert_eq!(first_moves, vec!["e4", "d4"]);

    // The e4/d4 branch point carries a stats comment (White's perspective).
    let e4 = tree.nodes[tree.root].children[0];
    assert_eq!(
        tree.nodes[e4].comment.as_deref(),
        Some("2 games, 100% (Carlsen–Nepo 2023, Carlsen–So 2022)")
    );
    // e5/c5 diverge under e4 → Black-perspective stats (White won both e4 games).
    let e4_replies = &tree.nodes[e4].children;
    let reply_comments: Vec<_> = e4_replies
        .iter()
        .filter_map(|&c| tree.nodes[c].comment.clone())
        .collect();
    assert!(reply_comments
        .iter()
        .any(|c| c == "1 game, 0% (Carlsen–Nepo 2023)"));
}

#[tokio::test]
async fn grafts_into_an_existing_study_and_is_idempotent() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let g1 = seed_game(&svc.db, db_id, "A", "B", "2023.01.01", "1-0", "1. e4 e5 *").await;

    let base = svc
        .create(&alice, db_id, "Repertoire", false)
        .await
        .unwrap();
    let merged = svc
        .merge_games(&alice, &[g1], Some(base.id), None, None)
        .await
        .unwrap();
    assert_eq!(merged.id, base.id);
    let before: MoveTree = serde_json::from_str(&merged.tree_json).unwrap();
    assert_eq!(before.mainline(), vec!["e4", "e5"]);

    // Re-merging the same game changes nothing (SAN-follow dedup).
    let again = svc
        .merge_games(&alice, &[g1], Some(base.id), None, None)
        .await
        .unwrap();
    assert_eq!(again.tree_json, merged.tree_json);
}

#[tokio::test]
async fn hidden_games_and_bad_requests_are_rejected() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");
    let bob = user("bob");
    let g1 = seed_game(&svc.db, db_id, "A", "B", "2023.01.01", "1-0", "1. e4 e5 *").await;

    // Bob can't see Alice's game → NotFound, never a leak.
    assert!(matches!(
        svc.merge_games(&bob, &[g1], None, Some("X".into()), None)
            .await
            .unwrap_err(),
        StudyError::NotFound
    ));
    // No games at all.
    assert!(matches!(
        svc.merge_games(&alice, &[], None, Some("X".into()), None)
            .await
            .unwrap_err(),
        StudyError::InvalidEdit(_)
    ));
    // A new study needs a name.
    assert!(matches!(
        svc.merge_games(&alice, &[g1], None, None, None)
            .await
            .unwrap_err(),
        StudyError::InvalidEdit(_)
    ));
}

#[tokio::test]
async fn skips_set_up_and_empty_games() {
    let (svc, db_id) = setup().await;
    let alice = user("alice");

    // A set-up-FEN game can't merge from the standard root.
    let setup_game = games::ActiveModel {
        database_id: Set(db_id),
        variant: Set("standard".to_string()),
        start_fen: Set(Some(
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR b KQkq - 0 1".to_string(),
        )),
        result: Set(Some("1-0".to_string())),
        pgn: Set(Some("1... e5 *".to_string())),
        ..Default::default()
    }
    .insert(&svc.db)
    .await
    .unwrap()
    .id;

    // With no mergeable game left, the merge is a clean 400, not a corrupt study.
    assert!(matches!(
        svc.merge_games(&alice, &[setup_game], None, Some("X".into()), None)
            .await
            .unwrap_err(),
        StudyError::InvalidEdit(_)
    ));
}
