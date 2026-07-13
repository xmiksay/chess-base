//! Post-merge transposition annotation (issue #174): walk a tree in
//! mainline-first preorder computing each node's Zobrist hash
//! ([`crate::position`]); when a node's position was already reached earlier in
//! the walk, tag it with a note pointing back at that earlier node instead of
//! letting its continuation quietly duplicate an existing line.
//!
//! Mainline-first preorder means the walk descends into `children[0]` (the
//! current mainline) all the way to a leaf before touching any variation, so a
//! variation that later reaches the same position is the one tagged — matching
//! "transposes to the main line", not the reverse.

use std::collections::HashMap;

use crate::position::{apply_san, replay, CastlingMode};

use super::MoveTree;

/// Studies are standard chess; castling rights parse the normal way.
const MODE: CastlingMode = CastlingMode::Standard;

const TRANSPOSITION_PREFIX: &str = "Transposes to the main line after ";
const TRANSPOSITION_MARKER: &str = " — Transposes to the main line after ";

impl MoveTree {
    /// Tag every node whose position was already reached earlier in a
    /// mainline-first preorder walk of the tree with a note pointing at the
    /// earlier (canonical) node, e.g. `"Transposes to the main line after
    /// 6.O-O"`. Appends to — never clobbers — an existing comment, so a
    /// merge-games stats comment survives alongside the note, and refreshes its
    /// own note on a re-run instead of stacking a duplicate. Returns the number
    /// of nodes tagged.
    pub fn mark_transpositions(&mut self) -> usize {
        let start = self.start_position().to_string();
        let mut seen = HashMap::new();
        let mut marked = 0;
        self.walk_transpositions(self.root, &start, &mut seen, &mut marked);
        marked
    }

    /// Recursive worker: walk `id`'s children (already mainline-first) from the
    /// position `fen`, tagging a collision against `seen` and recursing
    /// regardless — a tagged node's own subtree typically duplicates the
    /// canonical line further, so it keeps getting tagged too.
    fn walk_transpositions(
        &mut self,
        id: usize,
        fen: &str,
        seen: &mut HashMap<u64, usize>,
        marked: &mut usize,
    ) {
        let children = self.nodes[id].children.clone();
        for child in children {
            let Some(san) = self.nodes[child].san.clone() else {
                continue;
            };
            let Ok((after_fen, zobrist)) = apply_san(fen, &san, MODE) else {
                continue;
            };
            match seen.get(&zobrist) {
                Some(&canonical) => {
                    if self.tag_transposition(child, canonical) {
                        *marked += 1;
                    }
                }
                None => {
                    seen.insert(zobrist, child);
                }
            }
            self.walk_transpositions(child, &after_fen, seen, marked);
        }
    }

    /// Append (or refresh) a transposition note on `id` pointing at `canonical`.
    /// Returns `false` — touching nothing — only if `canonical`'s own line can't
    /// be replayed (a corrupt tree, never expected in practice).
    fn tag_transposition(&mut self, id: usize, canonical: usize) -> bool {
        let Some(description) = self.describe_move(canonical) else {
            return false;
        };
        let existing = self.nodes[id].comment.as_deref().unwrap_or("");
        let base = strip_transposition_note(existing);
        let comment = if base.is_empty() {
            format!("{TRANSPOSITION_PREFIX}{description}")
        } else {
            format!("{base}{TRANSPOSITION_MARKER}{description}")
        };
        self.nodes[id].comment = Some(comment);
        true
    }

    /// The last move into `id`, formatted as a PGN move annotation (`"6.O-O"` /
    /// `"5...Nf6"`) using the actual side-to-move/move-number at that point in
    /// this tree's start position — never assuming a standard start.
    fn describe_move(&self, id: usize) -> Option<String> {
        let line = self.line_to(id)?;
        let last = line.last()?.clone();
        let before_fen = match line.len() {
            0 => return None,
            1 => self.start_position().to_string(),
            _ => replay(self.start_position(), &line[..line.len() - 1], MODE)
                .ok()?
                .last()?
                .fen
                .clone(),
        };
        // FEN fields: board, active color, castling, en passant, halfmove,
        // fullmove — skip to the color, then the fullmove three fields later.
        let mut fields = before_fen.split_whitespace().skip(1);
        let color = fields.next()?;
        let fullmove: u32 = fields.nth(3)?.parse().ok()?;
        Some(match color {
            "b" => format!("{fullmove}...{last}"),
            _ => format!("{fullmove}.{last}"),
        })
    }
}

