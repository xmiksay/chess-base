//! Lichess board-annotation codec: [`Shape`]s ⇄ the `[%csl …]` / `[%cal …]`
//! commands Lichess (and ChessBase) embed inside PGN `{ comments }`.
//!
//! Pure and I/O-free. `[%csl Ge4,Rd5]` is a list of coloured **squares**
//! (highlights, `dest == None`); `[%cal Gg1f3]` a list of coloured **arrows**
//! (`orig → dest`). Colours map to the four standard chessground brushes
//! (green/red/yellow/blue); an unknown brush is exported as green and an unknown
//! colour letter is skipped, so a malformed command never panics or aborts the
//! whole comment.

use super::Shape;

/// Encode a node's pinned shapes as Lichess command(s): circles in one
/// `[%csl …]` block, arrows in one `[%cal …]`, in that order. Empty for no
/// shapes (so a node without shapes serializes exactly as before).
pub fn encode(shapes: &[Shape]) -> String {
    let mut circles = Vec::new();
    let mut arrows = Vec::new();
    for s in shapes {
        let color = color_letter(&s.brush);
        match &s.dest {
            None => circles.push(format!("{color}{}", s.orig)),
            Some(dest) => arrows.push(format!("{color}{}{dest}", s.orig)),
        }
    }
    let mut out = String::new();
    if !circles.is_empty() {
        out.push_str(&format!("[%csl {}]", circles.join(",")));
    }
    if !arrows.is_empty() {
        out.push_str(&format!("[%cal {}]", arrows.join(",")));
    }
    out
}

/// Split a raw PGN comment into the shapes it carries and its remaining text.
///
/// Every recognised `[%csl …]` / `[%cal …]` block is consumed into shapes; any
/// other `[%…]` command (e.g. `[%clk]`, `[%eval]`) and the free text are kept,
/// with surrounding whitespace collapsed.
pub fn parse(comment: &str) -> (Vec<Shape>, String) {
    let mut shapes = Vec::new();
    let mut text = String::new();
    let mut rest = comment;
    while let Some(start) = rest.find("[%") {
        text.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(end) = after.find(']') else {
            // Unterminated command: keep the remainder verbatim and stop.
            text.push_str(after);
            rest = "";
            break;
        };
        match parse_block(&after[2..end]) {
            Some(parsed) => shapes.extend(parsed),
            None => text.push_str(&after[..=end]), // not a shape command: keep it
        }
        rest = &after[end + 1..];
    }
    text.push_str(rest);
    (
        shapes,
        text.split_whitespace().collect::<Vec<_>>().join(" "),
    )
}

/// Parse the inside of a `[%kind payload]` block (without the brackets/`%`).
/// `None` for anything that is not a non-empty `csl`/`cal` command.
fn parse_block(inner: &str) -> Option<Vec<Shape>> {
    let (kind, payload) = inner.trim().split_once(char::is_whitespace)?;
    let arrow = match kind {
        "cal" => true,
        "csl" => false,
        _ => return None,
    };
    let mut out = Vec::new();
    for token in payload.split(',') {
        let token = token.trim();
        let mut chars = token.chars();
        let Some(color) = chars.next() else { continue };
        let Some(brush) = brush_name(color) else {
            continue;
        };
        let squares = chars.as_str();
        if arrow && squares.len() == 4 {
            out.push(Shape {
                orig: squares[..2].to_string(),
                dest: Some(squares[2..].to_string()),
                brush,
            });
        } else if !arrow && squares.len() == 2 {
            out.push(Shape {
                orig: squares.to_string(),
                dest: None,
                brush,
            });
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Brush name → Lichess colour letter; unknown brushes default to green.
fn color_letter(brush: &str) -> char {
    match brush {
        "red" => 'R',
        "blue" => 'B',
        "yellow" => 'Y',
        _ => 'G',
    }
}

/// Lichess colour letter → chessground brush name; `None` for an unknown letter.
fn brush_name(letter: char) -> Option<String> {
    let name = match letter {
        'G' => "green",
        'R' => "red",
        'B' => "blue",
        'Y' => "yellow",
        _ => return None,
    };
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arrow(orig: &str, dest: &str, brush: &str) -> Shape {
        Shape {
            orig: orig.into(),
            dest: Some(dest.into()),
            brush: brush.into(),
        }
    }

    fn circle(orig: &str, brush: &str) -> Shape {
        Shape {
            orig: orig.into(),
            dest: None,
            brush: brush.into(),
        }
    }

    #[test]
    fn encodes_circles_before_arrows_grouped_by_kind() {
        let shapes = vec![
            circle("e4", "green"),
            circle("d5", "red"),
            arrow("g1", "f3", "blue"),
        ];
        assert_eq!(encode(&shapes), "[%csl Ge4,Rd5][%cal Bg1f3]");
    }

    #[test]
    fn encode_is_empty_without_shapes() {
        assert_eq!(encode(&[]), "");
    }

    #[test]
    fn round_trips_through_a_comment() {
        // Circles first, then arrows — the order `encode` normalizes to.
        let shapes = vec![circle("e4", "green"), arrow("g1", "f3", "yellow")];
        let encoded = encode(&shapes);
        let (back, text) = parse(&encoded);
        assert_eq!(back, shapes);
        assert!(text.is_empty());
    }

    #[test]
    fn parses_commands_and_keeps_surrounding_text() {
        let (shapes, text) = parse("[%csl Gd4] a sharp idea [%cal Re5d6]");
        assert_eq!(
            shapes,
            vec![circle("d4", "green"), arrow("e5", "d6", "red")]
        );
        assert_eq!(text, "a sharp idea");
    }

    #[test]
    fn keeps_unknown_commands_as_text() {
        let (shapes, text) = parse("[%clk 0:05:00] still equal");
        assert!(shapes.is_empty());
        assert_eq!(text, "[%clk 0:05:00] still equal");
    }

    #[test]
    fn skips_unknown_colours_and_malformed_squares() {
        // 'X' is not a brush; "e44" is not a 2-char square; the rest survive.
        let (shapes, _) = parse("[%csl Xe4,Gd4,Re44,Bh8]");
        assert_eq!(shapes, vec![circle("d4", "green"), circle("h8", "blue")]);
    }

    #[test]
    fn plain_comment_has_no_shapes() {
        let (shapes, text) = parse("just a normal comment");
        assert!(shapes.is_empty());
        assert_eq!(text, "just a normal comment");
    }
}
