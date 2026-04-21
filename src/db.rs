//! Postgres connection pool — SSR only.
//!
//! Wraps `diesel-async` + `bb8` into a `DbPool` newtype that is stored in
//! axum's state and injected into every server function via Leptos context.

#![cfg(feature = "ssr")]

use std::env;

use diesel_async::pooled_connection::bb8::Pool;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::AsyncPgConnection;

use crate::error::AppError;

/// Type alias for the connection pool.
pub type DbPool = Pool<AsyncPgConnection>;

/// Build a `bb8` Postgres pool from `DATABASE_URL`.
///
/// # Errors
///
/// Returns `AppError::Db` if the URL is missing or the pool cannot be
/// established (e.g., Postgres is not running).
pub async fn build_pool() -> Result<DbPool, AppError> {
    let url =
        env::var("DATABASE_URL").map_err(|_| AppError::Db("DATABASE_URL not set".to_string()))?;

    let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(url);
    Pool::builder()
        .build(manager)
        .await
        .map_err(|e| AppError::Db(e.to_string()))
}
