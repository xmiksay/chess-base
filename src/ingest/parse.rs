//! The PGN-syntax layer of game ingest: turn raw PGN text into a [`ParsedGame`]
//! (headers + mainline SAN) and split a multi-game blob into individual games.
//!
//! Parsing only reads syntax — it never validates legality or touches storage;
//! [`super`] replays the mainline and writes the rows. Variations are dropped
//! here since only the mainline feeds the position index.

use std::borrow::Cow;
use std::io::Cursor;
use std::ops::ControlFlow;

use anyhow::{anyhow, Result};
use pgn_reader::{RawTag, Reader, SanPlus, Skip, Visitor};

/// PGN seven-tag-roster + indexing-relevant headers, all optional. `variant` and
/// `start_fen` (`[SetUp]`/`[FEN]`) make Chess960 and set-up positions first-class.
#[derive(Debug, Default, Clone)]
pub(crate) struct Headers {
    pub(crate) white: Option<String>,
    pub(crate) black: Option<String>,
    pub(crate) event: Option<String>,
    pub(crate) site: Option<String>,
    pub(crate) round: Option<String>,
    pub(crate) date: Option<String>,
    pub(crate) result: Option<String>,
    pub(crate) eco: Option<String>,
    pub(crate) white_elo: Option<i32>,
    pub(crate) black_elo: Option<i32>,
    pub(crate) variant: Option<String>,
    pub(crate) start_fen: Option<String>,
    /// Chess.com permalink (`[Link]`); Lichess carries its permalink in `site`.
    pub(crate) link: Option<String>,
}

/// A parsed PGN: its headers and the mainline SAN tokens (variations dropped).
pub(crate) struct ParsedGame {
    pub(crate) headers: Headers,
    pub(crate) mainline: Vec<String>,
}

/// Parse the first game's headers and mainline SAN from `pgn`.
pub(crate) fn parse_pgn(pgn: &str) -> Result<ParsedGame> {
    let mut reader = Reader::new(Cursor::new(pgn.as_bytes()));
    match reader.read_game(&mut Importer) {
        Ok(Some(game)) => Ok(game),
        Ok(None) => Err(anyhow!("no game found in PGN")),
        Err(e) => Err(anyhow!("malformed PGN: {e}")),
    }
}

/// Split a complete multi-game PGN blob into individual, trimmed game strings.
/// Games are delimited by a line beginning with `[Event `. Shared with the
/// streaming collectors (Lichess / Chess.com).
pub(crate) fn split_games(blob: &str) -> Vec<String> {
    let starts = event_offsets(blob.as_bytes());
    let mut games = Vec::with_capacity(starts.len());
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(blob.len());
        let game = blob[start..end].trim();
        if !game.is_empty() {
            games.push(game.to_string());
        }
    }
    games
}

/// Byte offsets of every line that begins a new game (`[Event `). ASCII-only
/// matching, so it is safe on the raw byte buffer regardless of UTF-8 framing.
pub(crate) fn event_offsets(buf: &[u8]) -> Vec<usize> {
    const MARKER: &[u8] = b"[Event ";
    let mut offsets = Vec::new();
    let mut at_line_start = true;
    for i in 0..buf.len() {
        if at_line_start && buf[i..].starts_with(MARKER) {
            offsets.push(i);
        }
        at_line_start = buf[i] == b'\n';
    }
    offsets
}

/// Streaming visitor collecting headers and the mainline; variations are skipped
/// since only the mainline is indexed.
struct Importer;

impl Visitor for Importer {
    type Tags = Headers;
    type Movetext = ParsedGame;
    type Output = ParsedGame;

    fn begin_tags(&mut self) -> ControlFlow<Self::Output, Self::Tags> {
        ControlFlow::Continue(Headers::default())
    }

    fn tag(
        &mut self,
        tags: &mut Self::Tags,
        name: &[u8],
        value: RawTag<'_>,
    ) -> ControlFlow<Self::Output> {
        set_header(tags, name, value.decode_utf8_lossy());
        ControlFlow::Continue(())
    }

    fn begin_movetext(&mut self, tags: Self::Tags) -> ControlFlow<Self::Output, Self::Movetext> {
        ControlFlow::Continue(ParsedGame {
            headers: tags,
            mainline: Vec::new(),
        })
    }

    fn san(&mut self, game: &mut Self::Movetext, san_plus: SanPlus) -> ControlFlow<Self::Output> {
        game.mainline.push(san_plus.to_string());
        ControlFlow::Continue(())
    }

    fn begin_variation(&mut self, _game: &mut Self::Movetext) -> ControlFlow<Self::Output, Skip> {
        // Only the mainline feeds the position index.
        ControlFlow::Continue(Skip(true))
    }

    fn end_game(&mut self, game: Self::Movetext) -> Self::Output {
        game
    }
}

/// Record one parsed PGN tag into `headers`. Blank and `?` placeholders are
/// dropped; Elo tags parse to integers (unparseable values are ignored).
fn set_header(headers: &mut Headers, name: &[u8], value: Cow<'_, str>) {
    let value = value.trim();
    if value.is_empty() || value == "?" {
        return;
    }
    match name {
        b"White" => headers.white = Some(value.to_string()),
        b"Black" => headers.black = Some(value.to_string()),
        b"Event" => headers.event = Some(value.to_string()),
        b"Site" => headers.site = Some(value.to_string()),
        b"Round" => headers.round = Some(value.to_string()),
        b"Date" => headers.date = Some(value.to_string()),
        b"Result" => headers.result = Some(value.to_string()),
        b"ECO" => headers.eco = Some(value.to_string()),
        b"WhiteElo" => headers.white_elo = value.parse().ok(),
        b"BlackElo" => headers.black_elo = value.parse().ok(),
        b"Variant" => headers.variant = Some(value.to_ascii_lowercase()),
        b"FEN" => headers.start_fen = Some(value.to_string()),
        b"Link" => headers.link = Some(value.to_string()),
        _ => {}
    }
}
