//! The shared game-ingest path: parse a PGN, store the game with its deduplicated
//! header roster, replay the mainline via [`position::replay`], and bulk-insert
//! the [`position_index`](crate::db::entities::position_index) rows that back
//! position search (ADR-0003).
//!
//! Every collector (Lichess / Chess.com / bulk import) funnels through
//! [`ingest_pgn`] so games are stored and indexed identically regardless of
//! source. Parsing only reads syntax; legality is enforced by `position::replay`,
//! so an illegal move aborts the ingest before any row is written.

use std::borrow::Cow;
use std::io::Cursor;
use std::ops::ControlFlow;

use anyhow::{anyhow, Context, Result};
use pgn_reader::{RawTag, Reader, SanPlus, Skip, Visitor};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    Set, TransactionTrait,
};

use crate::db::entities::{databases, events, games, players, position_index};
use crate::position::{self, zobrist_of_fen, CastlingMode, STARTPOS_FEN};

/// Outcome of a successful ingest: the new game's id and how many positions were
/// written to `position_index` (mainline length, capped by the database's
/// `index_depth`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ingested {
    pub game_id: i32,
    pub indexed_plies: usize,
}

/// PGN seven-tag-roster + indexing-relevant headers, all optional. `variant` and
/// `start_fen` (`[SetUp]`/`[FEN]`) make Chess960 and set-up positions first-class.
#[derive(Debug, Default, Clone)]
struct Headers {
    white: Option<String>,
    black: Option<String>,
    event: Option<String>,
    site: Option<String>,
    round: Option<String>,
    date: Option<String>,
    result: Option<String>,
    eco: Option<String>,
    white_elo: Option<i32>,
    black_elo: Option<i32>,
    variant: Option<String>,
    start_fen: Option<String>,
}

/// A parsed PGN: its headers and the mainline SAN tokens (variations dropped).
struct ParsedGame {
    headers: Headers,
    mainline: Vec<String>,
}

/// Parse, store and index the first game in `pgn` into `database_id`.
///
/// Players and the event are deduplicated by name; the mainline is replayed to
/// derive the per-ply Zobrist keys. All writes happen in one transaction, so a
/// malformed PGN or an illegal move leaves the database untouched.
pub async fn ingest_pgn(db: &DatabaseConnection, database_id: i32, pgn: &str) -> Result<Ingested> {
    let parsed = parse_pgn(pgn)?;
    let headers = &parsed.headers;

    let variant = headers
        .variant
        .clone()
        .unwrap_or_else(|| "standard".to_string());
    let mode = castling_mode(&variant);
    // The position to replay from: the `[FEN]` tag if set, else the startpos.
    let start_fen = headers.start_fen.as_deref().unwrap_or(STARTPOS_FEN);

    let plies =
        position::replay(start_fen, &parsed.mainline, mode).context("replaying PGN mainline")?;
    let start_zobrist = zobrist_of_fen(start_fen, mode).context("hashing start position")?;

    let txn = db.begin().await?;

    let white_id = intern_player(&txn, headers.white.as_deref()).await?;
    let black_id = intern_player(&txn, headers.black.as_deref()).await?;
    let event_id = intern_event(&txn, headers.event.as_deref()).await?;

    let game = games::ActiveModel {
        database_id: Set(database_id),
        white_player_id: Set(white_id),
        black_player_id: Set(black_id),
        event_id: Set(event_id),
        site: Set(headers.site.clone()),
        round: Set(headers.round.clone()),
        date: Set(headers.date.clone()),
        result: Set(headers.result.clone()),
        eco: Set(headers.eco.clone()),
        white_elo: Set(headers.white_elo),
        black_elo: Set(headers.black_elo),
        variant: Set(variant),
        // Stored only when non-standard; `None` means the startpos for the variant.
        start_fen: Set(headers.start_fen.clone()),
        ply_count: Set(Some(plies.len() as i32)),
        pgn: Set(Some(pgn.to_string())),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .context("inserting game")?;

    let depth = databases::Entity::find_by_id(database_id)
        .one(&txn)
        .await?
        .with_context(|| format!("database {database_id} not found"))?
        .index_depth
        .map(|d| d.max(0) as usize);
    let limit = depth.map_or(plies.len(), |cap| cap.min(plies.len()));

    // One row per indexed position: its Zobrist plus the move played *from* it.
    // Ply 0 is the start position; ply i's position is the position *after* move
    // i-1, i.e. `plies[i - 1]` (ADR-0003). The final position has no continuation
    // and so is not indexed.
    let rows: Vec<position_index::ActiveModel> = (0..limit)
        .map(|i| {
            let zobrist = if i == 0 {
                start_zobrist
            } else {
                plies[i - 1].zobrist
            };
            position_index::ActiveModel {
                zobrist: Set(position_index::to_i64(zobrist)),
                game_id: Set(game.id),
                ply: Set(i as i32),
                r#move: Set(plies[i].san.clone()),
                database_id: Set(database_id),
                ..Default::default()
            }
        })
        .collect();

    if !rows.is_empty() {
        position_index::Entity::insert_many(rows)
            .exec(&txn)
            .await
            .context("inserting position index")?;
    }

    txn.commit().await?;

    Ok(Ingested {
        game_id: game.id,
        indexed_plies: limit,
    })
}

/// Parse, store and index **every** game in a multi-game PGN into `database_id`,
/// returning one [`Ingested`] per game in document order.
///
/// Games are split on the `[Event ` line that opens each one (the export
/// convention shared by Lichess / Chess.com and `pgn-reader`). Each game is its
/// own [`ingest_pgn`] transaction, so a malformed or illegal game aborts that
/// game only — the ones already ingested before it stay committed.
pub async fn ingest_pgn_all(
    db: &DatabaseConnection,
    database_id: i32,
    pgn: &str,
) -> Result<Vec<Ingested>> {
    let games = split_games(pgn);
    // A blob with no `[Event]` header is still a single (headerless) game; defer
    // the empty-input rejection to `ingest_pgn`.
    if games.is_empty() {
        return Ok(vec![ingest_pgn(db, database_id, pgn).await?]);
    }
    let mut out = Vec::with_capacity(games.len());
    for (i, game) in games.iter().enumerate() {
        let ingested = ingest_pgn(db, database_id, game)
            .await
            .with_context(|| format!("ingesting game {}", i + 1))?;
        out.push(ingested);
    }
    Ok(out)
}

/// Split a complete multi-game PGN blob into individual, trimmed game strings.
/// Games are delimited by a line beginning with `[Event `. Shared with the
/// streaming collectors (Lichess / Chess.com).
pub(crate) fn split_games(blob: &str) -> Vec<String> {
    let starts = event_offsets(blob.as_bytes());
    let mut games = Vec::with_capacity(starts.len());
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(blob.len());
        let game = blob[start..end].trim();
        if !game.is_empty() {
            games.push(game.to_string());
        }
    }
    games
}

