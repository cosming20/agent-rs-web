//! `/login` — GET renders the form; POST authenticates via `login_action`.

use leptos::prelude::*;
use leptos_router::components::A;

// ---------------------------------------------------------------------------
// Server function
// ---------------------------------------------------------------------------

/// Authenticate an existing user and persist their id in the session cookie.
///
/// On success returns `Ok(())` and the client redirects to `/chat`.
///
/// # Errors
///
/// Returns a `ServerFnError` with a user-visible message on bad credentials
/// or internal failures.
#[server(LoginAction, "/api")]
pub async fn login_action(email: String, password: String) -> Result<(), ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use leptos_axum::extract;
        use tower_sessions::Session;

        use crate::auth::{session_set_user, verify_credentials};
        use crate::db::DbPool;
        use crate::error::AppError;

        let pool = use_context::<DbPool>()
            .ok_or_else(|| AppError::Internal("db pool missing from context".to_string()))
            .map_err(|e| e.into_server_fn_error())?;
        let session: Session = extract().await.map_err(|e| {
            AppError::Internal(format!("session extract failed: {e}"))
                .into_server_fn_error()
        })?;

        let mut conn = pool
            .get()
            .await
            .map_err(|e| AppError::from(e).into_server_fn_error())?;
        let user = verify_credentials(&mut conn, &email, &password)
            .await
            .map_err(|e| e.into_server_fn_error())?;

        session_set_user(&session, &user)
            .await
            .map_err(|e| e.into_server_fn_error())?;

        leptos_axum::redirect("/chat");
        Ok(())
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (email, password);
        Err(ServerFnError::ServerError(
            "server function called on client".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

#[component]
pub fn LoginPage() -> impl IntoView {
    let action = ServerAction::<LoginAction>::new();
    let error_msg = move || {
        action.value().get().and_then(|r| r.err()).map(|e| {
            let msg = e.to_string();
            // Strip the "server error: " prefix leptos prepends.
            msg.trim_start_matches("server error: ").to_string()
        })
    };

    view! {
        <div style="max-width:420px;margin:80px auto;font-family:sans-serif">
            <h1 style="margin-bottom:24px">"Sign in"</h1>
            <ActionForm action=action>
                <div style="margin-bottom:12px">
                    <label for="email">"Email"</label><br/>
                    <input
                        id="email"
                        name="email"
                        type="email"
                        required
                        autocomplete="email"
                        style="width:100%;padding:8px;box-sizing:border-box"
                    />
                </div>
                <div style="margin-bottom:16px">
                    <label for="password">"Password"</label><br/>
                    <input
                        id="password"
                        name="password"
                        type="password"
                        required
                        autocomplete="current-password"
                        style="width:100%;padding:8px;box-sizing:border-box"
                    />
                </div>
                <Show when=move || error_msg().is_some()>
                    <p style="color:red;margin-bottom:10px">{move || error_msg().unwrap_or_default()}</p>
                </Show>
                <button type="submit" style="width:100%;padding:10px;font-size:1rem">
                    "Sign in"
                </button>
            </ActionForm>
            <p style="margin-top:16px;text-align:center">
                "No account? " <A href="/signup">"Sign up"</A>
            </p>
        </div>
    }
}
