//! Tests for [`super`] (PGN ingest pipeline). Split out to keep the
//! module under the project's 500-line file cap.

use super::*;
use crate::db::{connect, DbConfig};

const SCHOLARS_MATE: &str = "[Event \"Casual Game\"]\n[Site \"London\"]\n[White \"Spassky, Boris\"]\n[Black \"Fischer, Robert\"]\n[Result \"1-0\"]\n[WhiteElo \"2660\"]\n[BlackElo \"2785\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n";

async fn db_with_own_collection() -> (DatabaseConnection, i32) {
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
    (conn, db.id)
}

#[tokio::test]
async fn ingests_game_with_one_index_row_per_mainline_ply() {
    let (conn, database_id) = db_with_own_collection().await;

    let result = ingest_pgn(&conn, database_id, SCHOLARS_MATE)
        .await
        .unwrap()
        .unwrap();
    // 7 half-moves: e4 e5 Bc4 Nc6 Qh5 Nf6 Qxf7#.
    assert_eq!(result.indexed_plies, 7);

    let rows = position_index::Entity::find()
        .filter(position_index::Column::GameId.eq(result.game_id))
        .all(&conn)
        .await
        .unwrap();
    assert_eq!(rows.len(), 7);

    let game = games::Entity::find_by_id(result.game_id)
        .one(&conn)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(game.ply_count, Some(7));
    assert_eq!(game.result.as_deref(), Some("1-0"));
    assert_eq!(game.white_elo, Some(2660));
    assert_eq!(game.variant, "standard");
    assert!(game.start_fen.is_none());
}

