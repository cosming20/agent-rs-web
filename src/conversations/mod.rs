//! Conversation persistence — stored in the web-app's own Postgres.
//!
//! A `Conversation` is this app's container for a single chat thread.
//! agent-rs is stateless about sessions: each Ask request carries the
//! history inline, so the conversation UUID is purely a web-side primary
//! key (not echoed into gRPC).
//!
//! Everything here is SSR-only; none of it crosses the wasm boundary.

#![cfg(feature = "ssr")]

use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Auto-title derived from the first user message is truncated to this many
/// characters to keep sidebar rows readable.
const AUTO_TITLE_MAX_CHARS: usize = 80;

/// Default label for a freshly-created conversation, shown until the first
/// user message upgrades the title.
const DEFAULT_TITLE: &str = "New conversation";

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

/// A single conversation row, owned by one user.
#[derive(Debug, Clone, Queryable, Selectable, Serialize, Deserialize)]
#[diesel(table_name = crate::schema::conversations)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Conversation {
    pub id: Uuid,
    pub user_id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// User-chosen active document set for this thread.
    /// - `None`         — not yet chosen; UI defaults to every
    ///   `complete` library doc (backwards-compatible auto mode).
    /// - `Some(vec![])` — user explicitly pinned nothing; agent runs
    ///   with empty `active_document_keys`.
    /// - `Some(ids)`    — honour exactly this set.
    pub pinned_document_ids: Option<Vec<Option<Uuid>>>,
}

/// A single persisted turn in a conversation.
///
/// `attached_document_ids` pins which library documents were active for
/// this turn; diesel maps the PG `uuid[]` column as
/// `Vec<Option<Uuid>>` because array elements are typed-nullable at the
/// protocol level (PG supports NULL in the array). We never store NULLs
/// ourselves, but the type has to carry the possibility.
///
/// The legacy `is_grounded` column is kept on the table for backwards
/// compatibility with rows written before the agent-rs grounding-
/// verifier was removed (proto field 4 of `AskFinal` is now reserved).
/// We no longer write it on new turns and the UI no longer reads it.
#[derive(Debug, Clone, Queryable, Selectable, Serialize, Deserialize)]
#[diesel(table_name = crate::schema::conversation_messages)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Message {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub role: String,
    pub content: String,
    pub attached_document_ids: Vec<Option<Uuid>>,
    pub citations: serde_json::Value,
    pub confidence: Option<f64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::conversations)]
struct NewConversation {
    user_id: Uuid,
    title: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::conversation_messages)]
struct NewMessage<'a> {
    conversation_id: Uuid,
    role: &'a str,
    content: &'a str,
    attached_document_ids: &'a [Option<Uuid>],
    citations: serde_json::Value,
    confidence: Option<f64>,
}

// ---------------------------------------------------------------------------
// CRUD
// ---------------------------------------------------------------------------

/// Create an empty conversation for `user_id` with the default title.
///
/// # Errors
///
/// `AppError::Db` on insert failure.
pub async fn create_conversation(
    conn: &mut AsyncPgConnection,
    user_id: Uuid,
) -> Result<Conversation, AppError> {
    use crate::schema::conversations::dsl;

    let row = NewConversation {
        user_id,
        title: DEFAULT_TITLE.to_string(),
    };

    diesel::insert_into(dsl::conversations)
        .values(&row)
        .returning(Conversation::as_returning())
        .get_result(conn)
        .await
        .map_err(AppError::from)
}

/// List conversations for `user_id`, most recently updated first.
///
/// # Errors
///
/// `AppError::Db` on query failure.
pub async fn list_conversations(
    conn: &mut AsyncPgConnection,
    user_id: Uuid,
) -> Result<Vec<Conversation>, AppError> {
    use crate::schema::conversations::dsl;

    dsl::conversations
        .filter(dsl::user_id.eq(user_id))
        .order(dsl::updated_at.desc())
        .select(Conversation::as_select())
        .load(conn)
        .await
        .map_err(AppError::from)
}

