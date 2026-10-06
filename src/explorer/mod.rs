//! Lichess Masters opening explorer (ADR-0053): a *remote* reference, never
//! bulk-imported — the local database stays slim (ADR-0052). A thin client over
//! `explorer.lichess.ovh/masters` with an in-memory TTL cache, plus the PGN
//! fetch behind "import this top game". Response parsing and query validation
//! are pure so they test without the network; HTTP (`routes.rs`) and the MCP
//! `masters_position_report` tool are both thin callers.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use shakmaty::CastlingMode;

use crate::position::position_from_fen;

pub mod routes;

/// Lichess's opening-explorer host (the masters DB lives here, not on lichess.org).
pub const DEFAULT_BASE: &str = "https://explorer.lichess.ovh";

/// Masters games are a fixed historical set, so a day-old answer is still right.
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Bound on cached positions so a long browsing session can't grow it forever.
const CACHE_CAP: usize = 4096;

/// Lichess mandates backing off at least one minute on HTTP 429.
const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(60);

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// Continuations / top games requested per position (15 is Lichess's top-games max).
const MOVES: u32 = 20;
const TOP_GAMES: u32 = 15;

/// Plausible bounds for the year filter; anything outside is a client typo.
const MIN_YEAR: u16 = 1800;
const MAX_YEAR: u16 = 2100;

/// A masters lookup: position plus the optional year window.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct MastersQuery {
    pub fen: String,
    #[serde(default)]
    pub since: Option<u16>,
    #[serde(default)]
    pub until: Option<u16>,
}

impl MastersQuery {
    /// Reject a bad FEN or year window before anything hits the network.
    pub fn validate(&self) -> Result<(), MastersError> {
        position_from_fen(&self.fen, CastlingMode::Standard)
            .map_err(|e| MastersError::BadRequest(format!("invalid FEN: {e}")))?;
        for year in [self.since, self.until].into_iter().flatten() {
            if !(MIN_YEAR..=MAX_YEAR).contains(&year) {
                return Err(MastersError::BadRequest(format!(
                    "year {year} is out of range"
                )));
            }
        }
        if let (Some(since), Some(until)) = (self.since, self.until) {
            if since > until {
                return Err(MastersError::BadRequest(
                    "`since` must not be after `until`".into(),
                ));
            }
        }
        Ok(())
    }

    fn cache_key(&self) -> String {
        format!("{}|{:?}|{:?}", self.fen, self.since, self.until)
    }
}

/// Opening name Lichess attaches to a known position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opening {
    pub eco: String,
    pub name: String,
}

/// One continuation. `san`/`count`/`white`/`draws`/`black` match the local
/// explorer's `MoveStat`, so the SPA renders both tabs with the same table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MastersMove {
    pub san: String,
    pub uci: String,
    pub count: u64,
    pub white: u64,
    pub draws: u64,
    pub black: u64,
    pub average_rating: Option<u32>,
}

/// A notable game through the position; `id` is what the import route takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MastersGame {
    pub id: String,
    /// The move this game played from the position.
    pub uci: Option<String>,
    /// `"white"`, `"black"`, or `None` for a draw.
    pub winner: Option<String>,
    pub white: String,
    pub white_rating: Option<u32>,
    pub black: String,
    pub black_rating: Option<u32>,
    pub year: Option<u16>,
    pub month: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MastersReport {
    pub total: u64,
    pub white: u64,
    pub draws: u64,
    pub black: u64,
    pub opening: Option<Opening>,
    pub moves: Vec<MastersMove>,
    pub top_games: Vec<MastersGame>,
}

