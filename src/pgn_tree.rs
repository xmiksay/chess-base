//! Pure move-tree model for studies: a PGN with variations, comments and NAGs.
//!
//! Storage-agnostic and I/O-free. A study is an arena of [`Node`]s; the mainline
//! is the first child at each step, variations are the remaining children.

use serde::{Deserialize, Serialize};

use crate::position::{apply_san, replay, CastlingMode, STARTPOS_FEN};

/// Studies are standard chess; castling rights parse the normal way (mirrors
/// [`crate::studies`]). The graft validates moves against this mode.
const MODE: CastlingMode = CastlingMode::Standard;

pub mod eval;
pub mod lichess;
pub mod merge;
pub mod pgn;
pub mod shapes;

/// An engine evaluation pinned to a node, serialized as a `[%eval …]` command in
/// the PGN comment (issue #120). Always from **White's** perspective, the PGN
/// `[%eval]` convention, so a re-import (here or into Lichess/ChessBase) reads
/// the same number back.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Eval {
    /// Centipawns from White's perspective (`[%eval 0.27]`).
    Cp(i32),
    /// Forced mate in this many moves; sign is White's perspective (`+` White
    /// mates, `−` Black mates — `[%eval #3]` / `[%eval #-2]`).
    Mate(i32),
}

/// A board annotation pinned to a node: an arrow or square highlight mirroring
/// the chessground shape model (`{ orig, dest?, brush }`) so it round-trips
/// straight to the board. A pinned [`crate::plans::Plan`] becomes a `Vec<Shape>`
/// (issue #61). `dest` is `None` for a single-square highlight, `Some` for an
/// arrow from `orig` to `dest`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Shape {
    pub orig: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest: Option<String>,
    pub brush: String,
}

/// Whether a NAG is a move-quality glyph ($1–$6: !, ?, !!, ??, !?, ?!). These
/// are mutually exclusive — a move carries at most one.
fn is_move_quality_nag(nag: u8) -> bool {
    (1..=6).contains(&nag)
}

/// SAN without its trailing check/mate marker, so `Qh5+` matches a generated
/// `Qh5` when deduping a graft.
fn san_core(san: &str) -> &str {
    san.trim_end_matches(['+', '#'])
}

/// A single node in a study move tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub id: usize,
    pub parent: Option<usize>,
    /// SAN of the move leading into this node (`None` for the root).
    pub san: Option<String>,
    /// Free-text annotation attached after the move.
    pub comment: Option<String>,
    /// Numeric Annotation Glyphs (e.g. 1 = `!`, 2 = `?`).
    pub nags: Vec<u8>,
    /// Pinned board shapes (arrows / highlights) rendered on the position at this
    /// node. `serde(default)` keeps pre-#61 `tree_json` rows (no `shapes` key)
    /// deserializing — the tree is a JSON blob, so there is no DB migration.
    #[serde(default)]
    pub shapes: Vec<Shape>,
    /// Engine evaluation after this move (issue #120), emitted as `[%eval …]`.
    /// `serde(default)`/`skip` keeps pre-#120 `tree_json` rows loading and omits
    /// the key entirely for the (common) unevaluated node — no DB migration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval: Option<Eval>,
    /// Child node ids; `children[0]` is the mainline continuation.
    pub children: Vec<usize>,
}

/// Why a structural edit (promote / reorder / delete) could not be applied.
/// Pure and transport-agnostic; the study service maps it onto its own error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TreeError {
    /// `id` is not a node in this tree.
    #[error("node {0} not found")]
    NoSuchNode(usize),
    /// The root carries no move and cannot be reordered or deleted.
    #[error("the root node has no parent")]
    NoParent(usize),
}

/// An arena-allocated move tree. Node ids are indices into `nodes`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MoveTree {
    pub nodes: Vec<Node>,
    pub root: usize,
    /// Set-up start position (`[FEN]`) the moves replay from, when it is not the
    /// standard start. `None` ⇒ the standard initial position (issue #135);
    /// `serde(default)`/`skip` keeps pre-existing `tree_json` rows loading and
    /// omits the key for the common standard-start study — no DB migration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_fen: Option<String>,
}

