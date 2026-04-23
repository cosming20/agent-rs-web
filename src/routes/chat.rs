//! `/chat` and `/chat/:id` — conversation list + single conversation view.
//!
//! Single conversation view flows a user prompt through the gRPC `Ask`
//! RPC (server-streaming), collects every event, renders the terminal
//! `Final` event as the assistant reply and persists both sides of the
//! turn. Live streaming UX (progressive `PartialAnswer` / `ToolCall`
//! events) is a follow-up; today we block on the stream and render the
//! final answer once it arrives.

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// DTOs (shared between SSR + hydrate)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversationView {
    pub id: Uuid,
    pub title: String,
}

/// Library document as rendered next to the pinning checkboxes.
///
/// `pinned` carries the current checkbox state for this conversation;
/// `available = true` means the user may actually pick this doc (the
/// indexer has reported `complete`). Rows where `available = false`
/// render disabled checkboxes with a status hint so the user can see
/// what is still pending.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentPinningView {
    pub id: Uuid,
    pub filename: String,
    pub ingest_status: String,
    pub available: bool,
    pub pinned: bool,
}

/// Wrapper for the two things the pinning UI needs in one Resource —
/// which conversation row we're looking at (for the "auto mode" flag)
/// and the library row shape the checkboxes render.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PinningState {
    /// When `true`, `pinned_document_ids` is NULL on the conversation
    /// row and the UI is implicitly tracking every `complete` doc.
    pub auto_mode: bool,
    pub documents: Vec<DocumentPinningView>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageView {
    pub id: Uuid,
    pub role: String,
    pub content: String,
    pub citations: Vec<CitationView>,
    pub is_grounded: Option<bool>,
    pub confidence: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CitationView {
    pub index: u32,
    pub snippet: String,
    pub minio_object_key: String,
    pub section_path: String,
}

// ---------------------------------------------------------------------------
// /chat — conversation list + "new conversation" form
// ---------------------------------------------------------------------------

#[component]
pub fn ChatPage() -> impl IntoView {
    let conversations = Resource::new(|| (), |_| async move { list_conversations_action().await });
    let create = ServerAction::<CreateConversationAction>::new();

    let _ = Effect::new(move |_| {
        let _ = create.value().get();
        conversations.refetch();
    });

    view! {
        <div class="chat-home" style="padding: 2rem; font-family: system-ui, sans-serif; max-width: 720px; margin: 0 auto;">
            <header style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 1.5rem;">
                <h1 style="margin: 0;">"Conversations"</h1>
                <nav>
                    <a href="/library" style="margin-right: 1rem;">"Library"</a>
                    <form action="/api/logout_action" method="post" style="display: inline;">
                        <button type="submit">"Log out"</button>
                    </form>
                </nav>
            </header>

            <ActionForm action=create>
                <button type="submit" style="padding: 0.5rem 1rem;">"+ New conversation"</button>
            </ActionForm>

            <section style="margin-top: 2rem;">
                <Suspense fallback=|| view! { <p>"Loading…"</p> }>
                    {move || conversations.get().map(|result| match result {
                        Ok(convs) if convs.is_empty() => {
                            view! { <p>"No conversations yet. Click the button above."</p> }.into_any()
                        }
                        Ok(convs) => view! {
                            <ul style="list-style: none; padding: 0;">
                                {convs.into_iter().map(|c| view! {
                                    <li style="padding: 0.6rem; border-bottom: 1px solid #eee;">
                                        <a href=format!("/chat/{}", c.id)>{c.title}</a>
                                    </li>
                                }).collect::<Vec<_>>()}
                            </ul>
                        }
                        .into_any(),
                        Err(e) => view! { <p style="color: red;">"Error: " {e.to_string()}</p> }
                            .into_any(),
                    })}
                </Suspense>
            </section>
        </div>
    }
}

// ---------------------------------------------------------------------------
// /chat/:id — single conversation
// ---------------------------------------------------------------------------

#[component]
pub fn ConversationPage() -> impl IntoView {
    let params = use_params_map();
    let conversation_id = Memo::new(move |_| {
        params
            .get()
            .get("id")
            .and_then(|s| Uuid::parse_str(&s).ok())
    });

    let messages = Resource::new(
        move || conversation_id.get(),
        |maybe_id| async move {
            let Some(id) = maybe_id else {
                return Err(ServerFnError::new("no conversation id"));
            };
            load_conversation_messages(id).await
        },
    );

    let send = ServerAction::<SendMessageAction>::new();
    let save_pinning = ServerAction::<SavePinningAction>::new();
    let clear_pinning = ServerAction::<ClearPinningAction>::new();

    let pinning = Resource::new(
        move || {
            (
                conversation_id.get(),
                send.version().get(),
                save_pinning.version().get(),
                clear_pinning.version().get(),
            )
        },
        |(maybe_id, _, _, _)| async move {
            let Some(id) = maybe_id else {
                return Err(ServerFnError::new("no conversation id"));
            };
            load_pinning_state(id).await
        },
    );

    let _ = Effect::new(move |_| {
        let _ = send.value().get();
        messages.refetch();
    });

    view! {
        <div class="chat-thread" style="padding: 2rem; font-family: system-ui, sans-serif; max-width: 720px; margin: 0 auto;">
            <header style="margin-bottom: 1rem;">
                <a href="/chat">"← Back to conversations"</a>
            </header>

            <section>
                <Suspense fallback=|| view! { <p>"Loading…"</p> }>
                    {move || messages.get().map(|result| match result {
                        Ok(msgs) => view! {
                            <ul style="list-style: none; padding: 0;">
                                {msgs.into_iter().map(render_message).collect::<Vec<_>>()}
                            </ul>
                        }
                        .into_any(),
                        Err(e) => view! { <p style="color: red;">"Error: " {e.to_string()}</p> }
                            .into_any(),
                    })}
                </Suspense>
            </section>

            <section style="margin-top: 2rem; padding: 1rem; border: 1px solid #ccc; border-radius: 4px; background: #fafafa;">
                <h3 style="margin-top: 0; font-size: 1rem;">"Pinned documents"</h3>
                <Suspense fallback=|| view! { <p style="color: #666;">"Loading library…"</p> }>
                    {move || conversation_id.get().and_then(|id| {
                        pinning.get().map(|result| match result {
                            Ok(state) => render_pinning_panel(id, state, save_pinning, clear_pinning).into_any(),
                            Err(e) => view! {
                                <p style="color: red;">"Pinning panel error: " {e.to_string()}</p>
                            }
                            .into_any(),
                        })
                    })}
                </Suspense>
            </section>

            <section style="margin-top: 1.5rem; padding-top: 1rem; border-top: 1px solid #ccc;">
                <ActionForm action=send>
                    {move || conversation_id.get().map(|id| view! {
                        <input type="hidden" name="conversation_id" value=id.to_string()/>
                    })}
                    <textarea
                        name="prompt"
                        placeholder="Ask a question about your pinned documents…"
                        required
                        style="width: 100%; min-height: 4rem; padding: 0.5rem; box-sizing: border-box;"
                    ></textarea>
                    <button type="submit" style="margin-top: 0.5rem;">"Send"</button>
                </ActionForm>
            </section>
        </div>
    }
}

/// Render the pinning checkboxes + the two management actions
/// ("Save pinning" on the form, and a standalone "Reset to all" button
/// that clears the explicit selection).
fn render_pinning_panel(
    conversation_id: Uuid,
    state: PinningState,
    save_action: ServerAction<SavePinningAction>,
    clear_action: ServerAction<ClearPinningAction>,
) -> impl IntoView {
    if state.documents.is_empty() {
        return view! {
            <p style="color: #666; font-size: 0.9rem; margin: 0;">
                "No library documents yet. Upload one from "
                <a href="/library">"/library"</a> "."
            </p>
        }
        .into_any();
    }

    let auto_label = if state.auto_mode {
        "auto (every complete doc)"
    } else {
        "custom subset"
    };

    view! {
        <p style="color: #666; font-size: 0.85rem; margin: 0 0 0.75rem 0;">
            "Mode: " <strong>{auto_label}</strong>
            ". Tick the docs you want the agent to see, then Save. "
            "Click Reset to go back to auto mode."
        </p>
        <ActionForm action=save_action>
            <input type="hidden" name="conversation_id" value=conversation_id.to_string()/>
            <ul style="list-style: none; padding: 0; margin: 0;">
                {state.documents.iter().cloned().map(|doc| {
                    let disabled = !doc.available;
                    let checked = doc.pinned;
                    let status_note = if doc.available {
                        String::new()
                    } else {
                        format!(" ({})", doc.ingest_status)
                    };
                    view! {
                        <li style="padding: 0.25rem 0;">
                            <label style=format!(
                                "display: flex; align-items: center; gap: 0.5rem; opacity: {};",
                                if disabled { "0.5" } else { "1" }
                            )>
                                <input
                                    type="checkbox"
                                    name="pinned_ids"
                                    value=doc.id.to_string()
                                    checked=checked
                                    disabled=disabled
                                />
                                <span>{doc.filename}</span>
                                <span style="color: #aa7700; font-size: 0.8rem;">{status_note}</span>
                            </label>
                        </li>
                    }
                }).collect::<Vec<_>>()}
            </ul>
            <button type="submit" style="margin-top: 0.5rem;">"Save pinning"</button>
        </ActionForm>
        <ActionForm action=clear_action>
            <input type="hidden" name="conversation_id" value=conversation_id.to_string()/>
            <button
                type="submit"
                style="margin-top: 0.25rem; background: transparent; border: none; color: #06c; text-decoration: underline; cursor: pointer;"
            >
                "Reset to all documents (auto mode)"
            </button>
        </ActionForm>
    }
    .into_any()
}

fn render_message(msg: MessageView) -> impl IntoView {
    let background = match msg.role.as_str() {
        "user" => "#eef",
        _ => "#efe",
    };
    let role_label = msg.role.clone();
    let role_badge = if msg.role == "assistant" {
        let mut badge = String::new();
        if let Some(g) = msg.is_grounded {
            badge.push_str(if g { "✓ grounded" } else { "⚠ ungrounded" });
        }
        if let Some(c) = msg.confidence {
            if !badge.is_empty() {
                badge.push_str(" · ");
            }
            badge.push_str(&format!("conf {c:.2}"));
        }
        badge
    } else {
        String::new()
    };

    view! {
        <li style=format!("padding: 0.8rem; margin-bottom: 0.5rem; background: {background}; border-radius: 4px;")>
            <div style="display: flex; justify-content: space-between; color: #666; font-size: 0.85rem;">
                <strong>{role_label}</strong>
                <span>{role_badge}</span>
            </div>
            <p style="margin: 0.4rem 0; white-space: pre-wrap;">{msg.content}</p>
            {if msg.citations.is_empty() {
                view! { <div></div> }.into_any()
            } else {
                view! {
                    <details style="margin-top: 0.5rem; color: #333; font-size: 0.85rem;">
                        <summary>{format!("{} citation(s)", msg.citations.len())}</summary>
                        <ul>
                            {msg.citations.into_iter().map(|c| view! {
                                <li>
                                    <strong>"[" {c.index} "]"</strong>
                                    " "
                                    <span style="color: #666;">{c.section_path}</span>
                                    " — "
                                    <span>{c.snippet}</span>
                                </li>
                            }).collect::<Vec<_>>()}
                        </ul>
                    </details>
                }
                .into_any()
            }}
        </li>
    }
}

// ---------------------------------------------------------------------------
// Server fns
// ---------------------------------------------------------------------------

#[leptos::server(ListConversationsAction, "/api/list_conversations_action")]
pub async fn list_conversations_action() -> Result<Vec<ConversationView>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let (user_id, mut conn) = ssr_auth_and_conn().await?;
        let rows = crate::conversations::list_conversations(&mut conn, user_id)
            .await
            .map_err(|e| ServerFnError::new(format!("list: {e}")))?;
        Ok(rows
            .into_iter()
            .map(|c| ConversationView {
                id: c.id,
                title: c.title,
            })
            .collect())
    }
    #[cfg(not(feature = "ssr"))]
    {
        Err(ServerFnError::new("ssr-only"))
    }
}

