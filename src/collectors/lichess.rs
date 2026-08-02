//! Lichess game export adapter.
//!
//! Games stream from `GET /api/games/user/{username}` (PGN). A personal API
//! token (optional) raises rate limits; on HTTP 429 callers must back off ≥60s.
//! Incremental sync uses the `since` query parameter set to the persisted
//! [`SyncCursor::last_game_ms`]; the boundary game it re-fetches is deduped by
//! ingest (issue #95), so games are never doubled.
//!
//! The networked [`Lichess::sync`] is a thin adapter: it streams the export body
//! chunk-by-chunk, splits it into individual games and funnels each through the
//! shared [`ingest_pgn`] pipeline. All boundary/cursor/back-off decisions live in
//! the pure helpers below so they can be unit-tested without the network.

use anyhow::{anyhow, Context, Result};
use sea_orm::DatabaseConnection;
use std::time::Duration;

use super::{backoff_delay, retry_after_secs, GameSource, SyncCursor, SyncFailure, SyncOutcome};
use crate::ingest::{event_offsets, ingest_pgn, split_games};

const API_BASE: &str = "https://lichess.org";

/// Lichess mandates backing off at least one minute on HTTP 429.
const MIN_BACKOFF: Duration = Duration::from_secs(60);

/// Number of 429 retries before giving up on a request.
const MAX_RETRIES: u32 = 5;

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

    /// Attach a personal access token to raise rate limits.
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// Export endpoint for this user's games as PGN. `since` is epoch-ms.
    pub fn games_url(&self, since: Option<i64>) -> String {
        // Lichess's game-export endpoint is `/api/games/user/{username}` — NOT
        // `/api/user/{username}/games`, which 404s (issue: lichess sync failed).
        let mut url = format!(
            "{API_BASE}/api/games/user/{}?pgnInJson=false",
            self.username
        );
        if let Some(ms) = since {
            url.push_str(&format!("&since={ms}"));
        }
        url
    }

    /// Sync this user's games into `database_id`, resuming from `cursor`.
    ///
    /// Streams the export, ingests every game and returns the advanced cursor. A
    /// re-sync resumes from the last game's second; the boundary game(s) it
    /// re-fetches are deduped by ingest, so games are never doubled (issue #95).
    /// A mid-run failure (429 past the retry budget, a dropped stream) returns
    /// [`SyncFailure`] carrying whatever cursor/counts had already advanced, so
    /// the caller can persist that partial progress instead of losing it (#197).
    pub async fn sync(
        &self,
        db: &DatabaseConnection,
        database_id: i32,
        cursor: SyncCursor,
    ) -> Result<SyncOutcome, SyncFailure> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| SyncFailure {
                cursor: cursor.clone(),
                imported: 0,
                duplicates: 0,
                error: anyhow::Error::from(e).context("building http client"),
            })?;
        self.sync_with(&client, db, database_id, cursor).await
    }

    /// [`sync`](Self::sync) against a caller-supplied client (kept separate so the
    /// transport can be configured/injected).
    async fn sync_with(
        &self,
        client: &reqwest::Client,
        db: &DatabaseConnection,
        database_id: i32,
        mut cursor: SyncCursor,
    ) -> Result<SyncOutcome, SyncFailure> {
        let url = self.games_url(since_param(&cursor));
        let mut resp = match self.fetch_with_backoff(client, &url).await {
            Ok(r) => r,
            Err(e) => {
                return Err(SyncFailure {
                    cursor,
                    imported: 0,
                    duplicates: 0,
                    error: e,
                })
            }
        };

        let mut imported = 0usize;
        let mut duplicates = 0usize;
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let chunk = match resp.chunk().await.context("streaming lichess export") {
                Ok(Some(c)) => c,
                Ok(None) => break,
                Err(e) => {
                    return Err(SyncFailure {
                        cursor,
                        imported,
                        duplicates,
                        error: e,
                    })
                }
            };
            buf.extend_from_slice(&chunk);
            // Drain every game that is provably complete (i.e. a later game has
            // started), leaving the trailing partial game in the buffer.
            if let Some(split) = trailing_game_offset(&buf) {
                let tail = buf.split_off(split);
                let head = std::mem::replace(&mut buf, tail);
                if let Err(e) = ingest_blob(
                    db,
                    database_id,
                    &String::from_utf8_lossy(&head),
                    &mut cursor,
                    &mut imported,
                    &mut duplicates,
                )
                .await
                {
                    return Err(SyncFailure {
                        cursor,
                        imported,
                        duplicates,
                        error: e,
                    });
                }
            }
        }
        // Flush the final game once the stream is exhausted.
        if let Err(e) = ingest_blob(
            db,
            database_id,
            &String::from_utf8_lossy(&buf),
            &mut cursor,
            &mut imported,
            &mut duplicates,
        )
        .await
        {
            return Err(SyncFailure {
                cursor,
                imported,
                duplicates,
                error: e,
            });
        }

        Ok(SyncOutcome {
            cursor,
            imported,
            duplicates,
        })
    }

    /// Issue the export request, honouring HTTP 429 with a ≥60s back-off and a
    /// bounded number of retries. A personal token, when present, is sent as a
    /// bearer credential to raise the rate limit.
    async fn fetch_with_backoff(
        &self,
        client: &reqwest::Client,
        url: &str,
    ) -> Result<reqwest::Response> {
        let mut attempt = 0u32;
        loop {
            let mut req = client.get(url).header("Accept", "application/x-chess-pgn");
            if let Some(token) = &self.token {
                req = req.bearer_auth(token);
            }
            let resp = req.send().await.context("requesting lichess export")?;

            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                attempt += 1;
                if attempt > MAX_RETRIES {
                    return Err(anyhow!(
                        "lichess rate limit: gave up after {MAX_RETRIES} retries"
                    ));
                }
                let delay = backoff_delay(retry_after_secs(&resp), MIN_BACKOFF);
                tracing::warn!(?delay, attempt, "lichess 429; backing off");
                tokio::time::sleep(delay).await;
                continue;
            }

            return resp
                .error_for_status()
                .context("lichess export request failed");
        }
    }
}

