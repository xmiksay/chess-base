//! Chess.com Published-Data adapter (no authentication required).
//!
//! Flow: list monthly archives at `/pub/player/{user}/games/archives`, then
//! fetch each month's multi-game PGN at
//! `/pub/player/{user}/games/{YYYY}/{MM}/pgn`. Requests must be serialized to
//! respect rate limits on parallel access.

use super::GameSource;

const API_BASE: &str = "https://api.chess.com/pub";

pub struct ChessCom {
    pub username: String,
}

impl ChessCom {
    pub fn new(username: impl Into<String>) -> Self {
        Self {
            username: username.into(),
        }
    }

    /// Endpoint returning the list of available monthly archive URLs.
    pub fn archives_url(&self) -> String {
        format!("{API_BASE}/player/{}/games/archives", self.username)
    }

    /// Multi-game PGN endpoint for one month.
    pub fn month_pgn_url(&self, year: u16, month: u8) -> String {
        format!(
            "{API_BASE}/player/{}/games/{year:04}/{month:02}/pgn",
            self.username
        )
    }
}

impl GameSource for ChessCom {
    fn kind(&self) -> &'static str {
        "chesscom"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_archive_and_month_urls() {
        let src = ChessCom::new("hikaru");
        assert_eq!(
            src.archives_url(),
            "https://api.chess.com/pub/player/hikaru/games/archives"
        );
        assert_eq!(
            src.month_pgn_url(2024, 3),
            "https://api.chess.com/pub/player/hikaru/games/2024/03/pgn"
        );
        assert_eq!(src.kind(), "chesscom");
    }
}
