//! `/chat` and `/chat/:id` — conversation list + single conversation view.
//!
//! Single-conversation flow (live-streaming since the Commit-B proto
//! refresh):
//!
//! 1. The textarea is wrapped in a plain `<form>` whose action points at
//!    the axum SSE handler [`ask_stream_handler`] (`POST /api/ask_stream`).
//! 2. On hydrate, [`install_ask_form_hook`] attaches a `submit`
//!    listener that hijacks the form, POSTs the same fields with
//!    `fetch`, and parses the `text/event-stream` body. Each
//!    `data: { … }` envelope updates Leptos signals so the streaming
//!    answer + budget snapshot render live without a full reload.
//! 3. The SSE handler drives the gRPC `Ask` stream, persists the user
//!    + assistant turns to Postgres, and emits these event kinds:
//!      - `delta`     — partial answer text chunk
//!      - `final`     — citations + confidence + coverage
//!      - `budget`    — token / cost snapshot (rendered as a footer)
//!      - `error`     — terminal failure
//! 4. With JS disabled the form still POSTs but the response is the
//!    raw event stream; that's an acceptable degradation since the
//!    rest of the app already requires hydration.

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "ssr")]
use std::convert::Infallible;

#[cfg(feature = "ssr")]
use axum::{
    extract::Form,
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    Extension,
};
#[cfg(feature = "ssr")]
use futures::stream::{Stream, StreamExt};
#[cfg(feature = "ssr")]
use tokio::sync::mpsc;
#[cfg(feature = "ssr")]
use tokio_stream::wrappers::ReceiverStream;
#[cfg(feature = "ssr")]
use tower_sessions::Session;
#[cfg(feature = "ssr")]
use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Form `action` URL for the streaming Ask endpoint. Centralised so the
/// hydrate-side hook and the axum router agree on the path.
const ASK_STREAM_PATH: &str = "/api/ask_stream";

/// Channel buffer for the SSE relay between the gRPC stream task and
/// the axum response stream. Sized for headroom on a bursty token-
/// delta channel without unbounded memory if the client is slow.
#[cfg(feature = "ssr")]
const ASK_STREAM_BUFFER: usize = 64;

/// Confidence threshold above which the assistant turn is rendered with
/// a "verified" indicator. Replaces the old `is_grounded` boolean badge
/// (proto field reserved in Commit B).
const CONFIDENCE_VERIFIED_THRESHOLD: f64 = 0.5;

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
    pub confidence: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CitationView {
    pub index: u32,
    pub snippet: String,
    pub minio_object_key: String,
    pub section_path: String,
}

/// SSE payload for the [`AskStreamEvent::Final`] event. Mirrors the
/// proto `AskFinal` message minus the reserved `is_grounded` field.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AskFinalView {
    pub answer: String,
    pub confidence: f64,
    pub citations: Vec<CitationView>,
}

