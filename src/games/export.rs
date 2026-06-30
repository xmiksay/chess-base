//! Extended-PGN export for stored games (issue #120): turn a game's mainline —
//! and, optionally, the #119 engine review — into a [`MoveTree`] the shared
//! [`pgn`](crate::pgn_tree::pgn) serializer renders. One emitter for games and
//! studies; no second PGN writer.
//!
//! Pure and I/O-free: it consumes the already-computed [`GameReview`] facts and
//! the game's header metadata, so it is unit-tested without an engine or DB.

use crate::games::GameDetail;
use crate::pgn_tree::pgn::{self, push_header_tag, PgnError};
use crate::pgn_tree::{Eval, MoveTree};
use crate::review::{GameReview, MoveReview};

/// Build a linear [`MoveTree`] from a game's mainline SANs, with no annotations.
pub fn linear_tree(sans: &[String]) -> MoveTree {
    let mut tree = MoveTree::new();
    let mut cur = tree.root;
    for san in sans {
        cur = tree.add_move(cur, san);
    }
    tree
}

/// Build the annotated tree: the mainline plus, per ply, the engine eval, a
/// move-quality NAG and the rule-based "why" note on the notable moves
/// (great / inaccuracy / mistake / blunder — the ones [`Classification::nag`]
/// flags). Best/good moves carry only their eval, so the comment stream stays
/// about the moves that matter.
///
/// [`Classification::nag`]: crate::review::Classification::nag
pub fn annotated_tree(sans: &[String], review: &GameReview) -> MoveTree {
    let mut tree = MoveTree::new();
    let mut cur = tree.root;
    let mut ids = Vec::with_capacity(sans.len());
    for san in sans {
        cur = tree.add_move(cur, san);
        ids.push(cur);
    }
    for (mv, &id) in review.moves.iter().zip(&ids) {
        tree.set_eval(id, eval_of(mv));
        if let Some(nag) = mv.classification.nag() {
            tree.add_nag(id, nag);
            tree.set_comment(id, mv.explanation.clone());
        }
    }
    tree
}

/// White-perspective [`Eval`] for a reviewed move (`eval_cp`/`mate` already are
/// White's perspective in [`MoveReview`]).
fn eval_of(mv: &MoveReview) -> Eval {
    match mv.mate {
        Some(mate) => Eval::Mate(mate),
        None => Eval::Cp(mv.eval_cp),
    }
}

/// Serialize a game as a self-contained `.pgn`: a Seven-Tag-Roster-style header
/// built from the game's metadata, then the annotated movetext. Used for the
/// `annotated=true` download so the eval-bearing game re-imports cleanly.
pub fn to_annotated_pgn(game: &GameDetail, tree: &MoveTree) -> Result<String, PgnError> {
    let mut out = headers(game);
    out.push('\n');
    out.push_str(&pgn::to_pgn(tree)?);
    out.push('\n');
    Ok(out)
}