/// Strip a previously-appended transposition note (our own marker) from
/// `comment`, leaving whatever else was there (a user's prose, a merge-games
/// stats comment, or nothing). Lets a re-run refresh the note without stacking a
/// duplicate.
fn strip_transposition_note(comment: &str) -> &str {
    if comment.starts_with(TRANSPOSITION_PREFIX) {
        return "";
    }
    match comment.rfind(TRANSPOSITION_MARKER) {
        Some(idx) => &comment[..idx],
        None => comment,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_a_variation_that_transposes_into_the_mainline() {
        let mut t = MoveTree::new();
        // Mainline: 1.d4 d5 2.c4. Variation: 1.c4 d5 2.d4 — the classic
        // Queen's-pawn/English transposition, same position after move 2.
        let d4 = t.add_move(t.root, "d4");
        let d5 = t.add_move(d4, "d5");
        let mainline_c4 = t.add_move(d5, "c4");

        let c4 = t.add_move(t.root, "c4");
        let v_d5 = t.add_move(c4, "d5");
        let transposed = t.add_move(v_d5, "d4");

        let marked = t.mark_transpositions();
        assert_eq!(marked, 1);
        assert_eq!(
            t.nodes[transposed].comment.as_deref(),
            Some("Transposes to the main line after 2.c4")
        );
        // The mainline node itself is untouched.
        assert!(t.nodes[mainline_c4].comment.is_none());
    }

    #[test]
    fn distinct_lines_are_left_uncommented() {
        let mut t = MoveTree::new();
        let e4 = t.add_move(t.root, "e4");
        t.add_move(e4, "e5");
        let d4 = t.add_move(t.root, "d4");
        t.add_move(d4, "d5");

        assert_eq!(t.mark_transpositions(), 0);
        assert!(t.nodes.iter().all(|n| n.comment.is_none()));
    }

    #[test]
    fn appends_to_an_existing_comment_instead_of_clobbering_it() {
        let mut t = MoveTree::new();
        let d4 = t.add_move(t.root, "d4");
        let d5 = t.add_move(d4, "d5");
        t.add_move(d5, "c4");

        let c4 = t.add_move(t.root, "c4");
        let v_d5 = t.add_move(c4, "d5");
        let transposed = t.add_move(v_d5, "d4");
        t.set_comment(transposed, "3 games, 66% (King's Indian setup)");

        t.mark_transpositions();
        assert_eq!(
            t.nodes[transposed].comment.as_deref(),
            Some("3 games, 66% (King's Indian setup) — Transposes to the main line after 2.c4")
        );
    }

    #[test]
    fn re_running_refreshes_the_note_without_stacking_a_duplicate() {
        let mut t = MoveTree::new();
        let d4 = t.add_move(t.root, "d4");
        let d5 = t.add_move(d4, "d5");
        t.add_move(d5, "c4");

        let c4 = t.add_move(t.root, "c4");
        let v_d5 = t.add_move(c4, "d5");
        let transposed = t.add_move(v_d5, "d4");

        t.mark_transpositions();
        let first_pass = t.nodes[transposed].comment.clone();
        assert_eq!(t.mark_transpositions(), 1);
        assert_eq!(t.nodes[transposed].comment, first_pass);
    }

    #[test]
    fn a_transposed_subtree_keeps_getting_tagged_further_down() {
        let mut t = MoveTree::new();
        let d4 = t.add_move(t.root, "d4");
        let d5 = t.add_move(d4, "d5");
        let mainline_c4 = t.add_move(d5, "c4");
        t.add_move(mainline_c4, "Nf6"); // mainline continues

        let c4 = t.add_move(t.root, "c4");
        let v_d5 = t.add_move(c4, "d5");
        let v_d4 = t.add_move(v_d5, "d4"); // transposes after move 2
        let v_nf6 = t.add_move(v_d4, "Nf6"); // same reply → transposes further too

        let marked = t.mark_transpositions();
        assert_eq!(marked, 2);
        assert!(t.nodes[v_d4]
            .comment
            .as_deref()
            .unwrap()
            .starts_with("Transposes to the main line after 2.c4"));
        assert!(t.nodes[v_nf6]
            .comment
            .as_deref()
            .unwrap()
            .starts_with("Transposes to the main line after 2...Nf6"));
    }

    #[test]
    fn black_to_move_start_formats_the_note_with_an_ellipsis() {
        let mut t = MoveTree::new();
        // A set-up start with Black to move (after 1.e4) — the collision's own
        // last move is Black's, so the note reads "1...c5", not "1.c5".
        t.start_fen =
            Some("rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1".to_string());
        let _canonical = t.add_move(t.root, "c5");
        let dup = t.add_move(t.root, "c5");

        assert_eq!(t.mark_transpositions(), 1);
        assert_eq!(
            t.nodes[dup].comment.as_deref(),
            Some("Transposes to the main line after 1...c5")
        );
    }
}
