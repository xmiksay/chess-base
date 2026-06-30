//! Service-level tests over an in-memory SQLite DB: the ownership read scope
//! (own ∪ global, never another user's), the write guards (another user's
//! database; a global database without admin), kind/name validation, and the
//! `kind`-derived `index_depth`.

use super::*;
use crate::db::entities::databases::MASTER_INDEX_DEPTH;
use crate::db::{connect, DbConfig};

fn user(id: &str) -> CurrentUser {
    CurrentUser {
        id: id.to_string(),
        is_admin: false,
    }
}

async fn service() -> DatabaseService {
    let conn = connect(&DbConfig::in_memory()).await.unwrap();
    DatabaseService::new(conn)
}

#[tokio::test]
async fn create_sets_owner_and_kind_derived_index_depth() {
    let svc = service().await;
    let alice = user("alice");

    let own = svc.create(&alice, "My games", "own", false).await.unwrap();
    assert_eq!(own.owner_id.as_deref(), Some("alice"));
    assert_eq!(own.kind, "own");
    assert_eq!(own.index_depth, None, "own DBs index every ply");

    // `master` gets the capped index depth (ADR-0003).
    let admin = CurrentUser::local_admin();
    let masters = svc.create(&admin, "Masters", "master", true).await.unwrap();
    assert!(masters.owner_id.is_none());
    assert_eq!(masters.index_depth, Some(MASTER_INDEX_DEPTH));
}

#[tokio::test]
async fn create_rejects_unknown_kind_and_blank_name() {
    let svc = service().await;
    let alice = user("alice");

    assert!(matches!(
        svc.create(&alice, "X", "bogus", false).await.unwrap_err(),
        DatabaseError::InvalidKind(k) if k == "bogus"
    ));
    assert!(matches!(
        svc.create(&alice, "   ", "own", false).await.unwrap_err(),
        DatabaseError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn list_and_get_scope_to_own_plus_global() {
    let svc = service().await;
    let alice = user("alice");
    let bob = user("bob");
    let admin = CurrentUser::local_admin();

    let mine = svc.create(&alice, "Mine", "own", false).await.unwrap();
    let theirs = svc.create(&bob, "Theirs", "own", false).await.unwrap();
    let global = svc.create(&admin, "Global", "master", true).await.unwrap();

    let names: Vec<_> = svc
        .list(&alice)
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert_eq!(names, vec!["Mine", "Global"]);

    // Direct get respects the same scope.
    assert!(svc.get(&alice, mine.id).await.is_ok());
    assert!(svc.get(&alice, global.id).await.is_ok());
    assert!(matches!(
        svc.get(&alice, theirs.id).await.unwrap_err(),
        DatabaseError::NotFound
    ));
}

#[tokio::test]
async fn rename_and_delete_require_ownership() {
    let svc = service().await;
    let alice = user("alice");
    let bob = user("bob");

    let mine = svc.create(&alice, "Mine", "own", false).await.unwrap();

    // Bob can neither rename nor delete Alice's database.
    assert!(matches!(
        svc.rename(&bob, mine.id, "Hacked").await.unwrap_err(),
        DatabaseError::Forbidden
    ));
    assert!(matches!(
        svc.delete(&bob, mine.id).await.unwrap_err(),
        DatabaseError::Forbidden
    ));

    // Alice can rename it, then delete it.
    let renamed = svc.rename(&alice, mine.id, "Renamed").await.unwrap();
    assert_eq!(renamed.name, "Renamed");
    svc.delete(&alice, mine.id).await.unwrap();
    assert!(matches!(
        svc.get(&alice, mine.id).await.unwrap_err(),
        DatabaseError::NotFound
    ));
}

#[tokio::test]
async fn global_databases_require_admin_to_create_and_mutate() {
    let svc = service().await;
    let alice = user("alice");
    let admin = CurrentUser::local_admin();

    // Only admin may create a global database.
    assert!(matches!(
        svc.create(&alice, "Global", "master", true)
            .await
            .unwrap_err(),
        DatabaseError::Forbidden
    ));
    let global = svc.create(&admin, "Global", "master", true).await.unwrap();

    // A non-admin can read it (global scope) but not write it.
    assert!(svc.get(&alice, global.id).await.is_ok());
    assert!(matches!(
        svc.rename(&alice, global.id, "Nope").await.unwrap_err(),
        DatabaseError::Forbidden
    ));
    assert!(matches!(
        svc.delete(&alice, global.id).await.unwrap_err(),
        DatabaseError::Forbidden
    ));

    // Admin can.
    svc.rename(&admin, global.id, "Curated").await.unwrap();
    svc.delete(&admin, global.id).await.unwrap();
}

#[tokio::test]
async fn list_with_counts_reports_game_counts_per_database() {
    use crate::db::entities::games;
    use sea_orm::{ActiveModelTrait, Set};

    let conn = connect(&DbConfig::in_memory()).await.unwrap();
    let svc = DatabaseService::new(conn.clone());
    let alice = user("alice");

    let full = svc.create(&alice, "Full", "own", false).await.unwrap();
    let empty = svc.create(&alice, "Empty", "own", false).await.unwrap();

    // Two games land in `full`, none in `empty`.
    for _ in 0..2 {
        games::ActiveModel {
            database_id: Set(full.id),
            variant: Set("standard".into()),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .unwrap();
    }

    let counts: std::collections::HashMap<i32, i64> = svc
        .list_with_counts(&alice)
        .await
        .unwrap()
        .into_iter()
        .map(|(d, n)| (d.id, n))
        .collect();
    assert_eq!(counts.get(&full.id), Some(&2));
    assert_eq!(counts.get(&empty.id), Some(&0), "empty DBs default to 0");
}

#[tokio::test]
async fn deleting_a_missing_database_is_not_found() {
    let svc = service().await;
    assert!(matches!(
        svc.delete(&user("alice"), 999).await.unwrap_err(),
        DatabaseError::NotFound
    ));
}
