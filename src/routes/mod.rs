//! Route modules — one file per page.

pub mod chat;
pub mod library;
pub mod login;
pub mod signup;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Redirect to a given path from inside a server function.
#[cfg(feature = "ssr")]
#[allow(dead_code)]
pub fn redirect_to(path: &str) {
    leptos_axum::redirect(path);
}

// ---------------------------------------------------------------------------
// Logout server function (no dedicated page; just a POST endpoint)
// ---------------------------------------------------------------------------

/// Clear the session and redirect to `/login`.
///
/// # Errors
///
/// Returns `ServerFnError` on session store failure.
#[leptos::server(LogoutAction, "/api")]
pub async fn logout_action() -> Result<(), leptos::prelude::ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use leptos_axum::extract;
        use tower_sessions::Session;

        use crate::auth::session_clear;
        use crate::error::AppError;

        let session: Session = extract().await.map_err(|e| {
            AppError::Internal(format!("session extract failed: {e}")).into_server_fn_error()
        })?;

        session_clear(&session)
            .await
            .map_err(|e| e.into_server_fn_error())?;
        leptos_axum::redirect("/login");
        Ok(())
    }
    #[cfg(not(feature = "ssr"))]
    {
        Err(leptos::prelude::ServerFnError::ServerError(
            "server function called on client".to_string(),
        ))
    }
}
