//! Pure helpers for the "Analyse study" pass (issue #162, full review-grade
//! classification #189): list every move-bearing node with the FEN before and
//! after its move, and classify it from engine search results the same way
//! `review::service::assemble` classifies a game — best-before / after-played /
//! played-rank / second-best — so a study gets the same `!`/`?!`/`?`/`??`
//! glyphs a full-game review does, not just a stored eval.
//!
//! [`node_searches`] and [`classify_search`] are I/O-free — the MultiPV engine
//! search and persistence live in
//! [`StudyService::analyse_study`](super::StudyService::analyse_study) — so
//! this module unit-tests with synthetic [`Analysis`] values, no engine
//! process required.

pub use crate::study_gen::tree::white_eval;

use crate::engine::{Analysis, Score};
use crate::features::features_of_fen;
use crate::pgn_tree::{is_move_quality_nag, Eval, MoveTree};
use crate::position::{black_to_move, replay, san_to_uci, CastlingMode, PositionError};
use crate::review::classify::{classify, Classification, MoveCost};

use super::san_core;

/// Studies are standard chess (matches [`super::MODE`]).
const MODE: CastlingMode = CastlingMode::Standard;

/// One move-bearing node paired with the FEN before and after its move. The
/// root carries no move, so it is skipped: a search pins to a played move.
pub struct NodeSearch {
    pub node_id: usize,
    pub fen_before: String,
    pub fen_after: String,
    pub san: String,
    /// Whether White played this move (the classification's mover).
    pub mover_white: bool,
}

/// Every move-bearing node with the FEN immediately before and after its move,
/// replaying each node's line from the tree's start position. A node's
/// `fen_before` is its parent's `fen_after` (or the tree's start position when
/// the parent is the root) — callers should cache engine searches by FEN so a
/// linear line costs one search per node, not two.
pub fn node_searches(tree: &MoveTree) -> Result<Vec<NodeSearch>, PositionError> {
    let start = tree.start_position();
    let mut out = Vec::with_capacity(tree.nodes.len());
    for node in &tree.nodes {
        let Some(san) = &node.san else { continue };
        let Some(line) = tree.line_to(node.id) else {
            continue;
        };
        let plies = replay(start, &line, MODE)?;
        let fen_after = plies
            .last()
            .map(|p| p.fen.clone())
            .unwrap_or_else(|| start.to_string());
        let fen_before = if plies.len() >= 2 {
            plies[plies.len() - 2].fen.clone()
        } else {
            start.to_string()
        };
        let mover_white = !black_to_move(&fen_before, MODE)?;
        out.push(NodeSearch {
            node_id: node.id,
            fen_before,
            fen_after,
            san: san.clone(),
            mover_white,
        });
    }
    Ok(out)
}

/// The mover's-perspective evaluation after a move: a delivered mate is a win
/// for the mover, stalemate/insufficient material a draw, otherwise
/// `opponent_score` (the side-to-move-at-`fen_after`'s own score, when that
/// position was searched) negated to the mover's view. Mirrors
/// `review::service::after_eval`.
pub fn after_eval(fen_after: &str, opponent_score: Option<Score>) -> Score {
    if let Ok(f) = features_of_fen(fen_after) {
        if f.checkmate {
            return Score::Mate { value: 1 };
        }
        if f.stalemate || f.insufficient_material {
            return Score::Cp { value: 0 };
        }
    }
    opponent_score
        .map(Score::negate)
        .unwrap_or(Score::Cp { value: 0 })
}

/// Classify one node from its engine search results: `before` is the MultiPV
/// lines searched at the position *before* the move (best line first, mirrors
/// `review::service::REVIEW_MULTIPV`), `after_top` the top line's score at the
/// position *after* it (`None` when that position is terminal and so was never
/// searched — [`after_eval`] derives the result straight off the board
/// instead). Returns the node's White-perspective [`Eval`], its
/// [`Classification`], and the [`MoveCost`] feeding the study's accuracy
/// roll-up. Pure — unit-tested with synthetic [`Analysis`] values, mirroring
/// `review::service::assemble` (issue #189).
pub fn classify_search(
    search: &NodeSearch,
    before: &[Analysis],
    after_top: Option<Score>,
) -> (Eval, Classification, MoveCost) {
    let best_before = before
        .first()
        .and_then(|a| a.score)
        .unwrap_or(Score::Cp { value: 0 });
    let best_uci = first_uci(before.first());
    let second_best = before.get(1).and_then(|a| a.score);

    let played_uci = san_to_uci(&search.fen_before, san_core(&search.san), MODE).ok();
    let played_is_best = matches!((&played_uci, &best_uci), (Some(p), Some(b)) if p == b);

    let after_played = after_eval(&search.fen_after, after_top);
    let classification = classify(best_before, after_played, played_is_best, second_best);
    let cost = MoveCost::from_eval(
        search.mover_white,
        best_before,
        after_played,
        classification,
    );
    let eval = white_eval(after_played, search.mover_white);
    (eval, classification, cost)
}

/// The engine's chosen move for a line, in UCI: its PV's first move, else the
/// raw `bestmove` when the PV is empty (mirrors `review::service::LineEval`).
fn first_uci(line: Option<&Analysis>) -> Option<String> {
    line.and_then(|a| {
        a.pv.first().cloned().or_else(|| {
            (!a.bestmove.is_empty() && a.bestmove != "(none)").then(|| a.bestmove.clone())
        })
    })
}

