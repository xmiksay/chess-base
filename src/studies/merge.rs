//! Merge many games' mainlines into one repertoire study (issue #170): the
//! transport-agnostic [`StudyService::merge_games`] and its pure PGN-metadata
//! helpers. Kept out of `mod.rs` so that already-large file stays under the cap.
//!
//! The move mechanics — SAN-follow dedup, frequency ordering, branch-point stats —
//! live in the pure [`crate::pgn_tree::merge`]; this layer only resolves each stored
//! game to a [`MergeGame`] (mainline + label + score) and persists the result.

use sea_orm::{ActiveModelTrait, Set};

use crate::db::entities::studies;
use crate::games::{GameDetail, GameService};
use crate::ingest::parse_pgn;
use crate::pgn_tree::merge::MergeGame;
use crate::pgn_tree::MoveTree;
use crate::position::STARTPOS_FEN;
use crate::server::identity::CurrentUser;

use super::{StudyError, StudyService};

impl StudyService {
    /// Fold the mainlines of `game_ids` (each visible to the caller: own ∪ global)
    /// into one repertoire study, ordering continuations by frequency and pinning
    /// per-node stats on the branch points (issue #170).
    ///
    /// `study_id` set ⇒ graft into that existing study (the caller must be able to
    /// write it); otherwise a new study is created from the standard start, `name`
    /// required, owned by the caller and filed into `folder_id`. Games that don't
    /// replay from the standard start (a set-up `[FEN]` or a non-standard variant),
    /// have no moves, or won't parse are skipped — transpositional entry orders stay
    /// visible as branches, matching the repertoire intent. Re-merging the same games
    /// is idempotent (SAN-follow dedup).
    pub async fn merge_games(
        &self,
        user: &CurrentUser,
        game_ids: &[i32],
        study_id: Option<i32>,
        name: Option<String>,
        folder_id: Option<i32>,
    ) -> Result<studies::Model, StudyError> {
        if game_ids.is_empty() {
            return Err(StudyError::InvalidEdit("no games to merge".into()));
        }

        let games = GameService::new(self.db.clone());
        let mut sources = Vec::new();
        let mut source_database = None;
        for &id in game_ids {
            // `get` enforces visibility (own ∪ global) — an invisible id is a clean
            // NotFound, never a leak.
            let game = games.get(user, id).await?;
            if !is_standard_start(&game) {
                continue;
            }
            let Some(pgn) = game.pgn.as_deref() else {
                continue;
            };
            let Ok(parsed) = parse_pgn(pgn) else {
                continue;
            };
            if parsed.mainline.is_empty() {
                continue;
            }
            source_database.get_or_insert(game.database_id);
            sources.push(MergeGame {
                sans: parsed.mainline,
                label: game_label(&game),
                white_score: white_score(game.result.as_deref()),
            });
        }
        if sources.is_empty() {
            return Err(StudyError::InvalidEdit(
                "no mergeable games (all empty, set-up or non-standard)".into(),
            ));
        }

        match study_id {
            Some(study_id) => {
                let study = self.load_writable(user, study_id).await?;
                let mut tree: MoveTree = serde_json::from_str(&study.tree_json)?;
                // A set-up start can't host standard-start repertoire lines.
                if tree.start_position() != STARTPOS_FEN {
                    return Err(StudyError::InvalidEdit(
                        "cannot merge games into a study with a set-up start position".into(),
                    ));
                }
                tree.merge_games(&sources);
                self.persist(study, &tree).await?;
                self.get(user, study_id).await
            }
            None => {
                let name = name
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty())
                    .ok_or_else(|| {
                        StudyError::InvalidEdit("a name is required for a new study".into())
                    })?;
                if let Some(folder_id) = folder_id {
                    self.assert_folder_writable(user, folder_id).await?;
                }
                let database_id = source_database
                    .ok_or_else(|| StudyError::InvalidEdit("no source database".into()))?;
                let mut tree = MoveTree::new();
                tree.merge_games(&sources);
                let model = studies::ActiveModel {
                    database_id: Set(database_id),
                    owner_id: Set(Some(user.id.clone())),
                    name: Set(name),
                    tree_json: Set(serde_json::to_string(&tree)?),
                    folder_id: Set(folder_id),
                    ..Default::default()
                }
                .insert(&self.db)
                .await?;
                Ok(model)
            }
        }
    }
}

/// Whether the game replays from the standard start — no set-up `[FEN]` and a
/// standard variant — so its mainline can graft from the repertoire root.
fn is_standard_start(game: &GameDetail) -> bool {
    let standard_variant = game.variant.is_empty()
        || game.variant.eq_ignore_ascii_case("standard")
        || game.variant.eq_ignore_ascii_case("chess");
    let standard_start = game
        .start_fen
        .as_deref()
        .is_none_or(|fen| fen.trim() == STARTPOS_FEN);
    standard_variant && standard_start
}

/// A compact `"White–Black Year"` label for a stats comment: each player's surname
/// (the part before a `", First"`), an en-dash, and the game's year when present.
fn game_label(game: &GameDetail) -> String {
    let white = surname(game.white.as_deref());
    let black = surname(game.black.as_deref());
    match year(game.date.as_deref()) {
        Some(year) => format!("{white}–{black} {year}"),
        None => format!("{white}–{black}"),
    }
}

/// The surname from a PGN player field: the part before the first comma
/// (`"Carlsen, Magnus"` → `"Carlsen"`), or `"?"` when the field is blank.
fn surname(name: Option<&str>) -> String {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) => name.split(',').next().unwrap_or(name).trim().to_string(),
        None => "?".to_string(),
    }
}

/// The four-digit year from a PGN date (`"2023.05.01"` → `"2023"`), or `None` when
/// it is unknown (`"????.??.??"`) or malformed.
fn year(date: Option<&str>) -> Option<String> {
    let year: String = date?.trim().chars().take(4).collect();
    (year.len() == 4 && year.chars().all(|c| c.is_ascii_digit())).then_some(year)
}

/// White-perspective score from a PGN result token: `1-0` → 1.0, `0-1` → 0.0, a
/// draw → 0.5; anything else (`*`, missing) is an unknown result.
fn white_score(result: Option<&str>) -> Option<f32> {
    match result.map(str::trim) {
        Some("1-0") => Some(1.0),
        Some("0-1") => Some(0.0),
        Some("1/2-1/2" | "½-½") => Some(0.5),
        _ => None,
    }
}

#[cfg(test)]
#[path = "merge_tests.rs"]
mod tests;