#[tokio::test]
async fn indexed_positions_are_searchable_by_zobrist() {
    let (conn, database_id) = db_with_own_collection().await;
    let result = ingest_pgn(&conn, database_id, SCHOLARS_MATE)
        .await
        .unwrap()
        .unwrap();

    // Spot-check a mid-game position: after 1. e4 e5 (the position from which
    // White plays 2. Bc4, i.e. ply 2 in the index).
    let plies = position::replay(STARTPOS_FEN, &["e4", "e5"], CastlingMode::Standard).unwrap();
    let mid_zobrist = plies.last().unwrap().zobrist;

    let hit = position_index::Entity::find()
        .filter(position_index::Column::Zobrist.eq(position_index::to_i64(mid_zobrist)))
        .filter(position_index::Column::GameId.eq(result.game_id))
        .one(&conn)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(hit.ply, 2);
    assert_eq!(hit.r#move, "Bc4");
}

#[tokio::test]
async fn start_position_is_indexed_at_ply_zero() {
    let (conn, database_id) = db_with_own_collection().await;
    let result = ingest_pgn(&conn, database_id, SCHOLARS_MATE)
        .await
        .unwrap()
        .unwrap();

    let start = zobrist_of_fen(STARTPOS_FEN, CastlingMode::Standard).unwrap();
    let hit = position_index::Entity::find()
        .filter(position_index::Column::GameId.eq(result.game_id))
        .filter(position_index::Column::Ply.eq(0))
        .one(&conn)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(position_index::from_i64(hit.zobrist), start);
    assert_eq!(hit.r#move, "e4");
}

#[tokio::test]
async fn index_depth_caps_indexed_positions() {
    let conn = connect(&DbConfig::in_memory()).await.unwrap();
    let db = databases::ActiveModel {
        owner_id: Set(None),
        name: Set("Master DB".to_string()),
        kind: Set("master".to_string()),
        index_depth: Set(Some(3)),
        ..Default::default()
    }
    .insert(&conn)
    .await
    .unwrap();

    let result = ingest_pgn(&conn, db.id, SCHOLARS_MATE)
        .await
        .unwrap()
        .unwrap();
    // Mainline is 7 plies but indexing is capped at 3; the game still records
    // the full ply count.
    assert_eq!(result.indexed_plies, 3);
    let count = position_index::Entity::find()
        .filter(position_index::Column::GameId.eq(result.game_id))
        .all(&conn)
        .await
        .unwrap()
        .len();
    assert_eq!(count, 3);
    let game = games::Entity::find_by_id(result.game_id)
        .one(&conn)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(game.ply_count, Some(7));
}

#[tokio::test]
async fn players_and_event_are_deduplicated_across_games() {
    let (conn, database_id) = db_with_own_collection().await;

    ingest_pgn(&conn, database_id, SCHOLARS_MATE).await.unwrap();
    ingest_pgn(&conn, database_id, SCHOLARS_MATE).await.unwrap();

    assert_eq!(players::Entity::find().all(&conn).await.unwrap().len(), 2);
    assert_eq!(events::Entity::find().all(&conn).await.unwrap().len(), 1);

    let games = games::Entity::find().all(&conn).await.unwrap();
    assert_eq!(games.len(), 2);
    assert_eq!(games[0].white_player_id, games[1].white_player_id);
    assert_eq!(games[0].event_id, games[1].event_id);
}

// Same scholars-mate movetext, but tagged with a Lichess permalink so it carries
// a stable `source_ref` (unlike the `[Site "London"]` of `SCHOLARS_MATE`).
const KEYED_GAME: &str = "[Event \"Rated blitz game\"]\n[Site \"https://lichess.org/abcd1234\"]\n[White \"alice\"]\n[Black \"bob\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n";

#[tokio::test]
async fn re_ingesting_a_keyed_game_is_deduped() {
    let (conn, database_id) = db_with_own_collection().await;

    // First ingest stores the game and records its permalink as `source_ref`.
    let first = ingest_pgn(&conn, database_id, KEYED_GAME).await.unwrap();
    assert!(first.is_some());
    // Re-syncing the same game returns `None` and writes nothing — the dedup that
    // makes revisiting the cursor boundary safe (issue #95).
    let second = ingest_pgn(&conn, database_id, KEYED_GAME).await.unwrap();
    assert!(second.is_none());

    assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 1);
}

#[tokio::test]
async fn same_keyed_game_dedups_per_database_not_across() {
    let conn = connect(&DbConfig::in_memory()).await.unwrap();
    let db_a = databases::ActiveModel {
        owner_id: Set(Some("alice".to_string())),
        name: Set("A".to_string()),
        kind: Set("lichess".to_string()),
        ..Default::default()
    }
    .insert(&conn)
    .await
    .unwrap();
    let db_b = databases::ActiveModel {
        owner_id: Set(Some("alice".to_string())),
        name: Set("B".to_string()),
        kind: Set("lichess".to_string()),
        ..Default::default()
    }
    .insert(&conn)
    .await
    .unwrap();

    // The same permalink lands once in each database (dedup is per-database).
    assert!(ingest_pgn(&conn, db_a.id, KEYED_GAME)
        .await
        .unwrap()
        .is_some());
    assert!(ingest_pgn(&conn, db_b.id, KEYED_GAME)
        .await
        .unwrap()
        .is_some());
    assert!(ingest_pgn(&conn, db_a.id, KEYED_GAME)
        .await
        .unwrap()
        .is_none());

    assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 2);
}

#[tokio::test]
async fn keyless_games_are_never_deduped() {
    let (conn, database_id) = db_with_own_collection().await;
    // `SCHOLARS_MATE` has `[Site "London"]`, not a permalink ⇒ no `source_ref`, so
    // re-ingesting it stores a second copy (the existing player/event-dedup case).
    assert!(ingest_pgn(&conn, database_id, SCHOLARS_MATE)
        .await
        .unwrap()
        .is_some());
    assert!(ingest_pgn(&conn, database_id, SCHOLARS_MATE)
        .await
        .unwrap()
        .is_some());
    assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 2);
}

