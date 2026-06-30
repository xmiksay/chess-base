//! PGN ⇄ [`MoveTree`] import/export with variations, comments and NAGs.
//!
//! Import streams a single game with `pgn-reader` into a [`MoveTree`]; every SAN
//! is validated for legality against [`position`](crate::position) as it is
//! replayed, so malformed PGN yields an error instead of a silently broken tree.
//! Export walks the tree back into standard PGN movetext, re-validating each SAN.
//! A set-up `[FEN]`/`[SetUp]` header is honoured on import (recorded as the
//! tree's [`MoveTree::start_fen`]) and re-emitted on export, so a study built
//! from a custom origin round-trips (issue #135).

use std::io::Cursor;
use std::ops::ControlFlow;

use pgn_reader::{Nag, RawComment, RawTag, Reader, SanPlus, Skip, Visitor};

use super::{eval, shapes, MoveTree};
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

/// Parse the first game in `pgn` into a [`MoveTree`], replaying from the
/// standard start position.
///
/// Mainline, nested variations, `{ comments }` and NAGs are preserved. Returns
/// [`PgnError::Empty`] for input with no game and an error (never a panic) for
/// malformed input or illegal moves.
pub fn from_pgn(pgn: &str) -> Result<MoveTree, PgnError> {
    read_game(
        pgn,
        Importer {
            explicit_start: None,
        },
    )
}

/// Like [`from_pgn`], but seed the importer from `start_fen`, which **overrides**
/// any `[FEN]` header in the PGN. Used when the origin is known out-of-band (e.g.
/// a repertoire spine whose start comes from the request, not the movetext).
pub fn from_pgn_with_start(pgn: &str, start_fen: &str) -> Result<MoveTree, PgnError> {
    read_game(
        pgn,
        Importer {
            explicit_start: Some(start_fen.to_string()),
        },
    )
}