/// Byte offsets of every line that begins a new game (`[Event `). ASCII-only
/// matching, so it is safe on the raw byte buffer regardless of UTF-8 framing.
pub(crate) fn event_offsets(buf: &[u8]) -> Vec<usize> {
    const MARKER: &[u8] = b"[Event ";
    let mut offsets = Vec::new();
    let mut at_line_start = true;
    for i in 0..buf.len() {
        if at_line_start && buf[i..].starts_with(MARKER) {
            offsets.push(i);
        }
        at_line_start = buf[i] == b'\n';
    }
    offsets
}

/// Find a player by exact name or create one, returning its id. `None`/blank
/// names (e.g. a missing `[White]` tag) yield `None`.
async fn intern_player<C: ConnectionTrait>(db: &C, name: Option<&str>) -> Result<Option<i32>> {
    let Some(name) = name else { return Ok(None) };
    if let Some(existing) = players::Entity::find()
        .filter(players::Column::Name.eq(name))
        .one(db)
        .await?
    {
        return Ok(Some(existing.id));
    }
    let inserted = players::ActiveModel {
        name: Set(name.to_string()),
        ..Default::default()
    }
    .insert(db)
    .await
    .context("inserting player")?;
    Ok(Some(inserted.id))
}

/// Find an event by exact name or create one, returning its id.
async fn intern_event<C: ConnectionTrait>(db: &C, name: Option<&str>) -> Result<Option<i32>> {
    let Some(name) = name else { return Ok(None) };
    if let Some(existing) = events::Entity::find()
        .filter(events::Column::Name.eq(name))
        .one(db)
        .await?
    {
        return Ok(Some(existing.id));
    }
    let inserted = events::ActiveModel {
        name: Set(name.to_string()),
        ..Default::default()
    }
    .insert(db)
    .await
    .context("inserting event")?;
    Ok(Some(inserted.id))
}

/// Castling-rights interpretation for a PGN `Variant`: Chess960 (Fischer Random)
/// reads castling rights as rook files; everything else is standard.
fn castling_mode(variant: &str) -> CastlingMode {
    match variant.to_ascii_lowercase().as_str() {
        "chess960" | "fischerandom" | "fischerrandom" => CastlingMode::Chess960,
        _ => CastlingMode::Standard,
    }
}

