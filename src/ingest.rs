//! The shared game-ingest path: parse a PGN, store the game with its deduplicated
//! header roster, replay the mainline via [`position::replay`], and bulk-insert
//! the [`position_index`](crate::db::entities::position_index) rows that back
//! position search (ADR-0003).
//!
//! Every collector (Lichess / Chess.com / bulk import) funnels through
//! [`ingest_pgn`] so games are stored and indexed identically regardless of
//! source. Parsing only reads syntax; legality is enforced by `position::replay`,
//! so an illegal move aborts the ingest before any row is written.

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, Set, TransactionTrait,
};
use sha2::{Digest, Sha256};

use crate::db::entities::{databases, events, games, players, position_index};
use crate::position::{self, zobrist_of_fen, CastlingMode, STARTPOS_FEN};

mod parse;
pub(crate) use parse::{event_offsets, parse_pgn, split_games, Headers, ParsedGame};

/// Outcome of a successful ingest: the new game's id and how many positions were
/// written to `position_index` (mainline length, capped by the database's
/// `index_depth`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ingested {
    pub game_id: i32,
    pub indexed_plies: usize,
}

/// Why a single game failed to ingest. The split lets a multi-game import skip a
/// bad game (`BadGame`) while still aborting on a genuine storage failure (`Db`).
#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    /// The PGN content is bad — malformed syntax, an illegal move, or an
    /// unparseable set-up position. Client-safe and actionable; carries a curated
    /// message, never a raw `DbErr` or provider chain.
    #[error("{0}")]
    BadGame(String),
    /// A storage failure while writing the game. Internal — never surfaced raw.
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// One game in a multi-game import that could not be stored. `index` is its
/// 1-based position in the blob; `message` is client-safe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameError {
    pub index: usize,
    pub message: String,
}

/// Outcome of a multi-game ingest under skip-and-continue: the games stored and
/// the ones skipped with a safe reason. A genuine storage failure aborts the
/// whole run instead (returned as `Err`), so a transient DB outage is never
/// silently reported as a pile of skipped games.
#[derive(Debug, Default)]
pub struct IngestReport {
    pub imported: Vec<Ingested>,
    pub errors: Vec<GameError>,
}

impl ParsedGame {
    /// A content-derived dedup key for a game that carries no provider permalink
    /// (master bases like TWIC / Lumbras), so the bulk importer can skip
    /// duplicates and restart safely (issue #4). Stable across re-runs.
    pub(crate) fn content_hash(&self) -> String {
        game_hash(&self.headers, &self.mainline)
    }
}

/// A validated game ready to store: its headers, the resolved variant, the
/// replayed mainline, and the start position's Zobrist key.
pub(crate) struct PreparedGame {
    headers: Headers,
    variant: String,
    plies: Vec<position::Ply>,
    start_zobrist: u64,
}

/// SHA-256 over a game's normalized seven-tag roster, variant/start position and
/// mainline SAN — the dedup key for permalink-less games (issue #4). The
/// `sha256:` prefix keeps it disjoint from URL `source_ref`s.
fn game_hash(headers: &Headers, mainline: &[String]) -> String {
    let mut hasher = Sha256::new();
    let fields = [
        headers.white.as_deref(),
        headers.black.as_deref(),
        headers.event.as_deref(),
        headers.site.as_deref(),
        headers.round.as_deref(),
        headers.date.as_deref(),
        headers.result.as_deref(),
        headers.variant.as_deref(),
        headers.start_fen.as_deref(),
    ];
    for field in fields {
        hasher.update(field.unwrap_or("").as_bytes());
        // NUL separator so adjacent fields can't blur into one another.
        hasher.update([0u8]);
    }
    for san in mainline {
        hasher.update(san.as_bytes());
        hasher.update([b' ']);
    }
    format!("sha256:{:x}", hasher.finalize())
}

