//! Axum middleware layer that gates page rendering on session state.
//!
//! Server functions already authenticate themselves via
//! `auth::session_current_user`; this layer exists to prevent the
//! authenticated SSR shell (`/chat`, `/library`, etc.) from rendering at all
//! for unauthenticated visitors, and to bounce already-logged-in users away
//! from `/login` and `/signup`.
//!
//! Static assets (`/pkg`, `/favicon.ico`) and server-function endpoints
//! (`/api/*`) are intentionally not gated — the server functions handle
//! their own auth and must be reachable by the client regardless of page
//! context.

#![cfg(feature = "ssr")]

use axum::{
    extract::Request,
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use tower_sessions::Session;

use crate::auth::session_user_id;

/// Gate rendered pages on authentication state.
///
/// Unauthenticated request for a protected page → 303 redirect to `/login`.
/// Authenticated request for a guest-only page → 303 redirect to `/chat`.
/// Everything else falls through to the next layer.
pub async fn auth_gate(session: Session, req: Request, next: Next) -> Response {
    let path = req.uri().path();

    let authed = session_user_id(&session).await.is_some();

    if is_protected_page(path) && !authed {
        return Redirect::to("/login").into_response();
    }
    if is_guest_only_page(path) && authed {
        return Redirect::to("/chat").into_response();
    }
    // Authenticated hit on `/` — jump straight to `/chat` server-side so
    // we never fall through to the client-side `<meta refresh>` fallback.
    if path == "/" && authed {
        return Redirect::to("/chat").into_response();
    }

    next.run(req).await
}

// ---------------------------------------------------------------------------
// Path classification
// ---------------------------------------------------------------------------

/// Pages that require a logged-in user.
///
/// `/` is included so unauthenticated visitors are routed to `/login`
/// instead of hitting the client-side refresh tag inside `RootRedirect`.
fn is_protected_page(path: &str) -> bool {
    path == "/"
        || path == "/chat"
        || path.starts_with("/chat/")
        || path == "/library"
        || path.starts_with("/library/")
}

/// Pages that only make sense for anonymous visitors.
fn is_guest_only_page(path: &str) -> bool {
    path == "/login" || path == "/signup"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_matches_expected_paths() {
        assert!(is_protected_page("/"));
        assert!(is_protected_page("/chat"));
        assert!(is_protected_page("/chat/abc"));
        assert!(is_protected_page("/library"));
        assert!(is_protected_page("/library/foo"));

        assert!(!is_protected_page("/login"));
        assert!(!is_protected_page("/signup"));
        assert!(!is_protected_page("/api/ask_action"));
        assert!(!is_protected_page("/pkg/agent-rs-web.css"));
    }

    #[test]
    fn guest_only_matches_expected_paths() {
        assert!(is_guest_only_page("/login"));
        assert!(is_guest_only_page("/signup"));
        assert!(!is_guest_only_page("/chat"));
        assert!(!is_guest_only_page("/"));
    }
}
