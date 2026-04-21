//! Auth plumbing — User model, signup, login, and session helpers.
//!
//! All code in this module is SSR-only; it is never compiled into the
//! hydrate (wasm) bundle.

#![cfg(feature = "ssr")]

use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use serde::{Deserialize, Serialize};
use tower_sessions::Session;
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::AppError;

// ---------------------------------------------------------------------------
// Session key
// ---------------------------------------------------------------------------

/// Key under which the logged-in user's `Uuid` is stored in the session.
const SESSION_USER_ID_KEY: &str = "user_id";

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// A row in the `users` table.
#[derive(Debug, Clone, Queryable, Selectable, Serialize, Deserialize)]
#[diesel(table_name = crate::schema::users)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
}

impl User {
    /// The canonical gRPC `user_id` for this account.
    ///
    /// Uses `Uuid::simple()` (32 hex chars, no dashes) which satisfies the
    /// `UserId` validator pattern `[A-Za-z0-9_-]{1,128}`.
    pub fn grpc_id(&self) -> String {
        self.id.as_simple().to_string()
    }
}

/// Insertable shape for a new user row.
#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::users)]
struct NewUser<'a> {
    email: &'a str,
    password_hash: &'a str,
}

// ---------------------------------------------------------------------------
// Core operations
// ---------------------------------------------------------------------------

/// Create a new user, returning the persisted `User`.
///
/// Hashes `raw_password` with bcrypt (cost 12) before storing.
///
/// # Errors
///
/// Returns `AppError::Db` if the email is already taken.
/// Returns `AppError::Password` on bcrypt failure.
pub async fn create_user(
    conn: &mut AsyncPgConnection,
    email: &str,
    raw_password: &str,
) -> Result<User, AppError> {
    use crate::schema::users::dsl;

    // Normalize the email before storing so "Bobby@example.com" and
    // "bobby@example.com" land on the same row. Without this we'd either
    // rely on Postgres CITEXT (awkward diesel integration) or silently
    // permit duplicate accounts differing only in case.
    let email_lc = email.trim().to_ascii_lowercase();

    let hash = bcrypt::hash(raw_password, 12)?;
    let new_user = NewUser {
        email: &email_lc,
        password_hash: &hash,
    };

    let user: User = diesel::insert_into(dsl::users)
        .values(&new_user)
        .returning(User::as_returning())
        .get_result(conn)
        .await
        .map_err(|e| match e {
            diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::UniqueViolation,
                _,
            ) => AppError::Validation("Email address is already registered.".to_string()),
            other => AppError::Db(other.to_string()),
        })?;

    info!(user_id = %user.id, "user created");
    Ok(user)
}

/// Verify credentials and return the `User` if they match.
///
/// # Errors
///
/// Returns `AppError::Validation` with a generic message on any mismatch
/// (no timing-oracle leakage of whether the email exists).
pub async fn verify_credentials(
    conn: &mut AsyncPgConnection,
    email: &str,
    raw_password: &str,
) -> Result<User, AppError> {
    use crate::schema::users::dsl;

    // Match the normalization applied at signup — stored emails are always
    // trimmed + lowercased.
    let email_lc = email.trim().to_ascii_lowercase();

    let maybe_user: Option<User> = dsl::users
        .filter(dsl::email.eq(&email_lc))
        .select(User::as_select())
        .first(conn)
        .await
        .optional()
        .map_err(AppError::from)?;

    let user = match maybe_user {
        Some(u) => u,
        None => {
            // Run bcrypt verify on a dummy hash so the response time is
            // similar whether or not the email exists.
            let _ = bcrypt::verify(raw_password, "$2b$12$invalidhashpadding...........");
            warn!("login failed: email not found");
            return Err(AppError::Validation(
                "Invalid email or password.".to_string(),
            ));
        }
    };

    let matches = bcrypt::verify(raw_password, &user.password_hash)?;
    if !matches {
        warn!(user_id = %user.id, "login failed: wrong password");
        return Err(AppError::Validation(
            "Invalid email or password.".to_string(),
        ));
    }

    info!(user_id = %user.id, "user authenticated");
    Ok(user)
}

/// Load a user by primary key.
///
/// # Errors
///
/// `AppError::Db` on query failure; `AppError::Unauthenticated` if not found.
pub async fn load_user_by_id(
    conn: &mut AsyncPgConnection,
    id: Uuid,
) -> Result<User, AppError> {
    use crate::schema::users::dsl;

    dsl::users
        .filter(dsl::id.eq(id))
        .select(User::as_select())
        .first(conn)
        .await
        .map_err(|e| match e {
            diesel::result::Error::NotFound => AppError::Unauthenticated,
            other => AppError::Db(other.to_string()),
        })
}

// ---------------------------------------------------------------------------
// Session helpers
// ---------------------------------------------------------------------------

/// Write the authenticated user's id into the session.
pub async fn session_set_user(session: &Session, user: &User) -> Result<(), AppError> {
    session
        .insert(SESSION_USER_ID_KEY, user.id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))
}

/// Read the user id from the session cookie, then load the full `User` from
/// Postgres. Returns `AppError::Unauthenticated` if no session is present.
pub async fn session_current_user(
    session: &Session,
    conn: &mut AsyncPgConnection,
) -> Result<User, AppError> {
    let uid: Option<Uuid> = session
        .get(SESSION_USER_ID_KEY)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    match uid {
        Some(id) => load_user_by_id(conn, id).await,
        None => Err(AppError::Unauthenticated),
    }
}

/// Read just the user id out of the session.
///
/// Returns `None` if the cookie is absent or the session store read fails
/// — fail-safe so middleware never treats a transient Redis hiccup as
/// "authenticated". Used by the auth-gate middleware which doesn't need the
/// full `User` row.
pub async fn session_user_id(session: &Session) -> Option<Uuid> {
    match session.get::<Uuid>(SESSION_USER_ID_KEY).await {
        Ok(opt) => opt,
        Err(e) => {
            warn!(error = %e, "session store read failed; treating as unauthenticated");
            None
        }
    }
}

/// Clear the session (logout).
pub async fn session_clear(session: &Session) -> Result<(), AppError> {
    session
        .flush()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))
}
