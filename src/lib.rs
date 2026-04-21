//! agent-rs-web — Leptos SSR frontend for the agent-rs platform.

pub mod app;
pub mod error;
pub mod grpc;
pub mod pb;
pub mod routes;

#[cfg(feature = "ssr")]
pub mod auth;
#[cfg(feature = "ssr")]
pub mod db;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    use crate::app::App;
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(App);
}
