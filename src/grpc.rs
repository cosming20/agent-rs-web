//! gRPC client helpers — SSR only. Every handler creates a fresh
//! `tonic::Channel` for now (simple, correct, ~2ms overhead on
//! localhost). Swap to a shared pool when p99 tail latency becomes a
//! concern.
//!
//! The 4-RPC surface exposed by `agent.v1.Agent`:
//!   - `Ask(AskRequest) returns stream AskEvent` — server-streaming
//!     query with indexing-wait ticks + terminal `Final` / `Error`
//!   - `EnqueueIndex(EnqueueIndexRequest) returns EnqueueIndexResponse`
//!   - `GetDocumentStatus(GetDocumentStatusRequest) returns GetDocumentStatusResponse`
//!   - `DeleteDocument(DeleteDocumentRequest) returns DeleteDocumentResponse`

#![cfg(feature = "ssr")]

use std::env;

use tonic::transport::Channel;

use crate::pb::agent_client::AgentClient;
use crate::pb::{
    AskEvent, AskRequest, DeleteDocumentRequest, DeleteDocumentResponse, EnqueueIndexRequest,
    EnqueueIndexResponse, GetDocumentStatusRequest, GetDocumentStatusResponse,
};

/// Connect to the agent gRPC service.
///
/// Endpoint resolution order:
/// 1. `AGENT_RS_GRPC_URL` env var (full `http://host:port`)
/// 2. Default `http://localhost:1072`
pub async fn connect() -> Result<AgentClient<Channel>, anyhow::Error> {
    let url = env::var("AGENT_RS_GRPC_URL")
        .unwrap_or_else(|_| "http://localhost:1072".to_string());
    let channel = Channel::from_shared(url)?.connect().await?;
    Ok(AgentClient::new(channel))
}

/// Fire `EnqueueIndex` and return the response. Errors map to `anyhow` so
/// the caller can attach route-specific context.
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

/// Drive the `Ask` server-streaming RPC and collect every event in order.
///
/// For the current MVP UI we block on the full stream and render the
/// terminal `Final` event; this simplifies the handler at the cost of
/// losing the live `AskIndexingWait` / `AskPartialAnswer` UX. Callers
/// that want streaming can consume the raw tonic stream via `connect`.
pub async fn ask_collect(request: AskRequest) -> Result<Vec<AskEvent>, anyhow::Error> {
    let mut client = connect().await?;
    let mut stream = client.ask(request).await?.into_inner();

    let mut events = Vec::new();
    while let Some(event) = stream.message().await? {
        events.push(event);
    }
    Ok(events)
}