/// SSE payload for the [`AskStreamEvent::Budget`] event. Mirrors the
/// proto `BudgetSnapshot` message field-for-field.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BudgetSnapshotView {
    pub total_tokens: u64,
    pub cached_input_tokens: u64,
    pub cost_usd: f64,
    pub call_count: u64,
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

    // Live-streaming state populated by the hydrate-side fetch hook.
    // Using `RwSignal` so the form-submit listener can write while the
    // view re-renders on every change.
    //
    // `streaming` flips true the moment the form is hijacked, and
    // resets to false when the backend fires `final` or `error`. The
    // resource refetch then loads the persisted assistant turn so the
    // server-side message list is the source of truth.
    let pending_prompt: RwSignal<String> = RwSignal::new(String::new());
    let streaming: RwSignal<bool> = RwSignal::new(false);
    let live_answer: RwSignal<String> = RwSignal::new(String::new());
    let live_error: RwSignal<Option<String>> = RwSignal::new(None);
    let live_budget: RwSignal<Option<BudgetSnapshotView>> = RwSignal::new(None);

    let messages = Resource::new(
        move || conversation_id.get(),
        |maybe_id| async move {
            let Some(id) = maybe_id else {
                return Err(ServerFnError::new("no conversation id"));
            };
            load_conversation_messages(id).await
        },
    );

    let save_pinning = ServerAction::<SavePinningAction>::new();
    let clear_pinning = ServerAction::<ClearPinningAction>::new();

    let pinning = Resource::new(
        move || {
            (
                conversation_id.get(),
                streaming.get(),
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

    // Install the hydrate-side form-submit interceptor exactly once.
    // SSR renders the static markup; the listener attaches the moment
    // the wasm bundle runs.
    #[cfg(feature = "hydrate")]
    {
        let pending_prompt_eff = pending_prompt;
        let streaming_eff = streaming;
        let live_answer_eff = live_answer;
        let live_error_eff = live_error;
        let live_budget_eff = live_budget;
        let messages_eff = messages;
        let _ = Effect::new(move |_| {
            install_ask_form_hook(
                pending_prompt_eff,
                streaming_eff,
                live_answer_eff,
                live_error_eff,
                live_budget_eff,
                messages_eff,
            );
        });
    }

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
                {move || render_streaming_block(
                    pending_prompt,
                    streaming,
                    live_answer,
                    live_error,
                    live_budget,
                )}
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
                <form
                    id="ask-stream-form"
                    action=ASK_STREAM_PATH
                    method="post"
                    enctype="application/x-www-form-urlencoded"
                >
                    {move || conversation_id.get().map(|id| view! {
                        <input type="hidden" name="conversation_id" value=id.to_string()/>
                    })}
                    <textarea
                        name="prompt"
                        placeholder="Ask a question about your pinned documents…"
                        required
                        style="width: 100%; min-height: 4rem; padding: 0.5rem; box-sizing: border-box;"
                    ></textarea>
                    <button
                        type="submit"
                        style="margin-top: 0.5rem;"
                        disabled=move || streaming.get()
                    >
                        {move || if streaming.get() { "Streaming…" } else { "Send" }}
                    </button>
                </form>
            </section>
        </div>
    }
}

/// Render the in-flight streaming block (user prompt echo + live
/// answer + optional budget footer). Returns an empty fragment when
/// nothing is in flight.
fn render_streaming_block(
    pending_prompt: RwSignal<String>,
    streaming: RwSignal<bool>,
    live_answer: RwSignal<String>,
    live_error: RwSignal<Option<String>>,
    live_budget: RwSignal<Option<BudgetSnapshotView>>,
) -> AnyView {
    let active = streaming.get() || !live_answer.get().is_empty() || live_error.get().is_some();
    if !active {
        return view! { <div></div> }.into_any();
    }
    view! {
        <ul style="list-style: none; padding: 0; margin: 0;">
            <li style="padding: 0.8rem; margin-bottom: 0.5rem; background: #eef; border-radius: 4px;">
                <strong style="color: #666; font-size: 0.85rem;">"user (sending)"</strong>
                <p style="margin: 0.4rem 0; white-space: pre-wrap;">{move || pending_prompt.get()}</p>
            </li>
            <li style="padding: 0.8rem; margin-bottom: 0.5rem; background: #efe; border-radius: 4px;">
                <div style="display: flex; justify-content: space-between; color: #666; font-size: 0.85rem;">
                    <strong>"assistant"</strong>
                    <span>{move || if streaming.get() { "streaming…" } else { "" }}</span>
                </div>
                <p style="margin: 0.4rem 0; white-space: pre-wrap;">{move || live_answer.get()}</p>
                {move || live_error.get().map(|e| view! {
                    <p style="margin: 0.4rem 0; color: #c00;">"error: " {e}</p>
                })}
                {move || live_budget.get().map(|b| view! {
                    <BudgetBadge snapshot=b/>
                })}
            </li>
        </ul>
    }
    .into_any()
}

/// Compact, subtle footer rendering token + cost totals from a
/// `BudgetSnapshot`. Hidden behind a `<details>` toggle so the count
/// doesn't dominate the chat surface, with a single-line summary
/// visible by default.
#[component]
fn BudgetBadge(snapshot: BudgetSnapshotView) -> impl IntoView {
    let summary = format!(
        "tokens {} · ${:.4} · {} call{}",
        snapshot.total_tokens,
        snapshot.cost_usd,
        snapshot.call_count,
        if snapshot.call_count == 1 { "" } else { "s" },
    );
    view! {
        <details style="margin-top: 0.5rem; color: #666; font-size: 0.8rem;">
            <summary style="cursor: pointer;">{summary}</summary>
            <ul style="margin: 0.3rem 0 0 1rem; padding: 0;">
                <li>{format!("total tokens: {}", snapshot.total_tokens)}</li>
                <li>{format!("cached input tokens: {}", snapshot.cached_input_tokens)}</li>
                <li>{format!("cost (USD): {:.6}", snapshot.cost_usd)}</li>
                <li>{format!("LLM calls: {}", snapshot.call_count)}</li>
            </ul>
        </details>
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
        match msg.confidence {
            Some(c) if c >= CONFIDENCE_VERIFIED_THRESHOLD => {
                format!("\u{2713} verified · conf {c:.2}")
            }
            Some(c) => format!("low confidence · conf {c:.2}"),
            None => String::new(),
        }
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
// Hydrate-side fetch + SSE-parsing hook
// ---------------------------------------------------------------------------

#[cfg(feature = "hydrate")]
fn install_ask_form_hook(
    pending_prompt: RwSignal<String>,
    streaming: RwSignal<bool>,
    live_answer: RwSignal<String>,
    live_error: RwSignal<Option<String>>,
    live_budget: RwSignal<Option<BudgetSnapshotView>>,
    messages: Resource<Result<Vec<MessageView>, ServerFnError>>,
) {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};
    use web_sys::{Event, FormData, HtmlFormElement, Request, RequestInit, Response};

    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Some(element) = document.get_element_by_id("ask-stream-form") else {
        // Hook fires on every effect tick; the form might not yet be in
        // the DOM the first time around. The next reactive tick will
        // re-run after the SSR markup hydrates.
        return;
    };
    let Ok(form) = element.dyn_into::<HtmlFormElement>() else {
        return;
    };

    // Idempotent: tag the form once we've installed the listener so a
    // second `Effect` tick (e.g. after a streaming-state change) is a
    // no-op rather than stacking handlers.
    const HOOK_FLAG: &str = "data-stream-hook";
    if form.get_attribute(HOOK_FLAG).is_some() {
        return;
    }
    let _ = form.set_attribute(HOOK_FLAG, "1");

    let form_clone = form.clone();
    let closure = Closure::<dyn FnMut(Event)>::new(move |evt: Event| {
        evt.prevent_default();
        evt.stop_propagation();

        // Snapshot the prompt now so even if the user keeps typing
        // mid-stream the in-flight echo is correct.
        let prompt_text = read_textarea_value(&form_clone);
        if prompt_text.trim().is_empty() {
            return;
        }
        pending_prompt.set(prompt_text);
        streaming.set(true);
        live_answer.set(String::new());
        live_error.set(None);
        live_budget.set(None);

        let form_data = match FormData::new_with_form(&form_clone) {
            Ok(fd) => fd,
            Err(_) => {
                streaming.set(false);
                live_error.set(Some("could not read form fields".into()));
                return;
            }
        };

        let body = form_data_to_urlencoded(&form_data);

        // Reset the textarea so the next prompt can be typed without
        // wiping it manually.
        clear_textarea(&form_clone);

        let init = RequestInit::new();
        init.set_method("POST");
        init.set_body(&JsValue::from_str(&body));
        if let Ok(headers) = web_sys::Headers::new() {
            let _ = headers.set("content-type", "application/x-www-form-urlencoded");
            let _ = headers.set("accept", "text/event-stream");
            init.set_headers(&headers);
        }

        let request = match Request::new_with_str_and_init(ASK_STREAM_PATH, &init) {
            Ok(r) => r,
            Err(_) => {
                streaming.set(false);
                live_error.set(Some("could not build streaming request".into()));
                return;
            }
        };

        let window_for_fetch = match web_sys::window() {
            Some(w) => w,
            None => {
                streaming.set(false);
                live_error.set(Some("no window object".into()));
                return;
            }
        };
        let promise = window_for_fetch.fetch_with_request(&request);

        let pending_for_task = pending_prompt;
        let streaming_for_task = streaming;
        let live_answer_for_task = live_answer;
        let live_error_for_task = live_error;
        let live_budget_for_task = live_budget;
        let messages_for_task = messages;

        wasm_bindgen_futures::spawn_local(async move {
            // Suppress the "field is unused" warning for the prompt
            // signal — we only set it; it's surfaced by the view.
            let _ = pending_for_task;

            let resp_value = match wasm_bindgen_futures::JsFuture::from(promise).await {
                Ok(v) => v,
                Err(err) => {
                    streaming_for_task.set(false);
                    live_error_for_task.set(Some(format!("fetch failed: {err:?}")));
                    return;
                }
            };
            let response: Response = match resp_value.dyn_into() {
                Ok(r) => r,
                Err(_) => {
                    streaming_for_task.set(false);
                    live_error_for_task.set(Some("not a Response".into()));
                    return;
                }
            };
            if !response.ok() {
                streaming_for_task.set(false);
                live_error_for_task.set(Some(format!("HTTP {}", response.status())));
                return;
            }
            let body = match response.body() {
                Some(b) => b,
                None => {
                    streaming_for_task.set(false);
                    live_error_for_task.set(Some("response had no body".into()));
                    return;
                }
            };
            let reader_value = body.get_reader();
            let reader: web_sys::ReadableStreamDefaultReader = match reader_value.dyn_into() {
                Ok(r) => r,
                Err(_) => {
                    streaming_for_task.set(false);
                    live_error_for_task.set(Some("cannot read response body".into()));
                    return;
                }
            };
            let decoder = match web_sys::TextDecoder::new() {
                Ok(d) => d,
                Err(_) => {
                    streaming_for_task.set(false);
                    live_error_for_task.set(Some("no TextDecoder".into()));
                    return;
                }
            };

            let mut buffer = String::new();
            loop {
                let chunk_promise = reader.read();
                let chunk = match wasm_bindgen_futures::JsFuture::from(chunk_promise).await {
                    Ok(c) => c,
                    Err(err) => {
                        live_error_for_task.set(Some(format!("read error: {err:?}")));
                        break;
                    }
                };
                let done = js_sys::Reflect::get(&chunk, &JsValue::from_str("done"))
                    .ok()
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let value = js_sys::Reflect::get(&chunk, &JsValue::from_str("value"))
                    .unwrap_or(JsValue::UNDEFINED);

                if !value.is_undefined() {
                    let array: js_sys::Uint8Array = match value.dyn_into() {
                        Ok(a) => a,
                        Err(_) => {
                            live_error_for_task.set(Some("non-bytes chunk".into()));
                            break;
                        }
                    };
                    let bytes = array.to_vec();
                    if let Ok(text) = decoder.decode_with_u8_array(&bytes) {
                        buffer.push_str(&text);
                        drain_sse_events(
                            &mut buffer,
                            live_answer_for_task,
                            live_error_for_task,
                            live_budget_for_task,
                            streaming_for_task,
                        );
                    }
                }

                if done {
                    break;
                }
            }

            streaming_for_task.set(false);
            messages_for_task.refetch();
        });
    });

    if form
        .add_event_listener_with_callback("submit", closure.as_ref().unchecked_ref())
        .is_err()
    {
        return;
    }
    closure.forget();
}

#[cfg(feature = "hydrate")]
fn read_textarea_value(form: &web_sys::HtmlFormElement) -> String {
    use wasm_bindgen::JsCast;
    let elements = form.elements();
    let len = elements.length();
    for i in 0..len {
        let Some(node) = elements.item(i) else {
            continue;
        };
        if let Ok(textarea) = node.dyn_into::<web_sys::HtmlTextAreaElement>() {
            if textarea.name() == "prompt" {
                return textarea.value();
            }
        }
    }
    String::new()
}

#[cfg(feature = "hydrate")]
fn clear_textarea(form: &web_sys::HtmlFormElement) {
    use wasm_bindgen::JsCast;
    let elements = form.elements();
    let len = elements.length();
    for i in 0..len {
        let Some(node) = elements.item(i) else {
            continue;
        };
        if let Ok(textarea) = node.dyn_into::<web_sys::HtmlTextAreaElement>() {
            if textarea.name() == "prompt" {
                textarea.set_value("");
                return;
            }
        }
    }
}

#[cfg(feature = "hydrate")]
fn form_data_to_urlencoded(form_data: &web_sys::FormData) -> String {
    use wasm_bindgen::JsCast;
    let mut out = String::new();
    let entries = js_sys::try_iter(form_data.as_ref()).ok().flatten();
    let Some(iter) = entries else {
        return out;
    };
    for entry in iter.flatten() {
        let pair: js_sys::Array = match entry.dyn_into() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let key = pair.get(0).as_string().unwrap_or_default();
        let val = pair.get(1).as_string().unwrap_or_default();
        if !out.is_empty() {
            out.push('&');
        }
        out.push_str(&urlencode(&key));
        out.push('=');
        out.push_str(&urlencode(&val));
    }
    out
}

#[cfg(feature = "hydrate")]
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            b' ' => out.push('+'),
            other => {
                out.push('%');
                out.push_str(&format!("{other:02X}"));
            }
        }
    }
    out
}

