//! Library page — per-user document uploads backed by MinIO + Postgres +
//! RabbitMQ via `EnqueueIndex`.
//!
//! Page routes:
//!   GET  `/library`           — list the user's documents (SSR).
//!
//! Plain axum routes wired in `main.rs` (leptos server fns don't handle
//! multipart cleanly):
//!   POST `/library/upload`    — receive the file, stream bytes into
//!                               MinIO, insert `ingested_documents`,
//!                               call `EnqueueIndex`.
//!   POST `/library/delete`    — cascade `DeleteDocument` gRPC + MinIO
//!                               delete + DB delete.

use leptos::prelude::*;

#[cfg(feature = "ssr")]
use {
    axum::extract::Multipart,
    axum::http::StatusCode,
    axum::response::Redirect,
    axum::{Extension, Form},
    chrono::{DateTime, Utc},
    diesel::prelude::*,
    diesel_async::{AsyncPgConnection, RunQueryDsl},
    serde::{Deserialize, Serialize},
    tower_sessions::Session,
    tracing::{error, info, warn},
    uuid::Uuid,
};

#[cfg(not(feature = "ssr"))]
use {
    serde::{Deserialize, Serialize},
    uuid::Uuid,
};

// ---------------------------------------------------------------------------
// Persisted row (SSR-only)
// ---------------------------------------------------------------------------

#[cfg(feature = "ssr")]
#[derive(Debug, Clone, Queryable, Selectable, Serialize, Deserialize)]
#[diesel(table_name = crate::schema::ingested_documents)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct IngestedDocument {
    pub id: Uuid,
    pub user_id: Uuid,
    pub minio_object_key: String,
    pub source_filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub sha256: Option<String>,
    pub n_pages: Option<i32>,
    pub n_chunks: Option<i32>,
    pub ingest_status: String,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(feature = "ssr")]
#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::ingested_documents)]
struct NewIngestedDocument<'a> {
    user_id: Uuid,
    minio_object_key: &'a str,
    source_filename: &'a str,
    content_type: &'a str,
    size_bytes: i64,
    ingest_status: &'a str,
}

/// Lightweight DTO for rendering — avoids leaking diesel types into the
/// hydrate bundle while keeping the `IngestedDocument` row SSR-private.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IngestedDocumentView {
    pub id: Uuid,
    pub source_filename: String,
    pub ingest_status: String,
    pub n_pages: Option<i32>,
    pub n_chunks: Option<i32>,
}

// ---------------------------------------------------------------------------
// Library page (SSR)
// ---------------------------------------------------------------------------

#[component]
pub fn LibraryPage() -> impl IntoView {
    let documents = Resource::new(|| (), |_| async move { list_documents_action().await });

    view! {
        <div class="library-page" style="padding: 2rem; font-family: system-ui, sans-serif; max-width: 880px; margin: 0 auto;">
            <header style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 1.5rem;">
                <h1 style="margin: 0;">"Your library"</h1>
                <nav>
                    <a href="/chat" style="margin-right: 1rem;">"Chat"</a>
                    <form action="/api/logout_action" method="post" style="display: inline;">
                        <button type="submit">"Log out"</button>
                    </form>
                </nav>
            </header>

            <section style="margin-bottom: 2rem; padding: 1rem; border: 1px dashed #ccc;">
                <h2 style="margin-top: 0;">"Upload a PDF or text document"</h2>
                <form action="/library/upload" method="post" enctype="multipart/form-data">
                    <input type="file" name="file" accept=".pdf,.txt,.md,.markdown,.html" required/>
                    <button type="submit" style="margin-left: 0.5rem;">"Upload"</button>
                </form>
                <p style="color: #666; margin-top: 0.5rem; font-size: 0.9rem;">
                    "Uploads are indexed asynchronously. Reload this page to watch the status go from "
                    <code>"pending"</code> " → " <code>"indexing"</code> " → " <code>"complete"</code> "."
                </p>
            </section>

            <section>
                <h2>"Documents"</h2>
                <Suspense fallback=|| view! { <p>"Loading…"</p> }>
                    {move || documents.get().map(|result| match result {
                        Ok(docs) if docs.is_empty() => {
                            view! { <p>"No documents yet. Upload one above."</p> }.into_any()
                        }
                        Ok(docs) => document_table(docs).into_any(),
                        Err(e) => view! {
                            <p style="color: red;">"Error loading documents: " {e.to_string()}</p>
                        }
                        .into_any(),
                    })}
                </Suspense>
            </section>
        </div>
    }
}

