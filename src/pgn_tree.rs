//! Pure move-tree model for studies: a PGN with variations, comments and NAGs.
//!
//! Storage-agnostic and I/O-free. A study is an arena of [`Node`]s; the mainline
//! is the first child at each step, variations are the remaining children.

use serde::{Deserialize, Serialize};

pub mod pgn;

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
    /// Child node ids; `children[0]` is the mainline continuation.
    pub children: Vec<usize>,
}

/// An arena-allocated move tree. Node ids are indices into `nodes`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MoveTree {
    pub nodes: Vec<Node>,
    pub root: usize,
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
            children: Vec::new(),
        };
        MoveTree {
            nodes: vec![root],
            root: 0,
        }
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
            children: Vec::new(),
        });
        self.nodes[parent].children.push(id);
        id
    }

    /// Attach a comment to a node.
    pub fn set_comment(&mut self, id: usize, comment: impl Into<String>) {
        self.nodes[id].comment = Some(comment.into());
    }

    /// Append a Numeric Annotation Glyph to a node.
    pub fn add_nag(&mut self, id: usize, nag: u8) {
        self.nodes[id].nags.push(nag);
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
    fn round_trips_through_json() {
        let mut t = MoveTree::new();
        let d4 = t.add_move(t.root, "d4");
        t.add_move(d4, "d5");
        let json = serde_json::to_string(&t).unwrap();
        let back: MoveTree = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }
}