/// Parse the first game's headers and mainline SAN from `pgn`.
fn parse_pgn(pgn: &str) -> Result<ParsedGame> {
    let mut reader = Reader::new(Cursor::new(pgn.as_bytes()));
    match reader.read_game(&mut Importer) {
        Ok(Some(game)) => Ok(game),
        Ok(None) => Err(anyhow!("no game found in PGN")),
        Err(e) => Err(anyhow!("malformed PGN: {e}")),
    }
}

/// Streaming visitor collecting headers and the mainline; variations are skipped
/// since only the mainline is indexed.
struct Importer;

impl Visitor for Importer {
    type Tags = Headers;
    type Movetext = ParsedGame;
    type Output = ParsedGame;

    fn begin_tags(&mut self) -> ControlFlow<Self::Output, Self::Tags> {
        ControlFlow::Continue(Headers::default())
    }

    fn tag(
        &mut self,
        tags: &mut Self::Tags,
        name: &[u8],
        value: RawTag<'_>,
    ) -> ControlFlow<Self::Output> {
        set_header(tags, name, value.decode_utf8_lossy());
        ControlFlow::Continue(())
    }

    fn begin_movetext(&mut self, tags: Self::Tags) -> ControlFlow<Self::Output, Self::Movetext> {
        ControlFlow::Continue(ParsedGame {
            headers: tags,
            mainline: Vec::new(),
        })
    }

    fn san(&mut self, game: &mut Self::Movetext, san_plus: SanPlus) -> ControlFlow<Self::Output> {
        game.mainline.push(san_plus.to_string());
        ControlFlow::Continue(())
    }

    fn begin_variation(&mut self, _game: &mut Self::Movetext) -> ControlFlow<Self::Output, Skip> {
        // Only the mainline feeds the position index.
        ControlFlow::Continue(Skip(true))
    }

    fn end_game(&mut self, game: Self::Movetext) -> Self::Output {
        game
    }
}

/// Record one parsed PGN tag into `headers`. Blank and `?` placeholders are
/// dropped; Elo tags parse to integers (unparseable values are ignored).
fn set_header(headers: &mut Headers, name: &[u8], value: Cow<'_, str>) {
    let value = value.trim();
    if value.is_empty() || value == "?" {
        return;
    }
    match name {
        b"White" => headers.white = Some(value.to_string()),
        b"Black" => headers.black = Some(value.to_string()),
        b"Event" => headers.event = Some(value.to_string()),
        b"Site" => headers.site = Some(value.to_string()),
        b"Round" => headers.round = Some(value.to_string()),
        b"Date" => headers.date = Some(value.to_string()),
        b"Result" => headers.result = Some(value.to_string()),
        b"ECO" => headers.eco = Some(value.to_string()),
        b"WhiteElo" => headers.white_elo = value.parse().ok(),
        b"BlackElo" => headers.black_elo = value.parse().ok(),
        b"Variant" => headers.variant = Some(value.to_ascii_lowercase()),
        b"FEN" => headers.start_fen = Some(value.to_string()),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
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

        let result = ingest_pgn(&conn, database_id, SCHOLARS_MATE).await.unwrap();
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
        let result = ingest_pgn(&conn, database_id, SCHOLARS_MATE).await.unwrap();

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
        let result = ingest_pgn(&conn, database_id, SCHOLARS_MATE).await.unwrap();

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

        let result = ingest_pgn(&conn, db.id, SCHOLARS_MATE).await.unwrap();
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

    #[tokio::test]
    async fn missing_player_tags_yield_null_ids() {
        let (conn, database_id) = db_with_own_collection().await;
        let result = ingest_pgn(&conn, database_id, "1. d4 d5 *").await.unwrap();

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

        let ingested = ingest_pgn_all(&conn, database_id, TWO_GAMES).await.unwrap();
        assert_eq!(ingested.len(), 2);
        assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn ingest_all_accepts_a_headerless_single_game() {
        let (conn, database_id) = db_with_own_collection().await;

        // No `[Event]` tag ⇒ split finds nothing; the whole blob is one game.
        let ingested = ingest_pgn_all(&conn, database_id, "1. d4 d5 *")
            .await
            .unwrap();
        assert_eq!(ingested.len(), 1);
        assert_eq!(ingested[0].indexed_plies, 2);
    }

    #[tokio::test]
    async fn ingest_all_rejects_a_blank_blob() {
        let (conn, database_id) = db_with_own_collection().await;
        assert!(ingest_pgn_all(&conn, database_id, "   \n  ").await.is_err());
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

        let result = ingest_pgn(&conn, database_id, &pgn).await.unwrap();
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

        let result = ingest_pgn(&conn, database_id, &pgn).await.unwrap();
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
}