fn document_table(docs: Vec<IngestedDocumentView>) -> impl IntoView {
    view! {
        <table style="width: 100%; border-collapse: collapse;">
            <thead>
                <tr style="border-bottom: 1px solid #ccc; text-align: left;">
                    <th style="padding: 0.5rem;">"Name"</th>
                    <th style="padding: 0.5rem;">"Status"</th>
                    <th style="padding: 0.5rem;">"Pages"</th>
                    <th style="padding: 0.5rem;">"Chunks"</th>
                    <th style="padding: 0.5rem;">""</th>
                </tr>
            </thead>
            <tbody>
                {docs.into_iter().map(|doc| {
                    let color = match doc.ingest_status.as_str() {
                        "complete" => "#0a0",
                        "failed" => "#c00",
                        _ => "#aa7700",
                    };
                    view! {
                        <tr style="border-bottom: 1px solid #eee;">
                            <td style="padding: 0.5rem;">{doc.source_filename}</td>
                            <td style="padding: 0.5rem;">
                                <span style=format!("color: {};", color)>
                                    {doc.ingest_status}
                                </span>
                            </td>
                            <td style="padding: 0.5rem;">
                                {doc.n_pages.map(|n| n.to_string()).unwrap_or_default()}
                            </td>
                            <td style="padding: 0.5rem;">
                                {doc.n_chunks.map(|n| n.to_string()).unwrap_or_default()}
                            </td>
                            <td style="padding: 0.5rem;">
                                <form action="/library/delete" method="post" style="display: inline;">
                                    <input type="hidden" name="document_id" value=doc.id.to_string()/>
                                    <button type="submit">"Delete"</button>
                                </form>
                            </td>
                        </tr>
                    }
                }).collect::<Vec<_>>()}
            </tbody>
        </table>
    }
}

// ---------------------------------------------------------------------------
// Server fn — list documents
// ---------------------------------------------------------------------------

#[leptos::server(ListDocumentsAction, "/api/list_documents_action")]
pub async fn list_documents_action() -> Result<Vec<IngestedDocumentView>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use leptos_axum::extract;
        let session: Session = extract()
            .await
            .map_err(|e| ServerFnError::new(format!("session extract: {e}")))?;

        let user_id = crate::auth::session_user_id(&session)
            .await
            .ok_or_else(|| ServerFnError::new("unauthenticated"))?;

        let pool = use_context::<crate::db::DbPool>()
            .ok_or_else(|| ServerFnError::new("DbPool context missing"))?;
        let mut conn = pool
            .get()
            .await
            .map_err(|e| ServerFnError::new(format!("db conn: {e}")))?;

        // Sync any in-flight docs with the agent before we render.
        let docs = list_documents(&mut conn, user_id)
            .await
            .map_err(|e| ServerFnError::new(format!("list docs: {e}")))?;
        let user_grpc_id = user_id.as_simple().to_string();
        for doc in docs
            .iter()
            .filter(|d| d.ingest_status != "complete" && d.ingest_status != "failed")
        {
            if let Ok(status) =
                crate::grpc::get_document_status(&user_grpc_id, &doc.minio_object_key).await
            {
                let _ = update_document_status(&mut conn, doc.id, &status).await;
            }
        }
        let docs = list_documents(&mut conn, user_id)
            .await
            .map_err(|e| ServerFnError::new(format!("list docs: {e}")))?;

        Ok(docs
            .into_iter()
            .map(|d| IngestedDocumentView {
                id: d.id,
                source_filename: d.source_filename,
                ingest_status: d.ingest_status,
                n_pages: d.n_pages,
                n_chunks: d.n_chunks,
            })
            .collect())
    }
    #[cfg(not(feature = "ssr"))]
    {
        Err(ServerFnError::new("ssr-only"))
    }
}