#[leptos::server(CreateConversationAction, "/api/create_conversation_action")]
pub async fn create_conversation_action() -> Result<(), ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let (user_id, mut conn) = ssr_auth_and_conn().await?;
        let conv = crate::conversations::create_conversation(&mut conn, user_id)
            .await
            .map_err(|e| ServerFnError::new(format!("create: {e}")))?;
        leptos_axum::redirect(&format!("/chat/{}", conv.id));
        Ok(())
    }
    #[cfg(not(feature = "ssr"))]
    {
        Err(ServerFnError::new("ssr-only"))
    }
}

#[leptos::server(LoadPinningState, "/api/load_pinning_state")]
pub async fn load_pinning_state(conversation_id: Uuid) -> Result<PinningState, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let (user_id, mut conn) = ssr_auth_and_conn().await?;
        let conv = crate::conversations::load_conversation(&mut conn, user_id, conversation_id)
            .await
            .map_err(|e| ServerFnError::new(format!("load: {e}")))?;

        let library = list_library_entries(&mut conn, user_id)
            .await
            .map_err(|e| ServerFnError::new(format!("library: {e}")))?;

        let (auto_mode, pinned_set) = match conv.pinned_document_ids {
            None => (true, Vec::new()),
            Some(ids) => (
                false,
                ids.into_iter().flatten().collect::<Vec<Uuid>>(),
            ),
        };

        let documents = library
            .into_iter()
            .map(|(id, filename, status)| {
                let available = status == "complete";
                let pinned = if auto_mode {
                    available
                } else {
                    pinned_set.contains(&id)
                };
                DocumentPinningView {
                    id,
                    filename,
                    ingest_status: status,
                    available,
                    pinned,
                }
            })
            .collect();

        Ok(PinningState {
            auto_mode,
            documents,
        })
    }
    #[cfg(not(feature = "ssr"))]
    {
        Err(ServerFnError::new("ssr-only"))
    }
}

