//! Pure helpers for the non-destructive "Analyse study" pass (issue #162): list
//! every move-bearing node with the FEN it reaches, and flip a side-to-move
//! engine [`Score`] to the White-perspective [`Eval`] that PGN `[%eval]` stores.
//!
//! Both functions are I/O-free — the engine search and persistence live in
//! [`StudyService::analyse_study`](super::StudyService::analyse_study) — so they
//! unit-test without an engine, like `review::assemble`.

use crate::engine::Score;
use crate::pgn_tree::{Eval, MoveTree};
use crate::position::{replay, CastlingMode, PositionError};

/// Studies are standard chess (matches [`super::MODE`]).
const MODE: CastlingMode = CastlingMode::Standard;

/// Every move-bearing node paired with the FEN it reaches — the position *after*
/// that node's move — replaying each node's line from the tree's start position.
/// The root carries no move, so it is skipped: an eval pins to a played move.
pub fn node_fens(tree: &MoveTree) -> Result<Vec<(usize, String)>, PositionError> {
    let start = tree.start_position();
    let mut out = Vec::with_capacity(tree.nodes.len());
    for node in &tree.nodes {
        // The root (and any node) without a move gets no eval.
        if node.san.is_none() {
            continue;
        }
        let Some(line) = tree.line_to(node.id) else {
            continue;
        };
        let fen = match replay(start, &line, MODE)?.last() {
            Some(ply) => ply.fen.clone(),
            None => start.to_string(),
        };
        out.push((node.id, fen));
    }
    Ok(out)
}

/// Flip a side-to-move engine [`Score`] to the White-perspective [`Eval`] PGN
/// `[%eval]` expects. When White is to move the score is already White's view;
/// otherwise negate it (mirrors `review::white_view` / `negate`).
pub fn white_eval(score: Score, white_to_move: bool) -> Eval {
    match score {
        Score::Cp { value } => Eval::Cp(if white_to_move { value } else { -value }),
        Score::Mate { value } => Eval::Mate(if white_to_move { value } else { -value }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_eval_flips_only_for_black_to_move() {
        // White to move: the score is already White's perspective.
        assert_eq!(white_eval(Score::Cp { value: 35 }, true), Eval::Cp(35));
        assert_eq!(white_eval(Score::Mate { value: 3 }, true), Eval::Mate(3));
        // Black to move: negate to White's perspective.
        assert_eq!(white_eval(Score::Cp { value: 35 }, false), Eval::Cp(-35));
        assert_eq!(white_eval(Score::Mate { value: 2 }, false), Eval::Mate(-2));
        // Sign is carried through the flip.
        assert_eq!(white_eval(Score::Cp { value: -120 }, false), Eval::Cp(120));
        assert_eq!(white_eval(Score::Mate { value: -1 }, false), Eval::Mate(1));
    }

    #[test]
    fn node_fens_skips_the_root_and_walks_variations() {
        // 1. e4 (1. d4) e5 — a mainline with one root variation.
        let mut tree = MoveTree::new();
        let e4 = tree.add_move(tree.root, "e4");
        let d4 = tree.add_move(tree.root, "d4");
        let e5 = tree.add_move(e4, "e5");

        let fens = node_fens(&tree).unwrap();
        // The root is skipped; every move-bearing node is present.
        let ids: Vec<usize> = fens.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![e4, d4, e5]);

        // Each FEN is the position reached at that node.
        let by_id = |id: usize| fens.iter().find(|(n, _)| *n == id).map(|(_, f)| f.clone());
        assert!(by_id(e4)
            .unwrap()
            .starts_with("rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b"));
        assert!(by_id(d4)
            .unwrap()
            .starts_with("rnbqkbnr/pppppppp/8/8/3P4/8/PPP1PPPP/RNBQKBNR b"));
        // After 1. e4 e5 it is White to move again.
        assert!(by_id(e5).unwrap().contains(" w "));
    }
}