// ---------------------------------------------------------------------------
// Plain axum multipart upload handler
// ---------------------------------------------------------------------------

#[cfg(feature = "ssr")]
const MAX_UPLOAD_MB: u64 = 100;

#[cfg(feature = "ssr")]
pub async fn upload_handler(
    session: Session,
    Extension(state): Extension<UploadState>,
    mut multipart: Multipart,
) -> Result<Redirect, (StatusCode, String)> {
    let user_id = crate::auth::session_user_id(&session)
        .await
        .ok_or((StatusCode::UNAUTHORIZED, "login required".into()))?;

    let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("multipart parse: {e}")))?
    else {
        return Err((StatusCode::BAD_REQUEST, "no file field in form".into()));
    };

    let filename = field
        .file_name()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "upload.bin".to_string());
    let content_type = field
        .content_type()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "application/octet-stream".to_string());

    let bytes = field
        .bytes()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("read body: {e}")))?;
    if bytes.len() as u64 > MAX_UPLOAD_MB * 1024 * 1024 {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("upload exceeds {MAX_UPLOAD_MB} MiB limit"),
        ));
    }

    let document_id = Uuid::new_v4();
    let ext = crate::minio_client::content_type_to_ext(&content_type);

    let key = state
        .minio
        .put_bytes(user_id, document_id, ext, &content_type, bytes.to_vec())
        .await
        .map_err(|e| {
            error!(error = %e, "MinIO upload failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "upload failed".into())
        })?;

    let size = bytes.len() as i64;

    let mut conn = state
        .pool
        .get()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db conn: {e}")))?;

    let inserted =
        insert_ingested_document(&mut conn, user_id, &key, &filename, &content_type, size)
            .await
            .map_err(|e| {
                error!(error = %e, "insert ingested_documents failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "db insert failed".into())
            })?;

    let user_grpc_id = user_id.as_simple().to_string();
    match crate::grpc::enqueue_index(&user_grpc_id, &key).await {
        Ok(resp) => info!(state = %resp.state, job_id = %resp.job_id, "enqueued"),
        Err(e) => warn!(error = %e, "enqueue_index failed — document left in pending"),
    }

    info!(
        user_id = %user_id,
        document_id = %inserted.id,
        key = %key,
        "upload complete"
    );
    Ok(Redirect::to("/library"))
}

/// Axum state bundle for the plain handlers.
#[cfg(feature = "ssr")]
#[derive(Clone)]
pub struct UploadState {
    pub pool: crate::db::DbPool,
    pub minio: crate::minio_client::MinioClient,
}

// ---------------------------------------------------------------------------
// Delete endpoint
// ---------------------------------------------------------------------------

#[cfg(feature = "ssr")]
#[derive(Debug, Deserialize)]
pub struct DeleteForm {
    pub document_id: Uuid,
}

#[cfg(feature = "ssr")]
pub async fn delete_handler(
    session: Session,
    Extension(state): Extension<UploadState>,
    Form(form): Form<DeleteForm>,
) -> Result<Redirect, (StatusCode, String)> {
    let user_id = crate::auth::session_user_id(&session)
        .await
        .ok_or((StatusCode::UNAUTHORIZED, "login required".into()))?;

    let mut conn = state
        .pool
        .get()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db conn: {e}")))?;

    let Some(doc) = load_document_for_user(&mut conn, user_id, form.document_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("load doc: {e}")))?
    else {
        return Ok(Redirect::to("/library"));
    };

    let user_grpc_id = user_id.as_simple().to_string();
    if let Err(e) = crate::grpc::delete_document(&user_grpc_id, &doc.minio_object_key).await {
        warn!(error = %e, "agent delete_document failed; continuing with MinIO+DB cleanup");
    }
    if let Err(e) = state.minio.delete_by_key(&doc.minio_object_key).await {
        warn!(error = %e, "MinIO delete failed; continuing with DB cleanup");
    }
    if let Err(e) = delete_document_row(&mut conn, user_id, form.document_id).await {
        error!(error = %e, "delete ingested_documents row failed");
        return Err((StatusCode::INTERNAL_SERVER_ERROR, "db delete failed".into()));
    }

    info!(user_id = %user_id, document_id = %doc.id, "document deleted");
    Ok(Redirect::to("/library"))
}

