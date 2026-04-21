//! Application-level error type.
//!
//! `AppError` is the central error enum used by every server function and
//! axum handler in this crate.  It maps to `ServerFnError` at the server-
//! function boundary so callers never see raw tonic / diesel internals.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    /// Database query or connection failure.
    #[error("database error: {0}")]
    Db(String),

    /// bcrypt hashing or verification failure.
    #[error("password error: {0}")]
    Password(String),

    /// The caller is not authenticated.
    #[error("not authenticated")]
    Unauthenticated,

    /// gRPC transport or application error.
    #[error("grpc error: {0}")]
    Grpc(String),

    /// A user-visible validation failure (e.g. email already taken).
    #[error("{0}")]
    Validation(String),

    /// Catch-all for unexpected failures.
    #[error("internal error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

#[cfg(feature = "ssr")]
impl From<diesel_async::pooled_connection::bb8::RunError> for AppError {
    fn from(e: diesel_async::pooled_connection::bb8::RunError) -> Self {
        Self::Db(e.to_string())
    }
}

#[cfg(feature = "ssr")]
impl From<diesel::result::Error> for AppError {
    fn from(e: diesel::result::Error) -> Self {
        Self::Db(e.to_string())
    }
}

#[cfg(feature = "ssr")]
impl From<bcrypt::BcryptError> for AppError {
    fn from(e: bcrypt::BcryptError) -> Self {
        Self::Password(e.to_string())
    }
}

#[cfg(feature = "ssr")]
impl From<tonic::Status> for AppError {
    fn from(e: tonic::Status) -> Self {
        Self::Grpc(e.message().to_string())
    }
}

#[cfg(feature = "ssr")]
impl From<tonic::transport::Error> for AppError {
    fn from(e: tonic::transport::Error) -> Self {
        Self::Grpc(e.to_string())
    }
}

#[cfg(feature = "ssr")]
impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

/// Helper: convert `AppError` into a `ServerFnError` string.
///
/// Use `AppError::into_server_fn_error()` instead of `From` to avoid
/// colliding with the blanket `impl<E: Error> From<E> for ServerFnError`.
impl AppError {
    pub fn into_server_fn_error(self) -> leptos::server_fn::ServerFnError {
        leptos::server_fn::ServerFnError::ServerError(self.to_string())
    }
}