/// Pull every fully-buffered SSE event off `buffer` and apply it to the
/// reactive signals. The SSE wire format separates events by a blank
/// line; we drain in-place so leftover bytes wait for the next chunk.
#[cfg(feature = "hydrate")]
fn drain_sse_events(
    buffer: &mut String,
    live_answer: RwSignal<String>,
    live_error: RwSignal<Option<String>>,
    live_budget: RwSignal<Option<BudgetSnapshotView>>,
    streaming: RwSignal<bool>,
) {
    while let Some(end_idx) = buffer.find("\n\n") {
        let event_block = buffer[..end_idx].to_string();
        buffer.drain(..end_idx + 2);
        apply_sse_block(
            &event_block,
            live_answer,
            live_error,
            live_budget,
            streaming,
        );
    }
}

#[cfg(feature = "hydrate")]
fn apply_sse_block(
    block: &str,
    live_answer: RwSignal<String>,
    live_error: RwSignal<Option<String>>,
    live_budget: RwSignal<Option<BudgetSnapshotView>>,
    streaming: RwSignal<bool>,
) {
    let mut event_kind: Option<String> = None;
    let mut data_lines: Vec<&str> = Vec::new();
    for raw in block.split('\n') {
        let line = raw.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("event:") {
            event_kind = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim_start_matches(' '));
        }
    }
    let data = data_lines.join("\n");
    let kind = event_kind.as_deref().unwrap_or("");
    match kind {
        "delta" => {
            // Server emits a JSON-string-encoded text delta so embedded
            // newlines survive the multi-line `data:` rule.
            let parsed: Result<String, _> = serde_json::from_str(&data);
            match parsed {
                Ok(text) => live_answer.update(|s| s.push_str(&text)),
                Err(_) => live_answer.update(|s| s.push_str(&data)),
            }
        }
        "final" => {
            // Final answer: replace the live answer with the canonical
            // version so any whitespace differences from the deltas
            // resolve to whatever the server committed.
            if let Ok(view) = serde_json::from_str::<AskFinalView>(&data) {
                live_answer.set(view.answer);
            }
        }
        "budget" => {
            if let Ok(snap) = serde_json::from_str::<BudgetSnapshotView>(&data) {
                live_budget.set(Some(snap));
            }
        }
        "error" => {
            live_error.set(Some(data.clone()));
            streaming.set(false);
        }
        _ => {
            // Unknown event — ignore for forward-compat.
        }
    }
}

