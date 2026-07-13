//! Frequency-ordered merge of many game mainlines into one repertoire tree
//! (issue #170): fold each source game's mainline in as a deduped, legality-checked
//! line (the SAN-follow dedup [`MoveTree::graft_subtree`] uses), then order every
//! node's continuations by how often the merged games played them and pin per-node
//! stats onto the branch points. A final Zobrist pass (issue #174,
//! [`super::transpositions`]) tags any branch that transposes into an
//! already-merged line, so the same tabiya reached via a different move order
//! doesn't read as two unrelated continuations.
//!
//! Pure and I/O-free: the caller resolves each game to SAN moves + a display label
//! + a White-perspective score, so this stays unit-testable without a DB or engine.

use std::collections::HashMap;

use crate::position::{apply_san, CastlingMode};

use super::MoveTree;

/// Repertoire studies are standard chess — the same castling mode the rest of the
/// study move-tree code parses with.
const MODE: CastlingMode = CastlingMode::Standard;

/// How many distinct game labels a branch-point comment names before eliding the
/// rest with `…`.
const MAX_SAMPLES: usize = 3;

/// One source game to fold into the repertoire, resolved to pure data by the caller.
pub struct MergeGame {
    /// The game's mainline as SAN moves, replayed from the standard start.
    pub sans: Vec<String>,
    /// A short label for the stats comment, e.g. `"Carlsen–Nepo 2023"`.
    pub label: String,
    /// Result from **White's** perspective (`1.0` win / `0.5` draw / `0.0` loss),
    /// or `None` when the game is unfinished / has no recorded result.
    pub white_score: Option<f32>,
}

/// Accumulated stats for a single tree node across the merged games.
#[derive(Default)]
struct NodeStat {
    /// How many merged games passed through this node.
    games: u32,
    /// Sum of White-perspective scores over the games with a known result.
    score_sum: f32,
    /// How many of those games had a known result (the `score_sum` denominator).
    scored: u32,
    /// Distinct game labels (first-seen order), capped at [`MAX_SAMPLES`].
    samples: Vec<String>,
}

impl NodeStat {
    /// Record one game reaching this node.
    fn record(&mut self, game: &MergeGame) {
        self.games += 1;
        if let Some(score) = game.white_score {
            self.score_sum += score;
            self.scored += 1;
        }
        if self.samples.len() < MAX_SAMPLES && !self.samples.iter().any(|l| l == &game.label) {
            self.samples.push(game.label.clone());
        }
    }

    /// The branch-point comment: `"12 games, 71% (Carlsen–Nepo 2023, …)"`. The
    /// percentage is the mover's expected score (White's when `mover_is_white`,
    /// else its complement) over the games with a known result; it is omitted when
    /// none of them do.
    fn comment(&self, mover_is_white: bool) -> String {
        let mut out = format!(
            "{} game{}",
            self.games,
            if self.games == 1 { "" } else { "s" }
        );
        if self.scored > 0 {
            let white_pct = self.score_sum / self.scored as f32;
            let pct = if mover_is_white {
                white_pct
            } else {
                1.0 - white_pct
            };
            out.push_str(&format!(", {}%", (pct * 100.0).round() as i32));
        }
        if !self.samples.is_empty() {
            let more = self.games as usize > self.samples.len();
            out.push_str(&format!(
                " ({}{})",
                self.samples.join(", "),
                if more { ", …" } else { "" }
            ));
        }
        out
    }
}

impl MoveTree {
    /// Fold each game's mainline into this tree as a deduped, legality-checked line,
    /// then order continuations by frequency, annotate the branch points with
    /// per-node stats, and tag any resulting transposition (issue #170 / #174).
    /// Returns the number of **newly added** nodes.
    ///
    /// Each game is walked from the root: a move that matches an existing child
    /// (ignoring a trailing check/mate marker) follows it, so a re-merge of the same
    /// games adds nothing (idempotent); a new move appends a variation. A move that
    /// is illegal in the running position stops that game's line rather than
    /// corrupting the tree. Games must start from this tree's start position — a
    /// set-up mismatch simply drops the line on the first illegal move.
    pub fn merge_games(&mut self, games: &[MergeGame]) -> usize {
        let start = self.start_position().to_string();
        let mut stats: HashMap<usize, NodeStat> = HashMap::new();
        let mut added = 0;
        for game in games {
            let mut cur = self.root;
            let mut fen = start.clone();
            for san in &game.sans {
                // Validate in the running position; a bad move ends this line so a
                // corrupt or off-start game never derails the merge.
                let Ok((after, _)) = apply_san(&fen, san, MODE) else {
                    break;
                };
                let child = match self.child_by_san(cur, san) {
                    Some(existing) => existing,
                    None => {
                        added += 1;
                        self.add_move(cur, san.clone())
                    }
                };
                stats.entry(child).or_default().record(game);
                cur = child;
                fen = after;
            }
        }
        self.order_by_frequency(&stats);
        self.annotate_branches(&stats);
        self.mark_transpositions();
        added
    }

    /// Reorder every node's children so the most-played continuation is the mainline
    /// (`children[0]`) and rarer moves fall to variations. Stable in a tie, so an
    /// equally-popular existing mainline keeps its place.
    fn order_by_frequency(&mut self, stats: &HashMap<usize, NodeStat>) {
        let count = |id: &usize| stats.get(id).map_or(0, |s| s.games);
        for id in 0..self.nodes.len() {
            let mut children = std::mem::take(&mut self.nodes[id].children);
            children.sort_by_key(|id| std::cmp::Reverse(count(id)));
            self.nodes[id].children = children;
        }
    }

