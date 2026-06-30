//! Lichess `[%eval …]` codec: an engine [`Eval`] ⇄ the PGN comment command
//! (issue #120). Pure and I/O-free, mirroring [`super::shapes`].
//!
//! The command is the Lichess/ChessBase convention: `[%eval 0.27]` is a
//! centipawn score in pawns from **White's** perspective; `[%eval #3]` / `[%eval
//! #-2]` a forced mate (positive = White mates). An unparseable payload is left
//! untouched in the comment text rather than dropped, so a malformed command
//! never silently loses information.

use super::Eval;

/// Encode an evaluation as a single `[%eval …]` command.
pub fn encode(eval: &Eval) -> String {
    match eval {
        Eval::Mate(n) => format!("[%eval #{n}]"),
        // Two decimals of pawns is the Lichess wire format (`0.27`, `-1.20`).
        Eval::Cp(cp) => format!("[%eval {:.2}]", *cp as f64 / 100.0),
    }
}

/// Pull the first `[%eval …]` command out of a comment, returning the parsed
/// evaluation (if any) and the remaining text with whitespace collapsed.
///
/// A command whose payload doesn't parse is kept verbatim in the text (and the
/// eval is `None`), so unknown future `[%eval …]` shapes degrade gracefully.
pub fn parse(comment: &str) -> (Option<Eval>, String) {
    let collapse = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let Some(start) = comment.find("[%eval") else {
        return (None, collapse(comment));
    };
    let after = &comment[start..];
    let Some(end) = after.find(']') else {
        return (None, collapse(comment));
    };
    // `after[2..end]` is `eval <payload>`; drop the keyword, keep the payload.
    let payload = after[2..end].trim_start_matches("eval").trim();
    let Some(eval) = parse_payload(payload) else {
        return (None, collapse(comment));
    };
    let remaining = format!("{}{}", &comment[..start], &after[end + 1..]);
    (Some(eval), collapse(&remaining))
}

/// Parse the inside of an `[%eval …]` command: `#N` is a mate, anything else a
/// decimal pawn score. `None` for an unrecognised payload.
fn parse_payload(payload: &str) -> Option<Eval> {
    if let Some(mate) = payload.strip_prefix('#') {
        return mate.trim().parse::<i32>().ok().map(Eval::Mate);
    }
    let pawns: f64 = payload.parse().ok()?;
    Some(Eval::Cp((pawns * 100.0).round() as i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_centipawns_in_pawns_with_two_decimals() {
        assert_eq!(encode(&Eval::Cp(27)), "[%eval 0.27]");
        assert_eq!(encode(&Eval::Cp(-120)), "[%eval -1.20]");
        assert_eq!(encode(&Eval::Cp(0)), "[%eval 0.00]");
    }

    #[test]
    fn encodes_mate_with_sign() {
        assert_eq!(encode(&Eval::Mate(3)), "[%eval #3]");
        assert_eq!(encode(&Eval::Mate(-2)), "[%eval #-2]");
    }

    #[test]
    fn parses_centipawn_and_mate_payloads() {
        assert_eq!(parse("[%eval 0.27]").0, Some(Eval::Cp(27)));
        assert_eq!(parse("[%eval -1.2]").0, Some(Eval::Cp(-120)));
        assert_eq!(parse("[%eval #3]").0, Some(Eval::Mate(3)));
        assert_eq!(parse("[%eval #-2]").0, Some(Eval::Mate(-2)));
    }

    #[test]
    fn keeps_surrounding_text_and_strips_the_command() {
        let (eval, text) = parse("[%eval 0.31] a quiet move");
        assert_eq!(eval, Some(Eval::Cp(31)));
        assert_eq!(text, "a quiet move");
    }

    #[test]
    fn round_trips_through_encode_then_parse() {
        for e in [Eval::Cp(42), Eval::Cp(-7), Eval::Mate(5), Eval::Mate(-1)] {
            assert_eq!(parse(&encode(&e)).0, Some(e));
        }
    }

    #[test]
    fn leaves_an_unparseable_command_as_text() {
        let (eval, text) = parse("[%eval n/a] still here");
        assert_eq!(eval, None);
        assert_eq!(text, "[%eval n/a] still here");
    }

    #[test]
    fn no_command_returns_collapsed_text() {
        let (eval, text) = parse("just  a   comment");
        assert_eq!(eval, None);
        assert_eq!(text, "just a comment");
    }
}
