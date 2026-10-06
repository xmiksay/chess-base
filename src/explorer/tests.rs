use super::*;
use crate::position::STARTPOS_FEN;

const SAMPLE: &str = r#"{
  "white": 10, "draws": 20, "black": 5,
  "moves": [
    {"uci":"e2e4","san":"e4","averageRating":2400,"white":6,"draws":10,"black":2,"game":null},
    {"uci":"d2d4","san":"d4","white":4,"draws":10,"black":3}
  ],
  "topGames": [
    {"uci":"e2e4","id":"abcd1234","winner":"white",
     "white":{"name":"Kasparov, G.","rating":2851},
     "black":{"name":"Karpov, A.","rating":2780},"year":1990,"month":"1990-10"},
    {"id":"wxyz9876","winner":null,"white":{"name":"A"},"black":{"name":"B"}}
  ],
  "opening": {"eco":"A00","name":"Start"}
}"#;

fn query(since: Option<u16>, until: Option<u16>) -> MastersQuery {
    MastersQuery {
        fen: STARTPOS_FEN.to_string(),
        since,
        until,
    }
}

/// A client whose upstream is a closed port, so any fetch fails fast.
fn offline_client() -> MastersClient {
    MastersClient::new("http://127.0.0.1:1", "token").expect("client")
}

#[test]
fn parse_report_normalises_counts_and_games() {
    let r = parse_report(SAMPLE).expect("parse");
    assert_eq!((r.total, r.white, r.draws, r.black), (35, 10, 20, 5));
    assert_eq!(r.opening.as_ref().map(|o| o.eco.as_str()), Some("A00"));
    assert_eq!(r.moves[0].san, "e4");
    assert_eq!(r.moves[0].count, 18);
    assert_eq!(r.moves[0].average_rating, Some(2400));
    assert_eq!(r.moves[1].average_rating, None);
    let g = &r.top_games[0];
    assert_eq!(g.id, "abcd1234");
    assert_eq!(g.white, "Kasparov, G.");
    assert_eq!(g.white_rating, Some(2851));
    assert_eq!(g.year, Some(1990));
    assert_eq!(r.top_games[1].winner, None);
    assert_eq!(r.top_games[1].black_rating, None);
}

#[test]
fn parse_report_tolerates_an_empty_position() {
    let r =
        parse_report(r#"{"white":0,"draws":0,"black":0,"moves":[],"topGames":[],"opening":null}"#)
            .expect("parse");
    assert_eq!(r.total, 0);
    assert!(r.moves.is_empty() && r.top_games.is_empty() && r.opening.is_none());
}

#[test]
fn validate_rejects_bad_fen_and_year_windows() {
    assert!(query(None, None).validate().is_ok());
    assert!(query(Some(1990), Some(2000)).validate().is_ok());
    assert!(query(Some(2000), Some(1990)).validate().is_err());
    assert!(query(Some(42), None).validate().is_err());
    let bad = MastersQuery {
        fen: "not a fen".into(),
        ..Default::default()
    };
    assert!(matches!(bad.validate(), Err(MastersError::BadRequest(_))));
}

#[test]
fn game_ids_must_be_short_alphanumerics() {
    assert!(valid_game_id("abcd1234"));
    assert!(!valid_game_id(""));
    assert!(!valid_game_id("../etc"));
    assert!(!valid_game_id("a?b=c"));
    assert!(!valid_game_id(&"a".repeat(17)));
}

#[tokio::test]
async fn fresh_cache_hit_skips_the_network() {
    let client = offline_client();
    let q = query(None, None);
    let report = parse_report(SAMPLE).expect("parse");
    client.store(q.cache_key(), report.clone());
    assert_eq!(client.report(&q).await.expect("cached"), report);
}

#[tokio::test]
async fn stale_entry_is_served_when_upstream_fails() {
    let client = offline_client();
    let q = query(Some(1950), None);
    let report = parse_report(SAMPLE).expect("parse");
    client.store(q.cache_key(), report.clone());
    if let Ok(mut cache) = client.cache.lock() {
        if let Some(c) = cache.get_mut(&q.cache_key()) {
            c.at = Instant::now() - CACHE_TTL - Duration::from_secs(1);
        }
    }
    assert_eq!(client.report(&q).await.expect("stale"), report);
}

#[tokio::test]
async fn upstream_failure_without_cache_is_an_error() {
    let client = offline_client();
    let err = client.report(&query(None, None)).await.unwrap_err();
    assert!(matches!(err, MastersError::Upstream(_)));
}

#[tokio::test]
async fn rate_limit_backoff_short_circuits_requests() {
    let client = offline_client();
    if let Ok(mut until) = client.backoff_until.lock() {
        *until = Some(Instant::now() + RATE_LIMIT_BACKOFF);
    }
    let err = client.report(&query(None, None)).await.unwrap_err();
    assert!(matches!(err, MastersError::RateLimited));
    let err = client.game_pgn("abcd1234").await.unwrap_err();
    assert!(matches!(err, MastersError::RateLimited));
}

#[tokio::test]
async fn invalid_game_id_never_reaches_upstream() {
    let err = offline_client().game_pgn("../x").await.unwrap_err();
    assert!(matches!(err, MastersError::BadRequest(_)));
}

#[test]
fn cache_is_bounded() {
    let client = offline_client();
    let report = parse_report(SAMPLE).expect("parse");
    for i in 0..=CACHE_CAP {
        client.store(format!("k{i}"), report.clone());
    }
    let len = client.cache.lock().map(|c| c.len()).unwrap_or(0);
    assert_eq!(len, CACHE_CAP);
}
