//! `/signup` — GET renders the form; POST creates a user via `signup_action`.

use leptos::prelude::*;
use leptos_router::components::A;

// ---------------------------------------------------------------------------
// Server function
// ---------------------------------------------------------------------------

/// Create a new user account, set the session cookie, and redirect to `/chat`.
///
/// # Errors
///
/// Returns a `ServerFnError` with a user-visible message when the email is
/// already taken or passwords are invalid.
#[server(SignupAction, "/api")]
pub async fn signup_action(email: String, password: String) -> Result<(), ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use leptos_axum::extract;
        use tower_sessions::Session;

        use crate::auth::{create_user, session_set_user};
        use crate::db::DbPool;
        use crate::error::AppError;

        if password.len() < 8 {
            return Err(AppError::Validation(
                "Password must be at least 8 characters.".to_string(),
            )
            .into_server_fn_error());
        }

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
        let user = create_user(&mut conn, &email, &password)
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
pub fn SignupPage() -> impl IntoView {
    let action = ServerAction::<SignupAction>::new();
    let error_msg = move || {
        action.value().get().and_then(|r| r.err()).map(|e| {
            let msg = e.to_string();
            msg.trim_start_matches("server error: ").to_string()
        })
    };

    view! {
        <div style="max-width:420px;margin:80px auto;font-family:sans-serif">
            <h1 style="margin-bottom:24px">"Create account"</h1>
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
                        minlength="8"
                        autocomplete="new-password"
                        style="width:100%;padding:8px;box-sizing:border-box"
                    />
                </div>
                <Show when=move || error_msg().is_some()>
                    <p style="color:red;margin-bottom:10px">{move || error_msg().unwrap_or_default()}</p>
                </Show>
                <button type="submit" style="width:100%;padding:10px;font-size:1rem">
                    "Create account"
                </button>
            </ActionForm>
            <p style="margin-top:16px;text-align:center">
                "Have an account? " <A href="/login">"Sign in"</A>
            </p>
        </div>
    }
}
