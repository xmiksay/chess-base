//! Tests for the database/ELO header-search additions: the `database_id`
//! scope-checked filter, the both-players ELO range, and the average-keyed
//! `elo` sort with its keyset cursor. Sibling of `headers_tests.rs` (included
//! from there) to stay under the file-size cap.

use super::*;

/// A PGN whose ELO tags are optional, so tests can exercise missing ratings.
fn rated_game(white_elo: Option<i32>, black_elo: Option<i32>, date: &str) -> String {
    let mut tags = format!(
        "[Event \"E\"]\n[White \"A\"]\n[Black \"B\"]\n[Date \"{date}\"]\n[Result \"1-0\"]\n"
    );
    if let Some(w) = white_elo {
        tags.push_str(&format!("[WhiteElo \"{w}\"]\n"));
    }
    if let Some(b) = black_elo {
        tags.push_str(&format!("[BlackElo \"{b}\"]\n"));
    }
    format!("{tags}\n1. e4 e5 1-0\n")
}

/// An in-memory DB owned by "alice" holding one game per `(white, black)` ELO pair.
async fn rated_db(elos: &[(Option<i32>, Option<i32>)]) -> (DatabaseConnection, i32) {
    let pgns: Vec<String> = elos
        .iter()
        .enumerate()
        .map(|(i, (w, b))| rated_game(*w, *b, &format!("2020.01.{:02}", i + 1)))
        .collect();
    let refs: Vec<&str> = pgns.iter().map(String::as_str).collect();
    db_with(Some("alice"), &refs).await
}

/// The white+black ELO sum of each returned game (`None` when either is missing).
fn sums(page: &HeaderPage) -> Vec<Option<i32>> {
    page.games
        .iter()
        .map(|g| g.white_elo.zip(g.black_elo).map(|(w, b)| w + b))
        .collect()
}

#[tokio::test]
async fn elo_bounds_apply_to_both_players_and_exclude_unrated() {
    let (conn, _) = rated_db(&[
        (Some(2600), Some(2700)),
        (Some(2400), Some(2450)),
        (None, Some(2700)), // half-rated
        (None, None),       // unrated
    ])
    .await;
    let svc = HeaderSearchService::new(conn);
    let alice = user("alice");

    // No bound set: ratings play no role, everything comes back.
    let page = svc.search(&alice, &query(params())).await.unwrap();
    assert_eq!(page.games.len(), 4);

    // elo_min alone: BOTH players must reach it — the half-rated game with a
    // 2700 black is excluded because its white ELO is missing.
    let page = svc
        .search(
            &alice,
            &query(HeaderParams {
                elo_min: Some(2500),
                ..params()
            }),
        )
        .await
        .unwrap();
    assert_eq!(page.games.len(), 1);
    assert_eq!(page.games[0].white_elo, Some(2600));

    // elo_max alone: both players must stay at or under it.
    let page = svc
        .search(
            &alice,
            &query(HeaderParams {
                elo_max: Some(2500),
                ..params()
            }),
        )
        .await
        .unwrap();
    assert_eq!(page.games.len(), 1);
    assert_eq!(page.games[0].white_elo, Some(2400));

    // A band wide enough for both fully-rated games still drops the unrated ones.
    let page = svc
        .search(
            &alice,
            &query(HeaderParams {
                elo_min: Some(2300),
                elo_max: Some(2800),
                ..params()
            }),
        )
        .await
        .unwrap();
    assert_eq!(page.games.len(), 2);
}

#[tokio::test]
async fn database_id_filter_honours_scope() {
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
    for db in [alice_db, bob_db, global_db] {
        ingest_pgn(&conn, db, &rated_game(None, None, "2020.01.01"))
            .await
            .unwrap();
    }
    let svc = HeaderSearchService::new(conn);
    let by_db = |id: i32| {
        query(HeaderParams {
            database_id: Some(id),
            ..params()
        })
    };

    // Her own database and a global one each pin the search to that collection.
    for db in [alice_db, global_db] {
        let page = svc.search(&user("alice"), &by_db(db)).await.unwrap();
        assert_eq!(page.games.len(), 1);
        assert_eq!(page.games[0].database_id, db);
    }

    // Someone else's database is hidden as not-found, not an empty page.
    assert!(matches!(
        svc.search(&user("alice"), &by_db(bob_db)).await,
        Err(HeaderSearchError::NotFound)
    ));
}

#[tokio::test]
async fn elo_sort_orders_by_average_with_unrated_last() {
    let (conn, _) = rated_db(&[
        (Some(2000), Some(2100)), // sum 4100
        (Some(2500), Some(2500)), // sum 5000
        (Some(2200), Some(2600)), // sum 4800
        (None, Some(2800)),       // half-rated → last
        (None, None),             // unrated → last
    ])
    .await;
    let svc = HeaderSearchService::new(conn);
    let sorted = |dir: &str| {
        query(HeaderParams {
            sort: Some("elo".to_string()),
            dir: Some(dir.to_string()),
            ..params()
        })
    };

    let page = svc.search(&user("alice"), &sorted("desc")).await.unwrap();
    assert_eq!(
        sums(&page),
        vec![Some(5000), Some(4800), Some(4100), None, None]
    );

    // Ascending flips the rated order but the unrated games still trail.
    let page = svc.search(&user("alice"), &sorted("asc")).await.unwrap();
    assert_eq!(
        sums(&page),
        vec![Some(4100), Some(4800), Some(5000), None, None]
    );
}

#[tokio::test]
async fn elo_sort_keyset_paginates_without_dupes_or_gaps() {
    let (conn, _) = rated_db(&[
        (Some(2100), Some(2100)), // 4200
        (Some(2300), Some(2300)), // 4600 ─ tie …
        (Some(2250), Some(2350)), // 4600 ─ … spanning a page boundary
        (Some(2500), Some(2400)), // 4900
        (Some(2000), Some(2050)), // 4050
        (None, Some(2600)),
        (None, None),
    ])
    .await;
    let svc = HeaderSearchService::new(conn);

    let mut seen: Vec<(Option<i32>, i32)> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = svc
            .search(
                &user("alice"),
                &query(HeaderParams {
                    sort: Some("elo".to_string()),
                    limit: Some(2),
                    cursor: cursor.clone(),
                    ..params()
                }),
            )
            .await
            .unwrap();
        assert!(page.games.len() <= 2);
        seen.extend(
            page.games
                .iter()
                .map(|g| (g.white_elo.zip(g.black_elo).map(|(w, b)| w + b), g.id)),
        );
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }

    // Every game exactly once, best average first, unrated trailing.
    let ids: HashSet<i32> = seen.iter().map(|(_, id)| *id).collect();
    assert_eq!(ids.len(), 7);
    let walked: Vec<Option<i32>> = seen.iter().map(|(s, _)| *s).collect();
    assert_eq!(
        walked,
        vec![
            Some(4900),
            Some(4600),
            Some(4600),
            Some(4200),
            Some(4050),
            None,
            None
        ]
    );
}
