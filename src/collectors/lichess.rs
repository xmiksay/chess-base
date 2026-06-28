//! Lichess game export adapter.
//!
//! Games stream from `GET /api/user/{username}/games` (PGN or NDJSON). A
//! personal API token (optional) raises rate limits; on HTTP 429 callers must
//! back off ≥60s. Incremental sync uses the `since` query parameter.

use super::GameSource;

const API_BASE: &str = "https://lichess.org";

pub struct Lichess {
    pub username: String,
    /// Optional personal access token (raises rate limits).
    pub token: Option<String>,
}

impl Lichess {
    pub fn new(username: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            token: None,
        }
    }

    /// Export endpoint for this user's games as PGN. `since` is epoch-ms.
    pub fn games_url(&self, since: Option<i64>) -> String {
        let mut url = format!(
            "{API_BASE}/api/user/{}/games?pgnInJson=false",
            self.username
        );
        if let Some(ms) = since {
            url.push_str(&format!("&since={ms}"));
        }
        url
    }
}

impl GameSource for Lichess {
    fn kind(&self) -> &'static str {
        "lichess"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_export_url_with_since() {
        let src = Lichess::new("DrNykterstein");
        assert_eq!(
            src.games_url(Some(1700000000000)),
            "https://lichess.org/api/user/DrNykterstein/games?pgnInJson=false&since=1700000000000"
        );
        assert_eq!(src.kind(), "lichess");
    }
}