#[leptos::server(SavePinningAction, "/api/save_pinning_action")]
pub async fn save_pinning_action(
    conversation_id: Uuid,
    #[server(default)] pinned_ids: Vec<String>,
) -> Result<(), ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let (user_id, mut conn) = ssr_auth_and_conn().await?;
        let ids: Vec<Uuid> = pinned_ids
            .into_iter()
            .filter_map(|s| Uuid::parse_str(&s).ok())
            .collect();
        crate::conversations::set_pinned_document_ids(
            &mut conn,
            user_id,
            conversation_id,
            Some(&ids),
        )
        .await
        .map_err(|e| ServerFnError::new(format!("save: {e}")))?;
        Ok(())
    }
    #[cfg(not(feature = "ssr"))]
    {
        Err(ServerFnError::new("ssr-only"))
    }
}

#[leptos::server(ClearPinningAction, "/api/clear_pinning_action")]
pub async fn clear_pinning_action(conversation_id: Uuid) -> Result<(), ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let (user_id, mut conn) = ssr_auth_and_conn().await?;
        crate::conversations::set_pinned_document_ids(
            &mut conn,
            user_id,
            conversation_id,
            None,
        )
        .await
        .map_err(|e| ServerFnError::new(format!("clear: {e}")))?;
        Ok(())
    }
    #[cfg(not(feature = "ssr"))]
    {
        Err(ServerFnError::new("ssr-only"))
    }
}