// ---------------------------------------------------------------------------
// DB helpers (private)
// ---------------------------------------------------------------------------

#[cfg(feature = "ssr")]
async fn list_documents(
    conn: &mut AsyncPgConnection,
    owner: Uuid,
) -> Result<Vec<IngestedDocument>, crate::error::AppError> {
    use crate::schema::ingested_documents::dsl;
    dsl::ingested_documents
        .filter(dsl::user_id.eq(owner))
        .order(dsl::created_at.desc())
        .select(IngestedDocument::as_select())
        .load(conn)
        .await
        .map_err(crate::error::AppError::from)
}

#[cfg(feature = "ssr")]
async fn load_document_for_user(
    conn: &mut AsyncPgConnection,
    owner: Uuid,
    id: Uuid,
) -> Result<Option<IngestedDocument>, crate::error::AppError> {
    use crate::schema::ingested_documents::dsl;
    dsl::ingested_documents
        .filter(dsl::id.eq(id))
        .filter(dsl::user_id.eq(owner))
        .select(IngestedDocument::as_select())
        .first(conn)
        .await
        .optional()
        .map_err(crate::error::AppError::from)
}

#[cfg(feature = "ssr")]
async fn insert_ingested_document(
    conn: &mut AsyncPgConnection,
    owner: Uuid,
    key: &str,
    source_filename: &str,
    content_type: &str,
    size_bytes: i64,
) -> Result<IngestedDocument, crate::error::AppError> {
    use crate::schema::ingested_documents::dsl;
    let row = NewIngestedDocument {
        user_id: owner,
        minio_object_key: key,
        source_filename,
        content_type,
        size_bytes,
        ingest_status: "pending",
    };
    diesel::insert_into(dsl::ingested_documents)
        .values(&row)
        .returning(IngestedDocument::as_returning())
        .get_result(conn)
        .await
        .map_err(crate::error::AppError::from)
}

#[cfg(feature = "ssr")]
async fn delete_document_row(
    conn: &mut AsyncPgConnection,
    owner: Uuid,
    id: Uuid,
) -> Result<usize, crate::error::AppError> {
    use crate::schema::ingested_documents::dsl;
    diesel::delete(
        dsl::ingested_documents
            .filter(dsl::id.eq(id))
            .filter(dsl::user_id.eq(owner)),
    )
    .execute(conn)
    .await
    .map_err(crate::error::AppError::from)
}

#[cfg(feature = "ssr")]
async fn update_document_status(
    conn: &mut AsyncPgConnection,
    id: Uuid,
    status: &crate::pb::GetDocumentStatusResponse,
) -> Result<(), crate::error::AppError> {
    use crate::schema::ingested_documents::dsl;
    let new_status = status.state.as_str();
    let sha = (!status.sha256.is_empty()).then(|| status.sha256.clone());
    let pages = (status.n_pages > 0).then_some(status.n_pages as i32);
    let chunks = (status.n_chunks > 0).then_some(status.n_chunks as i32);
    let err = (!status.error_message.is_empty()).then(|| status.error_message.clone());

    diesel::update(dsl::ingested_documents.filter(dsl::id.eq(id)))
        .set((
            dsl::ingest_status.eq(new_status),
            dsl::sha256.eq(sha),
            dsl::n_pages.eq(pages),
            dsl::n_chunks.eq(chunks),
            dsl::error_message.eq(err),
            dsl::updated_at.eq(diesel::dsl::now),
        ))
        .execute(conn)
        .await
        .map_err(crate::error::AppError::from)?;
    Ok(())
}
