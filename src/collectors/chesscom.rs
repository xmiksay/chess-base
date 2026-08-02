//! Chess.com Published-Data adapter (no authentication required).
//!
//! Flow: list monthly archives at `/pub/player/{user}/games/archives`, then
//! fetch each month's multi-game PGN at `{archive}/pgn`. Requests are serialized
//! (one month at a time) to respect the public-API rate limits; an HTTP 429 is
//! retried with a back-off. Incremental sync resumes from the last fully-synced
//! month held in [`SyncCursor::last_month`].
//!
//! As with [`lichess`](super::lichess), all archive/month/back-off decisions live
//! in the pure helpers below so they can be unit-tested without the network.

use anyhow::{anyhow, Context, Result};
use sea_orm::DatabaseConnection;
use std::time::Duration;

use super::{backoff_delay, retry_after_secs, GameSource, SyncCursor, SyncFailure, SyncOutcome};
use crate::ingest::ingest_pgn_all;

const API_BASE: &str = "https://api.chess.com/pub";

/// Polite back-off floor on HTTP 429 (chess.com does not mandate a minimum, but
/// honours `Retry-After`).
const MIN_BACKOFF: Duration = Duration::from_secs(2);

/// Number of 429 retries before giving up on a request.
const MAX_RETRIES: u32 = 5;

/// Chess.com rejects requests without a descriptive `User-Agent`.
const USER_AGENT: &str = concat!("chess-base/", env!("CARGO_PKG_VERSION"));

pub struct ChessCom {
    pub username: String,
    /// The Published-Data API root. Always `API_BASE` outside tests; overridden
    /// by [`with_base_url`](Self::with_base_url) to point at a local mock server
    /// so mid-run-failure behaviour can be exercised deterministically.
    base_url: String,
}

impl ChessCom {
    pub fn new(username: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            base_url: API_BASE.to_string(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Endpoint returning the list of available monthly archive URLs.
    pub fn archives_url(&self) -> String {
        format!("{}/player/{}/games/archives", self.base_url, self.username)
    }

    /// Multi-game PGN endpoint for one month.
    pub fn month_pgn_url(&self, year: u16, month: u8) -> String {
        format!(
            "{}/player/{}/games/{year:04}/{month:02}/pgn",
            self.base_url, self.username
        )
    }

    /// Sync this user's games into `database_id`, resuming from `cursor`.
    ///
    /// Lists the monthly archives, fetches every month at or after the cursor and
    /// ingests its games, returning the cursor advanced to the latest month
    /// synced. The cursor month is re-synced so games added to it after the last
    /// sync are caught; games already stored are deduped by ingest (issue #95),
    /// so a re-sync never doubles them. A mid-run failure (one bad month fetch)
    /// returns [`SyncFailure`] with the cursor left at the last *fully* synced
    /// month, so the next run resumes from there instead of restarting (#197).
    pub async fn sync(
        &self,
        db: &DatabaseConnection,
        database_id: i32,
        cursor: SyncCursor,
    ) -> Result<SyncOutcome, SyncFailure> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
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
        let archives_body = match self.fetch_text(client, &self.archives_url()).await {
            Ok(b) => b,
            Err(e) => {
                return Err(SyncFailure {
                    cursor,
                    imported: 0,
                    duplicates: 0,
                    error: e,
                })
            }
        };
        let archives = match parse_archives(&archives_body) {
            Ok(a) => a,
            Err(e) => {
                return Err(SyncFailure {
                    cursor,
                    imported: 0,
                    duplicates: 0,
                    error: e,
                })
            }
        };
        let months = months_to_sync(&archives, cursor.last_month.as_deref());

        let mut imported = 0usize;
        let mut duplicates = 0usize;
        for (month, pgn_url) in months {
            let pgn = match self.fetch_text(client, &pgn_url).await {
                Ok(p) => p,
                Err(e) => {
                    return Err(SyncFailure {
                        cursor,
                        imported,
                        duplicates,
                        error: e,
                    })
                }
            };
            // A month archive always has games, but guard against an empty body
            // so a blank month never trips the empty-PGN rejection.
            if !pgn.trim().is_empty() {
                let report = match ingest_pgn_all(db, database_id, &pgn)
                    .await
                    .with_context(|| format!("ingesting chess.com month {month}"))
                {
                    Ok(r) => r,
                    Err(e) => {
                        return Err(SyncFailure {
                            cursor,
                            imported,
                            duplicates,
                            error: e,
                        })
                    }
                };
                if !report.errors.is_empty() {
                    tracing::warn!(
                        month = %month,
                        skipped = report.errors.len(),
                        "skipped malformed games in chess.com archive"
                    );
                }
                imported += report.imported.len();
                duplicates += report.duplicates;
            }
            cursor.last_month = Some(month);
        }

        Ok(SyncOutcome {
            cursor,
            imported,
            duplicates,
        })
    }

    /// Issue a GET, honouring HTTP 429 with a back-off and a bounded number of
    /// retries, and return the response body as text.
    async fn fetch_text(&self, client: &reqwest::Client, url: &str) -> Result<String> {
        let mut attempt = 0u32;
        loop {
            let resp = client
                .get(url)
                .send()
                .await
                .context("requesting chess.com")?;

            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                attempt += 1;
                if attempt > MAX_RETRIES {
                    return Err(anyhow!(
                        "chess.com rate limit: gave up after {MAX_RETRIES} retries"
                    ));
                }
                let delay = backoff_delay(retry_after_secs(&resp), MIN_BACKOFF);
                tracing::warn!(?delay, attempt, "chess.com 429; backing off");
                tokio::time::sleep(delay).await;
                continue;
            }

            let resp = resp
                .error_for_status()
                .context("chess.com request failed")?;
            return resp.text().await.context("reading chess.com response body");
        }
    }
}