// ---------------------------------------------------------------------------
// Server fns (non-streaming pieces)
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
            Some(ids) => (false, ids.into_iter().flatten().collect::<Vec<Uuid>>()),
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
        crate::conversations::set_pinned_document_ids(&mut conn, user_id, conversation_id, None)
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

// ---------------------------------------------------------------------------
// /api/ask_stream — plain axum SSE handler
// ---------------------------------------------------------------------------

/// State bundle for the SSE handler. The pool is shared with every
/// other axum handler via Extension; we re-use the same wrapper here
/// rather than reaching into Leptos' `provide_context` since this
/// route is not a Leptos server function.
#[cfg(feature = "ssr")]
#[derive(Clone)]
pub struct AskStreamState {
    pub pool: crate::db::DbPool,
}

/// Form payload for `POST /api/ask_stream`. Mirrors the legacy
/// send-message form fields so the no-JS fallback (form action without
/// the hydrate hook) still works structurally.
#[cfg(feature = "ssr")]
#[derive(Debug, serde::Deserialize)]
pub struct AskStreamForm {
    pub conversation_id: Uuid,
    pub prompt: String,
}

/// Drive the gRPC `Ask` stream and forward events as SSE. Runs the DB
/// persistence inline so a closed client connection still records the
/// turns it generated up to that point.
///
/// # Errors
///
/// Returns 400/401/500 with a plain-text body for synchronous failures
/// before the stream opens. Per-stream failures arrive as SSE `error`
/// events rather than HTTP errors so the client can render them.
#[cfg(feature = "ssr")]
pub async fn ask_stream_handler(
    session: Session,
    Extension(state): Extension<AskStreamState>,
    Form(payload): Form<AskStreamForm>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    use crate::pb::{ask_event::Payload as PbPayload, AskRequest, ChatTurn};

    let user_id = crate::auth::session_user_id(&session)
        .await
        .ok_or((StatusCode::UNAUTHORIZED, "login required".into()))?;

    if payload.prompt.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty prompt".into()));
    }

    let mut conn = state
        .pool
        .get()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db conn: {e}")))?;

    // Verify ownership + cache the conversation row. Doing this before
    // we open any channel keeps the failure mode tidy.
    let conv = crate::conversations::load_conversation(&mut conn, user_id, payload.conversation_id)
        .await
        .map_err(|e| (StatusCode::FORBIDDEN, format!("load conversation: {e}")))?;

    // Build the active-document set the same way as the legacy server
    // function did (auto mode → every complete doc, explicit mode →
    // restricted to complete docs only).
    let active_keys = match conv.pinned_document_ids.as_ref() {
        None => list_complete_minio_keys(&mut conn, user_id)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("active docs: {e}"),
                )
            })?,
        Some(ids) => {
            let pinned: Vec<Uuid> = ids.iter().filter_map(|x| *x).collect();
            if pinned.is_empty() {
                Vec::new()
            } else {
                list_minio_keys_for_ids(&mut conn, user_id, &pinned)
                    .await
                    .map_err(|e| {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("pinned docs: {e}"),
                        )
                    })?
            }
        }
    };

    // Replay history inline; agent is stateless.
    let history_rows = crate::conversations::list_messages(&mut conn, payload.conversation_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("history: {e}")))?;
    let history: Vec<ChatTurn> = history_rows
        .iter()
        .map(|m| ChatTurn {
            role: m.role.clone(),
            content: m.content.clone(),
        })
        .collect();

    // Persist the user turn before opening the gRPC stream so a crash
    // mid-stream never loses the prompt.
    let prompt_for_grpc = payload.prompt.clone();
    let user_msg =
        crate::conversations::append_user_message(&mut conn, &conv, &payload.prompt, &[])
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("persist user: {e}"),
                )
            })?;

    let user_grpc_id = user_id.as_simple().to_string();
    let request = AskRequest {
        user_id: user_grpc_id,
        query: prompt_for_grpc,
        history,
        active_document_keys: active_keys,
        history_document_keys: Vec::new(),
        trace_id: user_msg.id.to_string(),
        strategy: String::new(),
        limit: 0,
    };

    let conversation_id = payload.conversation_id;
    let pool_for_task = state.pool.clone();

    // Channel that bridges the gRPC stream task and the axum SSE
    // response. We bound it so a slow client can apply backpressure
    // rather than letting the server buffer unboundedly.
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(ASK_STREAM_BUFFER);

    tokio::spawn(async move {
        let mut stream = match crate::grpc::ask_stream(request).await {
            Ok(s) => s,
            Err(e) => {
                let _ = tx
                    .send(Ok(sse_error_event(&format!("grpc connect: {e}"))))
                    .await;
                return;
            }
        };

        let mut answer_buf = String::new();
        let mut final_view: Option<AskFinalView> = None;
        let mut error_emitted = false;

        while let Some(event_res) = stream.next().await {
            let event = match event_res {
                Ok(e) => e,
                Err(e) => {
                    let _ = tx
                        .send(Ok(sse_error_event(&format!("grpc transport: {e}"))))
                        .await;
                    error_emitted = true;
                    break;
                }
            };

            let Some(payload) = event.payload else {
                continue;
            };
            match payload {
                PbPayload::PartialAnswer(p) => {
                    answer_buf.push_str(&p.text_delta);
                    let json = serde_json::to_string(&p.text_delta).unwrap_or_else(|_| {
                        warn!("text_delta failed to serialize");
                        "\"\"".into()
                    });
                    let evt = Event::default().event("delta").data(json);
                    if tx.send(Ok(evt)).await.is_err() {
                        return;
                    }
                }
                PbPayload::Final(f) => {
                    let citations: Vec<CitationView> = f
                        .citations
                        .iter()
                        .map(|c| CitationView {
                            index: c.index,
                            snippet: c.content_snippet.clone(),
                            minio_object_key: c.minio_object_key.clone(),
                            section_path: c.section_path.clone(),
                        })
                        .collect();
                    let view = AskFinalView {
                        answer: f.answer.clone(),
                        confidence: f.confidence,
                        citations,
                    };
                    final_view = Some(view.clone());
                    let json = serde_json::to_string(&view).unwrap_or_else(|_| "{}".into());
                    let evt = Event::default().event("final").data(json);
                    if tx.send(Ok(evt)).await.is_err() {
                        return;
                    }
                }
                PbPayload::BudgetSnapshot(b) => {
                    let view = BudgetSnapshotView {
                        total_tokens: b.total_tokens,
                        cached_input_tokens: b.cached_input_tokens,
                        cost_usd: b.cost_usd,
                        call_count: b.call_count,
                    };
                    let json = serde_json::to_string(&view).unwrap_or_else(|_| "{}".into());
                    let evt = Event::default().event("budget").data(json);
                    if tx.send(Ok(evt)).await.is_err() {
                        return;
                    }
                }
                PbPayload::Error(e) => {
                    let _ = tx
                        .send(Ok(sse_error_event(&format!("{}: {}", e.code, e.message))))
                        .await;
                    error_emitted = true;
                    break;
                }
                PbPayload::IndexingWait(_) | PbPayload::ToolCall(_) => {
                    // No live UI for these progress events yet; ignore.
                }
            }
        }

        // Persist the assistant turn. Prefer the canonical Final
        // payload (it carries citations + confidence). Fall back to
        // whatever we accumulated from text deltas — for resilience
        // when an Error event terminates the stream early.
        let assistant_content = match final_view.as_ref() {
            Some(f) => f.answer.clone(),
            None if !answer_buf.is_empty() => answer_buf.clone(),
            None if error_emitted => "agent error (see live event)".to_string(),
            None => "agent produced no terminal event".to_string(),
        };
        let citations_json = match final_view.as_ref() {
            Some(f) => serde_json::Value::Array(
                f.citations
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "index": c.index,
                            "snippet": c.snippet,
                            "minio_object_key": c.minio_object_key,
                            "section_path": c.section_path,
                        })
                    })
                    .collect(),
            ),
            None => serde_json::Value::Array(Vec::new()),
        };
        let confidence = final_view.as_ref().map(|f| f.confidence);

        match pool_for_task.get().await {
            Ok(mut conn) => {
                if let Err(e) = crate::conversations::append_assistant_message(
                    &mut conn,
                    conversation_id,
                    &assistant_content,
                    citations_json,
                    confidence,
                )
                .await
                {
                    error!(error = %e, "persist assistant turn failed");
                }
            }
            Err(e) => {
                error!(error = %e, "db pool checkout failed during persist");
            }
        }

        info!(
            conversation_id = %conversation_id,
            had_final = final_view.is_some(),
            "ask stream complete"
        );
    });

    let stream = ReceiverStream::new(rx);
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[cfg(feature = "ssr")]
fn sse_error_event(msg: &str) -> Event {
    Event::default().event("error").data(msg)
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
