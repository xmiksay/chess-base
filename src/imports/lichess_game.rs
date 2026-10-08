//! Import one Lichess game by id or URL (ADR-0057). A bare id doesn't say
//! whether it names a lichess.org game or a Masters game (separate id spaces),
//! so lichess.org's export is tried first and a 404 falls back to the Masters
//! explorer — the caller never has to know which kind of game it holds.

use std::time::Duration;

use super::{ImportError, ImportService, ImportSummary};
use crate::explorer::{MastersClient, MastersError};
use crate::server::identity::CurrentUser;

const LICHESS_BASE: &str = "https://lichess.org";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// Extract the 8-char game id from a bare id or a Lichess URL
/// (`lichess.org/AbCd1234`, `…/AbCd1234/black`, `…#23`, a 12-char player id,
/// `/game/export/…`, `explorer.lichess.ovh/masters/pgn/…`). Anything else is
/// rejected here, so only a plain alphanumeric id ever reaches an upstream URL.
pub fn parse_game_ref(input: &str) -> Result<String, ImportError> {
    let raw = input.trim();
    let invalid = || ImportError::InvalidInput(format!("'{raw}' is not a Lichess game id or URL"));
    let path = raw.split(['?', '#']).next().unwrap_or_default();
    // A foreign host or a profile URL (`/@/name`) never names a game.
    if path.contains('/') && (!path.contains("lichess.") || path.contains("/@/")) {
        return Err(invalid());
    }
    let id = path
        .split('/')
        .rev()
        .find(|seg| matches!(seg.len(), 8 | 12) && seg.bytes().all(|b| b.is_ascii_alphanumeric()))
        .ok_or_else(invalid)?;
    // A 12-char id is the game id plus a 4-char player suffix.
    Ok(id[..8].to_string())
}

impl ImportService {
    /// Fetch one Lichess game (lichess.org, else Masters) and ingest it into a
    /// database the caller may write. Deduped like any PGN import, so a repeat
    /// reports `duplicates: 1`. `masters` is `None` when the server has no
    /// `LICHESS_TOKEN`; a lichess.org miss then can't fall back.
    pub async fn import_lichess_game(
        &self,
        user: &CurrentUser,
        database_id: i32,
        game: &str,
        masters: Option<&MastersClient>,
    ) -> Result<ImportSummary, ImportError> {
        // Guard before fetching: a caller who can't write never costs an upstream call.
        self.load_writable(user, database_id).await?;
        let id = parse_game_ref(game)?;
        let pgn = match self.fetch_lichess_pgn(&id).await? {
            Some(pgn) => pgn,
            None => fetch_masters_pgn(&id, masters).await?,
        };
        self.import_pgn(user, database_id, &pgn).await
    }

    /// `Ok(None)` ⇔ lichess.org has no such game (404).
    async fn fetch_lichess_pgn(&self, id: &str) -> Result<Option<String>, ImportError> {
        #[cfg(test)]
        let base = self.lichess_base_url.as_deref().unwrap_or(LICHESS_BASE);
        #[cfg(not(test))]
        let base = LICHESS_BASE;
        let url = format!("{base}/game/export/{id}?clocks=false&evals=false");
        let resp = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .map(|http| http.get(url))
            .map_err(upstream_failed)?
            .send()
            .await
            .map_err(upstream_failed)?;
        match resp.status() {
            s if s.is_success() => resp.text().await.map(Some).map_err(upstream_failed),
            reqwest::StatusCode::NOT_FOUND => Ok(None),
            reqwest::StatusCode::TOO_MANY_REQUESTS => Err(rate_limited()),
            s => Err(upstream_failed(anyhow::anyhow!("lichess.org returned {s}"))),
        }
    }
}

async fn fetch_masters_pgn(
    id: &str,
    masters: Option<&MastersClient>,
) -> Result<String, ImportError> {
    let Some(masters) = masters else {
        return Err(ImportError::Failed(format!(
            "game '{id}' not found on lichess.org (Masters lookup needs LICHESS_TOKEN on the server)"
        )));
    };
    masters.game_pgn(id).await.map_err(|e| match e {
        MastersError::NotFound => ImportError::Failed(format!(
            "game '{id}' not found on lichess.org or in the Masters database"
        )),
        MastersError::RateLimited => rate_limited(),
        MastersError::BadRequest(msg) => ImportError::InvalidInput(msg),
        MastersError::Upstream(e) => upstream_failed(e),
    })
}

fn rate_limited() -> ImportError {
    ImportError::Failed("Lichess rate limit reached; try again in a minute".into())
}

/// Log the reqwest/anyhow chain server-side; the client gets a generic message.
fn upstream_failed(e: impl Into<anyhow::Error>) -> ImportError {
    tracing::warn!(error = %format!("{:#}", e.into()), "lichess game fetch failed");
    ImportError::Failed("Lichess request failed; try again later".into())
}

#[cfg(test)]
#[path = "lichess_game_tests.rs"]
mod tests;
