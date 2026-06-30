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

use super::{backoff_delay, retry_after_secs, GameSource, SyncCursor, SyncOutcome};
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
        let mut url = format!("{API_BASE}/api/games/user/{}?pgnInJson=false", self.username);
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
    pub async fn sync(
        &self,
        db: &DatabaseConnection,
        database_id: i32,
        cursor: SyncCursor,
    ) -> Result<SyncOutcome> {
        let client = reqwest::Client::builder()
            .build()
            .context("building http client")?;
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
    ) -> Result<SyncOutcome> {
        let url = self.games_url(since_param(&cursor));
        let mut resp = self.fetch_with_backoff(client, &url).await?;

        let mut imported = 0usize;
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = resp.chunk().await.context("streaming lichess export")? {
            buf.extend_from_slice(&chunk);
            // Drain every game that is provably complete (i.e. a later game has
            // started), leaving the trailing partial game in the buffer.
            if let Some(split) = trailing_game_offset(&buf) {
                let tail = buf.split_off(split);
                let head = std::mem::replace(&mut buf, tail);
                imported += ingest_blob(
                    db,
                    database_id,
                    &String::from_utf8_lossy(&head),
                    &mut cursor,
                )
                .await?;
            }
        }
        // Flush the final game once the stream is exhausted.
        imported +=
            ingest_blob(db, database_id, &String::from_utf8_lossy(&buf), &mut cursor).await?;

        Ok(SyncOutcome { cursor, imported })
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

/// Ingest every complete game in `blob`, advancing `cursor` past the newest one.
/// Returns the number of games ingested.
async fn ingest_blob(
    db: &DatabaseConnection,
    database_id: i32,
    blob: &str,
    cursor: &mut SyncCursor,
) -> Result<usize> {
    let mut imported = 0;
    for game in split_games(blob) {
        let ingested = ingest_pgn(db, database_id, &game)
            .await
            .context("ingesting lichess game")?;
        // Advance the cursor for every game seen (even a deduped re-fetch), so it
        // always tracks the newest game's timestamp.
        cursor.last_game_ms = advance_ms(cursor.last_game_ms, game_epoch_ms(&game));
        if ingested.is_some() {
            imported += 1;
        }
    }
    Ok(imported)
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
mod tests {
    use super::*;
    use crate::db::entities::{databases, games};
    use crate::db::{connect, DbConfig};
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};

    // Two-game export blob as Lichess streams it: `[Event ` per game, blank-line
    // separated, second game one minute after the first.
    const TWO_GAMES: &str = "[Event \"Rated blitz game\"]\n[Site \"https://lichess.org/abcd1234\"]\n[White \"alice\"]\n[Black \"bob\"]\n[Result \"1-0\"]\n[UTCDate \"2024.01.15\"]\n[UTCTime \"20:30:45\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n\n\n[Event \"Rated blitz game\"]\n[Site \"https://lichess.org/efgh5678\"]\n[White \"carol\"]\n[Black \"alice\"]\n[Result \"0-1\"]\n[UTCDate \"2024.01.15\"]\n[UTCTime \"20:31:45\"]\n\n1. d4 d5 2. c4 e6 0-1\n";

    #[test]
    fn builds_export_url_with_since() {
        let src = Lichess::new("DrNykterstein");
        assert_eq!(
            src.games_url(Some(1700000000000)),
            "https://lichess.org/api/games/user/DrNykterstein?pgnInJson=false&since=1700000000000"
        );
        assert_eq!(
            src.games_url(None),
            "https://lichess.org/api/games/user/DrNykterstein?pgnInJson=false"
        );
        assert_eq!(src.kind(), "lichess");
    }

    #[test]
    fn first_sync_has_no_since_then_resumes_at_last_game() {
        // No cursor ⇒ full sync.
        assert_eq!(since_param(&SyncCursor::default()), None);
        // With a cursor ⇒ resume *at* the last synced game's second (not past it),
        // so games sharing that second are not skipped. The boundary game is
        // deduped by ingest rather than skipped by the cursor (issue #95).
        let cursor = SyncCursor {
            last_game_ms: Some(1_705_350_645_000),
            ..Default::default()
        };
        assert_eq!(since_param(&cursor), Some(1_705_350_645_000));
    }

    #[test]
    fn cursor_advances_to_newest_game_only() {
        assert_eq!(advance_ms(None, Some(100)), Some(100));
        assert_eq!(advance_ms(Some(100), Some(50)), Some(100)); // older ignored
        assert_eq!(advance_ms(Some(100), Some(200)), Some(200));
        assert_eq!(advance_ms(Some(100), None), Some(100)); // untimed game
        assert_eq!(advance_ms(None, None), None);
    }

    #[test]
    fn backoff_is_at_least_one_minute() {
        assert_eq!(backoff_delay(None, MIN_BACKOFF), Duration::from_secs(60));
        // Server asks for less than the mandated minimum ⇒ floored to 60s.
        assert_eq!(
            backoff_delay(Some(10), MIN_BACKOFF),
            Duration::from_secs(60)
        );
        // Server asks for longer ⇒ honoured.
        assert_eq!(
            backoff_delay(Some(120), MIN_BACKOFF),
            Duration::from_secs(120)
        );
    }

    #[test]
    fn splits_stream_into_individual_games() {
        let games = split_games(TWO_GAMES);
        assert_eq!(games.len(), 2);
        assert!(games[0].contains("Qxf7#"));
        assert!(games[1].contains("carol"));
        assert!(games[1].starts_with("[Event "));
    }

    #[test]
    fn trailing_offset_withholds_the_last_partial_game() {
        // One game so far ⇒ nothing provably complete.
        let one = b"[Event \"x\"]\n\n1. e4 *";
        assert_eq!(trailing_game_offset(one), None);
        // Two markers ⇒ the second game's start is the withhold point.
        let split = trailing_game_offset(TWO_GAMES.as_bytes()).unwrap();
        assert!(TWO_GAMES[split..].starts_with("[Event "));
        assert!(TWO_GAMES[..split].contains("Qxf7#"));
        assert!(!TWO_GAMES[..split].contains("carol"));
    }

    #[test]
    fn parses_game_timestamp_from_utc_tags() {
        let games = split_games(TWO_GAMES);
        // 2024-01-15 20:30:45 UTC.
        assert_eq!(game_epoch_ms(&games[0]), Some(1_705_350_645_000));
        // One minute later.
        assert_eq!(game_epoch_ms(&games[1]), Some(1_705_350_705_000));
        assert_eq!(game_epoch_ms("[Event \"x\"]\n\n1. e4 *"), None);
    }

    async fn own_database() -> (DatabaseConnection, i32) {
        let conn = connect(&DbConfig::in_memory()).await.unwrap();
        let db = databases::ActiveModel {
            owner_id: Set(Some("alice".to_string())),
            name: Set("Alice's lichess".to_string()),
            kind: Set("lichess".to_string()),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();
        (conn, db.id)
    }

    #[tokio::test]
    async fn ingests_a_blob_and_advances_cursor_to_newest_game() {
        let (conn, database_id) = own_database().await;
        let mut cursor = SyncCursor::default();

        let imported = ingest_blob(&conn, database_id, TWO_GAMES, &mut cursor)
            .await
            .unwrap();

        assert_eq!(imported, 2);
        assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 2);
        // Cursor sits on the newer (second) game.
        assert_eq!(cursor.last_game_ms, Some(1_705_350_705_000));
        // A re-sync resumes *at* it; the boundary game is deduped, not skipped.
        assert_eq!(since_param(&cursor), Some(1_705_350_705_000));
    }

    #[tokio::test]
    async fn resync_with_only_new_games_appends_without_rewinding_cursor() {
        let (conn, database_id) = own_database().await;
        let mut cursor = SyncCursor::default();
        ingest_blob(&conn, database_id, TWO_GAMES, &mut cursor)
            .await
            .unwrap();

        // A later sync returns a single, newer game; the cursor only moves forward.
        let newer = "[Event \"Rated blitz game\"]\n[White \"alice\"]\n[Black \"dave\"]\n[Result \"1-0\"]\n[UTCDate \"2024.01.16\"]\n[UTCTime \"09:00:00\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n";
        let imported = ingest_blob(&conn, database_id, newer, &mut cursor)
            .await
            .unwrap();

        assert_eq!(imported, 1);
        assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 3);
        assert_eq!(cursor.last_game_ms, Some(1_705_395_600_000)); // 2024-01-16 09:00 UTC
    }

    #[tokio::test]
    async fn re_ingesting_the_same_blob_imports_nothing() {
        let (conn, database_id) = own_database().await;
        let mut cursor = SyncCursor::default();
        ingest_blob(&conn, database_id, TWO_GAMES, &mut cursor)
            .await
            .unwrap();

        // The boundary re-fetch a resumed sync produces: the same two games (same
        // Lichess permalinks) are deduped, so nothing is added and the cursor
        // still tracks the newest game.
        let again = ingest_blob(&conn, database_id, TWO_GAMES, &mut cursor)
            .await
            .unwrap();

        assert_eq!(again, 0);
        assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 2);
        assert_eq!(cursor.last_game_ms, Some(1_705_350_705_000));
    }
}