impl GameSource for Lichess {
    fn kind(&self) -> &'static str {
        "lichess"
    }
}

/// Ingest every complete game in `blob`, advancing `cursor` past the newest one
/// and bumping `imported`/`duplicates` in place — so a game already accounted
/// for survives even if a later game in the same blob fails to ingest (#197).
async fn ingest_blob(
    db: &DatabaseConnection,
    database_id: i32,
    blob: &str,
    cursor: &mut SyncCursor,
    imported: &mut usize,
    duplicates: &mut usize,
) -> Result<()> {
    for game in split_games(blob) {
        let ingested = ingest_pgn(db, database_id, &game)
            .await
            .context("ingesting lichess game")?;
        // Advance the cursor for every game seen (even a deduped re-fetch), so it
        // always tracks the newest game's timestamp.
        cursor.last_game_ms = advance_ms(cursor.last_game_ms, game_epoch_ms(&game));
        match ingested {
            Some(_) => *imported += 1,
            None => *duplicates += 1,
        }
    }
    Ok(())
}

/// `since` query value for an incremental sync: the epoch-ms of the last synced
/// game (or `None` for a first, full sync). It is deliberately *not* nudged
/// forward — Lichess game times are second-precision, so advancing past the
/// boundary would skip other games sharing that same second. The boundary game
/// is re-fetched and dropped by ingest dedup instead (issue #95).
fn since_param(cursor: &SyncCursor) -> Option<i64> {
    cursor.last_game_ms
}

/// Advance a cursor timestamp to the newer of the current value and `candidate`.
/// A game without a parseable timestamp leaves the cursor untouched.
fn advance_ms(current: Option<i64>, candidate: Option<i64>) -> Option<i64> {
    match (current, candidate) {
        (Some(cur), Some(new)) => Some(cur.max(new)),
        (cur, None) => cur,
        (None, new) => new,
    }
}

/// Byte offset at which the trailing, possibly-incomplete game begins, or `None`
/// when at most one game has arrived (nothing is provably complete yet). Games
/// are delimited by a line starting with `[Event `.
fn trailing_game_offset(buf: &[u8]) -> Option<usize> {
    let starts = event_offsets(buf);
    (starts.len() >= 2).then(|| starts[starts.len() - 1])
}

/// Game start time in epoch-ms parsed from the `UTCDate`/`UTCTime` tags (second
/// precision). `None` if either tag is missing or malformed.
fn game_epoch_ms(pgn: &str) -> Option<i64> {
    let date = tag_value(pgn, "UTCDate")?; // "YYYY.MM.DD"
    let time = tag_value(pgn, "UTCTime")?; // "HH:MM:SS"

    let mut d = date.split('.');
    let (y, mo, day) = (next_int(&mut d)?, next_int(&mut d)?, next_int(&mut d)?);
    let mut t = time.split(':');
    let (h, mi, s) = (next_int(&mut t)?, next_int(&mut t)?, next_int(&mut t)?);

    chrono::NaiveDate::from_ymd_opt(y, mo as u32, day as u32)?
        .and_hms_opt(h as u32, mi as u32, s as u32)
        .map(|dt| dt.and_utc().timestamp_millis())
}

fn next_int<'a>(parts: &mut impl Iterator<Item = &'a str>) -> Option<i32> {
    parts.next()?.trim().parse().ok()
}

/// Value of a PGN tag (`[Name "value"]`) from the header block, or `None`.
fn tag_value<'a>(pgn: &'a str, name: &str) -> Option<&'a str> {
    for line in pgn.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix('[') else {
            continue;
        };
        let Some(rest) = rest.strip_prefix(name) else {
            continue;
        };
        let rest = rest.trim_start().strip_prefix('"')?;
        return rest.split('"').next();
    }
    None
}

#[cfg(test)]
#[path = "lichess_tests.rs"]
mod tests;
