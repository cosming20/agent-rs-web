//! Binary entry point — SSR only.
//!
//! Sets up:
//! - `diesel-async` Postgres pool (from `DATABASE_URL`)
//! - `tower-sessions` `MemoryStore` session layer
//! - Leptos + axum router with context injection

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use axum::Router;
    use leptos::logging::log;
    use leptos::prelude::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use time::Duration;
    use tower_sessions::{Expiry, MemoryStore, SessionManagerLayer};
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

    use agent_rs_web::app::{shell, App};
    use agent_rs_web::db::{build_pool, DbPool};

    // ---------------------------------------------------------------------------
    // Observability
    // ---------------------------------------------------------------------------
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // ---------------------------------------------------------------------------
    // Load .env (dev convenience; silently ignored if absent)
    // ---------------------------------------------------------------------------
    let _ = dotenvy::dotenv();

    // ---------------------------------------------------------------------------
    // Database pool
    // ---------------------------------------------------------------------------
    let pool: DbPool = build_pool()
        .await
        .expect("failed to build Postgres connection pool");

    // ---------------------------------------------------------------------------
    // Session layer
    // ---------------------------------------------------------------------------
    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store)
        .with_name("agent-rs-web-session")
        .with_secure(false) // set true behind TLS in production
        .with_expiry(Expiry::OnInactivity(Duration::days(30)));

    // ---------------------------------------------------------------------------
    // Leptos config + routes
    // ---------------------------------------------------------------------------
    let conf = get_configuration(None).expect("failed to read Leptos config");
    let addr = conf.leptos_options.site_addr;
    let leptos_options = conf.leptos_options;
    let routes = generate_route_list(App);

    let pool_clone = pool.clone();
    let app = Router::new()
        .leptos_routes_with_context(
            &leptos_options,
            routes,
            move || {
                provide_context(pool_clone.clone());
            },
            {
                let leptos_options = leptos_options.clone();
                move || shell(leptos_options.clone())
            },
        )
        .fallback(leptos_axum::file_and_error_handler(shell))
        .layer(session_layer)
        .with_state(leptos_options);

    log!("listening on http://{}", &addr);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind TCP listener");
    axum::serve(listener, app.into_make_service())
        .await
        .expect("server error");
}

#[cfg(not(feature = "ssr"))]
pub fn main() {
    // Hydration entry point is in lib.rs (`hydrate()`).
}