#[leptos::server(LoadConversationMessages, "/api/load_conversation_messages")]
pub async fn load_conversation_messages(
    conversation_id: Uuid,
) -> Result<Vec<MessageView>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let (user_id, mut conn) = ssr_auth_and_conn().await?;
        crate::conversations::load_conversation(&mut conn, user_id, conversation_id)
            .await
            .map_err(|e| ServerFnError::new(format!("load: {e}")))?;
        let rows = crate::conversations::list_messages(&mut conn, conversation_id)
            .await
            .map_err(|e| ServerFnError::new(format!("messages: {e}")))?;
        Ok(rows.into_iter().map(persisted_to_view).collect())
    }
    #[cfg(not(feature = "ssr"))]
    {
        Err(ServerFnError::new("ssr-only"))
    }
}

#[leptos::server(SendMessageAction, "/api/send_message_action")]
pub async fn send_message_action(
    conversation_id: Uuid,
    prompt: String,
) -> Result<(), ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use crate::pb::{ask_event::Payload, AskRequest, ChatTurn};

        let (user_id, mut conn) = ssr_auth_and_conn().await?;
        let user_grpc_id = user_id.as_simple().to_string();

        // Verify ownership + cache the conversation header for title promotion.
        let conv = crate::conversations::load_conversation(&mut conn, user_id, conversation_id)
            .await
            .map_err(|e| ServerFnError::new(format!("load conversation: {e}")))?;

        // Active-document set:
        //   - auto mode (pinned_document_ids IS NULL) → every `complete` doc
        //   - explicit mode (pinned_document_ids = list) → only those docs,
        //     restricted to ones that are actually `complete`
        let active_keys = match conv.pinned_document_ids.as_ref() {
            None => list_complete_minio_keys(&mut conn, user_id)
                .await
                .map_err(|e| ServerFnError::new(format!("active docs: {e}")))?,
            Some(ids) => {
                let pinned: Vec<Uuid> = ids.iter().filter_map(|x| *x).collect();
                if pinned.is_empty() {
                    Vec::new()
                } else {
                    list_minio_keys_for_ids(&mut conn, user_id, &pinned)
                        .await
                        .map_err(|e| ServerFnError::new(format!("pinned docs: {e}")))?
                }
            }
        };

        // Replay history inline (agent is stateless).
        let history_rows = crate::conversations::list_messages(&mut conn, conversation_id)
            .await
            .map_err(|e| ServerFnError::new(format!("history: {e}")))?;
        let history: Vec<ChatTurn> = history_rows
            .iter()
            .map(|m| ChatTurn {
                role: m.role.clone(),
                content: m.content.clone(),
            })
            .collect();

        // Persist the user turn BEFORE the gRPC call so a crash mid-
        // stream doesn't lose the prompt.
        let user_msg = crate::conversations::append_user_message(&mut conn, &conv, &prompt, &[])
            .await
            .map_err(|e| ServerFnError::new(format!("persist user: {e}")))?;

        let request = AskRequest {
            user_id: user_grpc_id,
            query: prompt,
            history,
            active_document_keys: active_keys,
            history_document_keys: Vec::new(),
            trace_id: user_msg.id.to_string(),
            strategy: String::new(),
            limit: 0,
        };

        let events = crate::grpc::ask_collect(request)
            .await
            .map_err(|e| ServerFnError::new(format!("ask: {e}")))?;

        let mut final_answer: Option<crate::pb::AskFinal> = None;
        let mut error_msg: Option<String> = None;
        for ev in events {
            match ev.payload {
                Some(Payload::Final(f)) => final_answer = Some(f),
                Some(Payload::Error(e)) => error_msg = Some(format!("{}: {}", e.code, e.message)),
                _ => {}
            }
        }

        match (final_answer, error_msg) {
            (Some(f), _) => {
                let citations_json: Vec<_> = f
                    .citations
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "index": c.index,
                            "chunk_id": c.chunk_id,
                            "snippet": c.content_snippet,
                            "minio_object_key": c.minio_object_key,
                            "section_path": c.section_path,
                            "score": c.score,
                        })
                    })
                    .collect();
                crate::conversations::append_assistant_message(
                    &mut conn,
                    conversation_id,
                    &f.answer,
                    serde_json::Value::Array(citations_json),
                    Some(f.confidence),
                    Some(f.is_grounded),
                )
                .await
                .map_err(|e| ServerFnError::new(format!("persist assistant: {e}")))?;
            }
            (None, Some(err)) => {
                let _ = crate::conversations::append_assistant_message(
                    &mut conn,
                    conversation_id,
                    &format!("agent error: {err}"),
                    serde_json::Value::Array(Vec::new()),
                    Some(0.0),
                    Some(false),
                )
                .await;
            }
            (None, None) => {
                let _ = crate::conversations::append_assistant_message(
                    &mut conn,
                    conversation_id,
                    "agent produced no terminal event",
                    serde_json::Value::Array(Vec::new()),
                    Some(0.0),
                    Some(false),
                )
                .await;
            }
        }

        Ok(())
    }
    #[cfg(not(feature = "ssr"))]
    {
        Err(ServerFnError::new("ssr-only"))
    }
}