/// PGN header tags from a game's stored metadata (issue #120). Missing values
/// fall back to the PGN placeholders (`?`, `????.??.??`, `*`); `Variant` is only
/// emitted for non-standard games so a normal game's header stays clean.
fn headers(game: &GameDetail) -> String {
    let mut out = String::new();
    push_header_tag(&mut out, "Event", "chess-base export");
    push_header_tag(&mut out, "Site", game.site.as_deref().unwrap_or("?"));
    push_header_tag(
        &mut out,
        "Date",
        game.date.as_deref().unwrap_or("????.??.??"),
    );
    push_header_tag(&mut out, "Round", game.round.as_deref().unwrap_or("?"));
    push_header_tag(&mut out, "White", game.white.as_deref().unwrap_or("?"));
    push_header_tag(&mut out, "Black", game.black.as_deref().unwrap_or("?"));
    push_header_tag(&mut out, "Result", game.result.as_deref().unwrap_or("*"));
    if let Some(eco) = game.eco.as_deref() {
        push_header_tag(&mut out, "ECO", eco);
    }
    if !game.variant.eq_ignore_ascii_case("standard") {
        push_header_tag(&mut out, "Variant", &game.variant);
    }
    push_header_tag(&mut out, "Annotator", "chess-base");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pgn_tree::pgn::{from_pgn, to_pgn};
    use crate::review::{Classification, ReviewSummary, SideSummary};

    fn sans(moves: &[&str]) -> Vec<String> {
        moves.iter().map(|s| s.to_string()).collect()
    }

    fn move_review(
        ply: usize,
        san: &str,
        eval_cp: i32,
        mate: Option<i32>,
        classification: Classification,
        explanation: &str,
    ) -> MoveReview {
        MoveReview {
            ply,
            san: san.to_string(),
            eval_cp,
            mate,
            best_move: None,
            best_line: Vec::new(),
            played_rank: None,
            classification,
            explanation: explanation.to_string(),
        }
    }

    fn empty_summary() -> ReviewSummary {
        let side = SideSummary {
            acpl: 0,
            accuracy: 100.0,
            inaccuracies: 0,
            mistakes: 0,
            blunders: 0,
        };
        ReviewSummary {
            white: side.clone(),
            black: side,
        }
    }

    #[test]
    fn linear_tree_has_one_node_per_move() {
        let tree = linear_tree(&sans(&["e4", "e5", "Nf3"]));
        assert_eq!(tree.mainline(), vec!["e4", "e5", "Nf3"]);
        assert!(tree.nodes.iter().all(|n| n.eval.is_none()));
        assert_eq!(to_pgn(&tree).unwrap(), "1. e4 e5 2. Nf3 *");
    }

    #[test]
    fn annotated_tree_attaches_eval_to_all_and_notes_to_faults() {
        let moves = vec![
            move_review(1, "e4", 30, None, Classification::Best, "Best move."),
            move_review(
                2,
                "f6",
                -90,
                None,
                Classification::Mistake,
                "−1.2: best was e5.",
            ),
        ];
        let review = GameReview {
            start_fen: crate::position::STARTPOS_FEN.to_string(),
            moves,
            summary: empty_summary(),
        };
        let tree = annotated_tree(&sans(&["e4", "f6"]), &review);

        // Every move carries its eval…
        assert_eq!(tree.nodes[1].eval, Some(Eval::Cp(30)));
        assert_eq!(tree.nodes[2].eval, Some(Eval::Cp(-90)));
        // …but only the fault gets a NAG + why-note.
        assert!(tree.nodes[1].nags.is_empty());
        assert!(tree.nodes[1].comment.is_none());
        assert_eq!(tree.nodes[2].nags, vec![2]); // ? = mistake
        assert_eq!(tree.nodes[2].comment.as_deref(), Some("−1.2: best was e5."));

        // The annotated movetext round-trips back to an equal tree.
        let pgn = to_pgn(&tree).unwrap();
        assert_eq!(from_pgn(&pgn).unwrap(), tree);
    }

    #[test]
    fn annotated_tree_maps_a_mate_score() {
        let moves = vec![move_review(
            1,
            "Qxf7#",
            1000,
            Some(1),
            Classification::Best,
            "Best move.",
        )];
        let review = GameReview {
            start_fen: crate::position::STARTPOS_FEN.to_string(),
            moves,
            summary: empty_summary(),
        };
        let tree = annotated_tree(&sans(&["e4"]), &review); // san irrelevant here
        assert_eq!(tree.nodes[1].eval, Some(Eval::Mate(1)));
    }

    #[test]
    fn headers_emit_metadata_and_skip_standard_variant() {
        let game = GameDetail {
            id: 7,
            database_id: 1,
            white: Some("Spassky".into()),
            black: Some("Fischer".into()),
            site: Some("Reykjavik".into()),
            round: Some("6".into()),
            date: Some("1972.07.23".into()),
            result: Some("1-0".into()),
            eco: Some("D59".into()),
            white_elo: None,
            black_elo: None,
            variant: "standard".into(),
            start_fen: None,
            ply_count: None,
            pgn: Some("1. c4 *".into()),
        };
        let pgn = to_annotated_pgn(&game, &linear_tree(&sans(&["c4"]))).unwrap();
        assert!(pgn.contains("[White \"Spassky\"]"));
        assert!(pgn.contains("[Black \"Fischer\"]"));
        assert!(pgn.contains("[Result \"1-0\"]"));
        assert!(pgn.contains("[ECO \"D59\"]"));
        assert!(!pgn.contains("[Variant"), "standard variant is omitted");
        assert!(pgn.trim_end().ends_with("1. c4 *"));
    }
}
