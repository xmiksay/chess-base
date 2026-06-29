//! Lichess-study export: wrap a [`MoveTree`]'s annotated movetext in the PGN
//! header tags a Lichess study chapter carries, so the output imports straight
//! into lichess.org/study (and is a git-versionable artifact, issue #32).
//!
//! The movetext itself — mainline, variations, `{ comments }`, `$N` NAGs and the
//! `[%csl]`/`[%cal]` shape commands — is produced by [`pgn::to_pgn`], so the two
//! exports never drift. This module only adds the seven-tag-style header; one
//! study maps to one chapter (`Event = study name`).

use super::pgn::{self, PgnError};
use super::MoveTree;

/// Serialize a study as a single Lichess-study chapter: header tags followed by
/// the annotated movetext. `study_name` becomes the `Event` (and chapter) name.
pub fn to_lichess_study(study_name: &str, tree: &MoveTree) -> Result<String, PgnError> {
    let mut out = String::new();
    push_tag(&mut out, "Event", study_name);
    push_tag(&mut out, "Result", "*");
    push_tag(&mut out, "Variant", "Standard");
    push_tag(&mut out, "ECO", "?");
    push_tag(&mut out, "Annotator", "chess-base");
    out.push('\n');
    out.push_str(&pgn::to_pgn(tree)?);
    out.push('\n');
    Ok(out)
}

/// Append a single `[Key "Value"]` PGN header tag (value escaped) plus newline.
fn push_tag(out: &mut String, key: &str, value: &str) {
    let value = value.replace('\\', "\\\\").replace('"', "\\\"");
    out.push_str(&format!("[{key} \"{value}\"]\n"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pgn_tree::pgn::from_pgn;

    fn sample_tree() -> MoveTree {
        let mut t = MoveTree::new();
        let e4 = t.add_move(t.root, "e4");
        let e5 = t.add_move(e4, "e5"); // mainline
        let c5 = t.add_move(e4, "c5"); // variation
        t.add_move(c5, "Nf3");
        let nf3 = t.add_move(e5, "Nf3");
        t.add_nag(e5, 1);
        t.set_comment(nf3, "develops the knight");
        t.set_shapes(
            nf3,
            vec![crate::pgn_tree::Shape {
                orig: "g1".into(),
                dest: Some("f3".into()),
                brush: "green".into(),
            }],
        );
        t
    }

    #[test]
    fn emits_header_tags_then_movetext() {
        let pgn = to_lichess_study("My Study", &sample_tree()).unwrap();
        assert!(pgn.starts_with("[Event \"My Study\"]\n[Result \"*\"]\n"));
        assert!(pgn.contains("[Variant \"Standard\"]"));
        // The movetext carries the NAG, comment and the shape command.
        assert!(pgn.contains("e5 $1"));
        assert!(pgn.contains("{[%cal Gg1f3] develops the knight}"));
    }

    #[test]
    fn escapes_quotes_in_the_study_name() {
        let pgn = to_lichess_study("The \"Best\" Line", &MoveTree::new()).unwrap();
        assert!(pgn.contains("[Event \"The \\\"Best\\\" Line\"]"));
    }

    #[test]
    fn round_trips_back_through_the_pgn_reader() {
        // A Lichess export is valid PGN: re-importing it (header tags ignored)
        // reconstructs the same annotated tree, shapes included.
        let tree = sample_tree();
        let exported = to_lichess_study("My Study", &tree).unwrap();
        let back = from_pgn(&exported).unwrap();
        assert_eq!(tree, back);
    }
}