// ---------------------------------------------------------------------------
// SSR helpers
// ---------------------------------------------------------------------------

#[cfg(feature = "ssr")]
async fn ssr_auth_and_conn() -> Result<
    (
        Uuid,
        diesel_async::pooled_connection::bb8::PooledConnection<
            'static,
            diesel_async::AsyncPgConnection,
        >,
    ),
    ServerFnError,
> {
    use leptos_axum::extract;
    use tower_sessions::Session;

    let session: Session = extract()
        .await
        .map_err(|e| ServerFnError::new(format!("session extract: {e}")))?;
    let user_id = crate::auth::session_user_id(&session)
        .await
        .ok_or_else(|| ServerFnError::new("unauthenticated"))?;

    let pool = use_context::<crate::db::DbPool>()
        .ok_or_else(|| ServerFnError::new("DbPool context missing"))?;
    let conn = pool
        .get_owned()
        .await
        .map_err(|e| ServerFnError::new(format!("db conn: {e}")))?;
    Ok((user_id, conn))
}

#[cfg(feature = "ssr")]
fn persisted_to_view(m: crate::conversations::Message) -> MessageView {
    let citations: Vec<CitationView> = m
        .citations
        .as_array()
        .map(|arr| arr.iter().filter_map(citation_from_json).collect())
        .unwrap_or_default();
    MessageView {
        id: m.id,
        role: m.role,
        content: m.content,
        citations,
        is_grounded: m.is_grounded,
        confidence: m.confidence,
    }
}