impl GameSource for ChessCom {
    fn kind(&self) -> &'static str {
        "chesscom"
    }
}

/// Parse the archive-list response (`{ "archives": [url, …] }`) into its URLs.
fn parse_archives(body: &str) -> Result<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct Archives {
        archives: Vec<String>,
    }
    let parsed: Archives = serde_json::from_str(body).context("parsing chess.com archives list")?;
    Ok(parsed.archives)
}

/// The `"YYYY/MM"` month key from an archive URL's trailing `…/games/YYYY/MM`,
/// or `None` if the tail is not two numeric segments of the expected width.
fn month_key(url: &str) -> Option<String> {
    let mut segs = url.trim_end_matches('/').rsplit('/');
    let month = segs.next()?;
    let year = segs.next()?;
    let numeric = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if year.len() == 4 && month.len() == 2 && numeric(year) && numeric(month) {
        Some(format!("{year}/{month}"))
    } else {
        None
    }
}

/// The months to sync, ascending: every archive at or after `last_month` (all of
/// them on a first sync), paired with its `…/pgn` URL. String comparison on the
/// `"YYYY/MM"` key is chronological. The cursor month itself is re-synced (`>=`,
/// not `>`) so games added to it after the last sync are not missed forever; the
/// already-imported games are deduped by ingest (issue #95).
fn months_to_sync(archives: &[String], last_month: Option<&str>) -> Vec<(String, String)> {
    let mut months: Vec<(String, String)> = archives
        .iter()
        .filter_map(|url| {
            let key = month_key(url)?;
            if last_month.is_none_or(|last| key.as_str() >= last) {
                Some((key, format!("{}/pgn", url.trim_end_matches('/'))))
            } else {
                None
            }
        })
        .collect();
    months.sort_by(|a, b| a.0.cmp(&b.0));
    months
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARCHIVES: &str = r#"{"archives":[
        "https://api.chess.com/pub/player/hikaru/games/2023/12",
        "https://api.chess.com/pub/player/hikaru/games/2024/01",
        "https://api.chess.com/pub/player/hikaru/games/2024/02"
    ]}"#;

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

    #[test]
    fn parses_archive_url_list() {
        let urls = parse_archives(ARCHIVES).unwrap();
        assert_eq!(urls.len(), 3);
        assert!(urls[0].ends_with("2023/12"));
        assert!(parse_archives("not json").is_err());
    }

    #[test]
    fn extracts_month_key_from_archive_url() {
        assert_eq!(
            month_key("https://api.chess.com/pub/player/hikaru/games/2024/01"),
            Some("2024/01".to_string())
        );
        // Trailing slash tolerated.
        assert_eq!(
            month_key("https://api.chess.com/pub/player/hikaru/games/2024/01/"),
            Some("2024/01".to_string())
        );
        // Non-numeric / wrong width tails are rejected.
        assert_eq!(month_key("https://api.chess.com/.../games/archives"), None);
        assert_eq!(month_key("https://api.chess.com/.../2024/1"), None);
    }

    #[test]
    fn first_sync_returns_every_month_ascending_with_pgn_urls() {
        let urls = parse_archives(ARCHIVES).unwrap();
        let months = months_to_sync(&urls, None);
        let keys: Vec<&str> = months.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["2023/12", "2024/01", "2024/02"]);
        assert_eq!(
            months[0].1,
            "https://api.chess.com/pub/player/hikaru/games/2023/12/pgn"
        );
    }

    #[test]
    fn incremental_sync_reincludes_the_cursor_month() {
        let urls = parse_archives(ARCHIVES).unwrap();
        // The cursor month is re-synced (its later games are caught; already-stored
        // ones are deduped by ingest) — only strictly earlier months are skipped.
        let months = months_to_sync(&urls, Some("2024/01"));
        let keys: Vec<&str> = months.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["2024/01", "2024/02"]);
    }

    #[test]
    fn backoff_honours_retry_after_above_the_floor() {
        assert_eq!(backoff_delay(None, MIN_BACKOFF), MIN_BACKOFF);
        assert_eq!(backoff_delay(Some(1), MIN_BACKOFF), MIN_BACKOFF); // below floor ⇒ floored
        assert_eq!(
            backoff_delay(Some(30), MIN_BACKOFF),
            Duration::from_secs(30)
        );
    }

    // --- mid-run failure / resume, against a local mock server (issue #197) ---

    use crate::db::entities::{databases, games};
    use crate::db::{connect, DbConfig};
    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;
    use axum::Router;
    use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
    use std::collections::HashMap;
    use std::sync::Arc;

    async fn own_database() -> (DatabaseConnection, i32) {
        let conn = connect(&DbConfig::in_memory()).await.unwrap();
        let db = databases::ActiveModel {
            owner_id: Set(Some("alice".to_string())),
            name: Set("Alice's chess.com".to_string()),
            kind: Set("chesscom".to_string()),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();
        (conn, db.id)
    }

    fn game_pgn(white: &str) -> String {
        format!(
            "[Event \"Casual\"]\n[White \"{white}\"]\n[Black \"Opponent\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0\n"
        )
    }

    #[derive(Clone)]
    struct MockChessCom {
        archives_json: Arc<String>,
        // "YYYY/MM" -> Ok(pgn body) or Err(http status).
        months: Arc<HashMap<String, Result<String, u16>>>,
    }

    async fn archives_handler(State(state): State<MockChessCom>) -> Response {
        (*state.archives_json).clone().into_response()
    }

    async fn month_handler(
        State(state): State<MockChessCom>,
        Path((year, month)): Path<(String, String)>,
    ) -> Response {
        match state.months.get(&format!("{year}/{month}")) {
            Some(Ok(pgn)) => pgn.clone().into_response(),
            Some(Err(status)) => (
                StatusCode::from_u16(*status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                "mock failure",
            )
                .into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    /// Spawn a local mock Published-Data server exposing `archive_months`
    /// (ascending) and the fixed `months` responses, returning its base URL
    /// (to feed [`ChessCom::with_base_url`]).
    async fn start_mock(
        archive_months: &[&str],
        months: HashMap<String, Result<String, u16>>,
    ) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let archive_urls: Vec<String> = archive_months
            .iter()
            .map(|m| format!("{base}/player/mocker/games/{m}"))
            .collect();
        let state = MockChessCom {
            archives_json: Arc::new(serde_json::json!({ "archives": archive_urls }).to_string()),
            months: Arc::new(months),
        };
        let app = Router::new()
            .route("/player/mocker/games/archives", get(archives_handler))
            .route(
                "/player/mocker/games/{year}/{month}/pgn",
                get(month_handler),
            )
            .with_state(state);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base
    }

    #[tokio::test]
    async fn sync_mid_run_failure_persists_cursor_and_counts_up_to_the_failure() {
        let (conn, database_id) = own_database().await;
        let mut months = HashMap::new();
        months.insert("2024/01".to_string(), Ok(game_pgn("Alpha")));
        months.insert("2024/02".to_string(), Err(500u16));
        let base = start_mock(&["2024/01", "2024/02"], months).await;

        let failure = ChessCom::new("mocker")
            .with_base_url(base)
            .sync(&conn, database_id, SyncCursor::default())
            .await
            .unwrap_err();

        // The cursor/counts reflect the fully-synced 2024/01 only — the failed
        // month never advanced it — so a retry resumes instead of restarting.
        assert_eq!(failure.cursor.last_month, Some("2024/01".to_string()));
        assert_eq!(failure.imported, 1);
        assert_eq!(failure.duplicates, 0);
        assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn resumed_sync_does_not_refetch_months_before_the_cursor() {
        let (conn, database_id) = own_database().await;
        let mut months = HashMap::new();
        // Predates the persisted cursor — must never be fetched on resume.
        months.insert("2023/12".to_string(), Ok(game_pgn("Early")));
        months.insert("2024/01".to_string(), Ok(game_pgn("Alpha")));
        months.insert("2024/02".to_string(), Ok(game_pgn("Bravo")));
        let base = start_mock(&["2023/12", "2024/01", "2024/02"], months).await;

        let cursor = SyncCursor {
            last_month: Some("2024/01".to_string()),
            ..Default::default()
        };
        let outcome = ChessCom::new("mocker")
            .with_base_url(base)
            .sync(&conn, database_id, cursor)
            .await
            .unwrap();

        assert_eq!(outcome.cursor.last_month, Some("2024/02".to_string()));
        // 2024/01 (the cursor month) is re-synced, 2024/02 is new; 2023/12 is
        // skipped entirely — a resumed sync picks up where it left off instead
        // of restarting from scratch (#197).
        assert_eq!(outcome.imported, 2);
        assert_eq!(games::Entity::find().all(&conn).await.unwrap().len(), 2);
    }
}