#[derive(Debug, thiserror::Error)]
pub enum MastersError {
    #[error("{0}")]
    BadRequest(String),
    /// Lichess answered 429 and nothing usable is cached.
    #[error("Lichess rate limit reached; try again in a minute")]
    RateLimited,
    #[error("game not found on Lichess")]
    NotFound,
    /// Network/HTTP/parse failure talking to Lichess. Logged, never echoed.
    #[error("Lichess explorer request failed")]
    Upstream(#[source] anyhow::Error),
}

#[derive(Deserialize)]
struct RawReport {
    white: u64,
    draws: u64,
    black: u64,
    #[serde(default)]
    moves: Vec<RawMove>,
    #[serde(default, rename = "topGames")]
    top_games: Vec<RawGame>,
    #[serde(default)]
    opening: Option<Opening>,
}

#[derive(Deserialize)]
struct RawMove {
    uci: String,
    san: String,
    white: u64,
    draws: u64,
    black: u64,
    #[serde(default, rename = "averageRating")]
    average_rating: Option<u32>,
}

#[derive(Deserialize)]
struct RawGame {
    id: String,
    #[serde(default)]
    uci: Option<String>,
    #[serde(default)]
    winner: Option<String>,
    white: RawPlayer,
    black: RawPlayer,
    #[serde(default)]
    year: Option<u16>,
    #[serde(default)]
    month: Option<String>,
}

#[derive(Deserialize)]
struct RawPlayer {
    name: String,
    #[serde(default)]
    rating: Option<u32>,
}

/// Parse a `/masters` JSON body into the report shape the app serves.
pub fn parse_report(body: &str) -> Result<MastersReport, serde_json::Error> {
    let raw: RawReport = serde_json::from_str(body)?;
    Ok(MastersReport {
        total: raw.white + raw.draws + raw.black,
        white: raw.white,
        draws: raw.draws,
        black: raw.black,
        opening: raw.opening,
        moves: raw
            .moves
            .into_iter()
            .map(|m| MastersMove {
                count: m.white + m.draws + m.black,
                san: m.san,
                uci: m.uci,
                white: m.white,
                draws: m.draws,
                black: m.black,
                average_rating: m.average_rating,
            })
            .collect(),
        top_games: raw
            .top_games
            .into_iter()
            .map(|g| MastersGame {
                id: g.id,
                uci: g.uci,
                winner: g.winner,
                white: g.white.name,
                white_rating: g.white.rating,
                black: g.black.name,
                black_rating: g.black.rating,
                year: g.year,
                month: g.month,
            })
            .collect(),
    })
}

/// A Lichess game id is short and alphanumeric; anything else must never be
/// spliced into the upstream URL path.
pub fn valid_game_id(id: &str) -> bool {
    (1..=16).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

struct Cached {
    at: Instant,
    report: MastersReport,
}

pub struct MastersClient {
    http: reqwest::Client,
    base: String,
    token: String,
    cache: Mutex<HashMap<String, Cached>>,
    /// Set on a 429: until then we don't call Lichess at all.
    backoff_until: Mutex<Option<Instant>>,
}

impl MastersClient {
    /// `base` is [`DEFAULT_BASE`] in production; tests point it at a fake.
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> anyhow::Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder().timeout(HTTP_TIMEOUT).build()?,
            base: base.into(),
            token: token.into(),
            cache: Mutex::new(HashMap::new()),
            backoff_until: Mutex::new(None),
        })
    }

    /// Masters stats for a position: fresh cache hit, else Lichess, else (on any
    /// upstream failure) a stale cache entry rather than an error.
    pub async fn report(&self, q: &MastersQuery) -> Result<MastersReport, MastersError> {
        q.validate()?;
        let key = q.cache_key();
        if let Some(hit) = self.cached(&key, true) {
            return Ok(hit);
        }
        match self.fetch_report(q).await {
            Ok(report) => {
                self.store(key, report.clone());
                Ok(report)
            }
            Err(err) => match self.cached(&key, false) {
                Some(stale) => {
                    tracing::warn!(error = %format!("{err:#}"), "masters explorer failed; serving stale cache");
                    Ok(stale)
                }
                None => Err(err),
            },
        }
    }

    /// The PGN of one masters game, for import into a local database.
    pub async fn game_pgn(&self, id: &str) -> Result<String, MastersError> {
        if !valid_game_id(id) {
            return Err(MastersError::BadRequest(format!("invalid game id '{id}'")));
        }
        let url = format!("{}/masters/pgn/{id}", self.base);
        self.get(&url, &[]).await?.text().await.map_err(upstream)
    }

    async fn fetch_report(&self, q: &MastersQuery) -> Result<MastersReport, MastersError> {
        let mut params = vec![
            ("fen", q.fen.clone()),
            ("moves", MOVES.to_string()),
            ("topGames", TOP_GAMES.to_string()),
        ];
        if let Some(since) = q.since {
            params.push(("since", since.to_string()));
        }
        if let Some(until) = q.until {
            params.push(("until", until.to_string()));
        }
        let url = format!("{}/masters", self.base);
        let body = self
            .get(&url, &params)
            .await?
            .text()
            .await
            .map_err(upstream)?;
        parse_report(&body).map_err(upstream)
    }

    async fn get(
        &self,
        url: &str,
        params: &[(&str, String)],
    ) -> Result<reqwest::Response, MastersError> {
        if self.backing_off() {
            return Err(MastersError::RateLimited);
        }
        // reqwest 0.13 gates `.query()` behind a feature; `Url` encodes the same.
        let url = reqwest::Url::parse_with_params(url, params).map_err(upstream)?;
        let resp = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(upstream)?;
        match resp.status() {
            s if s.is_success() => Ok(resp),
            reqwest::StatusCode::TOO_MANY_REQUESTS => {
                if let Ok(mut until) = self.backoff_until.lock() {
                    *until = Some(Instant::now() + RATE_LIMIT_BACKOFF);
                }
                Err(MastersError::RateLimited)
            }
            reqwest::StatusCode::NOT_FOUND => Err(MastersError::NotFound),
            s => Err(MastersError::Upstream(anyhow::anyhow!(
                "lichess explorer returned {s}"
            ))),
        }
    }

    fn backing_off(&self) -> bool {
        self.backoff_until
            .lock()
            .ok()
            .and_then(|until| *until)
            .is_some_and(|until| Instant::now() < until)
    }

    fn cached(&self, key: &str, fresh_only: bool) -> Option<MastersReport> {
        let cache = self.cache.lock().ok()?;
        let hit = cache.get(key)?;
        (!fresh_only || hit.at.elapsed() < CACHE_TTL).then(|| hit.report.clone())
    }

    fn store(&self, key: String, report: MastersReport) {
        let Ok(mut cache) = self.cache.lock() else {
            return;
        };
        if cache.len() >= CACHE_CAP && !cache.contains_key(&key) {
            cache.retain(|_, c| c.at.elapsed() < CACHE_TTL);
            if cache.len() >= CACHE_CAP {
                let oldest = cache
                    .iter()
                    .min_by_key(|(_, c)| c.at)
                    .map(|(k, _)| k.clone());
                if let Some(oldest) = oldest {
                    cache.remove(&oldest);
                }
            }
        }
        cache.insert(
            key,
            Cached {
                at: Instant::now(),
                report,
            },
        );
    }
}

fn upstream(e: impl Into<anyhow::Error>) -> MastersError {
    MastersError::Upstream(e.into())
}

#[cfg(test)]
mod tests;
