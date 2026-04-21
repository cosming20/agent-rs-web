//! `/library` — document list (GET) and upload (POST).

use leptos::prelude::*;
use leptos::web_sys;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Response types (cross-wasm-boundary)
// ---------------------------------------------------------------------------

/// A trimmed-down view of `pb::Document` that can cross the wasm boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocRow {
    pub document_id: String,
    pub source: String,
    pub ingest_status: String,
    pub n_pages: u32,
    pub n_chunks: u32,
}

// ---------------------------------------------------------------------------
// Server functions
// ---------------------------------------------------------------------------

/// List the current user's documents via the gRPC `ListDocuments` RPC.
///
/// # Errors
///
/// Returns `ServerFnError` when unauthenticated or the gRPC call fails.
#[server(ListDocumentsAction, "/api")]
pub async fn list_documents_action() -> Result<Vec<DocRow>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use leptos_axum::extract;
        use tower_sessions::Session;

        use crate::auth::session_current_user;
        use crate::db::DbPool;
        use crate::error::AppError;

        let pool = use_context::<DbPool>()
            .ok_or_else(|| AppError::Internal("db pool missing from context".to_string()))
            .map_err(|e| e.into_server_fn_error())?;
        let session: Session = extract().await.map_err(|e| {
            AppError::Internal(format!("session extract failed: {e}"))
                .into_server_fn_error()
        })?;

        let mut conn = pool
            .get()
            .await
            .map_err(|e| AppError::from(e).into_server_fn_error())?;
        let user = session_current_user(&session, &mut conn)
            .await
            .map_err(|e| e.into_server_fn_error())?;

        let mut client = crate::grpc::connect()
            .await
            .map_err(|e| AppError::Grpc(e.to_string()).into_server_fn_error())?;

        let req = tonic::Request::new(crate::pb::UserRef {
            user_id: user.grpc_id(),
        });

        let doc_list = client
            .list_documents(req)
            .await
            .map_err(|e| AppError::from(e).into_server_fn_error())?
            .into_inner();

        let rows = doc_list
            .documents
            .into_iter()
            .map(|d| DocRow {
                document_id: d.document_id,
                source: d.source,
                ingest_status: d.ingest_status,
                n_pages: d.n_pages,
                n_chunks: d.n_chunks,
            })
            .collect();

        Ok(rows)
    }
    #[cfg(not(feature = "ssr"))]
    {
        Err(ServerFnError::ServerError(
            "server function called on client".to_string(),
        ))
    }
}

