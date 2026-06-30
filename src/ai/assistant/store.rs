//! Persistence for assistant sessions + their transcript. Sessions are private:
//! every read/write is scoped to the caller's `CurrentUser::id` (a session the
//! caller doesn't own reads back as [`AssistantError::NotFound`], hiding its
//! existence). Each transcript row stores one [`Message`] serialized as JSON.

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, Set,
};

use crate::ai::llm::{Message, ProviderError};
use crate::db::entities::{assistant_messages, assistant_sessions};
use crate::server::identity::CurrentUser;

/// Why an assistant operation failed. Transport-agnostic — the HTTP layer maps
/// each variant onto a status/envelope.
#[derive(Debug, thiserror::Error)]
pub enum AssistantError {
    /// No session with that id is visible to the caller.
    #[error("session not found")]
    NotFound,
    /// The session is paused awaiting a tool approval; resolve it before posting
    /// a new message (or vice-versa).
    #[error("{0}")]
    Conflict(String),
    /// The LLM provider call failed.
    #[error("assistant provider error: {0}")]
    Provider(#[from] ProviderError),
    /// A stored transcript row could not be parsed back into a [`Message`].
    #[error("corrupt transcript: {0}")]
    Corrupt(#[from] serde_json::Error),
    /// Underlying database error (never surfaced verbatim to clients).
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// CRUD for `assistant_sessions` + `assistant_messages`. Holds a connection
/// handle (cheap to clone — SeaORM wraps an `Arc`'d pool).
#[derive(Clone)]
pub struct AssistantStore {
    db: DatabaseConnection,
}

impl AssistantStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Create an empty session owned by the caller.
    pub async fn create(
        &self,
        user: &CurrentUser,
        title: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<assistant_sessions::Model, AssistantError> {
        let now = Utc::now().naive_utc();
        let model = assistant_sessions::ActiveModel {
            owner_id: Set(user.id.clone()),
            title: Set(title.into()),
            model: Set(model.into()),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(&self.db)
        .await?;
        Ok(model)
    }

    /// Sessions owned by the caller, most-recently-updated first.
    pub async fn list(
        &self,
        user: &CurrentUser,
    ) -> Result<Vec<assistant_sessions::Model>, AssistantError> {
        let rows = assistant_sessions::Entity::find()
            .filter(assistant_sessions::Column::OwnerId.eq(user.id.clone()))
            .order_by_desc(assistant_sessions::Column::UpdatedAt)
            .order_by_desc(assistant_sessions::Column::Id)
            .all(&self.db)
            .await?;
        Ok(rows)
    }

    /// A session the caller owns, or [`AssistantError::NotFound`].
    pub async fn get_owned(
        &self,
        user: &CurrentUser,
        id: i32,
    ) -> Result<assistant_sessions::Model, AssistantError> {
        let session = assistant_sessions::Entity::find_by_id(id)
            .one(&self.db)
            .await?
            .ok_or(AssistantError::NotFound)?;
        if session.owner_id != user.id {
            return Err(AssistantError::NotFound);
        }
        Ok(session)
    }

    /// Delete a session the caller owns (and its transcript).
    pub async fn delete(&self, user: &CurrentUser, id: i32) -> Result<(), AssistantError> {
        let session = self.get_owned(user, id).await?;
        // Delete messages explicitly: SQLite enforces FK cascade only with the
        // `foreign_keys` pragma on, which we don't assume.
        assistant_messages::Entity::delete_many()
            .filter(assistant_messages::Column::SessionId.eq(session.id))
            .exec(&self.db)
            .await?;
        assistant_sessions::Entity::delete_by_id(session.id)
            .exec(&self.db)
            .await?;
        Ok(())
    }

    /// The full transcript of a session, oldest first, parsed back into messages.
    pub async fn load_messages(&self, session_id: i32) -> Result<Vec<Message>, AssistantError> {
        let rows = assistant_messages::Entity::find()
            .filter(assistant_messages::Column::SessionId.eq(session_id))
            .order_by_asc(assistant_messages::Column::Seq)
            .all(&self.db)
            .await?;
        rows.iter()
            .map(|r| serde_json::from_str::<Message>(&r.content_json).map_err(AssistantError::from))
            .collect()
    }

    /// Append a message to a session's transcript, assigning the next `seq`, and
    /// bump the session's `updated_at`.
    pub async fn append(&self, session_id: i32, message: &Message) -> Result<(), AssistantError> {
        let seq = assistant_messages::Entity::find()
            .filter(assistant_messages::Column::SessionId.eq(session_id))
            .count(&self.db)
            .await? as i32;
        let content_json = serde_json::to_string(message)?;
        assistant_messages::ActiveModel {
            session_id: Set(session_id),
            seq: Set(seq),
            role: Set(role_tag(message).to_string()),
            content_json: Set(content_json),
            created_at: Set(Utc::now().naive_utc()),
            ..Default::default()
        }
        .insert(&self.db)
        .await?;
        self.touch(session_id).await?;
        Ok(())
    }

    /// Bump a session's `updated_at` so the sidebar orders it first.
    async fn touch(&self, session_id: i32) -> Result<(), AssistantError> {
        if let Some(session) = assistant_sessions::Entity::find_by_id(session_id)
            .one(&self.db)
            .await?
        {
            let mut active: assistant_sessions::ActiveModel = session.into();
            active.updated_at = Set(Utc::now().naive_utc());
            active.update(&self.db).await?;
        }
        Ok(())
    }
}

/// The serialized role tag stored alongside the message JSON.
fn role_tag(message: &Message) -> &'static str {
    match message {
        Message::User { .. } => "user",
        Message::Assistant { .. } => "assistant",
        Message::ToolResults { .. } => "tool_results",
    }
}