/// Load a conversation by id, verifying ownership.
///
/// # Errors
///
/// `AppError::Validation` ("conversation not found") if the id doesn't exist
/// or belongs to a different user. The two cases collapse so we never leak
/// ownership to an attacker probing ids.
pub async fn load_conversation(
    conn: &mut AsyncPgConnection,
    user_id: Uuid,
    conversation_id: Uuid,
) -> Result<Conversation, AppError> {
    use crate::schema::conversations::dsl;

    let row: Option<Conversation> = dsl::conversations
        .filter(dsl::id.eq(conversation_id))
        .filter(dsl::user_id.eq(user_id))
        .select(Conversation::as_select())
        .first(conn)
        .await
        .optional()
        .map_err(AppError::from)?;

    row.ok_or_else(|| AppError::Validation("conversation not found".to_string()))
}

/// Load every message in `conversation_id`, oldest first.
///
/// Caller must have already verified ownership via `load_conversation`.
///
/// # Errors
///
/// `AppError::Db` on query failure.
pub async fn list_messages(
    conn: &mut AsyncPgConnection,
    conversation_id: Uuid,
) -> Result<Vec<Message>, AppError> {
    use crate::schema::conversation_messages::dsl;

    dsl::conversation_messages
        .filter(dsl::conversation_id.eq(conversation_id))
        .order(dsl::created_at.asc())
        .select(Message::as_select())
        .load(conn)
        .await
        .map_err(AppError::from)
}

/// Append a user message to a conversation and bump its `updated_at`.
///
/// If the conversation's title is still the default, promote the first
/// non-empty prefix of `content` as the title so the sidebar label becomes
/// self-describing.
///
/// # Errors
///
/// `AppError::Db` on write failure.
pub async fn append_user_message(
    conn: &mut AsyncPgConnection,
    conversation: &Conversation,
    content: &str,
    attached_document_ids: &[Uuid],
) -> Result<Message, AppError> {
    use crate::schema::conversation_messages::dsl as msg_dsl;
    use crate::schema::conversations::dsl as conv_dsl;

    // Array elements on PG uuid[] are typed-nullable; wrap in Some(_)
    // so the column shape lines up with the Queryable side.
    let attached: Vec<Option<Uuid>> = attached_document_ids.iter().copied().map(Some).collect();

    let row = NewMessage {
        conversation_id: conversation.id,
        role: "user",
        content,
        attached_document_ids: &attached,
        citations: serde_json::json!([]),
        confidence: None,
    };

    let inserted: Message = diesel::insert_into(msg_dsl::conversation_messages)
        .values(&row)
        .returning(Message::as_returning())
        .get_result(conn)
        .await
        .map_err(AppError::from)?;

    // Only overwrite the title while it's still the default — never stomp a
    // user-edited name. Trim + truncate keeps the sidebar tidy.
    let new_title = if conversation.title == DEFAULT_TITLE {
        Some(derive_title(content))
    } else {
        None
    };

    match new_title {
        Some(t) => {
            diesel::update(conv_dsl::conversations.filter(conv_dsl::id.eq(conversation.id)))
                .set((
                    conv_dsl::title.eq(t),
                    conv_dsl::updated_at.eq(diesel::dsl::now),
                ))
                .execute(conn)
                .await
                .map_err(AppError::from)?;
        }
        None => {
            diesel::update(conv_dsl::conversations.filter(conv_dsl::id.eq(conversation.id)))
                .set(conv_dsl::updated_at.eq(diesel::dsl::now))
                .execute(conn)
                .await
                .map_err(AppError::from)?;
        }
    }

    Ok(inserted)
}