/// Validate a parsed game — replay the mainline and hash the start position.
/// These are the steps that can fail on *bad game data* (an illegal move or an
/// unparseable set-up FEN); failures return a client-safe message. No storage
/// happens here, so a caller can skip a bad game without ever opening a
/// transaction.
pub(crate) fn prepare_game(parsed: ParsedGame) -> std::result::Result<PreparedGame, String> {
    let variant = parsed
        .headers
        .variant
        .clone()
        .unwrap_or_else(|| "standard".to_string());
    let mode = castling_mode(&variant);
    // The position to replay from: the `[FEN]` tag if set, else the startpos.
    let start_fen = parsed.headers.start_fen.as_deref().unwrap_or(STARTPOS_FEN);
    let plies = position::replay(start_fen, &parsed.mainline, mode)
        .map_err(|e| format!("illegal move in mainline: {e}"))?;
    let start_zobrist =
        zobrist_of_fen(start_fen, mode).map_err(|e| format!("invalid start position: {e}"))?;
    Ok(PreparedGame {
        headers: parsed.headers,
        variant,
        plies,
        start_zobrist,
    })
}

/// Parse, store and index the first game in `pgn` into `database_id`.
///
/// Players and the event are deduplicated by name; the mainline is replayed to
/// derive the per-ply Zobrist keys. All writes happen in one transaction, so a
/// malformed PGN or an illegal move leaves the database untouched.
///
/// Returns `Ok(None)` when the game is a duplicate — a game with the same
/// provider `source_ref` (permalink) already exists in `database_id` — so a
/// re-sync revisiting the cursor boundary appends nothing instead of doubling
/// every game (issue #95). Games without a permalink are always ingested. Bad
/// game data fails with [`IngestError::BadGame`]; a storage failure with
/// [`IngestError::Db`].
pub async fn ingest_pgn(
    db: &DatabaseConnection,
    database_id: i32,
    pgn: &str,
) -> std::result::Result<Option<Ingested>, IngestError> {
    let parsed = parse_pgn(pgn).map_err(|e| IngestError::BadGame(e.to_string()))?;

    // Dedup before the (more expensive) replay so re-syncing mostly-seen games
    // stays cheap (issue #95).
    let source_ref = source_ref(&parsed.headers);
    if let Some(key) = &source_ref {
        if game_exists(db, database_id, key).await? {
            return Ok(None);
        }
    }

    let prepared = prepare_game(parsed).map_err(IngestError::BadGame)?;
    let index_depth = load_index_depth(db, database_id).await?;

    let txn = db.begin().await?;
    let ingested =
        store_prepared(&txn, database_id, &prepared, pgn, source_ref, index_depth).await?;
    txn.commit().await?;

    Ok(Some(ingested))
}

/// The `index_depth` policy for a database (ADR-0003), looked up once so callers
/// can pass it into [`store_prepared`] instead of re-querying per game. The
/// caller has already confirmed the database exists; a miss is an internal
/// inconsistency, not a bad game.
pub(crate) async fn load_index_depth(
    db: &DatabaseConnection,
    database_id: i32,
) -> std::result::Result<Option<i32>, DbErr> {
    let model = databases::Entity::find_by_id(database_id)
        .one(db)
        .await?
        .ok_or_else(|| DbErr::RecordNotFound(format!("database {database_id}")))?;
    Ok(model.index_depth)
}

