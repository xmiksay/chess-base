//! PGN ⇄ [`MoveTree`] import/export with variations, comments and NAGs.
//!
//! Import streams a single game with `pgn-reader` into a [`MoveTree`]; every SAN
//! is validated for legality against [`position`](crate::position) as it is
//! replayed, so malformed PGN yields an error instead of a silently broken tree.
//! Export walks the tree back into standard PGN movetext, re-validating each SAN.
//! Both directions assume the standard start position (set-up `[FEN]` tags are
//! out of scope for the move tree).

use std::io::Cursor;
use std::ops::ControlFlow;

use pgn_reader::{Nag, RawComment, Reader, SanPlus, Skip, Visitor};

use super::MoveTree;
use crate::position::{apply_san, CastlingMode, PositionError, STARTPOS_FEN};

/// Move trees are standard chess; castling rights parse the normal way.
const MODE: CastlingMode = CastlingMode::Standard;

/// Failure parsing or serializing a PGN move tree.
#[derive(Debug, thiserror::Error)]
pub enum PgnError {
    /// The reader hit irrecoverable, malformed input (bad tags, I/O, …).
    #[error("malformed PGN: {0}")]
    Parse(String),
    /// The input contained no game at all.
    #[error("no game found in PGN")]
    Empty,
    /// A move in the PGN (or tree) is illegal in its position.
    #[error(transparent)]
    Position(#[from] PositionError),
    /// A non-root node was missing its SAN (corrupt tree).
    #[error("move-tree node {0} has no SAN")]
    MissingSan(usize),
}

/// Parse the first game in `pgn` into a [`MoveTree`].
///
/// Mainline, nested variations, `{ comments }` and NAGs are preserved. Returns
/// [`PgnError::Empty`] for input with no game and an error (never a panic) for
/// malformed input or illegal moves.
pub fn from_pgn(pgn: &str) -> Result<MoveTree, PgnError> {
    let mut reader = Reader::new(Cursor::new(pgn.as_bytes()));
    match reader.read_game(&mut Importer) {
        Ok(Some(result)) => result,
        Ok(None) => Err(PgnError::Empty),
        Err(e) => Err(PgnError::Parse(e.to_string())),
    }
}

/// Serialize a [`MoveTree`] to standard PGN movetext (no header tags).
///
/// Each SAN is re-validated against [`position`](crate::position) while walking
/// the tree; an illegal move yields [`PgnError::Position`]. Variations nest in
/// `( … )`, comments render as `{ … }` and NAGs as `$N`.
pub fn to_pgn(tree: &MoveTree) -> Result<String, PgnError> {
    let mut out = String::new();
    if let Some(comment) = &tree.nodes[tree.root].comment {
        out.push('{');
        out.push_str(comment);
        out.push('}');
    }
    write_line(tree, tree.root, STARTPOS_FEN, 0, false, &mut out)?;
    separate(&mut out);
    out.push('*');
    Ok(out)
}

// ---- Import ---------------------------------------------------------------

/// Per-game accumulator: the tree under construction plus a parallel FEN per
/// node (so a variation can resume from its parent's position) and a stack of
/// nodes to return to when a variation closes.
struct Build {
    tree: MoveTree,
    /// `fens[id]` is the FEN of the position *after* node `id`'s move; `fens[0]`
    /// is the start position. Kept in lockstep with `tree.nodes`.
    fens: Vec<String>,
    /// Node whose move was played last (where SAN/NAG/comment attach).
    cur: usize,
    /// Saved `cur` for each open variation, restored on `)`.
    stack: Vec<usize>,
}

impl Build {
    fn new() -> Self {
        let tree = MoveTree::new();
        Build {
            cur: tree.root,
            fens: vec![STARTPOS_FEN.to_string()],
            stack: Vec::new(),
            tree,
        }
    }
}

/// Streaming visitor that replays one game into a [`MoveTree`].
struct Importer;

impl Visitor for Importer {
    type Tags = ();
    type Movetext = Build;
    type Output = Result<MoveTree, PgnError>;

