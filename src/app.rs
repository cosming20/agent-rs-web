//! Root Leptos application — shell + router.

use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
use leptos_router::{
    components::{Route, Router, Routes},
    StaticSegment,
};

use crate::routes::{
    chat::ChatPage,
    library::LibraryPage,
    login::LoginPage,
    signup::SignupPage,
};

// ---------------------------------------------------------------------------
// HTML shell (SSR entry point)
// ---------------------------------------------------------------------------

pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <AutoReload options=options.clone() />
                <HydrationScripts options/>
                <MetaTags/>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

// ---------------------------------------------------------------------------
// Root application component
// ---------------------------------------------------------------------------

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    view! {
        <Stylesheet id="leptos" href="/pkg/agent-rs-web.css"/>
        <Title text="agent-rs"/>
        <Router>
            <main>
                <Routes fallback=|| "Page not found.".into_view()>
                    // Public
                    <Route path=StaticSegment("login")  view=LoginPage/>
                    <Route path=StaticSegment("signup") view=SignupPage/>
                    // Authenticated
                    <Route path=StaticSegment("chat")    view=ChatPage/>
                    <Route path=StaticSegment("library") view=LibraryPage/>
                    // Root redirect — handled server-side in main.rs
                    <Route path=StaticSegment("") view=RootRedirect/>
                </Routes>
            </main>
        </Router>
    }
}

// ---------------------------------------------------------------------------
// Root redirect component
// ---------------------------------------------------------------------------

/// Placeholder that performs a client-side redirect.
/// The server-side redirect is injected via `main.rs` middleware.
#[component]
fn RootRedirect() -> impl IntoView {
    view! {
        <meta http-equiv="refresh" content="0;url=/chat"/>
        <p>"Redirecting…"</p>
    }
}