#[tokio::test]
async fn missing_player_tags_yield_null_ids() {
    let (conn, database_id) = db_with_own_collection().await;
    let result = ingest_pgn(&conn, database_id, "1. d4 d5 *")
        .await
        .unwrap()
        .unwrap();

    let game = games::Entity::find_by_id(result.game_id)
        .one(&conn)
        .await
        .unwrap()
        .unwrap();
    assert!(game.white_player_id.is_none());
    assert!(game.black_player_id.is_none());
    assert!(game.event_id.is_none());
    assert_eq!(result.indexed_plies, 2);
}

#[tokio::test]
async fn illegal_move_aborts_without_writing_rows() {
    let (conn, database_id) = db_with_own_collection().await;
    // Black cannot answer 1. e4 with a second e4.
    let err = ingest_pgn(&conn, database_id, "1. e4 e4 *").await;
    assert!(err.is_err());

    // The transaction rolled back: no game and no index rows.
    assert!(games::Entity::find().all(&conn).await.unwrap().is_empty());
    assert!(position_index::Entity::find()
        .all(&conn)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn empty_pgn_is_rejected() {
    let (conn, database_id) = db_with_own_collection().await;
    assert!(ingest_pgn(&conn, database_id, "   \n  ").await.is_err());
}

// Two complete games in one blob, as a `.pgn` file / export stream carries them.
const TWO_GAMES: &str = "[Event \"Game 1\"]\n[White \"Spassky\"]\n[Black \"Fischer\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n\n[Event \"Game 2\"]\n[White \"Carlsen\"]\n[Black \"Caruana\"]\n[Result \"1/2-1/2\"]\n\n1. d4 d5 2. c4 e6 1/2-1/2\n";

#[tokio::test]
async fn ingest_all_stores_every_game_in_a_multi_game_pgn() {
    let (conn, database_id) = db_with_own_collection().await;

    let report = ingest_pgn_all(&conn, database_id, TWO_GAMES).await.unwrap();
    assert_eq!(report.imported.len(), 2);
    assert!(report.errors.is_empty());
    assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 2);
}

#[tokio::test]
async fn ingest_all_accepts_a_headerless_single_game() {
    let (conn, database_id) = db_with_own_collection().await;

    // No `[Event]` tag ⇒ split finds nothing; the whole blob is one game.
    let report = ingest_pgn_all(&conn, database_id, "1. d4 d5 *")
        .await
        .unwrap();
    assert_eq!(report.imported.len(), 1);
    assert_eq!(report.imported[0].indexed_plies, 2);
    assert!(report.errors.is_empty());
}

#[tokio::test]
async fn ingest_all_skips_a_blank_blob_without_aborting() {
    let (conn, database_id) = db_with_own_collection().await;
    // A blank blob is one (empty) game: skipped, not a hard error.
    let report = ingest_pgn_all(&conn, database_id, "   \n  ").await.unwrap();
    assert!(report.imported.is_empty());
    assert_eq!(report.errors.len(), 1);
    assert!(games::Entity::find().all(&conn).await.unwrap().is_empty());
}

// good · illegal (Black answers 1. e4 with another e4) · good.
const GOOD_BAD_GOOD: &str = "[Event \"G1\"]\n[White \"A\"]\n[Black \"B\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n\n[Event \"G2\"]\n[White \"C\"]\n[Black \"D\"]\n[Result \"*\"]\n\n1. e4 e4 *\n\n[Event \"G3\"]\n[White \"E\"]\n[Black \"F\"]\n[Result \"1/2-1/2\"]\n\n1. d4 d5 *\n";

#[tokio::test]
async fn ingest_all_skips_a_bad_game_and_keeps_the_rest() {
    let (conn, database_id) = db_with_own_collection().await;

    let report = ingest_pgn_all(&conn, database_id, GOOD_BAD_GOOD)
        .await
        .unwrap();
    // The two legal games are committed; the illegal one is recorded, not fatal.
    assert_eq!(report.imported.len(), 2);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].index, 2);
    assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 2);
}