impl Default for MoveTree {
    fn default() -> Self {
        Self::new()
    }
}

impl MoveTree {
    /// Create a tree containing only the (move-less) root node.
    pub fn new() -> Self {
        let root = Node {
            id: 0,
            parent: None,
            san: None,
            comment: None,
            nags: Vec::new(),
            shapes: Vec::new(),
            eval: None,
            children: Vec::new(),
        };
        MoveTree {
            nodes: vec![root],
            root: 0,
            start_fen: None,
        }
    }

    /// The position the tree replays from: the set-up `start_fen` when present,
    /// else the standard start position. Single source of truth for every replay.
    pub fn start_position(&self) -> &str {
        self.start_fen.as_deref().unwrap_or(STARTPOS_FEN)
    }

    /// Append a move as a child of `parent`, returning the new node id.
    ///
    /// The first child added to a node is its mainline; later children are
    /// variations.
    pub fn add_move(&mut self, parent: usize, san: impl Into<String>) -> usize {
        let id = self.nodes.len();
        self.nodes.push(Node {
            id,
            parent: Some(parent),
            san: Some(san.into()),
            comment: None,
            nags: Vec::new(),
            shapes: Vec::new(),
            eval: None,
            children: Vec::new(),
        });
        self.nodes[parent].children.push(id);
        id
    }

    /// Attach a comment to a node.
    pub fn set_comment(&mut self, id: usize, comment: impl Into<String>) {
        self.nodes[id].comment = Some(comment.into());
    }

    /// Replace the pinned board shapes on a node (an empty vec clears them).
    pub fn set_shapes(&mut self, id: usize, shapes: Vec<Shape>) {
        self.nodes[id].shapes = shapes;
    }

    /// Attach an engine evaluation (White's perspective) to a node, emitted as
    /// `[%eval …]` on export (issue #120).
    pub fn set_eval(&mut self, id: usize, eval: Eval) {
        self.nodes[id].eval = Some(eval);
    }

    /// Append a Numeric Annotation Glyph to a node (used when building a tree).
    pub fn add_nag(&mut self, id: usize, nag: u8) {
        self.nodes[id].nags.push(nag);
    }

    /// Toggle a NAG on a node for interactive editing: remove it if already
    /// present, otherwise add it. Adding a move-quality glyph ($1–$6: !, ?, !!,
    /// ??, !?, ?!) first clears any other move-quality glyph, so a move never
    /// carries two contradictory assessments (matches the editor's single-select
    /// quality buttons). Positional NAGs stack freely.
    pub fn toggle_nag(&mut self, id: usize, nag: u8) {
        let nags = &mut self.nodes[id].nags;
        if let Some(pos) = nags.iter().position(|&n| n == nag) {
            nags.remove(pos);
        } else {
            if is_move_quality_nag(nag) {
                nags.retain(|&n| !is_move_quality_nag(n));
            }
            nags.push(nag);
        }
    }

    /// SAN moves from the root down to `id` (inclusive), or `None` if `id` is not
    /// a node in this tree. Lets a caller replay to a node's position to validate
    /// the next move. The root contributes no SAN, so the root yields `Some([])`.
    pub fn line_to(&self, id: usize) -> Option<Vec<String>> {
        let mut node = self.nodes.get(id)?;
        let mut sans = Vec::new();
        loop {
            if let Some(san) = &node.san {
                sans.push(san.clone());
            }
            match node.parent {
                Some(p) => node = &self.nodes[p],
                None => break,
            }
        }
        sans.reverse();
        Some(sans)
    }

    /// The mainline as a sequence of SAN strings, from the root.
    pub fn mainline(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = self.root;
        while let Some(&next) = self.nodes[cur].children.first() {
            if let Some(san) = &self.nodes[next].san {
                out.push(san.clone());
            }
            cur = next;
        }
        out
    }