    /// Pin a stats comment onto every branch alternative the merge touched — a node
    /// whose parent has more than one continuation. A user comment (or a node no
    /// merged game reached) is left untouched; a prior stats comment is refreshed,
    /// keeping a re-merge idempotent.
    fn annotate_branches(&mut self, stats: &HashMap<usize, NodeStat>) {
        for id in 0..self.nodes.len() {
            let Some(parent) = self.nodes[id].parent else {
                continue;
            };
            if self.nodes[parent].children.len() < 2 {
                continue;
            }
            let Some(stat) = stats.get(&id) else {
                continue;
            };
            let overwritable = match self.nodes[id].comment.as_deref() {
                None => true,
                Some(existing) => is_stats_comment(existing),
            };
            if overwritable {
                let comment = stat.comment(self.mover_is_white(id));
                self.nodes[id].comment = Some(comment);
            }
        }
    }

    /// Whether the move leading into `id` was White's, from the node's depth: the
    /// standard start has White on the odd plies (1st, 3rd, …).
    fn mover_is_white(&self, id: usize) -> bool {
        let mut depth = 0;
        let mut node = &self.nodes[id];
        while let Some(parent) = node.parent {
            depth += 1;
            node = &self.nodes[parent];
        }
        depth % 2 == 1
    }
}

/// Whether `comment` looks like a stats comment this module generated
/// (`"<n> game…"`), so a re-merge refreshes it instead of clobbering a user's prose.
fn is_stats_comment(comment: &str) -> bool {
    let rest = comment.trim_start();
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    digits > 0 && rest[digits..].starts_with(" game")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(sans: &[&str], label: &str, white_score: Option<f32>) -> MergeGame {
        MergeGame {
            sans: sans.iter().map(|s| s.to_string()).collect(),
            label: label.to_string(),
            white_score,
        }
    }

    #[test]
    fn merges_shared_prefix_and_dedups() {
        let mut tree = MoveTree::new();
        let added = tree.merge_games(&[
            game(&["e4", "e5", "Nf3"], "A", Some(1.0)),
            game(&["e4", "e5", "Nc3"], "B", Some(0.0)),
        ]);
        // e4, e5, Nf3, Nc3 — the shared e4/e5 prefix is not duplicated.
        assert_eq!(added, 4);
        assert_eq!(tree.mainline(), vec!["e4", "e5", "Nf3"]);
    }

    #[test]
    fn reorders_by_frequency() {
        let mut tree = MoveTree::new();
        // Two games open 1.d4, one opens 1.e4 → d4 is the most-common first move.
        tree.merge_games(&[
            game(&["e4"], "A", None),
            game(&["d4"], "B", None),
            game(&["d4"], "C", None),
        ]);
        assert_eq!(tree.mainline(), vec!["d4"]);
        // Both first moves survive as siblings under the root.
        let root_children: Vec<_> = tree.nodes[tree.root]
            .children
            .iter()
            .map(|&c| tree.nodes[c].san.clone().unwrap())
            .collect();
        assert_eq!(root_children, vec!["d4", "e4"]);
    }

    #[test]
    fn annotates_branch_points_with_count_and_score() {
        let mut tree = MoveTree::new();
        tree.merge_games(&[
            game(&["e4", "e5"], "Carlsen–Nepo 2023", Some(1.0)),
            game(&["e4", "c5"], "Carlsen–So 2022", Some(0.0)),
        ]);
        // e4 is not a branch point (single first move) → no stats comment.
        let e4 = tree.nodes[tree.root].children[0];
        assert!(tree.nodes[e4].comment.is_none());
        // e5 / c5 diverge → each carries "1 game, <white%> (label)".
        let replies = &tree.nodes[e4].children;
        let comments: Vec<_> = replies
            .iter()
            .map(|&c| tree.nodes[c].comment.clone().unwrap())
            .collect();
        // Black replied, so the score shown is Black's perspective: after 1.e4 e5
        // White won → Black scored 0%.
        assert!(comments
            .iter()
            .any(|c| c == "1 game, 0% (Carlsen–Nepo 2023)"));
        assert!(comments
            .iter()
            .any(|c| c == "1 game, 100% (Carlsen–So 2022)"));
    }

    #[test]
    fn re_merge_is_idempotent() {
        let games = [
            game(&["e4", "e5"], "A", Some(0.5)),
            game(&["d4", "d5"], "B", Some(0.5)),
        ];
        let mut tree = MoveTree::new();
        tree.merge_games(&games);
        let before = serde_json::to_string(&tree).unwrap();
        let added = tree.merge_games(&games);
        assert_eq!(added, 0);
        assert_eq!(serde_json::to_string(&tree).unwrap(), before);
    }

    #[test]
    fn illegal_move_stops_a_line_without_panicking() {
        let mut tree = MoveTree::new();
        // "e5" is illegal as White's first move → the whole line is dropped.
        let added = tree.merge_games(&[game(&["e5", "Nf3"], "bad", None)]);
        assert_eq!(added, 0);
        assert!(tree.mainline().is_empty());
    }

    #[test]
    fn user_comment_survives_a_merge_but_a_stats_comment_refreshes() {
        assert!(is_stats_comment("12 games, 71% (x)"));
        assert!(is_stats_comment("1 game"));
        assert!(!is_stats_comment("A key tabiya"));
        assert!(!is_stats_comment("games galore"));
    }
}
