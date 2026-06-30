//! Service-level tests over an in-memory SQLite DB: nesting, the read scope and
//! write guards, cycle rejection on move, the duplicate-sibling guard, and the
//! cascade delete that unfiles (never deletes) contained studies.

use super::*;
use crate::db::entities::databases;
use crate::db::{connect, DbConfig};
use crate::studies::StudyService;
use sea_orm::{ActiveModelTrait, Set};

fn user(id: &str) -> CurrentUser {
    CurrentUser {
        id: id.to_string(),
        is_admin: false,
    }
}

fn admin(id: &str) -> CurrentUser {
    CurrentUser {
        id: id.to_string(),
        is_admin: true,
    }
}

/// Fresh in-memory DB with one owned games database, returning the folder + study
/// services and that database's id (studies still scope to a games collection).
async fn setup() -> (FolderService, StudyService, i32) {
    let conn = connect(&DbConfig::in_memory()).await.unwrap();
    let db = databases::ActiveModel {
        owner_id: Set(Some("alice".to_string())),
        name: Set("Alice's games".to_string()),
        kind: Set("own".to_string()),
        ..Default::default()
    }
    .insert(&conn)
    .await
    .unwrap();
    (
        FolderService::new(conn.clone()),
        StudyService::new(conn),
        db.id,
    )
}

#[tokio::test]
async fn create_nested_and_list_scopes_to_owner() {
    let (folders, _studies, _db) = setup().await;
    let alice = user("alice");
    let bob = user("bob");

    let openings = folders
        .create(&alice, None, "Openings", false)
        .await
        .unwrap();
    let sicilian = folders
        .create(&alice, Some(openings.id), "Sicilian", false)
        .await
        .unwrap();
    assert_eq!(sicilian.parent_id, Some(openings.id));

    // Alice sees her two folders; Bob sees none of them.
    assert_eq!(folders.list(&alice).await.unwrap().len(), 2);
    assert!(folders.list(&bob).await.unwrap().is_empty());
}

#[tokio::test]
async fn duplicate_sibling_name_is_rejected() {
    let (folders, _studies, _db) = setup().await;
    let alice = user("alice");

    folders
        .create(&alice, None, "Openings", false)
        .await
        .unwrap();
    let err = folders
        .create(&alice, None, "Openings", false)
        .await
        .unwrap_err();
    assert!(matches!(err, FolderError::Duplicate));

    // Same name under a different parent is fine.
    let other = folders
        .create(&alice, None, "Endgames", false)
        .await
        .unwrap();
    folders
        .create(&alice, Some(other.id), "Openings", false)
        .await
        .unwrap();
}

#[tokio::test]
async fn reparent_into_own_descendant_is_a_cycle() {
    let (folders, _studies, _db) = setup().await;
    let alice = user("alice");

    let a = folders.create(&alice, None, "A", false).await.unwrap();
    let b = folders
        .create(&alice, Some(a.id), "B", false)
        .await
        .unwrap();
    let c = folders
        .create(&alice, Some(b.id), "C", false)
        .await
        .unwrap();

    // Move A under C (its own grandchild) → cycle; move into itself → cycle.
    assert!(matches!(
        folders
            .reparent(&alice, a.id, Some(c.id))
            .await
            .unwrap_err(),
        FolderError::Cycle
    ));
    assert!(matches!(
        folders
            .reparent(&alice, a.id, Some(a.id))
            .await
            .unwrap_err(),
        FolderError::Cycle
    ));

    // A legal move (C up to the root) succeeds.
    let moved = folders.reparent(&alice, c.id, None).await.unwrap();
    assert_eq!(moved.parent_id, None);
}

#[tokio::test]
async fn write_guards_block_other_users_and_nonadmin_global() {
    let (folders, _studies, _db) = setup().await;
    let alice = user("alice");
    let bob = user("bob");
    let root = admin("root");

    let f = folders
        .create(&alice, None, "Private", false)
        .await
        .unwrap();
    // Bob cannot rename/move/delete Alice's folder.
    assert!(matches!(
        folders.rename(&bob, f.id, "Hijacked").await.unwrap_err(),
        FolderError::Forbidden
    ));
    assert!(matches!(
        folders.delete(&bob, f.id).await.unwrap_err(),
        FolderError::Forbidden
    ));

    // Only an admin can create a global folder.
    assert!(matches!(
        folders
            .create(&alice, None, "Shared", true)
            .await
            .unwrap_err(),
        FolderError::Forbidden
    ));
    let global = folders.create(&root, None, "Shared", true).await.unwrap();
    assert!(global.owner_id.is_none());
    // Alice can see the global folder but cannot write it.
    assert!(folders
        .list(&alice)
        .await
        .unwrap()
        .iter()
        .any(|x| x.id == global.id));
    assert!(matches!(
        folders.rename(&alice, global.id, "Mine").await.unwrap_err(),
        FolderError::Forbidden
    ));
}

#[tokio::test]
async fn cannot_nest_own_folder_under_global() {
    let (folders, _studies, _db) = setup().await;
    let alice = user("alice");
    let root = admin("root");

    let global = folders.create(&root, None, "Shared", true).await.unwrap();
    // Alice (own folder) cannot parent under the global tree.
    assert!(matches!(
        folders
            .create(&alice, Some(global.id), "Mine", false)
            .await
            .unwrap_err(),
        FolderError::Forbidden
    ));
}

#[tokio::test]
async fn delete_cascades_children_and_unfiles_studies() {
    let (folders, studies, db_id) = setup().await;
    let alice = user("alice");

    let parent = folders.create(&alice, None, "Parent", false).await.unwrap();
    let child = folders
        .create(&alice, Some(parent.id), "Child", false)
        .await
        .unwrap();

    // A study filed into the child folder.
    let study = studies.create(&alice, db_id, "Line", false).await.unwrap();
    studies
        .set_folder(&alice, study.id, Some(child.id))
        .await
        .unwrap();

    // Delete the parent: both folders go, the study survives but is unfiled.
    folders.delete(&alice, parent.id).await.unwrap();
    assert!(folders.list(&alice).await.unwrap().is_empty());

    let reloaded = studies.get(&alice, study.id).await.unwrap();
    assert_eq!(reloaded.folder_id, None);
}
