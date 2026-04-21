//! gRPC client helpers — SSR only. Every request creates a fresh
//! `tonic::Channel` for now (simple, correct, ~2ms overhead on
//! localhost). Swap to a shared pool when p99 tail latency becomes
//! a concern.

#![cfg(feature = "ssr")]

use std::env;

use tonic::transport::Channel;

use crate::pb::agent_client::AgentClient;

/// Connect to the agent gRPC service.
///
/// Endpoint resolution order:
/// 1. `AGENT_RS_GRPC_URL` env var (full `http://host:port`)
/// 2. Default `http://localhost:1072`
///
/// Callers clone the returned client cheaply (tonic Channels are
/// `Clone` and multiplex HTTP/2 streams underneath).
pub async fn connect() -> Result<AgentClient<Channel>, anyhow::Error> {
    let url = env::var("AGENT_RS_GRPC_URL")
        .unwrap_or_else(|_| "http://localhost:1072".to_string());
    let channel = Channel::from_shared(url)?.connect().await?;
    Ok(AgentClient::new(channel))
}
