//! Unit tests for the admin-only [`ServiceTokenService`].

use super::*;
use crate::db::{connect, DbConfig};

async fn mem_db() -> DatabaseConnection {
    connect(&DbConfig::in_memory())
        .await
        .expect("connect in-memory db")
}

fn admin() -> CurrentUser {
    CurrentUser::local_admin()
}

fn plain(id: &str) -> CurrentUser {
    CurrentUser {
        id: id.to_string(),
        is_admin: false,
        public: false,
        read_only: false,
        global_only: false,
    }
}

#[tokio::test]
async fn admin_creates_lists_and_revokes_a_token() {
    let svc = ServiceTokenService::new(mem_db().await);

    let minted = svc
        .create(
            &admin(),
            "alice",
            "ci",
            SERVICE_SCOPE_READ_ONLY,
            false,
            None,
        )
        .await
        .expect("create");
    assert_eq!(minted.view.owner_id, "alice");
    assert_eq!(minted.view.scope, SERVICE_SCOPE_READ_ONLY);
    assert!(!minted.token.is_empty());
    assert_ne!(minted.token, minted.view.id, "id must not be the secret");

    let listed = svc.list(&admin()).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, minted.view.id);

    svc.revoke(&admin(), &minted.view.id).await.expect("revoke");
    assert!(svc.list(&admin()).await.expect("list").is_empty());
}

#[tokio::test]
async fn non_admin_is_forbidden_on_every_operation() {
    let svc = ServiceTokenService::new(mem_db().await);
    let bob = plain("bob");

    assert!(matches!(
        svc.create(&bob, "bob", "ci", SERVICE_SCOPE_FULL, false, None)
            .await,
        Err(ServiceTokenError::Forbidden)
    ));
    assert!(matches!(
        svc.list(&bob).await,
        Err(ServiceTokenError::Forbidden)
    ));
    assert!(matches!(
        svc.revoke(&bob, "whatever").await,
        Err(ServiceTokenError::Forbidden)
    ));
}

#[tokio::test]
async fn create_rejects_an_unrecognized_scope() {
    let svc = ServiceTokenService::new(mem_db().await);
    assert!(matches!(
        svc.create(&admin(), "alice", "ci", "bogus", false, None)
            .await,
        Err(ServiceTokenError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn revoke_unknown_id_is_not_found() {
    let svc = ServiceTokenService::new(mem_db().await);
    assert!(matches!(
        svc.revoke(&admin(), "nope").await,
        Err(ServiceTokenError::NotFound)
    ));
}

#[tokio::test]
async fn expires_in_days_sets_a_future_expiry() {
    let svc = ServiceTokenService::new(mem_db().await);
    let minted = svc
        .create(&admin(), "alice", "ci", SERVICE_SCOPE_FULL, false, Some(7))
        .await
        .expect("create");
    let expires_at = minted.view.expires_at.expect("expiry set");
    assert!(expires_at > minted.view.created_at);
}
