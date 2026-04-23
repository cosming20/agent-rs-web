//! MinIO / S3-compatible storage client for per-user document uploads.
//!
//! Upload flow:
//! 1. Web receives a browser multipart upload.
//! 2. Handler derives a fresh `document_uuid` + MinIO key
//!    `users/{user_id}/docs/{document_uuid}.{ext}` (matching
//!    agent-rs-stores::minio conventions).
//! 3. `put_bytes` uploads the bytes.
//! 4. Handler inserts an `ingested_documents` row + calls
//!    `EnqueueIndex` gRPC so the indexer worker picks it up.
//!
//! The client is SSR-only — its types never cross the wasm boundary.

#![cfg(feature = "ssr")]

use std::env;

use aws_sdk_s3::Client;
use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use uuid::Uuid;

use crate::error::AppError;

/// Fallback endpoint matching the dev-loop docker-compose.
const DEFAULT_ENDPOINT: &str = "http://localhost:1069";
const DEFAULT_BUCKET: &str = "agent-rs-docs";
const DEFAULT_ACCESS_KEY: &str = "minioadmin";
const DEFAULT_SECRET_KEY: &str = "minioadmin";

/// Settings loaded once at app startup.
#[derive(Debug, Clone)]
pub struct MinioConfig {
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
    pub bucket: String,
}

impl MinioConfig {
    /// Build a config from env vars, falling back to docker-compose defaults.
    ///
    /// Override each field via `AGENT_RS__MINIO__{ENDPOINT,ACCESS_KEY,SECRET_KEY,BUCKET}`.
    pub fn from_env() -> Self {
        Self {
            endpoint: env::var("AGENT_RS__MINIO__ENDPOINT")
                .unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string()),
            access_key: env::var("AGENT_RS__MINIO__ACCESS_KEY")
                .unwrap_or_else(|_| DEFAULT_ACCESS_KEY.to_string()),
            secret_key: env::var("AGENT_RS__MINIO__SECRET_KEY")
                .unwrap_or_else(|_| DEFAULT_SECRET_KEY.to_string()),
            bucket: env::var("AGENT_RS__MINIO__BUCKET")
                .unwrap_or_else(|_| DEFAULT_BUCKET.to_string()),
        }
    }
}

/// Thin wrapper over the AWS S3 SDK pinned to one bucket.
#[derive(Clone)]
pub struct MinioClient {
    client: Client,
    bucket: String,
}

impl MinioClient {
    /// Connect using static credentials + path-style addressing (MinIO
    /// doesn't support virtual-host style without DNS work).
    pub fn new(cfg: &MinioConfig) -> Self {
        let creds = Credentials::new(&cfg.access_key, &cfg.secret_key, None, None, "static");
        let s3_cfg = aws_sdk_s3::config::Builder::new()
            .endpoint_url(&cfg.endpoint)
            .region(Region::new("us-east-1"))
            .credentials_provider(creds)
            .force_path_style(true)
            .behavior_version_latest()
            .build();
        Self {
            client: Client::from_conf(s3_cfg),
            bucket: cfg.bucket.clone(),
        }
    }

    /// Upload `bytes` under `users/{user_id}/docs/{document_uuid}.{ext}`.
    ///
    /// Returns the full MinIO object key so the caller persists it verbatim
    /// into `ingested_documents` and passes it to `EnqueueIndex`.
    ///
    /// # Errors
    ///
    /// [`AppError::Internal`] wrapping the S3 SDK error on any failure.
    pub async fn put_bytes(
        &self,
        user_id: Uuid,
        document_id: Uuid,
        ext: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<String, AppError> {
        let key = object_key(user_id, document_id, ext);
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(ByteStream::from(bytes))
            .content_type(content_type)
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("MinIO put_object {key}: {e}")))?;
        Ok(key)
    }

    /// Remove an object by its full key. Idempotent on MinIO (404 is
    /// swallowed into `Ok(())`).
    ///
    /// # Errors
    ///
    /// [`AppError::Internal`] on any failure other than "not found".
    pub async fn delete_by_key(&self, key: &str) -> Result<(), AppError> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("MinIO delete_object {key}: {e}")))?;
        Ok(())
    }
}

/// Build the canonical object key. Must match
/// `agent_rs_stores::minio::object_key` so the IndexerWorker's ownership
/// guard in `MinioStore::get_bytes_by_key` accepts what we wrote.
///
/// The agent side keys the prefix on `UserId::as_str()` which is the
/// `Uuid::as_simple()` form (32 hex, no dashes) — we must match it
/// byte-for-byte or the IndexerWorker rejects the fetch with
/// `DocumentNotOwnedByUser`. The document segment keeps the hyphenated
/// form because it is opaque downstream.
///
/// Layout: `users/{user_id_simple}/docs/{document_uuid}.{ext}`.
pub fn object_key(user_id: Uuid, document_id: Uuid, ext: &str) -> String {
    format!(
        "users/{}/docs/{}.{}",
        user_id.as_simple(),
        document_id,
        ext
    )
}

/// Map a MIME type to a canonical filename extension. Unknown types fall
/// back to `"bin"`; the IndexerWorker rejects unknown extensions at
/// `extract_text`, which is the right place to error.
pub fn content_type_to_ext(ct: &str) -> &'static str {
    match ct {
        "application/pdf" => "pdf",
        "text/html" => "html",
        "text/plain" => "txt",
        "text/markdown" | "text/x-markdown" => "md",
        _ => "bin",
    }
}