/// Replace any prior move-quality NAG ($1..$6: !, ?, !!, ??, !?, ?!) on `id`
/// with `nag` (`None` clears without adding a new one). Positional NAGs
/// (everything outside 1..=6) are left alone, so re-analysing never disturbs a
/// NAG the user pinned for its own sake (issue #189).
pub fn set_quality_nag(tree: &mut MoveTree, id: usize, nag: Option<u8>) {
    tree.nodes[id].nags.retain(|n| !is_move_quality_nag(*n));
    if let Some(nag) = nag {
        tree.nodes[id].nags.push(nag);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_searches_skips_the_root_and_walks_variations() {
        // 1. e4 (1. d4) e5 — a mainline with one root variation.
        let mut tree = MoveTree::new();
        let e4 = tree.add_move(tree.root, "e4");
        let d4 = tree.add_move(tree.root, "d4");
        let e5 = tree.add_move(e4, "e5");

        let searches = node_searches(&tree).unwrap();
        let ids: Vec<usize> = searches.iter().map(|s| s.node_id).collect();
        assert_eq!(ids, vec![e4, d4, e5]);

        let by_id = |id: usize| searches.iter().find(|s| s.node_id == id).unwrap();
        // Both first moves start from the standard start position.
        assert!(by_id(e4)
            .fen_before
            .starts_with("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w"));
        assert!(by_id(d4)
            .fen_before
            .starts_with("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w"));
        assert!(by_id(e4)
            .fen_after
            .starts_with("rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b"));
        // e5's before-position is e4's after-position.
        assert_eq!(by_id(e5).fen_before, by_id(e4).fen_after);
        assert!(by_id(e5).fen_after.contains(" w "));

        assert!(by_id(e4).mover_white);
        assert!(!by_id(e5).mover_white);
    }

    fn line(uci: &str, cp: i32) -> Analysis {
        Analysis {
            bestmove: uci.to_string(),
            score: Some(Score::Cp { value: cp }),
            pv: vec![uci.to_string()],
            ..Default::default()
        }
    }

    fn search(fen_before: &str, fen_after: &str, san: &str, mover_white: bool) -> NodeSearch {
        NodeSearch {
            node_id: 1,
            fen_before: fen_before.to_string(),
            fen_after: fen_after.to_string(),
            san: san.to_string(),
            mover_white,
        }
    }

    #[test]
    fn classify_search_flags_the_engines_top_move_as_best() {
        use crate::position::STARTPOS_FEN;
        let fen_after = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq e3 0 1";
        let s = search(STARTPOS_FEN, fen_after, "e4", true);
        let before = vec![line("e2e4", 30), line("d2d4", 20)];
        let (eval, classification, cost) =
            classify_search(&s, &before, Some(Score::Cp { value: -30 }));
        assert_eq!(classification, Classification::Best);
        assert_eq!(eval, Eval::Cp(30));
        assert!(cost.white);
    }

    #[test]
    fn classify_search_flags_a_blunder() {
        use crate::position::STARTPOS_FEN;
        let fen_after = "rnbqkbnr/pppppppp/8/8/P7/8/1PPPPPPP/RNBQKBNR b KQkq a3 0 1";
        // Before: best e4 at +4.0; a4 wasn't among the searched lines.
        let s = search(STARTPOS_FEN, fen_after, "a4", true);
        let before = vec![line("e2e4", 400), line("d2d4", 350)];
        // After a4, Black (to move) is winning by +4.0.
        let (eval, classification, _) =
            classify_search(&s, &before, Some(Score::Cp { value: 400 }));
        assert_eq!(classification, Classification::Blunder);
        assert!(matches!(eval, Eval::Cp(v) if v < 0));
    }

    #[test]
    fn classify_search_handles_a_delivered_mate_with_no_after_search() {
        use crate::position::STARTPOS_FEN;
        // Scholar's mate: replay to get the real before/after FENs, then feed a
        // synthetic before-search (no engine search of the mated position).
        let sans: Vec<String> = ["e4", "e5", "Bc4", "Nc6", "Qh5", "Nf6", "Qxf7#"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let plies = replay(STARTPOS_FEN, &sans, MODE).unwrap();
        let fen_before = plies[plies.len() - 2].fen.clone();
        let fen_after = plies.last().unwrap().fen.clone();
        assert!(features_of_fen(&fen_after).unwrap().checkmate);

        let s = search(&fen_before, &fen_after, "Qxf7#", true);
        let before = vec![line("h5f7", 900)];
        let (eval, classification, _) = classify_search(&s, &before, None);
        assert_eq!(eval, Eval::Mate(1));
        assert_eq!(classification, Classification::Best);
    }

    #[test]
    fn set_quality_nag_replaces_only_the_quality_glyph() {
        let mut tree = MoveTree::new();
        let e4 = tree.add_move(tree.root, "e4");
        tree.add_nag(e4, 1); // pre-existing quality NAG (!)
        tree.add_nag(e4, 14); // positional NAG (=), left alone

        set_quality_nag(&mut tree, e4, Some(2)); // now a mistake (?)

        assert_eq!(tree.nodes[e4].nags, vec![14, 2]);
    }

    #[test]
    fn set_quality_nag_none_clears_without_adding() {
        let mut tree = MoveTree::new();
        let e4 = tree.add_move(tree.root, "e4");
        tree.add_nag(e4, 4);

        set_quality_nag(&mut tree, e4, None);

        assert!(tree.nodes[e4].nags.is_empty());
    }
}