#[cfg(feature = "ssr")]
fn citation_from_json(v: &serde_json::Value) -> Option<CitationView> {
    Some(CitationView {
        index: v.get("index")?.as_u64()? as u32,
        snippet: v.get("snippet")?.as_str().unwrap_or("").to_string(),
        minio_object_key: v
            .get("minio_object_key")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        section_path: v
            .get("section_path")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

#[cfg(feature = "ssr")]
async fn list_complete_minio_keys(
    conn: &mut diesel_async::AsyncPgConnection,
    owner: Uuid,
) -> Result<Vec<String>, crate::error::AppError> {
    use crate::schema::ingested_documents::dsl;
    use diesel::prelude::*;
    use diesel_async::RunQueryDsl;

    dsl::ingested_documents
        .filter(dsl::user_id.eq(owner))
        .filter(dsl::ingest_status.eq("complete"))
        .select(dsl::minio_object_key)
        .load(conn)
        .await
        .map_err(crate::error::AppError::from)
}

/// Fetch the MinIO keys for a specific set of documents owned by `owner`.
/// Drops any id that isn't `complete` (the agent refuses to retrieve
/// half-indexed docs anyway, and silently skipping is better UX than
/// failing the whole Ask).
#[cfg(feature = "ssr")]
async fn list_minio_keys_for_ids(
    conn: &mut diesel_async::AsyncPgConnection,
    owner: Uuid,
    ids: &[Uuid],
) -> Result<Vec<String>, crate::error::AppError> {
    use crate::schema::ingested_documents::dsl;
    use diesel::prelude::*;
    use diesel_async::RunQueryDsl;

    dsl::ingested_documents
        .filter(dsl::user_id.eq(owner))
        .filter(dsl::id.eq_any(ids))
        .filter(dsl::ingest_status.eq("complete"))
        .select(dsl::minio_object_key)
        .load(conn)
        .await
        .map_err(crate::error::AppError::from)
}

/// Return the library rows a pinning panel needs — id, filename, and
/// the current ingest status so the UI can disable rows that aren't
/// `complete` yet.
#[cfg(feature = "ssr")]
async fn list_library_entries(
    conn: &mut diesel_async::AsyncPgConnection,
    owner: Uuid,
) -> Result<Vec<(Uuid, String, String)>, crate::error::AppError> {
    use crate::schema::ingested_documents::dsl;
    use diesel::prelude::*;
    use diesel_async::RunQueryDsl;

    dsl::ingested_documents
        .filter(dsl::user_id.eq(owner))
        .order(dsl::created_at.desc())
        .select((dsl::id, dsl::source_filename, dsl::ingest_status))
        .load(conn)
        .await
        .map_err(crate::error::AppError::from)
}