/// Store one prepared game and its position-index rows inside an existing
/// transaction. Shared by [`ingest_pgn`] (one game per txn) and the bulk importer
/// (many games per batched txn, issue #4). `source_ref` is the game's dedup key
/// (permalink or content hash); `index_depth` is the caller's pre-fetched policy.
pub(crate) async fn store_prepared<C: ConnectionTrait>(
    txn: &C,
    database_id: i32,
    prepared: &PreparedGame,
    pgn: &str,
    source_ref: Option<String>,
    index_depth: Option<i32>,
) -> std::result::Result<Ingested, DbErr> {
    let PreparedGame {
        headers,
        variant,
        plies,
        start_zobrist,
    } = prepared;

    let white_id = intern_player(txn, headers.white.as_deref()).await?;
    let black_id = intern_player(txn, headers.black.as_deref()).await?;
    let event_id = intern_event(txn, headers.event.as_deref()).await?;

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
        variant: Set(variant.clone()),
        // Stored only when non-standard; `None` means the startpos for the variant.
        start_fen: Set(headers.start_fen.clone()),
        ply_count: Set(Some(plies.len() as i32)),
        pgn: Set(Some(pgn.to_string())),
        source_ref: Set(source_ref),
        ..Default::default()
    }
    .insert(txn)
    .await?;

    let depth = index_depth.map(|d| d.max(0) as usize);
    let limit = depth.map_or(plies.len(), |cap| cap.min(plies.len()));

    // One row per indexed position: its Zobrist plus the move played *from* it.
    // Ply 0 is the start position; ply i's position is the position *after* move
    // i-1, i.e. `plies[i - 1]` (ADR-0003). The final position has no continuation
    // and so is not indexed.
    let rows: Vec<position_index::ActiveModel> = (0..limit)
        .map(|i| {
            let zobrist = if i == 0 {
                *start_zobrist
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
        position_index::Entity::insert_many(rows).exec(txn).await?;
    }

    Ok(Ingested {
        game_id: game.id,
        indexed_plies: limit,
    })
}

/// The stable provider key for a game — the permalink — used to dedup re-syncs
/// (issue #95). Chess.com carries it in `[Link]`; Lichess puts the game URL in
/// `[Site]`. A non-URL `Site` (e.g. `"London"`) is not a game key, so it yields
/// `None` and the game is never deduped.
fn source_ref(headers: &Headers) -> Option<String> {
    if let Some(link) = &headers.link {
        return Some(link.clone());
    }
    match &headers.site {
        Some(site) if site.starts_with("http") => Some(site.clone()),
        _ => None,
    }
}

/// Whether `database_id` already holds a game with this `source_ref` (permalink
/// or content hash). The cross-run dedup the bulk importer relies on to restart.
pub(crate) async fn game_exists(
    db: &DatabaseConnection,
    database_id: i32,
    source_ref: &str,
) -> std::result::Result<bool, DbErr> {
    let found = games::Entity::find()
        .filter(games::Column::DatabaseId.eq(database_id))
        .filter(games::Column::SourceRef.eq(source_ref))
        .one(db)
        .await?;
    Ok(found.is_some())
}

/// Parse, store and index **every** game in a multi-game PGN into `database_id`
/// under skip-and-continue, returning an [`IngestReport`].
///
/// Games are split on the `[Event ` line that opens each one (the export
/// convention shared by Lichess / Chess.com and `pgn-reader`). A malformed or
/// illegal game is recorded in [`IngestReport::errors`] and the rest still
/// import; only a genuine storage failure aborts the whole run (returned as
/// `Err`), so a 500-game upload with one bad game no longer rolls the client into
/// a re-upload. Games skipped as duplicates (see [`ingest_pgn`]) are silently
/// omitted from both lists.
pub async fn ingest_pgn_all(
    db: &DatabaseConnection,
    database_id: i32,
    pgn: &str,
) -> std::result::Result<IngestReport, DbErr> {
    let games = split_games(pgn);
    let mut report = IngestReport::default();
    // A blob with no `[Event]` header is still a single (headerless) game; defer
    // the empty-input rejection to `ingest_pgn`.
    if games.is_empty() {
        ingest_into_report(db, database_id, pgn, 1, &mut report).await?;
        return Ok(report);
    }
    for (i, game) in games.iter().enumerate() {
        ingest_into_report(db, database_id, game, i + 1, &mut report).await?;
    }
    Ok(report)
}

/// Ingest one game into `report`: a newly stored game is recorded as imported, a
/// duplicate is silently dropped, a bad game is skipped with a safe message, and
/// only a real storage failure aborts (`Err`).
async fn ingest_into_report(
    db: &DatabaseConnection,
    database_id: i32,
    pgn: &str,
    index: usize,
    report: &mut IngestReport,
) -> std::result::Result<(), DbErr> {
    match ingest_pgn(db, database_id, pgn).await {
        Ok(Some(ingested)) => report.imported.push(ingested),
        // A deduped game is neither imported nor an error.
        Ok(None) => {}
        Err(IngestError::BadGame(message)) => report.errors.push(GameError { index, message }),
        Err(IngestError::Db(e)) => return Err(e),
    }
    Ok(())
}

/// Find a player by exact name or create one, returning its id. `None`/blank
/// names (e.g. a missing `[White]` tag) yield `None`.
async fn intern_player<C: ConnectionTrait>(
    db: &C,
    name: Option<&str>,
) -> std::result::Result<Option<i32>, DbErr> {
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
    .await?;
    Ok(Some(inserted.id))
}

/// Find an event by exact name or create one, returning its id.
async fn intern_event<C: ConnectionTrait>(
    db: &C,
    name: Option<&str>,
) -> std::result::Result<Option<i32>, DbErr> {
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
    .await?;
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

#[cfg(test)]
#[path = "ingest_tests.rs"]
mod tests;
