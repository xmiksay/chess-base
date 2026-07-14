//! Tests for [`super`] (header/metadata search). Split out to keep the
//! module under the project's 500-line file cap; the database/ELO filter and
//! `elo`-sort tests live in the `elo` submodule for the same reason.

#[path = "headers_elo_tests.rs"]
mod elo;

use std::collections::HashSet;

use super::*;
use crate::db::entities::databases;
use crate::db::{connect, DbConfig};
use crate::ingest::ingest_pgn;
use sea_orm::{ActiveModelTrait, Set};

fn user(id: &str) -> CurrentUser {
    CurrentUser {
        id: id.to_string(),
        is_admin: false,
    }
}

fn params() -> HeaderParams {
    HeaderParams::default()
}

fn query(p: HeaderParams) -> HeaderQuery {
    HeaderQuery::try_from(p).unwrap()
}

async fn db_with(owner: Option<&str>, pgns: &[&str]) -> (DatabaseConnection, i32) {
    let conn = connect(&DbConfig::in_memory()).await.unwrap();
    let db = databases::ActiveModel {
        owner_id: Set(owner.map(str::to_string)),
        name: Set("games".to_string()),
        kind: Set("own".to_string()),
        ..Default::default()
    }
    .insert(&conn)
    .await
    .unwrap();
    for pgn in pgns {
        ingest_pgn(&conn, db.id, pgn).await.unwrap();
    }
    (conn, db.id)
}

fn game(white: &str, black: &str, event: &str, eco: &str, date: &str, result: &str) -> String {
    format!(
        "[Event \"{event}\"]\n[White \"{white}\"]\n[Black \"{black}\"]\n[ECO \"{eco}\"]\n[Date \"{date}\"]\n[Result \"{result}\"]\n\n1. e4 e5 {result}\n"
    )
}

#[test]
fn cursor_round_trips_through_base64() {
    let c = Cursor {
        d: Some("1990.01.01".to_string()),
        e: None,
        id: 42,
    };
    let decoded = Cursor::decode(&c.encode().unwrap()).unwrap();
    assert_eq!(decoded, c);

    let id_only = Cursor {
        d: None,
        e: None,
        id: 7,
    };
    assert_eq!(Cursor::decode(&id_only.encode().unwrap()).unwrap(), id_only);

    let elo = Cursor {
        d: None,
        e: Some(5250),
        id: 3,
    };
    assert_eq!(Cursor::decode(&elo.encode().unwrap()).unwrap(), elo);
}

#[test]
fn garbage_cursor_is_rejected() {
    assert!(matches!(
        Cursor::decode("!!!not-base64!!!"),
        Err(HeaderSearchError::InvalidCursor)
    ));
    assert!(matches!(
        Cursor::decode(&URL_SAFE_NO_PAD.encode("not json")),
        Err(HeaderSearchError::InvalidCursor)
    ));
}

#[test]
fn params_validate_and_default() {
    let q = query(params());
    assert_eq!(q.sort, SortField::Date);
    assert_eq!(q.dir, SortDir::Desc);
    assert_eq!(q.limit, DEFAULT_LIMIT);
    assert!(q.player.is_none());

    // Blank strings normalize to unset.
    let q = query(HeaderParams {
        player: Some("   ".to_string()),
        ..params()
    });
    assert!(q.player.is_none());

    // Limit is clamped into range.
    let q = query(HeaderParams {
        limit: Some(10_000),
        ..params()
    });
    assert_eq!(q.limit, MAX_LIMIT);
}

#[test]
fn invalid_enum_params_are_rejected() {
    for p in [
        HeaderParams {
            color: Some("green".to_string()),
            ..params()
        },
        HeaderParams {
            sort: Some("rating".to_string()),
            ..params()
        },
        HeaderParams {
            elo_min: Some(2600),
            elo_max: Some(2500),
            ..params()
        },
        HeaderParams {
            dir: Some("sideways".to_string()),
            ..params()
        },
    ] {
        assert!(matches!(
            HeaderQuery::try_from(p),
            Err(HeaderSearchError::BadRequest(_))
        ));
    }
}

#[tokio::test]
async fn filters_by_player_event_eco_date_and_result() {
    let pgns = [
        game("Carlsen", "Nepo", "Tata Steel", "B90", "2021.01.16", "1-0"),
        game("Nepo", "Carlsen", "Candidates", "C42", "2021.04.20", "0-1"),
        game(
            "Ding",
            "Nakamura",
            "Tata Steel",
            "B33",
            "2022.01.20",
            "1/2-1/2",
        ),
    ];
    let refs: Vec<&str> = pgns.iter().map(String::as_str).collect();
    let (conn, _) = db_with(Some("alice"), &refs).await;
    let svc = HeaderSearchService::new(conn);

    // Player matches either color by default.
    let page = svc
        .search(
            &user("alice"),
            &query(HeaderParams {
                player: Some("Carlsen".to_string()),
                ..params()
            }),
        )
        .await
        .unwrap();
    assert_eq!(page.games.len(), 2);

    // Player constrained to the white side.
    let page = svc
        .search(
            &user("alice"),
            &query(HeaderParams {
                player: Some("Carlsen".to_string()),
                color: Some("white".to_string()),
                ..params()
            }),
        )
        .await
        .unwrap();
    assert_eq!(page.games.len(), 1);
    assert_eq!(page.games[0].white.as_deref(), Some("Carlsen"));

    // ECO prefix (B9 → B90, not B33).
    let page = svc
        .search(
            &user("alice"),
            &query(HeaderParams {
                eco: Some("B9".to_string()),
                ..params()
            }),
        )
        .await
        .unwrap();
    assert_eq!(page.games.len(), 1);
    assert_eq!(page.games[0].eco.as_deref(), Some("B90"));

    // Event substring spanning two games.
    let page = svc
        .search(
            &user("alice"),
            &query(HeaderParams {
                event: Some("Tata".to_string()),
                ..params()
            }),
        )
        .await
        .unwrap();
    assert_eq!(page.games.len(), 2);

    // Date range (2021 only).
    let page = svc
        .search(
            &user("alice"),
            &query(HeaderParams {
                date_from: Some("2021.01.01".to_string()),
                date_to: Some("2021.12.31".to_string()),
                ..params()
            }),
        )
        .await
        .unwrap();
    assert_eq!(page.games.len(), 2);

    // Result.
    let page = svc
        .search(
            &user("alice"),
            &query(HeaderParams {
                result: Some("1/2-1/2".to_string()),
                ..params()
            }),
        )
        .await
        .unwrap();
    assert_eq!(page.games.len(), 1);
}