    /// Move `id` to position `index` among its siblings (its parent's children).
    /// Index 0 is the mainline continuation, later indices are variations; the
    /// index is clamped to the sibling count. Errors if `id` is absent or the
    /// root (which has no siblings).
    pub fn reorder(&mut self, id: usize, index: usize) -> Result<(), TreeError> {
        let parent = self.parent_of(id)?;
        let children = &mut self.nodes[parent].children;
        // `parent_of` proved `id` is a child of `parent`, so this never fails.
        let cur = children
            .iter()
            .position(|&c| c == id)
            .ok_or(TreeError::NoSuchNode(id))?;
        children.remove(cur);
        let dest = index.min(children.len());
        children.insert(dest, id);
        Ok(())
    }

    /// Promote a variation to the mainline: move it to the front of its parent's
    /// child list. Shorthand for `reorder(id, 0)`.
    pub fn promote(&mut self, id: usize) -> Result<(), TreeError> {
        self.reorder(id, 0)
    }

    /// Delete `id` and its whole subtree, detaching it from its parent. Node ids
    /// are arena indices, so the surviving tree is rebuilt and **reindexed**
    /// compactly — callers must reload to learn the new ids. Errors if `id` is
    /// absent or the root.
    pub fn delete(&mut self, id: usize) -> Result<(), TreeError> {
        let parent = self.parent_of(id)?;
        self.nodes[parent].children.retain(|&c| c != id);
        // The detached subtree is now unreachable from the root, so rebuilding
        // from the root naturally drops it.
        self.compact();
        Ok(())
    }

    /// Graft another tree's moves into this one at node `at`, as deduped
    /// variations, returning the count of **newly added** nodes (issue: danger-map
    /// merge, ADR-0032). Walks `src` from its root; for each move it follows an
    /// existing child with the same SAN when present (so a re-graft adds nothing),
    /// else appends a new child (a variation). Each move is validated for legality
    /// in the running position — an illegal or unparseable move (and its subtree)
    /// is skipped, never panicking. An unknown `at` or a corrupt line to it grafts
    /// nothing.
    pub fn graft_subtree(&mut self, at: usize, src: &MoveTree) -> usize {
        let Some(line) = self.line_to(at) else {
            return 0;
        };
        let Ok(plies) = replay(self.start_position(), &line, MODE) else {
            return 0;
        };
        let fen = plies
            .last()
            .map(|p| p.fen.clone())
            .unwrap_or_else(|| self.start_position().to_string());
        self.graft_children(at, &fen, src, src.root)
    }

    /// Recursive worker for [`graft_subtree`](Self::graft_subtree): graft the
    /// children of `src_id` (in `src`) under `dst` (whose position is `dst_fen`).
    fn graft_children(
        &mut self,
        dst: usize,
        dst_fen: &str,
        src: &MoveTree,
        src_id: usize,
    ) -> usize {
        let Some(src_node) = src.nodes.get(src_id) else {
            return 0;
        };
        let mut added = 0;
        for &child in &src_node.children.clone() {
            let Some(san) = src.nodes.get(child).and_then(|n| n.san.clone()) else {
                continue;
            };
            // Validate the move in the current position; skip illegal/unparseable
            // moves (and their subtree) so a bad source never corrupts the tree.
            let Ok((after_fen, _)) = apply_san(dst_fen, &san, MODE) else {
                continue;
            };
            let target = match self.child_by_san(dst, &san) {
                Some(existing) => existing,
                None => {
                    added += 1;
                    self.add_move(dst, san)
                }
            };
            added += self.graft_children(target, &after_fen, src, child);
        }
        added
    }

