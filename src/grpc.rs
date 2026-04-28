//! gRPC client helpers — SSR only. Every handler creates a fresh
//! `tonic::Channel` for now (simple, correct, ~2ms overhead on
//! localhost). Swap to a shared pool when p99 tail latency becomes a
//! concern.
//!
//! The 4-RPC surface exposed by `agent.v1.Agent`:
//!   - `Ask(AskRequest) returns stream AskEvent` — server-streaming
//!     query with indexing-wait ticks, partial-answer text deltas,
//!     and a terminal `Final` (followed by `BudgetSnapshot`) or
//!     `Error` event.
//!   - `EnqueueIndex(EnqueueIndexRequest) returns EnqueueIndexResponse`
//!   - `GetDocumentStatus(GetDocumentStatusRequest) returns GetDocumentStatusResponse`
//!   - `DeleteDocument(DeleteDocumentRequest) returns DeleteDocumentResponse`
//!
//! The Ask flow exposed to the rest of the crate is `ask_stream`,
//! which yields `AskEvent`s as they arrive so the SSE handler in
//! `routes/chat.rs` can forward partial-answer deltas + the terminal
//! events to the browser without buffering.

#![cfg(feature = "ssr")]

use std::env;

use futures::stream::{Stream, StreamExt};
use tonic::transport::Channel;

use crate::pb::agent_client::AgentClient;
use crate::pb::{
    AskEvent, AskRequest, DeleteDocumentRequest, DeleteDocumentResponse, EnqueueIndexRequest,
    EnqueueIndexResponse, GetDocumentStatusRequest, GetDocumentStatusResponse,
};

/// Default agent gRPC endpoint when `AGENT_RS_GRPC_URL` is unset. Matches
/// the agent-rs Tonic listener port baked into its `docker-compose.yml`.
const DEFAULT_AGENT_GRPC_URL: &str = "http://localhost:1072";

/// Connect to the agent gRPC service.
///
/// Endpoint resolution order:
/// 1. `AGENT_RS_GRPC_URL` env var (full `http://host:port`)
/// 2. Default [`DEFAULT_AGENT_GRPC_URL`]
///
/// # Errors
///
/// Returns the underlying tonic transport error if the URL fails to
/// parse or the TCP/HTTP2 handshake never completes.
pub async fn connect() -> Result<AgentClient<Channel>, anyhow::Error> {
    let url = env::var("AGENT_RS_GRPC_URL").unwrap_or_else(|_| DEFAULT_AGENT_GRPC_URL.to_string());
    let channel = Channel::from_shared(url)?.connect().await?;
    Ok(AgentClient::new(channel))
}

/// Fire `EnqueueIndex` and return the response. Errors map to `anyhow` so
/// the caller can attach route-specific context.
///
/// # Errors
///
/// Returns an error on transport failure or non-OK status from the
/// agent service.
pub async fn enqueue_index(
    user_id: &str,
    minio_object_key: &str,
) -> Result<EnqueueIndexResponse, anyhow::Error> {
    let mut client = connect().await?;
    let resp = client
        .enqueue_index(EnqueueIndexRequest {
            user_id: user_id.to_string(),
            minio_object_key: minio_object_key.to_string(),
        })
        .await?;
    Ok(resp.into_inner())
}

/// Fire `GetDocumentStatus` and return the response.
///
/// # Errors
///
/// Returns an error on transport failure or non-OK status from the
/// agent service.
pub async fn get_document_status(
    user_id: &str,
    minio_object_key: &str,
) -> Result<GetDocumentStatusResponse, anyhow::Error> {
    let mut client = connect().await?;
    let resp = client
        .get_document_status(GetDocumentStatusRequest {
            user_id: user_id.to_string(),
            minio_object_key: minio_object_key.to_string(),
        })
        .await?;
    Ok(resp.into_inner())
}

/// Fire `DeleteDocument` and return the response.
///
/// # Errors
///
/// Returns an error on transport failure or non-OK status from the
/// agent service.
pub async fn delete_document(
    user_id: &str,
    minio_object_key: &str,
) -> Result<DeleteDocumentResponse, anyhow::Error> {
    let mut client = connect().await?;
    let resp = client
        .delete_document(DeleteDocumentRequest {
            user_id: user_id.to_string(),
            minio_object_key: minio_object_key.to_string(),
        })
        .await?;
    Ok(resp.into_inner())
}

/// Open the `Ask` server-streaming RPC and yield each `AskEvent` as it
/// arrives.
///
/// Returning a `Stream` keeps the network I/O lazy: callers can pipe
/// the event stream directly into an SSE response without ever
/// buffering the whole conversation in memory. Callers that want a
/// flat `Vec` can `.collect()` the stream — but no production caller
/// should need that, so we don't ship a sync helper.
///
/// # Errors
///
/// Returns an error on the initial connect / RPC dispatch. Per-event
/// transport errors arrive as `Err` items inside the returned stream.
pub async fn ask_stream(
    request: AskRequest,
) -> Result<impl Stream<Item = Result<AskEvent, anyhow::Error>>, anyhow::Error> {
    let mut client = connect().await?;
    let stream = client.ask(request).await?.into_inner();
    Ok(stream.map(|res| res.map_err(anyhow::Error::from)))
}
