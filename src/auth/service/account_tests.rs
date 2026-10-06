//! Unit tests for the account flows (ADR-0055) against in-memory SQLite.

use chrono::{Duration, Utc};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};

use super::super::{AuthService, AuthServiceError, Authenticated};
use crate::db::entities::{oauth_tokens, service_tokens};
use crate::db::{connect, DbConfig};
use crate::server::identity::CurrentUser;

async fn setup() -> (AuthService, DatabaseConnection) {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    (AuthService::new(db.clone()), db)
}

async fn seed_oauth(db: &DatabaseConnection, user_id: &str, access: &str) {
    let now = Utc::now().naive_utc();
    oauth_tokens::ActiveModel {
        access_token: Set(access.to_string()),
        refresh_token: Set(format!("r-{access}")),
        client_id: Set("client".to_string()),
        user_id: Set(user_id.to_string()),
        scope: Set("mcp".to_string()),
        family_id: Set(format!("f-{access}")),
        revoked: Set(false),
        created_at: Set(now),
        expires_at: Set(now + Duration::hours(1)),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn seed_service_token(db: &DatabaseConnection, owner_id: &str) {
    service_tokens::ActiveModel {
        token: Set("svc-secret".to_string()),
        id: Set("svc".to_string()),
        owner_id: Set(owner_id.to_string()),
        is_admin: Set(false),
        scope: Set("full".to_string()),
        label: Set("automation".to_string()),
        created_at: Set(Utc::now().naive_utc()),
        expires_at: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn oauth_revoked(db: &DatabaseConnection, access: &str) -> bool {
    oauth_tokens::Entity::find_by_id(access.to_string())
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .revoked
}

fn admin_of(a: &Authenticated) -> CurrentUser {
    a.user.clone()
}

#[tokio::test]
async fn change_password_rotates_credentials_and_keeps_only_the_calling_session() {
    let (auth, db) = setup().await;
    let first = auth.register("alice", "password123").await.unwrap();
    let other = auth.login("alice", "password123").await.unwrap();
    seed_oauth(&db, &first.user.id, "at-1").await;
    seed_service_token(&db, &first.user.id).await;

    auth.change_password(&first.token, "password123", "newpass456")
        .await
        .unwrap();

    assert!(
        auth.authenticate(&first.token).await.is_ok(),
        "caller stays in"
    );
    assert!(
        auth.authenticate(&other.token).await.is_err(),
        "others signed out"
    );
    assert!(oauth_revoked(&db, "at-1").await, "OAuth tokens revoked");
    assert!(
        service_tokens::Entity::find_by_id("svc-secret".to_string())
            .one(&db)
            .await
            .unwrap()
            .is_some(),
        "service tokens survive"
    );
    assert!(auth.login("alice", "newpass456").await.is_ok());
    assert!(matches!(
        auth.login("alice", "password123").await.unwrap_err(),
        AuthServiceError::InvalidCredentials
    ));
}

#[tokio::test]
async fn change_password_rejects_a_wrong_current_password() {
    let (auth, _db) = setup().await;
    let a = auth.register("alice", "password123").await.unwrap();
    assert!(matches!(
        auth.change_password(&a.token, "nope-nope", "newpass456")
            .await
            .unwrap_err(),
        AuthServiceError::WrongPassword
    ));
    assert!(
        auth.login("alice", "password123").await.is_ok(),
        "unchanged"
    );
}

#[tokio::test]
async fn change_password_validates_the_new_password() {
    let (auth, _db) = setup().await;
    let a = auth.register("alice", "password123").await.unwrap();
    for bad in ["short", "password123"] {
        assert!(matches!(
            auth.change_password(&a.token, "password123", bad)
                .await
                .unwrap_err(),
            AuthServiceError::InvalidInput(_)
        ));
    }
}

#[tokio::test]
async fn change_password_requires_a_login_session() {
    let (auth, db) = setup().await;
    let a = auth.register("alice", "password123").await.unwrap();
    seed_oauth(&db, &a.user.id, "at-1").await;
    // An OAuth access token is not a session.
    assert!(matches!(
        auth.change_password("at-1", "password123", "newpass456")
            .await
            .unwrap_err(),
        AuthServiceError::NotASession
    ));
}

#[tokio::test]
async fn admin_reset_signs_the_target_out_everywhere() {
    let (auth, db) = setup().await;
    let admin = auth.register("alice", "password123").await.unwrap();
    let bob = auth.register("bob", "password123").await.unwrap();
    seed_oauth(&db, &bob.user.id, "at-bob").await;

    auth.admin_reset_password(&admin_of(&admin), &bob.user.id, "fresh-pass1")
        .await
        .unwrap();

    assert!(auth.authenticate(&bob.token).await.is_err());
    assert!(oauth_revoked(&db, "at-bob").await);
    assert!(auth.login("bob", "fresh-pass1").await.is_ok());
    assert!(
        auth.authenticate(&admin.token).await.is_ok(),
        "admin untouched"
    );
}

#[tokio::test]
async fn admin_reset_is_admin_only_and_not_for_yourself() {
    let (auth, _db) = setup().await;
    let admin = auth.register("alice", "password123").await.unwrap();
    let bob = auth.register("bob", "password123").await.unwrap();

    assert!(matches!(
        auth.admin_reset_password(&admin_of(&bob), &admin.user.id, "fresh-pass1")
            .await
            .unwrap_err(),
        AuthServiceError::Forbidden
    ));
    assert!(matches!(
        auth.admin_reset_password(&admin_of(&admin), &admin.user.id, "fresh-pass1")
            .await
            .unwrap_err(),
        AuthServiceError::InvalidInput(_)
    ));
    assert!(matches!(
        auth.admin_reset_password(&admin_of(&admin), "ghost", "fresh-pass1")
            .await
            .unwrap_err(),
        AuthServiceError::UserNotFound
    ));
    assert!(matches!(
        auth.admin_reset_password(&CurrentUser::anonymous(), &bob.user.id, "fresh-pass1")
            .await
            .unwrap_err(),
        AuthServiceError::Forbidden
    ));
}

#[tokio::test]
async fn list_users_is_admin_only_and_never_exposes_hashes() {
    let (auth, _db) = setup().await;
    let admin = auth.register("alice", "password123").await.unwrap();
    let bob = auth.register("bob", "password123").await.unwrap();

    let users = auth.list_users(&admin_of(&admin)).await.unwrap();
    let names: Vec<_> = users.iter().map(|u| u.username.as_str()).collect();
    assert_eq!(names, ["alice", "bob"]);
    let json = serde_json::to_string(&users).unwrap();
    assert!(!json.contains("argon2"), "no password hash in the listing");

    assert!(matches!(
        auth.list_users(&admin_of(&bob)).await.unwrap_err(),
        AuthServiceError::Forbidden
    ));
}
