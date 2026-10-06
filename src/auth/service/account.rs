//! Account management on top of [`AuthService`] (ADR-0055): a user changing
//! their own password, an admin resetting someone else's, and the admin user
//! listing that backs the reset UI.
//!
//! Both password flows revoke the user's OAuth tokens (MCP clients must
//! re-authorize) but never their service tokens — those are admin-minted
//! machine credentials managed explicitly (ADR-0044).

use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use serde::Serialize;

use super::{validate_password, AuthService, AuthServiceError};
use crate::auth::password::{hash_password, verify_password};
use crate::db::entities::{oauth_tokens, sessions, users};
use crate::server::identity::{assert_admin, CurrentUser};

/// One row of the admin user listing — never carries the password hash.
#[derive(Debug, Serialize)]
pub struct UserSummary {
    pub id: String,
    pub username: String,
    pub is_admin: bool,
    pub created_at: chrono::NaiveDateTime,
}

impl AuthService {
    /// Change the password of the user behind `session_token`, after checking
    /// `current`. Every *other* session of theirs is closed and their OAuth
    /// tokens are revoked; the calling session survives so the user stays
    /// signed in. Only a login session qualifies — an OAuth or service token
    /// resolves to `NotASession`, so an MCP client can't change a password.
    pub async fn change_password(
        &self,
        session_token: &str,
        current: &str,
        new: &str,
    ) -> Result<(), AuthServiceError> {
        let user = self
            .authenticate(session_token)
            .await
            .map_err(|_| AuthServiceError::NotASession)?;
        let row = users::Entity::find_by_id(user.id.clone())
            .one(&self.db)
            .await?
            .ok_or(AuthServiceError::NotASession)?;

        if !verify_password(current, &row.password_hash).map_err(|_| AuthServiceError::Hash)? {
            return Err(AuthServiceError::WrongPassword);
        }
        validate_password(new)?;
        if new == current {
            return Err(AuthServiceError::InvalidInput(
                "new password must differ from the current one",
            ));
        }
        self.replace_password(&row.id, new, Some(session_token))
            .await
    }

    /// Admin sets another user's password. The target is signed out everywhere
    /// and their OAuth tokens are revoked. An admin changing their *own*
    /// password must go through [`Self::change_password`], which demands the
    /// current one.
    pub async fn admin_reset_password(
        &self,
        admin: &CurrentUser,
        target_id: &str,
        new: &str,
    ) -> Result<(), AuthServiceError> {
        assert_admin(admin).map_err(|_| AuthServiceError::Forbidden)?;
        if admin.id == target_id {
            return Err(AuthServiceError::InvalidInput(
                "use the account settings to change your own password",
            ));
        }
        validate_password(new)?;
        users::Entity::find_by_id(target_id.to_string())
            .one(&self.db)
            .await?
            .ok_or(AuthServiceError::UserNotFound)?;
        self.replace_password(target_id, new, None).await
    }

    /// Every account, oldest first. Admin only.
    pub async fn list_users(
        &self,
        admin: &CurrentUser,
    ) -> Result<Vec<UserSummary>, AuthServiceError> {
        assert_admin(admin).map_err(|_| AuthServiceError::Forbidden)?;
        let rows = users::Entity::find()
            .order_by_asc(users::Column::CreatedAt)
            .all(&self.db)
            .await?;
        Ok(rows
            .into_iter()
            .map(|u| UserSummary {
                id: u.id,
                username: u.username,
                is_admin: u.is_admin,
                created_at: u.created_at,
            })
            .collect())
    }

    /// Store the new hash, drop the user's sessions (except `keep`) and revoke
    /// their OAuth tokens — atomically, so a failure can't leave a changed
    /// password with the old sessions still live.
    async fn replace_password(
        &self,
        user_id: &str,
        new: &str,
        keep: Option<&str>,
    ) -> Result<(), AuthServiceError> {
        let hash = hash_password(new).map_err(|_| AuthServiceError::Hash)?;
        let txn = self.db.begin().await?;

        users::ActiveModel {
            id: Set(user_id.to_string()),
            password_hash: Set(hash),
            ..Default::default()
        }
        .update(&txn)
        .await?;

        let mut drop_sessions =
            sessions::Entity::delete_many().filter(sessions::Column::UserId.eq(user_id));
        if let Some(token) = keep {
            drop_sessions = drop_sessions.filter(sessions::Column::Token.ne(token));
        }
        drop_sessions.exec(&txn).await?;

        revoke_oauth_tokens(&txn, user_id).await?;
        txn.commit().await?;
        Ok(())
    }
}

/// Mark every OAuth token of the user revoked (in place, like refresh
/// rotation does) — a later refresh with one of them trips reuse detection.
async fn revoke_oauth_tokens(
    conn: &impl ConnectionTrait,
    user_id: &str,
) -> Result<(), AuthServiceError> {
    oauth_tokens::Entity::update_many()
        .col_expr(oauth_tokens::Column::Revoked, Expr::value(true))
        .filter(oauth_tokens::Column::UserId.eq(user_id))
        .exec(conn)
        .await?;
    Ok(())
}

#[cfg(test)]
#[path = "account_tests.rs"]
mod tests;
