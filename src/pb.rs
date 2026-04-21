//! Re-exported Prost-generated client types for agent.v1.
//!
//! Scope: SSR only. The wasm hydrate bundle never links the network
//! stack; all gRPC calls run server-side inside axum handlers / leptos
//! server functions. Keeping this module behind `cfg(feature = "ssr")`
//! avoids polluting the browser with protobuf codegen.

#![cfg(feature = "ssr")]
#![allow(clippy::all)] // generated code

tonic::include_proto!("agent.v1");