/// Upload a document via the gRPC `Ingest` client-streaming RPC.
///
/// Accepts the file bytes and its original filename in-memory (MVP).
///
/// # Errors
///
/// Returns `ServerFnError` when unauthenticated or the gRPC call fails.
#[server(UploadAction, "/api")]
pub async fn upload_action(
    filename: String,
    content_type: String,
    data: Vec<u8>,
) -> Result<String, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use leptos_axum::extract;
        use tower_sessions::Session;

        use crate::auth::session_current_user;
        use crate::db::DbPool;
        use crate::error::AppError;
        use crate::pb::{IngestChunk, IngestHeader};

        let pool = use_context::<DbPool>()
            .ok_or_else(|| AppError::Internal("db pool missing from context".to_string()))
            .map_err(|e| e.into_server_fn_error())?;
        let session: Session = extract().await.map_err(|e| {
            AppError::Internal(format!("session extract failed: {e}"))
                .into_server_fn_error()
        })?;

        let mut conn = pool
            .get()
            .await
            .map_err(|e| AppError::from(e).into_server_fn_error())?;
        let user = session_current_user(&session, &mut conn)
            .await
            .map_err(|e| e.into_server_fn_error())?;

        let mut client = crate::grpc::connect()
            .await
            .map_err(|e| AppError::Grpc(e.to_string()).into_server_fn_error())?;

        // Build the stream: header first, then 64 KiB data chunks.
        let header_chunk = IngestChunk {
            payload: Some(crate::pb::ingest_chunk::Payload::Header(IngestHeader {
                user_id: user.grpc_id(),
                source: filename.clone(),
                content_type,
            })),
        };

        const CHUNK_SIZE: usize = 65_536;
        let data_chunks: Vec<IngestChunk> = data
            .chunks(CHUNK_SIZE)
            .map(|slice| IngestChunk {
                payload: Some(crate::pb::ingest_chunk::Payload::Data(slice.to_vec())),
            })
            .collect();

        let stream = futures::stream::iter(
            std::iter::once(header_chunk).chain(data_chunks.into_iter()),
        );

        let reply = client
            .ingest(tonic::Request::new(stream))
            .await
            .map_err(|e| AppError::from(e).into_server_fn_error())?
            .into_inner();

        tracing::info!(document_id = %reply.document_id, source = %filename, "document ingested");
        Ok(reply.document_id)
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (filename, content_type, data);
        Err(ServerFnError::ServerError(
            "server function called on client".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

#[component]
pub fn LibraryPage() -> impl IntoView {
    // Document list resource — refetched after each successful upload.
    let docs_res = Resource::new(|| (), |_| list_documents_action());

    // Upload state.
    let upload_err: RwSignal<Option<String>> = RwSignal::new(None);
    let upload_ok: RwSignal<Option<String>> = RwSignal::new(None);
    let uploading = RwSignal::new(false);

    // File upload handler.
    // Browser-specific code (FileList, ArrayBuffer, JsFuture) is gated to the
    // hydrate feature so it never touches the SSR compile path.
    #[cfg(not(feature = "ssr"))]
    let on_upload = move |ev: web_sys::SubmitEvent| {
        use leptos::wasm_bindgen::JsCast;
        ev.prevent_default();
        upload_err.set(None);
        upload_ok.set(None);

        let Some(form) = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlFormElement>().ok())
        else {
            return;
        };

        let file_input: Option<web_sys::HtmlInputElement> = form
            .query_selector("input[type='file']")
            .ok()
            .flatten()
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok());
        let Some(file_input) = file_input else { return };

        let file: Option<web_sys::File> = file_input.files().and_then(|fl| fl.get(0));
        let Some(file) = file else {
            upload_err.set(Some("No file selected.".to_string()));
            return;
        };

        let filename: String = file.name();
        let content_type: String = {
            let t = file.type_();
            if t.is_empty() {
                "application/octet-stream".to_string()
            } else {
                t
            }
        };

        uploading.set(true);

        leptos::task::spawn_local(async move {
            use wasm_bindgen_futures::JsFuture;

            let array_buffer = match JsFuture::from(file.array_buffer()).await {
                Ok(ab) => ab,
                Err(e) => {
                    upload_err.set(Some(format!("Failed to read file: {e:?}")));
                    uploading.set(false);
                    return;
                }
            };
            let uint8 = js_sys::Uint8Array::new(&array_buffer);
            let data = uint8.to_vec();

            match upload_action(filename.clone(), content_type, data).await {
                Ok(doc_id) => {
                    upload_ok.set(Some(format!(
                        "Uploaded '{filename}' → document {doc_id}"
                    )));
                    docs_res.refetch();
                }
                Err(e) => {
                    let msg = e.to_string();
                    upload_err.set(Some(
                        msg.trim_start_matches("server error: ").to_string(),
                    ));
                }
            }
            uploading.set(false);
        });
    };
    // Under SSR the event handler is never invoked; provide a no-op to keep
    // the view! macro type-consistent across both feature sets.
    #[cfg(feature = "ssr")]
    let on_upload = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
    };

    view! {
        <div style="max-width:960px;margin:0 auto;padding:20px;font-family:sans-serif">
            <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:20px">
                <h1 style="margin:0">"Library"</h1>
                <a href="/chat" style="text-decoration:none">"← Chat"</a>
            </div>

            // Upload form
            <section style="border:1px solid #ddd;border-radius:6px;padding:16px;margin-bottom:24px">
                <h2 style="margin:0 0 12px 0;font-size:1.1rem">"Upload document"</h2>
                <form on:submit=on_upload style="display:flex;gap:8px;align-items:center">
                    <input type="file" accept=".pdf,application/pdf" style="flex:1" />
                    <button
                        type="submit"
                        disabled=move || uploading.get()
                        style="padding:6px 14px"
                    >
                        {move || if uploading.get() { "Uploading…" } else { "Upload" }}
                    </button>
                </form>
                <Show when=move || upload_err.get().is_some()>
                    <p style="color:red;margin:8px 0 0 0">{move || upload_err.get().unwrap_or_default()}</p>
                </Show>
                <Show when=move || upload_ok.get().is_some()>
                    <p style="color:green;margin:8px 0 0 0">{move || upload_ok.get().unwrap_or_default()}</p>
                </Show>
            </section>

            // Document list
            <section>
                <h2 style="margin:0 0 12px 0;font-size:1.1rem">"Your documents"</h2>
                <Suspense fallback=|| view! { <p style="color:#888">"Loading…"</p> }>
                    {move || Suspend::new(async move {
                        match docs_res.await {
                            Ok(rows) if rows.is_empty() => view! {
                                <p style="color:#888">"No documents yet."</p>
                            }.into_any(),
                            Ok(rows) => view! {
                                <table style="width:100%;border-collapse:collapse;font-size:0.9rem">
                                    <thead>
                                        <tr style="background:#f6f8fa">
                                            <th style="padding:8px;text-align:left;border-bottom:1px solid #ddd">"Source"</th>
                                            <th style="padding:8px;text-align:left;border-bottom:1px solid #ddd">"Status"</th>
                                            <th style="padding:8px;text-align:right;border-bottom:1px solid #ddd">"Pages"</th>
                                            <th style="padding:8px;text-align:right;border-bottom:1px solid #ddd">"Chunks"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {rows.into_iter().map(|doc| view! {
                                            <tr>
                                                <td style="padding:8px;border-bottom:1px solid #eee">{doc.source}</td>
                                                <td style="padding:8px;border-bottom:1px solid #eee">{doc.ingest_status}</td>
                                                <td style="padding:8px;text-align:right;border-bottom:1px solid #eee">{doc.n_pages}</td>
                                                <td style="padding:8px;text-align:right;border-bottom:1px solid #eee">{doc.n_chunks}</td>
                                            </tr>
                                        }).collect::<Vec<_>>()}
                                    </tbody>
                                </table>
                            }.into_any(),
                            Err(e) => view! {
                                <p style="color:red">"Error: " {e.to_string()}</p>
                            }.into_any(),
                        }
                    })}
                </Suspense>
            </section>
        </div>
    }
}
