//! Transport-agnostic auth service: registration, login, logout and token
//! resolution over the `users` + `sessions` tables. HTTP handlers (and any
//! future MCP tool) are thin callers that map [`AuthServiceError`] to a response.
//!
//! Server mode only — local mode never calls this (it uses the implicit
//! `local-admin`). Bootstrapping rule: the **first** registered user becomes the
//! admin, so a fresh deployment has a way to manage global databases.

use chrono::{Duration, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, PaginatorTrait,
    QueryFilter, Set,
};
use uuid::Uuid;

use crate::auth::password::{hash_password, verify_password};
use crate::db::entities::{sessions, users};
use crate::server::identity::{AuthError, CurrentUser};

/// How long a freshly issued session stays valid.
const SESSION_TTL_DAYS: i64 = 30;

/// Minimum lengths; cheap guards so we never store an empty/trivial credential.
const MIN_USERNAME_LEN: usize = 3;
const MIN_PASSWORD_LEN: usize = 8;

/// Why an auth operation failed. Transport-agnostic — the HTTP layer maps each
/// variant onto a status. Deliberately coarse so we never leak which half of a
/// credential was wrong.
#[derive(Debug, thiserror::Error)]
pub enum AuthServiceError {
    /// Username/password failed a length/format check. `400`.
    #[error("{0}")]
    InvalidInput(&'static str),
    /// The username is already registered. `409`.
    #[error("username already taken")]
    UsernameTaken,
    /// No such user, or the password did not match. `401`.
    #[error("invalid username or password")]
    InvalidCredentials,
    /// Password hashing/verification failed (never surfaced verbatim).
    #[error("password hashing failed")]
    Hash,
    /// Underlying database error (never surfaced verbatim to clients).
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// A successful authentication: the issued session token and the resolved caller.
#[derive(Debug)]
pub struct Authenticated {
    pub token: String,
    pub user: CurrentUser,
}

/// Auth operations over the `users` + `sessions` tables. Holds a connection
/// handle (cheap to clone — SeaORM wraps an `Arc`'d pool).
#[derive(Clone)]
pub struct AuthService {
    db: DatabaseConnection,
}

impl AuthService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Register a new account and open a session for it. The first user to
    /// register on a deployment is made admin (bootstrap); everyone after is a
    /// plain user.
    pub async fn register(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Authenticated, AuthServiceError> {
        let username = validate_username(username)?;
        validate_password(password)?;

        if users::Entity::find()
            .filter(users::Column::Username.eq(username))
            .one(&self.db)
            .await?
            .is_some()
        {
            return Err(AuthServiceError::UsernameTaken);
        }

        // First account bootstraps the admin so global databases are manageable.
        let is_admin = users::Entity::find().count(&self.db).await? == 0;
        let password_hash = hash_password(password).map_err(|_| AuthServiceError::Hash)?;

        let user = users::ActiveModel {
            id: Set(Uuid::new_v4().simple().to_string()),
            username: Set(username.to_string()),
            password_hash: Set(password_hash),
            is_admin: Set(is_admin),
            created_at: Set(Utc::now().naive_utc()),
        }
        .insert(&self.db)
        .await?;

        let token = self.open_session(&user.id).await?;
        Ok(Authenticated {
            token,
            user: CurrentUser {
                id: user.id,
                is_admin: user.is_admin,
            },
        })
    }

    /// Verify credentials and open a session. Returns `InvalidCredentials` for
    /// both an unknown user and a bad password (no user enumeration).
    pub async fn login(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Authenticated, AuthServiceError> {
        let user = users::Entity::find()
            .filter(users::Column::Username.eq(username))
            .one(&self.db)
            .await?
            .ok_or(AuthServiceError::InvalidCredentials)?;

        let ok =
            verify_password(password, &user.password_hash).map_err(|_| AuthServiceError::Hash)?;
        if !ok {
            return Err(AuthServiceError::InvalidCredentials);
        }

        let token = self.open_session(&user.id).await?;
        Ok(Authenticated {
            token,
            user: CurrentUser {
                id: user.id,
                is_admin: user.is_admin,
            },
        })
    }

    /// Invalidate a session token. Idempotent — a missing token is a no-op.
    pub async fn logout(&self, token: &str) -> Result<(), AuthServiceError> {
        sessions::Entity::delete_by_id(token.to_string())
            .exec(&self.db)
            .await?;
        Ok(())
    }

    /// Resolve a session token to the calling user, or `Unauthorized` if the
    /// token is unknown or expired. This is what server-mode identity resolution
    /// calls on every request.
    pub async fn authenticate(&self, token: &str) -> Result<CurrentUser, AuthError> {
        let session = sessions::Entity::find_by_id(token.to_string())
            .one(&self.db)
            .await
            .map_err(|_| AuthError::Unauthorized)?
            .ok_or(AuthError::Unauthorized)?;

        if session.expires_at <= Utc::now().naive_utc() {
            return Err(AuthError::Unauthorized);
        }

        let user = users::Entity::find_by_id(session.user_id)
            .one(&self.db)
            .await
            .map_err(|_| AuthError::Unauthorized)?
            .ok_or(AuthError::Unauthorized)?;

        Ok(CurrentUser {
            id: user.id,
            is_admin: user.is_admin,
        })
    }

    /// Insert a fresh, unguessable session row for a user and return its token.
    async fn open_session(&self, user_id: &str) -> Result<String, AuthServiceError> {
        // Two v4 UUIDs ⇒ ~244 bits of entropy, comfortably beyond guessing.
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let now = Utc::now().naive_utc();
        sessions::ActiveModel {
            token: Set(token.clone()),
            user_id: Set(user_id.to_string()),
            created_at: Set(now),
            expires_at: Set(now + Duration::days(SESSION_TTL_DAYS)),
        }
        .insert(&self.db)
        .await?;
        Ok(token)
    }
}

/// Trim and length-check a username.
fn validate_username(username: &str) -> Result<&str, AuthServiceError> {
    let trimmed = username.trim();
    if trimmed.len() < MIN_USERNAME_LEN {
        return Err(AuthServiceError::InvalidInput(
            "username must be at least 3 characters",
        ));
    }
    Ok(trimmed)
}

/// Length-check a password (not trimmed — leading/trailing spaces are allowed).
fn validate_password(password: &str) -> Result<(), AuthServiceError> {
    if password.len() < MIN_PASSWORD_LEN {
        return Err(AuthServiceError::InvalidInput(
            "password must be at least 8 characters",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