/// Append an assistant reply to a conversation.
///
/// # Errors
///
/// `AppError::Db` on write failure.
pub async fn append_assistant_message(
    conn: &mut AsyncPgConnection,
    conversation_id: Uuid,
    content: &str,
    citations: serde_json::Value,
    confidence: Option<f64>,
) -> Result<Message, AppError> {
    use crate::schema::conversation_messages::dsl as msg_dsl;
    use crate::schema::conversations::dsl as conv_dsl;

    // Assistant replies don't pin documents; the attachment
    // column is always empty for the assistant row.
    let attached: Vec<Option<Uuid>> = Vec::new();

    let row = NewMessage {
        conversation_id,
        role: "assistant",
        content,
        attached_document_ids: &attached,
        citations,
        confidence,
    };

    let inserted: Message = diesel::insert_into(msg_dsl::conversation_messages)
        .values(&row)
        .returning(Message::as_returning())
        .get_result(conn)
        .await
        .map_err(AppError::from)?;

    diesel::update(conv_dsl::conversations.filter(conv_dsl::id.eq(conversation_id)))
        .set(conv_dsl::updated_at.eq(diesel::dsl::now))
        .execute(conn)
        .await
        .map_err(AppError::from)?;

    Ok(inserted)
}

/// Overwrite the conversation's pinned-document set.
///
/// `None` clears any prior explicit selection back to the default
/// "every complete library doc" mode. `Some(ids)` stores the chosen
/// subset (empty vector == "no documents for this thread").
///
/// Ownership is re-verified in the WHERE clause.
///
/// # Errors
///
/// `AppError::Db` on write failure.
pub async fn set_pinned_document_ids(
    conn: &mut AsyncPgConnection,
    user_id: Uuid,
    conversation_id: Uuid,
    pinned: Option<&[Uuid]>,
) -> Result<usize, AppError> {
    use crate::schema::conversations::dsl;

    let value: Option<Vec<Option<Uuid>>> =
        pinned.map(|ids| ids.iter().copied().map(Some).collect());

    diesel::update(
        dsl::conversations
            .filter(dsl::id.eq(conversation_id))
            .filter(dsl::user_id.eq(user_id)),
    )
    .set((
        dsl::pinned_document_ids.eq(value),
        dsl::updated_at.eq(diesel::dsl::now),
    ))
    .execute(conn)
    .await
    .map_err(AppError::from)
}

/// Delete a conversation. Ownership is re-verified in the WHERE clause so
/// a stolen id from one user can't drop another user's thread.
///
/// Returns the number of rows deleted (0 = not found or not owned).
///
/// # Errors
///
/// `AppError::Db` on write failure.
pub async fn delete_conversation(
    conn: &mut AsyncPgConnection,
    user_id: Uuid,
    conversation_id: Uuid,
) -> Result<usize, AppError> {
    use crate::schema::conversations::dsl;

    diesel::delete(
        dsl::conversations
            .filter(dsl::id.eq(conversation_id))
            .filter(dsl::user_id.eq(user_id)),
    )
    .execute(conn)
    .await
    .map_err(AppError::from)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Derive a short title from the first user message. Trims whitespace,
/// takes the leading line, and caps the character count.
fn derive_title(content: &str) -> String {
    let first_line = content.trim().lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        return DEFAULT_TITLE.to_string();
    }
    let mut out: String = first_line.chars().take(AUTO_TITLE_MAX_CHARS).collect();
    if first_line.chars().count() > AUTO_TITLE_MAX_CHARS {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_title_caps_length() {
        let long = "a".repeat(200);
        let t = derive_title(&long);
        assert!(t.chars().count() <= AUTO_TITLE_MAX_CHARS + 1); // +1 for ellipsis
        assert!(t.ends_with('…'));
    }

    #[test]
    fn derive_title_takes_first_line() {
        assert_eq!(derive_title("first\nsecond"), "first");
    }

    #[test]
    fn derive_title_falls_back_on_empty() {
        assert_eq!(derive_title("   \n "), DEFAULT_TITLE);
    }
}