/// Read the first game of `pgn` with `importer`, mapping the reader's outcomes
/// onto [`PgnError`] (empty input / malformed PGN) without ever panicking.
fn read_game(pgn: &str, mut importer: Importer) -> Result<MoveTree, PgnError> {
    let mut reader = Reader::new(Cursor::new(pgn.as_bytes()));
    match reader.read_game(&mut importer) {
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
    // A set-up origin is re-emitted as `[SetUp]`/`[FEN]` so the export is
    // self-contained and re-importable; standard-start trees stay header-free.
    if let Some(fen) = &tree.start_fen {
        push_header_tag(&mut out, "SetUp", "1");
        push_header_tag(&mut out, "FEN", fen);
        out.push('\n');
    }
    out.push_str(&movetext(tree)?);
    Ok(out)
}

/// The numbered movetext alone (no header tags), replayed from the tree's start
/// position. Shared by [`to_pgn`] and the Lichess-study export so a set-up
/// position's move numbering and per-SAN legality validation use one path.
pub(crate) fn movetext(tree: &MoveTree) -> Result<String, PgnError> {
    let mut out = String::new();
    if let Some(comment) = &tree.nodes[tree.root].comment {
        out.push('{');
        out.push_str(comment);
        out.push('}');
    }
    let start = tree.start_position();
    let (ply, force) = start_ply_and_force(start);
    write_line(tree, tree.root, start, ply, force, &mut out)?;
    separate(&mut out);
    out.push('*');
    Ok(out)
}

/// Half-move count and whether Black is to move at `fen` — the seed for move
/// numbering when a tree starts from a set-up position. Falls back to a
/// white-to-move ply-0 start for a malformed FEN (the SANs still re-validate on
/// the way out, so a bad origin surfaces as a move error, never silently).
fn start_ply_and_force(fen: &str) -> (usize, bool) {
    let fields: Vec<&str> = fen.split_whitespace().collect();
    let black = fields.get(1) == Some(&"b");
    let fullmove: usize = fields.get(5).and_then(|s| s.parse().ok()).unwrap_or(1);
    ((fullmove.max(1) - 1) * 2 + black as usize, black)
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
    fn new(start_fen: &str) -> Self {
        let mut tree = MoveTree::new();
        // Record a non-standard origin on the tree so every later replay (export,
        // study editing, the SPA board) seeds from it; the standard start stays
        // `None` so existing studies are byte-for-byte unchanged.
        if start_fen != STARTPOS_FEN {
            tree.start_fen = Some(start_fen.to_string());
        }
        Build {
            cur: tree.root,
            fens: vec![start_fen.to_string()],
            stack: Vec::new(),
            tree,
        }
    }
}

/// Streaming visitor that replays one game into a [`MoveTree`], seeding the
/// first position from (in priority) an explicit caller origin, the game's
/// `[FEN]` header, or the standard start position.
struct Importer {
    /// An explicit start position supplied by the caller ([`from_pgn_with_start`]);
    /// when set it overrides any `[FEN]` header. `None` for [`from_pgn`], which
    /// then honours the header (or the standard start when absent).
    explicit_start: Option<String>,
}

impl Visitor for Importer {
    /// The captured `[FEN]` header value, if the game carried one.
    type Tags = Option<String>;
    type Movetext = Build;
    type Output = Result<MoveTree, PgnError>;

    fn begin_tags(&mut self) -> ControlFlow<Self::Output, Self::Tags> {
        ControlFlow::Continue(None)
    }

    fn tag(
        &mut self,
        tags: &mut Self::Tags,
        name: &[u8],
        value: RawTag<'_>,
    ) -> ControlFlow<Self::Output> {
        if name == b"FEN" {
            *tags = Some(value.decode_utf8_lossy().to_string());
        }
        ControlFlow::Continue(())
    }

    fn begin_movetext(&mut self, fen_tag: Self::Tags) -> ControlFlow<Self::Output, Self::Movetext> {
        // Explicit caller origin wins; otherwise the `[FEN]` header; else startpos.
        let start = self
            .explicit_start
            .take()
            .or(fen_tag)
            .unwrap_or_else(|| STARTPOS_FEN.to_string());
        ControlFlow::Continue(Build::new(&start))
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
        let raw = String::from_utf8_lossy(comment.as_bytes());
        // Pull any `[%csl]`/`[%cal]` shape commands out into the node's shapes,
        // then the `[%eval …]` command into its evaluation, leaving the free text
        // (including a preserved `[%clk …]`) as the comment.
        let (parsed, text) = shapes::parse(&raw);
        let (parsed_eval, text) = eval::parse(&text);
        let node = &mut b.tree.nodes[b.cur];
        node.shapes.extend(parsed);
        if let Some(parsed_eval) = parsed_eval {
            node.eval = Some(parsed_eval);
        }
        if !text.is_empty() {
            match &mut node.comment {
                Some(existing) => {
                    existing.push(' ');
                    existing.push_str(&text);
                }
                None => node.comment = Some(text),
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

/// Append a single `[Key "Value"]` PGN header tag (value escaped) plus a
/// newline. Shared by the Lichess-study and game exports so the tag escaping
/// lives in one place (issue #120).
pub(crate) fn push_header_tag(out: &mut String, key: &str, value: &str) {
    let value = value.replace('\\', "\\\\").replace('"', "\\\"");
    out.push_str(&format!("[{key} \"{value}\"]\n"));
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
    // The evaluation and shapes serialize as Lichess `[%eval]`/`[%csl]`/`[%cal]`
    // commands at the head of the comment, so a node carrying only an eval (or
    // only shapes) still emits a `{ … }` block.
    let mut commands = String::new();
    if let Some(eval) = &node.eval {
        commands.push_str(&eval::encode(eval));
    }
    commands.push_str(&shapes::encode(&node.shapes));
    match (commands.is_empty(), &node.comment) {
        (true, None) => {}
        (false, None) => out.push_str(&format!(" {{{commands}}}")),
        (true, Some(comment)) => out.push_str(&format!(" {{{comment}}}")),
        (false, Some(comment)) => out.push_str(&format!(" {{{commands} {comment}}}")),
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
    fn imports_from_a_non_startpos_origin() {
        // A SetUp endgame: White queen on d1, kings on e1/e8. `Qd8+` is legal
        // here but not from the standard start, so the seed must be honoured.
        let fen = "4k3/8/8/8/8/8/8/3QK3 w - - 0 1";
        let tree = from_pgn_with_start("1. Qd8+ *", fen).unwrap();
        assert_eq!(tree.mainline(), vec!["Qd8+"]);
        assert_eq!(tree.start_fen.as_deref(), Some(fen));
        // The same movetext is illegal from the standard start position.
        assert!(matches!(from_pgn("1. Qd8+ *"), Err(PgnError::Position(_))));
    }

    #[test]
    fn from_pgn_honours_a_setup_fen_header() {
        // The Catalan after 1.d4 Nf6 2.c4 e6 3.g3, Black to move. `d5` is illegal
        // from the standard start, so plain from_pgn must read the header to
        // replay it — the bug that motivated issue #135 on the study import path.
        let fen = "rnbqkb1r/pppp1ppp/4pn2/8/2PP4/6P1/PP2PP1P/RNBQKBNR b KQkq - 0 3";
        let pgn = format!("[SetUp \"1\"]\n[FEN \"{fen}\"]\n\n3... d5 4. Bg2 *");
        let tree = from_pgn(&pgn).unwrap();
        assert_eq!(tree.start_fen.as_deref(), Some(fen));
        assert_eq!(tree.mainline(), vec!["d5", "Bg2"]);
    }

    #[test]
    fn round_trips_a_setup_position_with_correct_numbering() {
        let fen = "rnbqkb1r/pppp1ppp/4pn2/8/2PP4/6P1/PP2PP1P/RNBQKBNR b KQkq - 0 3";
        let tree = from_pgn_with_start("3... d5 4. Bg2 *", fen).unwrap();
        let pgn = to_pgn(&tree).unwrap();
        // Self-contained: re-emits the origin and numbers from the FEN's move 3.
        assert!(pgn.contains(&format!("[FEN \"{fen}\"]")));
        assert!(pgn.contains("[SetUp \"1\"]"));
        assert!(pgn.contains("3... d5"));
        assert!(pgn.contains("4. Bg2"));
        // …and the export re-imports to an identical tree.
        assert_eq!(from_pgn(&pgn).unwrap(), tree);
    }

    #[test]
    fn explicit_start_overrides_the_header() {
        // A header FEN is present, but the caller's explicit origin wins.
        let header_fen = "4k3/8/8/8/8/8/8/3QK3 w - - 0 1";
        let pgn = format!("[SetUp \"1\"]\n[FEN \"{header_fen}\"]\n\n1. e4 *");
        let tree = from_pgn_with_start(&pgn, STARTPOS_FEN).unwrap();
        // Explicit STARTPOS ⇒ no recorded origin, and `e4` is legal from startpos.
        assert_eq!(tree.start_fen, None);
        assert_eq!(tree.mainline(), vec!["e4"]);
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
    fn exports_and_reimports_pinned_shapes() {
        use super::super::Shape;
        let mut t = MoveTree::new();
        let e4 = t.add_move(t.root, "e4");
        t.set_shapes(
            e4,
            vec![
                Shape {
                    orig: "e4".into(),
                    dest: None,
                    brush: "green".into(),
                },
                Shape {
                    orig: "g1".into(),
                    dest: Some("f3".into()),
                    brush: "red".into(),
                },
            ],
        );
        t.set_comment(e4, "grabs the centre");

        let pgn = to_pgn(&t).unwrap();
        assert_eq!(pgn, "1. e4 {[%csl Ge4][%cal Rg1f3] grabs the centre} *");
        assert_eq!(from_pgn(&pgn).unwrap(), t);
    }

    #[test]
    fn exports_and_reimports_an_eval_with_a_nag_and_comment() {
        use super::super::Eval;
        let mut t = MoveTree::new();
        let e4 = t.add_move(t.root, "e4");
        let d5 = t.add_move(e4, "d5"); // a dubious reply
        t.set_eval(e4, Eval::Cp(31));
        t.set_eval(d5, Eval::Cp(95));
        t.add_nag(d5, 6); // ?!
        t.set_comment(d5, "loosens the centre");

        let pgn = to_pgn(&t).unwrap();
        assert_eq!(
            pgn,
            "1. e4 {[%eval 0.31]} d5 $6 {[%eval 0.95] loosens the centre} *"
        );
        // export → re-import → equal tree (the round-trip the issue asks for).
        assert_eq!(from_pgn(&pgn).unwrap(), t);
    }

    #[test]
    fn exports_a_mate_eval_and_preserves_a_clk_tag() {
        use super::super::Eval;
        let mut t = MoveTree::new();
        let e4 = t.add_move(t.root, "e4");
        t.set_eval(e4, Eval::Mate(3));
        // A `[%clk]` is not an eval/shape: it stays in the comment text verbatim.
        t.set_comment(e4, "[%clk 0:05:00]");

        let pgn = to_pgn(&t).unwrap();
        assert_eq!(pgn, "1. e4 {[%eval #3] [%clk 0:05:00]} *");
        let back = from_pgn(&pgn).unwrap();
        assert_eq!(back.nodes[1].eval, Some(Eval::Mate(3)));
        assert_eq!(back.nodes[1].comment.as_deref(), Some("[%clk 0:05:00]"));
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