    /// The first child of `parent` whose move matches `san` (ignoring a trailing
    /// check/mate marker), if any. Used to dedup the graft onto existing lines.
    fn child_by_san(&self, parent: usize, san: &str) -> Option<usize> {
        self.nodes.get(parent)?.children.iter().copied().find(|&c| {
            self.nodes
                .get(c)
                .and_then(|n| n.san.as_deref())
                .map(san_core)
                == Some(san_core(san))
        })
    }

    /// The parent of `id`, erroring if `id` is absent (`NoSuchNode`) or the root
    /// (`NoParent`). Shared precondition check for the structural edits.
    fn parent_of(&self, id: usize) -> Result<usize, TreeError> {
        let node = self.nodes.get(id).ok_or(TreeError::NoSuchNode(id))?;
        node.parent.ok_or(TreeError::NoParent(id))
    }

    /// Rebuild the arena from the root in preorder, reassigning ids so they stay
    /// dense indices after a deletion. Any node unreachable from the root is
    /// dropped.
    fn compact(&mut self) {
        fn visit(old: &[Node], old_id: usize, parent: Option<usize>, out: &mut Vec<Node>) {
            let new_id = out.len();
            let src = &old[old_id];
            out.push(Node {
                id: new_id,
                parent,
                san: src.san.clone(),
                comment: src.comment.clone(),
                nags: src.nags.clone(),
                shapes: src.shapes.clone(),
                eval: src.eval,
                children: Vec::with_capacity(src.children.len()),
            });
            for &child in &src.children {
                let child_id = out.len();
                out[new_id].children.push(child_id);
                visit(old, child, Some(new_id), out);
            }
        }
        let mut rebuilt = Vec::with_capacity(self.nodes.len());
        visit(&self.nodes, self.root, None, &mut rebuilt);
        self.nodes = rebuilt;
        self.root = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_mainline_with_a_variation() {
        let mut t = MoveTree::new();
        let e4 = t.add_move(t.root, "e4");
        let c5 = t.add_move(e4, "c5"); // mainline: Sicilian
        let _e5 = t.add_move(e4, "e5"); // variation: Open Game
        let nf3 = t.add_move(c5, "Nf3");
        t.set_comment(nf3, "Open Sicilian");

        assert_eq!(t.mainline(), vec!["e4", "c5", "Nf3"]);
        assert_eq!(t.nodes[e4].children.len(), 2, "e4 has a variation");
        assert_eq!(t.nodes[nf3].comment.as_deref(), Some("Open Sicilian"));
    }

    #[test]
    fn line_to_collects_sans_along_a_variation() {
        let mut t = MoveTree::new();
        let e4 = t.add_move(t.root, "e4");
        let c5 = t.add_move(e4, "c5");
        let _e5 = t.add_move(e4, "e5"); // sibling variation, off the c5 line
        let nf3 = t.add_move(c5, "Nf3");

        assert_eq!(t.line_to(t.root), Some(vec![]));
        assert_eq!(
            t.line_to(nf3),
            Some(vec!["e4".into(), "c5".into(), "Nf3".into()])
        );
        assert_eq!(t.line_to(999), None);
    }

    #[test]
    fn add_nag_appends_glyphs() {
        let mut t = MoveTree::new();
        let e4 = t.add_move(t.root, "e4");
        t.add_nag(e4, 1);
        t.add_nag(e4, 22);
        assert_eq!(t.nodes[e4].nags, vec![1, 22]);
    }

    #[test]
    fn toggle_nag_adds_removes_and_replaces() {
        let mut t = MoveTree::new();
        let e4 = t.add_move(t.root, "e4");

        // First click adds; re-clicking the same glyph removes it.
        t.toggle_nag(e4, 1);
        assert_eq!(t.nodes[e4].nags, vec![1]);
        t.toggle_nag(e4, 1);
        assert_eq!(t.nodes[e4].nags, Vec::<u8>::new());

        // Move-quality glyphs ($1–$6) are mutually exclusive: a new one replaces
        // the old instead of stacking (the f5!!??!?… bug).
        t.toggle_nag(e4, 1); // !
        t.toggle_nag(e4, 6); // ?!  replaces !
        assert_eq!(t.nodes[e4].nags, vec![6]);

        // A positional NAG ($14) coexists with the move-quality one.
        t.toggle_nag(e4, 14);
        assert_eq!(t.nodes[e4].nags, vec![6, 14]);
        t.toggle_nag(e4, 3); // !!  replaces ?! but keeps $14
        assert_eq!(t.nodes[e4].nags, vec![14, 3]);
    }

    #[test]
    fn promote_makes_a_variation_the_mainline() {
        let mut t = MoveTree::new();
        let e4 = t.add_move(t.root, "e4");
        let c5 = t.add_move(e4, "c5"); // mainline
        let e5 = t.add_move(e4, "e5"); // variation
        assert_eq!(t.mainline(), vec!["e4", "c5"]);

        t.promote(e5).unwrap();
        assert_eq!(t.mainline(), vec!["e4", "e5"]);
        assert_eq!(t.nodes[e4].children, vec![e5, c5]);
    }

    #[test]
    fn reorder_moves_a_child_and_clamps_the_index() {
        let mut t = MoveTree::new();
        let e4 = t.add_move(t.root, "e4");
        let a = t.add_move(e4, "c5");
        let b = t.add_move(e4, "e5");
        let c = t.add_move(e4, "Nf6");

        t.reorder(c, 1).unwrap();
        assert_eq!(t.nodes[e4].children, vec![a, c, b]);

        // An out-of-range index lands the node last.
        t.reorder(a, 99).unwrap();
        assert_eq!(t.nodes[e4].children, vec![c, b, a]);
    }

    #[test]
    fn reorder_and_promote_reject_the_root() {
        let mut t = MoveTree::new();
        assert_eq!(t.promote(t.root), Err(TreeError::NoParent(0)));
        assert_eq!(t.reorder(0, 0), Err(TreeError::NoParent(0)));
        assert_eq!(t.delete(0), Err(TreeError::NoParent(0)));
        assert_eq!(t.promote(42), Err(TreeError::NoSuchNode(42)));
    }

    #[test]
    fn delete_drops_a_subtree_and_reindexes() {
        let mut t = MoveTree::new();
        let e4 = t.add_move(t.root, "e4");
        let c5 = t.add_move(e4, "c5"); // mainline, kept
        let e5 = t.add_move(e4, "e5"); // variation, deleted with its child
        let _e5_nf3 = t.add_move(e5, "Nf3");
        let nf3 = t.add_move(c5, "Nf3");
        t.set_comment(nf3, "Open Sicilian");

        t.delete(e5).unwrap();

        // The kept line survives with its comment; the dropped subtree is gone.
        assert_eq!(t.mainline(), vec!["e4", "c5", "Nf3"]);
        assert_eq!(t.nodes.len(), 4); // root, e4, c5, Nf3
        assert!(t.nodes.iter().all(|n| n.san.as_deref() != Some("e5")));
        // Ids are dense indices and self-consistent after the rebuild.
        for (i, n) in t.nodes.iter().enumerate() {
            assert_eq!(n.id, i);
        }
        assert_eq!(
            t.nodes
                .iter()
                .find(|n| n.comment.is_some())
                .unwrap()
                .comment,
            Some("Open Sicilian".to_string())
        );
    }

    #[test]
    fn round_trips_through_json() {
        let mut t = MoveTree::new();
        let d4 = t.add_move(t.root, "d4");
        t.add_move(d4, "d5");
        let json = serde_json::to_string(&t).unwrap();
        let back: MoveTree = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn set_shapes_pins_and_clears_board_annotations() {
        let mut t = MoveTree::new();
        let e4 = t.add_move(t.root, "e4");
        assert!(t.nodes[e4].shapes.is_empty(), "fresh node has no shapes");

        let shapes = vec![
            Shape {
                orig: "g1".into(),
                dest: Some("f3".into()),
                brush: "green".into(),
            },
            Shape {
                orig: "e4".into(),
                dest: None,
                brush: "blue".into(),
            },
        ];
        t.set_shapes(e4, shapes.clone());
        assert_eq!(t.nodes[e4].shapes, shapes);

        // Shapes survive a JSON round trip (arrow keeps its dest, highlight drops it).
        let back: MoveTree = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back.nodes[e4].shapes, shapes);

        // An empty vec clears them again.
        t.set_shapes(e4, Vec::new());
        assert!(t.nodes[e4].shapes.is_empty());
    }

    #[test]
    fn graft_adds_variations_and_dedups_existing_children() {
        let mut dst = MoveTree::new();
        let e4 = dst.add_move(dst.root, "e4");
        dst.add_move(e4, "e5"); // mainline: 1.e4 e5

        // Source: 1.e4 with two replies — e5 (shared) and c5 (new).
        let mut src = MoveTree::new();
        let s_e4 = src.add_move(src.root, "e4");
        src.add_move(s_e4, "e5");
        src.add_move(s_e4, "c5");

        let added = dst.graft_subtree(dst.root, &src);
        assert_eq!(added, 1, "only c5 is new; e4 and e5 already exist");

        // e4 now branches into e5 (kept) + c5 (grafted as a variation).
        let sans: Vec<String> = dst.nodes[e4]
            .children
            .iter()
            .filter_map(|&c| dst.nodes[c].san.clone())
            .collect();
        assert_eq!(sans.len(), 2);
        assert!(sans.contains(&"c5".to_string()));

        // Re-grafting the same source follows the now-existing children: adds 0.
        assert_eq!(dst.graft_subtree(dst.root, &src), 0);
    }

    #[test]
    fn graft_skips_illegal_moves() {
        let mut dst = MoveTree::new();

        // Source rooted at the start position: an illegal first move carrying a
        // child (the whole subtree must be skipped) plus a legal sibling.
        let mut src = MoveTree::new();
        let bad = src.add_move(src.root, "Nf6"); // illegal as White's first move
        src.add_move(bad, "d4");
        src.add_move(src.root, "d4"); // legal

        let added = dst.graft_subtree(dst.root, &src);
        assert_eq!(
            added, 1,
            "only the legal d4 grafts; the Nf6 subtree is skipped"
        );
        assert_eq!(dst.mainline(), vec!["d4"]);
    }

    #[test]
    fn graft_at_a_non_root_node_uses_that_position() {
        let mut dst = MoveTree::new();
        let e4 = dst.add_move(dst.root, "e4"); // 1.e4, Black to move

        // Source rooted at the post-1.e4 position (one reply, c5).
        let mut src = MoveTree::new();
        src.start_fen =
            Some("rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1".to_string());
        src.add_move(src.root, "c5");

        let added = dst.graft_subtree(e4, &src);
        assert_eq!(added, 1);
        assert_eq!(dst.nodes[e4].children.len(), 1);
        let c5 = dst.nodes[e4].children[0];
        assert_eq!(dst.nodes[c5].san.as_deref(), Some("c5"));
    }

    #[test]
    fn old_tree_json_without_shapes_still_deserializes() {
        // A pre-#61 row: nodes carry no `shapes` key. `serde(default)` must fill
        // an empty vec so existing studies keep loading (no DB migration).
        let legacy = r#"{
            "root": 0,
            "nodes": [
                {"id":0,"parent":null,"san":null,"comment":null,"nags":[],"children":[1]},
                {"id":1,"parent":0,"san":"e4","comment":"good","nags":[1],"children":[]}
            ]
        }"#;
        let tree: MoveTree = serde_json::from_str(legacy).unwrap();
        assert_eq!(tree.mainline(), vec!["e4"]);
        assert!(tree.nodes.iter().all(|n| n.shapes.is_empty()));
        assert_eq!(tree.nodes[1].comment.as_deref(), Some("good"));
    }
}