#[tokio::test]
async fn player_event_filters_treat_like_wildcards_as_literals() {
    let pgns = [
        game(
            "Smith_J",
            "Real %Match",
            "100% Open",
            "C00",
            "2020.01.01",
            "1-0",
        ),
        game(
            "Smithers",
            "Decoy",
            "Closed Cup",
            "C00",
            "2020.01.02",
            "0-1",
        ),
    ];
    let refs: Vec<&str> = pgns.iter().map(String::as_str).collect();
    let (conn, _) = db_with(Some("alice"), &refs).await;
    let svc = HeaderSearchService::new(conn);

    // `_` must match a literal underscore, not "any character" — so "Smithers"
    // (which an unescaped `%Smith_%` would catch) is excluded.
    let page = svc
        .search(
            &user("alice"),
            &query(HeaderParams {
                player: Some("Smith_".to_string()),
                ..params()
            }),
        )
        .await
        .unwrap();
    assert_eq!(page.games.len(), 1);
    assert_eq!(page.games[0].white.as_deref(), Some("Smith_J"));

    // A bare `%` must match the literal percent sign, not every row.
    let page = svc
        .search(
            &user("alice"),
            &query(HeaderParams {
                event: Some("%".to_string()),
                ..params()
            }),
        )
        .await
        .unwrap();
    assert_eq!(page.games.len(), 1);
    // Only the "100% Open" game (white "Smith_J") carries a literal '%'.
    assert_eq!(page.games[0].white.as_deref(), Some("Smith_J"));
}

#[tokio::test]
async fn keyset_pagination_walks_all_rows_without_overlap() {
    let pgns: Vec<String> = (0..5)
        .map(|i| game("A", "B", "E", "C00", &format!("2020.01.0{}", i + 1), "1-0"))
        .collect();
    let refs: Vec<&str> = pgns.iter().map(String::as_str).collect();
    let (conn, _) = db_with(Some("alice"), &refs).await;
    let svc = HeaderSearchService::new(conn);

    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = svc
            .search(
                &user("alice"),
                &query(HeaderParams {
                    limit: Some(2),
                    cursor: cursor.clone(),
                    ..params()
                }),
            )
            .await
            .unwrap();
        seen.extend(page.games.iter().map(|g| g.id));
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    // Every game once, newest date first, no duplicates.
    assert_eq!(seen.len(), 5);
    let unique: HashSet<i32> = seen.iter().copied().collect();
    assert_eq!(unique.len(), 5);
    let mut sorted = seen.clone();
    sorted.sort_unstable();
    sorted.reverse();
    assert_eq!(seen, sorted);
}

#[tokio::test]
async fn scope_excludes_other_users_and_includes_global() {
    let conn = connect(&DbConfig::in_memory()).await.unwrap();
    let mk = |owner: Option<&str>, name: &str| databases::ActiveModel {
        owner_id: Set(owner.map(str::to_string)),
        name: Set(name.to_string()),
        kind: Set("own".to_string()),
        ..Default::default()
    };
    let alice_db = mk(Some("alice"), "alice").insert(&conn).await.unwrap().id;
    let bob_db = mk(Some("bob"), "bob").insert(&conn).await.unwrap().id;
    let global_db = mk(None, "masters").insert(&conn).await.unwrap().id;
    ingest_pgn(
        &conn,
        alice_db,
        &game("A", "B", "E", "C00", "2020.01.01", "1-0"),
    )
    .await
    .unwrap();
    ingest_pgn(
        &conn,
        bob_db,
        &game("X", "Y", "E", "C00", "2020.01.02", "0-1"),
    )
    .await
    .unwrap();
    ingest_pgn(
        &conn,
        global_db,
        &game("M", "N", "E", "C00", "2020.01.03", "1/2-1/2"),
    )
    .await
    .unwrap();
    let svc = HeaderSearchService::new(conn);

    // Alice sees her own game plus the global one, never bob's.
    let page = svc.search(&user("alice"), &query(params())).await.unwrap();
    let dbs: HashSet<i32> = page.games.iter().map(|g| g.database_id).collect();
    assert_eq!(page.games.len(), 2);
    assert!(dbs.contains(&alice_db));
    assert!(dbs.contains(&global_db));
    assert!(!dbs.contains(&bob_db));
}