    fn begin_tags(&mut self) -> ControlFlow<Self::Output, Self::Tags> {
        ControlFlow::Continue(())
    }

    fn begin_movetext(&mut self, _tags: ()) -> ControlFlow<Self::Output, Self::Movetext> {
        ControlFlow::Continue(Build::new())
    }

    fn san(&mut self, b: &mut Build, san_plus: SanPlus) -> ControlFlow<Self::Output> {
        let san = san_plus.to_string();
        match apply_san(&b.fens[b.cur], &san, MODE) {
            Ok((fen_after, _)) => {
                let id = b.tree.add_move(b.cur, san);
                b.fens.push(fen_after); // id == fens.len() before push
                b.cur = id;
                ControlFlow::Continue(())
            }
            Err(e) => ControlFlow::Break(Err(e.into())),
        }
    }

    fn nag(&mut self, b: &mut Build, nag: Nag) -> ControlFlow<Self::Output> {
        b.tree.nodes[b.cur].nags.push(nag.0);
        ControlFlow::Continue(())
    }

    fn comment(&mut self, b: &mut Build, comment: RawComment<'_>) -> ControlFlow<Self::Output> {
        let text = String::from_utf8_lossy(comment.as_bytes());
        let text = text.trim();
        if !text.is_empty() {
            let slot = &mut b.tree.nodes[b.cur].comment;
            match slot {
                Some(existing) => {
                    existing.push(' ');
                    existing.push_str(text);
                }
                None => *slot = Some(text.to_string()),
            }
        }
        ControlFlow::Continue(())
    }

    fn begin_variation(&mut self, b: &mut Build) -> ControlFlow<Self::Output, Skip> {
        // A variation is an alternative to the move just played: resume from its
        // parent so the first variation move becomes a sibling, not a child.
        b.stack.push(b.cur);
        if let Some(parent) = b.tree.nodes[b.cur].parent {
            b.cur = parent;
        }
        ControlFlow::Continue(Skip(false))
    }

    fn end_variation(&mut self, b: &mut Build) -> ControlFlow<Self::Output> {
        if let Some(prev) = b.stack.pop() {
            b.cur = prev;
        }
        ControlFlow::Continue(())
    }

