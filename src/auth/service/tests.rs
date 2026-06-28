//! Unit tests for [`AuthService`] against an in-memory SQLite database.

use super::*;
use crate::db::{connect, DbConfig};

async fn service() -> AuthService {
    let db = connect(&DbConfig::in_memory()).await.unwrap();
    AuthService::new(db)
}

#[tokio::test]
async fn first_user_is_admin_rest_are_not() {
    let auth = service().await;

    let first = auth.register("alice", "password123").await.unwrap();
    assert!(first.user.is_admin, "the bootstrap user is admin");

    let second = auth.register("bob", "password123").await.unwrap();
    assert!(!second.user.is_admin, "later users are plain");
}

#[tokio::test]
async fn duplicate_username_is_rejected() {
    let auth = service().await;
    auth.register("alice", "password123").await.unwrap();

    let err = auth.register("alice", "different1").await.unwrap_err();
    assert!(matches!(err, AuthServiceError::UsernameTaken));
}

#[tokio::test]
async fn short_credentials_are_rejected() {
    let auth = service().await;
    assert!(matches!(
        auth.register("ab", "password123").await.unwrap_err(),
        AuthServiceError::InvalidInput(_)
    ));
    assert!(matches!(
        auth.register("alice", "short").await.unwrap_err(),
        AuthServiceError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn login_succeeds_with_right_password_only() {
    let auth = service().await;
    auth.register("alice", "password123").await.unwrap();

    assert!(auth.login("alice", "password123").await.is_ok());
    assert!(matches!(
        auth.login("alice", "wrongpass").await.unwrap_err(),
        AuthServiceError::InvalidCredentials
    ));
    assert!(matches!(
        auth.login("nobody", "password123").await.unwrap_err(),
        AuthServiceError::InvalidCredentials
    ));
}

#[tokio::test]
async fn authenticate_resolves_a_valid_token() {
    let auth = service().await;
    let registered = auth.register("alice", "password123").await.unwrap();

    let user = auth.authenticate(&registered.token).await.unwrap();
    assert_eq!(user.id, registered.user.id);
    assert!(user.is_admin);

    assert_eq!(
        auth.authenticate("bogus-token").await.unwrap_err(),
        AuthError::Unauthorized
    );
}

#[tokio::test]
async fn logout_invalidates_the_token() {
    let auth = service().await;
    let registered = auth.register("alice", "password123").await.unwrap();
    assert!(auth.authenticate(&registered.token).await.is_ok());

    auth.logout(&registered.token).await.unwrap();
    assert_eq!(
        auth.authenticate(&registered.token).await.unwrap_err(),
        AuthError::Unauthorized
    );
    // Logging out an already-cleared token is a no-op.
    assert!(auth.logout(&registered.token).await.is_ok());
}