#[test]
fn split_games_separates_each_event_block() {
    let games = split_games(TWO_GAMES);
    assert_eq!(games.len(), 2);
    assert!(games[0].contains("Qxf7#"));
    assert!(games[1].starts_with("[Event "));
    assert!(games[1].contains("Caruana"));
    assert!(split_games("   \n").is_empty());
}

// A real Chess960 start array (king e1 between rooks on b1/g1, bishops on
// opposite colors). The `KQkq` rights reference those rook files, so this FEN
// only parses under Chess960 castling mode (ADR-0010).
const CHESS960_FEN: &str = "nrbqkbrn/pppppppp/8/8/8/8/PPPPPPPP/NRBQKBRN w KQkq - 0 1";

#[tokio::test]
async fn ingests_chess960_game_honoring_variant_and_setup() {
    let (conn, database_id) = db_with_own_collection().await;
    let pgn = format!(
        "[Event \"Casual Chess960\"]\n[White \"Carlsen, Magnus\"]\n[Black \"Nakamura, Hikaru\"]\n[Result \"*\"]\n[Variant \"Chess960\"]\n[SetUp \"1\"]\n[FEN \"{CHESS960_FEN}\"]\n\n1. d4 d5 *\n"
    );

    let result = ingest_pgn(&conn, database_id, &pgn).await.unwrap().unwrap();
    assert_eq!(result.indexed_plies, 2);

    let game = games::Entity::find_by_id(result.game_id)
        .one(&conn)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(game.variant, "chess960");
    assert_eq!(game.start_fen.as_deref(), Some(CHESS960_FEN));

    // Ply 0 indexes the start array under Chess960 mode (variant-agnostic key).
    let start = zobrist_of_fen(CHESS960_FEN, CastlingMode::Chess960).unwrap();
    let hit = position_index::Entity::find()
        .filter(position_index::Column::GameId.eq(result.game_id))
        .filter(position_index::Column::Ply.eq(0))
        .one(&conn)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(position_index::from_i64(hit.zobrist), start);
    assert_eq!(hit.r#move, "d4");
}

#[tokio::test]
async fn setup_position_game_replays_from_fen() {
    let (conn, database_id) = db_with_own_collection().await;
    // A standard set-up (study) position, not the startpos.
    let setup_fen = "4k3/8/8/8/8/8/4P3/4K3 w - - 0 1";
    let pgn = format!("[Event \"Study\"]\n[SetUp \"1\"]\n[FEN \"{setup_fen}\"]\n\n1. e4 *\n");

    let result = ingest_pgn(&conn, database_id, &pgn).await.unwrap().unwrap();
    assert_eq!(result.indexed_plies, 1);

    let game = games::Entity::find_by_id(result.game_id)
        .one(&conn)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(game.variant, "standard");
    assert_eq!(game.start_fen.as_deref(), Some(setup_fen));

    let start = zobrist_of_fen(setup_fen, CastlingMode::Standard).unwrap();
    let hit = position_index::Entity::find()
        .filter(position_index::Column::GameId.eq(result.game_id))
        .filter(position_index::Column::Ply.eq(0))
        .one(&conn)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(position_index::from_i64(hit.zobrist), start);
    assert_eq!(hit.r#move, "e4");
}

#[tokio::test]
async fn chess960_position_without_variant_tag_is_rejected() {
    let (conn, database_id) = db_with_own_collection().await;
    // Same 960 array but no `[Variant]`: it defaults to standard, where the
    // `KQkq` rights have no a1/h1 rook layout. shakmaty rejects it rather than
    // letting it be silently mis-parsed as standard (ADR-0010 guard).
    let pgn = format!("[SetUp \"1\"]\n[FEN \"{CHESS960_FEN}\"]\n\n1. d4 d5 *\n");
    assert!(ingest_pgn(&conn, database_id, &pgn).await.is_err());

    // Nothing was written: the transaction never opened.
    assert!(games::Entity::find().all(&conn).await.unwrap().is_empty());
}