    fn end_game(&mut self, b: Build) -> Self::Output {
        Ok(b.tree)
    }
}

// ---- Export ---------------------------------------------------------------

/// Append a separating space unless we are at the start or just opened a `(`.
fn separate(out: &mut String) {
    if !matches!(out.chars().last(), None | Some('(')) {
        out.push(' ');
    }
}

/// Write a single move token: its number (when needed), SAN, NAGs and comment.
///
/// `ply` is the number of half-moves already played; white moves always show
/// their number, black moves only when `force_number` is set (line start, or
/// right after a variation or comment).
fn write_move(out: &mut String, ply: usize, san: &str, node: &super::Node, force_number: bool) {
    separate(out);
    let number = ply / 2 + 1;
    if ply.is_multiple_of(2) {
        out.push_str(&format!("{number}. "));
    } else if force_number {
        out.push_str(&format!("{number}... "));
    }
    out.push_str(san);
    for nag in &node.nags {
        out.push_str(&format!(" ${nag}"));
    }
    if let Some(comment) = &node.comment {
        out.push_str(&format!(" {{{comment}}}"));
    }
}

/// Write the continuation of `parent`: its mainline child, any sibling
/// variations, then recurse. `fen` is the position after `parent`'s move and
/// `ply` the half-move count at that position.
fn write_line(
    tree: &MoveTree,
    parent: usize,
    fen: &str,
    ply: usize,
    force_number: bool,
    out: &mut String,
) -> Result<(), PgnError> {
    let children = &tree.nodes[parent].children;
    let Some(&main) = children.first() else {
        return Ok(());
    };

    let main_san = tree.nodes[main]
        .san
        .as_deref()
        .ok_or(PgnError::MissingSan(main))?;
    let (main_fen, _) = apply_san(fen, main_san, MODE)?;
    write_move(out, ply, main_san, &tree.nodes[main], force_number);

    let variations: Vec<usize> = children[1..].to_vec();
    for var in &variations {
        let var = *var;
        let var_san = tree.nodes[var]
            .san
            .as_deref()
            .ok_or(PgnError::MissingSan(var))?;
        let (var_fen, _) = apply_san(fen, var_san, MODE)?;
        separate(out);
        out.push('(');
        write_move(out, ply, var_san, &tree.nodes[var], true);
        write_line(
            tree,
            var,
            &var_fen,
            ply + 1,
            tree.nodes[var].comment.is_some(),
            out,
        )?;
        out.push(')');
    }

    // After a variation or a comment, the next black move must repeat its number.
    let next_force = !variations.is_empty() || tree.nodes[main].comment.is_some();
    write_line(tree, main, &main_fen, ply + 1, next_force, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tree with a mainline, a nested variation, a comment and a NAG, in
    /// the same node order the importer produces (so equality holds on round-trip).
    fn sample_tree() -> MoveTree {
        let mut t = MoveTree::new();
        let e4 = t.add_move(t.root, "e4");
        let e5 = t.add_move(e4, "e5"); // mainline
        let c5 = t.add_move(e4, "c5"); // variation: 1... c5
        t.add_move(c5, "Nf3"); // nested in the variation
        let nf3 = t.add_move(e5, "Nf3"); // mainline continuation
        t.nodes[e5].nags.push(1); // e5!
        t.set_comment(nf3, "develops the knight");
        t
    }

    #[test]
    fn exports_mainline_variation_comment_and_nag() {
        let pgn = to_pgn(&sample_tree()).unwrap();
        assert_eq!(
            pgn,
            "1. e4 e5 $1 (1... c5 2. Nf3) 2. Nf3 {develops the knight} *"
        );
    }

    #[test]
    fn round_trips_tree_to_pgn_and_back() {
        let tree = sample_tree();
        let pgn = to_pgn(&tree).unwrap();
        let back = from_pgn(&pgn).unwrap();
        assert_eq!(tree, back);
    }

    #[test]
    fn imports_nested_variations() {
        let tree = from_pgn("1. e4 e5 (1... c5 2. Nf3 d6) 2. Nf3 *").unwrap();
        assert_eq!(tree.mainline(), vec!["e4", "e5", "Nf3"]);
        // e5's parent (e4) carries the c5 variation as a second child.
        let e4 = tree.nodes[tree.root].children[0];
        assert_eq!(
            tree.nodes[e4].children.len(),
            2,
            "e4 has mainline + variation"
        );
        let c5 = tree.nodes[e4].children[1];
        assert_eq!(tree.nodes[c5].san.as_deref(), Some("c5"));
    }

    #[test]
    fn imports_comments_and_nags() {
        let tree = from_pgn("1. e4 {best by test} e5 $2 *").unwrap();
        let e4 = tree.nodes[tree.root].children[0];
        let e5 = tree.nodes[e4].children[0];
        assert_eq!(tree.nodes[e4].comment.as_deref(), Some("best by test"));
        assert_eq!(tree.nodes[e5].nags, vec![2]);
    }

    #[test]
    fn empty_pgn_errors() {
        assert!(matches!(from_pgn("   \n  "), Err(PgnError::Empty)));
    }

    #[test]
    fn illegal_move_errors_without_panic() {
        // Black cannot answer 1. e4 with a second "e4".
        let err = from_pgn("1. e4 e4 *").unwrap_err();
        assert!(matches!(err, PgnError::Position(_)));
    }

    #[test]
    fn exports_node_without_san_errors() {
        // A child node with no SAN is a corrupt tree, not a panic.
        let mut t = MoveTree::new();
        let id = t.add_move(t.root, "e4");
        t.nodes[id].san = None;
        assert!(matches!(to_pgn(&t), Err(PgnError::MissingSan(_))));
    }
}
