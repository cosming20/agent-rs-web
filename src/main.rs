//! Binary entry point — SSR only.
//!
//! Sets up:
//! - `diesel-async` Postgres pool (from `DATABASE_URL`)
//! - `tower-sessions` `MemoryStore` session layer
//! - Leptos + axum router with context injection

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use axum::{routing::post, Extension, Router};
    use fred::prelude::{ClientLike, Config as FredConfig, Pool as FredPool};
    use leptos::logging::log;
    use leptos::prelude::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use time::Duration;
    use tower_sessions::{Expiry, SessionManagerLayer};
    use tower_sessions_redis_store::RedisStore;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

    use agent_rs_web::app::{shell, App};
    use agent_rs_web::db::{build_pool, DbPool};
    use agent_rs_web::minio_client::{MinioClient, MinioConfig};
    use agent_rs_web::routes::chat::AskStreamState;
    use agent_rs_web::routes::library::UploadState;

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
    // Session layer — Redis backed (persistent across restarts, shardable)
    // ---------------------------------------------------------------------------
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:1074".into());
    let fred_cfg = FredConfig::from_url(&redis_url).expect("invalid REDIS_URL");
    let fred_pool = FredPool::new(fred_cfg, None, None, None, 4).expect("build Redis pool");
    // Fred 10: `init()` spawns the reconnect loop and returns a
    // JoinHandle. `connect()` then drives the actual TCP connect, and
    // `wait_for_connect` blocks until the pool is ready so the first
    // session read/write doesn't race against cold start.
    let _reconnect_handle = fred_pool.init().await.expect("start Redis reconnect loop");
    fred_pool.wait_for_connect().await.expect("connect Redis");
    let session_store = RedisStore::new(fred_pool);
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

    let minio_client = MinioClient::new(&MinioConfig::from_env());
    let upload_state = UploadState {
        pool: pool.clone(),
        minio: minio_client.clone(),
    };
    let ask_stream_state = AskStreamState { pool: pool.clone() };

    let pool_clone = pool.clone();
    let app = Router::new()
        // Plain axum handlers for routes leptos server fns can't model
        // (multipart upload, form-encoded redirect, server-streaming
        // SSE for the chat answer pipeline).
        .route(
            "/library/upload",
            post(agent_rs_web::routes::library::upload_handler),
        )
        .route(
            "/library/delete",
            post(agent_rs_web::routes::library::delete_handler),
        )
        .route(
            "/api/ask_stream",
            post(agent_rs_web::routes::chat::ask_stream_handler),
        )
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
        // UploadState is shared via Extension (not .with_state) so the
        // Router's `S` generic stays `LeptosOptions` — `.with_state`
        // here would change the type between our plain routes and the
        // leptos-registered server fns, quietly dropping server-fn
        // registration (observed: /api/logout_action returned 404
        // because the leptos_routes segment was attached to the wrong
        // type stack).
        .layer(Extension(upload_state))
        .layer(Extension(ask_stream_state))
        // Order matters: inner layer runs first. `from_fn(auth_gate)` needs
        // `Session` extracted, which is only available after `session_layer`
        // has run on the request path. Because tower layers compose inside-
        // out, the session layer must be added LAST here so it wraps the
        // auth gate.
        .layer(axum::middleware::from_fn(
            agent_rs_web::middleware::auth_gate,
        ))
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
