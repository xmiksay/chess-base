//! Request identity: who is calling, and the shared authorization helpers every
//! service uses to scope queries and gate admin-only actions.
//!
//! Local mode has a single implicit admin user (zero config). Server mode will
//! resolve the caller from session / Bearer auth — wired in #14; until then a
//! server-mode request is rejected as unauthorized. Call sites only ever see a
//! [`CurrentUser`], so #14 swaps in real resolution (in [`AppState`]) without
//! touching them.
//!
//! [`AppState`]: crate::server::state::AppState

use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use sea_orm::{ColumnTrait, Condition};
use serde_json::json;

use crate::server::state::AppState;

/// Stable id of the implicit local-mode admin user.
pub const LOCAL_ADMIN_ID: &str = "local-admin";

/// Stable id stamped on the anonymous public identity (ADR-0043). Never matches
/// a real `owner_id` (user ids come from `users.id` or [`LOCAL_ADMIN_ID`]), but
/// `public` is what every check actually gates on — this is just a readable
/// value for logs.
pub const ANONYMOUS_ID: &str = "anonymous";

/// The authenticated caller for a request. The one identity type every service
/// takes; nothing downstream cares how it was resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentUser {
    /// Matches `databases.owner_id` for resources this user owns.
    pub id: String,
    pub is_admin: bool,
    /// The anonymous public MCP tier (ADR-0043, issue #192): no sign-in, reads
    /// only, global (`owner_id IS NULL`) rows only. `scope` drops the
    /// own-rows half of its filter for this caller, and `assert_admin` /
    /// `assert_can_write` hard-deny regardless of `is_admin` (always `false`
    /// here anyway).
    pub public: bool,
}

impl CurrentUser {
    /// The implicit local-mode admin: single user, full admin rights.
    pub fn local_admin() -> Self {
        Self {
            id: LOCAL_ADMIN_ID.to_string(),
            is_admin: true,
            public: false,
        }
    }

    /// The anonymous public identity (ADR-0043): no credential presented, data
    /// reads on global databases only, never writes.
    pub fn anonymous() -> Self {
        Self {
            id: ANONYMOUS_ID.to_string(),
            is_admin: false,
            public: true,
        }
    }
}

/// Read-scope condition for an ownable resource: rows the caller owns plus global
/// (`owner_id IS NULL`) rows. The one place the ownership rule lives, used by
/// every service that filters by owner (see ADR 0007 / 0011). The anonymous
/// public caller (ADR-0043) never owns anything, so its scope is `owner_col IS
/// NULL` only.
pub fn scope<C: ColumnTrait>(owner_col: C, user: &CurrentUser) -> Condition {
    if user.public {
        return Condition::any().add(owner_col.is_null());
    }
    Condition::any()
        .add(owner_col.eq(user.id.clone()))
        .add(owner_col.is_null())
}

/// Gate an admin-only action (e.g. writing a global database). The anonymous
/// public caller is hard-denied (ADR-0043): it can never be an admin.
pub fn assert_admin(user: &CurrentUser) -> Result<(), AuthError> {
    if user.public {
        return Err(AuthError::Unauthorized);
    }
    if user.is_admin {
        Ok(())
    } else {
        Err(AuthError::Forbidden)
    }
}

/// Write guard (ADR 0007 / 0011): a resource is writable only by its owner; a
/// global resource (`owner_id` NULL) requires admin. The anonymous public
/// caller is hard-denied before the ownership check (ADR-0043) — it has no
/// rows of its own to write. Returns the shared [`AuthError`]; each service
/// maps it onto its own error type.
pub(crate) fn assert_can_write(
    owner_id: Option<&str>,
    user: &CurrentUser,
) -> Result<(), AuthError> {
    if user.public {
        return Err(AuthError::Unauthorized);
    }
    match owner_id {
        None => assert_admin(user),
        Some(owner) if owner == user.id => Ok(()),
        Some(_) => Err(AuthError::Forbidden),
    }
}

/// Why identity resolution or an admin check failed. Maps to an HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// No (valid) credentials — server-mode auth is not yet resolved (#14).
    Unauthorized,
    /// Authenticated but not permitted (non-admin attempting an admin action).
    Forbidden,
}

impl AuthError {
    fn status(self) -> StatusCode {
        match self {
            AuthError::Unauthorized => StatusCode::UNAUTHORIZED,
            AuthError::Forbidden => StatusCode::FORBIDDEN,
        }
    }

    fn message(self) -> &'static str {
        match self {
            AuthError::Unauthorized => "authentication required",
            AuthError::Forbidden => "admin privileges required",
        }
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        (self.status(), Json(json!({ "error": self.message() }))).into_response()
    }
}

/// Extract the caller from the request. Resolution lives on [`AppState`] so #14
/// can replace server-mode auth there without touching any handler signature.
impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        state.resolve_current_user(parts).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_admin_is_admin_with_stable_id() {
        let u = CurrentUser::local_admin();
        assert_eq!(u.id, LOCAL_ADMIN_ID);
        assert!(u.is_admin);
        assert!(!u.public);
    }

    #[test]
    fn anonymous_is_public_and_never_admin() {
        let u = CurrentUser::anonymous();
        assert_eq!(u.id, ANONYMOUS_ID);
        assert!(!u.is_admin);
        assert!(u.public);
    }

    #[test]
    fn assert_admin_passes_for_admin_and_fails_otherwise() {
        assert!(assert_admin(&CurrentUser::local_admin()).is_ok());

        let plain = CurrentUser {
            id: "alice".to_string(),
            is_admin: false,
            public: false,
        };
        assert_eq!(assert_admin(&plain), Err(AuthError::Forbidden));
    }

    #[test]
    fn assert_admin_hard_denies_the_anonymous_caller() {
        assert_eq!(
            assert_admin(&CurrentUser::anonymous()),
            Err(AuthError::Unauthorized)
        );
    }

    #[test]
    fn assert_can_write_hard_denies_the_anonymous_caller_even_on_its_own_id() {
        let anon = CurrentUser::anonymous();
        // Even a resource "owned" by the anonymous id (which should never occur)
        // must not be writable — the public check comes first.
        assert_eq!(
            assert_can_write(Some(ANONYMOUS_ID), &anon),
            Err(AuthError::Unauthorized)
        );
        assert_eq!(assert_can_write(None, &anon), Err(AuthError::Unauthorized));
    }

    #[test]
    fn scope_restricts_the_anonymous_caller_to_global_rows_only() {
        use crate::db::entities::databases;

        let anon = CurrentUser::anonymous();
        let cond = scope(databases::Column::OwnerId, &anon);
        // The anonymous condition must not reference the caller's id at all —
        // rendered SQL only has an IS NULL check, no equality branch.
        let sql = format!("{cond:?}");
        assert!(!sql.contains(ANONYMOUS_ID), "leaked anonymous id: {sql}");
    }
}
