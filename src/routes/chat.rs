//! `/chat` — authenticated chat UI backed by the Ask gRPC.

use leptos::prelude::*;
use leptos::web_sys;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Response types (cross-wasm-boundary — no tonic/prost)
// ---------------------------------------------------------------------------

/// A single source citation returned with an answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    pub source: String,
    pub chunk_id: String,
    /// Relevance score from the proto (f64 in proto, stored as f64 here).
    pub score: f64,
}

/// The full response payload from `ask_action`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AskResponse {
    pub answer: String,
    pub citations: Vec<Citation>,
    pub confidence: f64,
    pub is_grounded: bool,
}

// ---------------------------------------------------------------------------
// Server function
// ---------------------------------------------------------------------------

/// Send a question to the agent-service Ask RPC and return a complete answer.
///
/// Collects all streaming events and returns the `FinalAnswer` payload.
///
/// # Errors
///
/// Returns `ServerFnError` when unauthenticated or the gRPC call fails.
#[server(AskAction, "/api")]
pub async fn ask_action(question: String) -> Result<AskResponse, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use leptos_axum::extract;
        use tower_sessions::Session;

        use crate::auth::session_current_user;
        use crate::db::DbPool;
        use crate::error::AppError;
        use crate::pb::ask_event::Payload;

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

        let req = tonic::Request::new(crate::pb::AskRequest {
            user_id: user.grpc_id(),
            session_id: String::new(), // MVP: fresh session each call
            question,
            strategy: String::new(),
            limit: 0,
        });

        let mut stream = client
            .ask(req)
            .await
            .map_err(|e| AppError::from(e).into_server_fn_error())?
            .into_inner();

        let mut resp = AskResponse::default();
        while let Some(event) = stream
            .message()
            .await
            .map_err(|e| AppError::Grpc(e.to_string()).into_server_fn_error())?
        {
            if let Some(Payload::FinalAnswer(fa)) = event.payload {
                resp.answer = fa.answer;
                resp.confidence = fa.confidence;
                resp.is_grounded = fa.is_grounded;
                resp.citations = fa
                    .citations
                    .into_iter()
                    .map(|c| Citation {
                        source: c.source,
                        chunk_id: c.chunk_id,
                        score: c.score,
                    })
                    .collect();
            }
            // SessionBound / ToolCall / PartialAnswer events are ignored in
            // the MVP — no streaming UI yet.
        }
        Ok(resp)
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = question;
        Err(ServerFnError::ServerError(
            "server function called on client".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// A single rendered message bubble.
#[derive(Clone)]
struct Message {
    role: &'static str,
    content: String,
    citations: Vec<Citation>,
}

#[component]
pub fn ChatPage() -> impl IntoView {
    // Message history stored client-side for MVP.
    let messages: RwSignal<Vec<Message>> = RwSignal::new(vec![]);
    let question = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let error_msg: RwSignal<Option<String>> = RwSignal::new(None);

    let do_submit = move || {
        let q = question.get_untracked();
        if q.trim().is_empty() {
            return;
        }
        let q_clone = q.clone();
        messages.update(|m| {
            m.push(Message {
                role: "user",
                content: q,
                citations: vec![],
            })
        });
        question.set(String::new());
        pending.set(true);
        error_msg.set(None);

        leptos::task::spawn_local(async move {
            match ask_action(q_clone).await {
                Ok(resp) => {
                    messages.update(|m| {
                        m.push(Message {
                            role: "assistant",
                            content: resp.answer,
                            citations: resp.citations,
                        })
                    });
                }
                Err(e) => {
                    let msg = e.to_string();
                    error_msg.set(Some(
                        msg.trim_start_matches("server error: ").to_string(),
                    ));
                }
            }
            pending.set(false);
        });
    };

    let on_submit = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        do_submit();
    };

    view! {
        <div style="max-width:800px;margin:0 auto;padding:20px;font-family:sans-serif">
            <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:16px">
                <h1 style="margin:0">"Chat"</h1>
                <div style="display:flex;gap:12px;align-items:center">
                    <a href="/library" style="text-decoration:none">"Library"</a>
                    <form method="post" action="/api/logout_action">
                        <button type="submit" style="padding:6px 12px">"Sign out"</button>
                    </form>
                </div>
            </div>

            // Message history
            <ul style="list-style:none;padding:0;margin:0 0 16px 0;min-height:200px;border:1px solid #ddd;border-radius:6px;overflow-y:auto;max-height:500px">
                <For
                    each=move || messages.get().into_iter().enumerate()
                    key=|(i, _)| *i
                    children=move |(_, msg)| {
                        let is_user = msg.role == "user";
                        let bg = if is_user { "#e8f0fe" } else { "#f6f8fa" };
                        let align = if is_user { "flex-end" } else { "flex-start" };
                        let has_cits = !msg.citations.is_empty();
                        let cits = msg.citations.clone();
                        view! {
                            <li style=format!("display:flex;justify-content:{align};padding:8px 12px")>
                                <div style=format!("background:{bg};padding:10px 14px;border-radius:8px;max-width:85%")>
                                    <p style="margin:0;white-space:pre-wrap">{msg.content}</p>
                                    <Show when=move || has_cits>
                                        <details style="margin-top:8px;font-size:0.85em;color:#555">
                                            <summary>"Sources"</summary>
                                            <ul style="margin:4px 0 0 0;padding-left:16px">
                                                {cits.iter().map(|c| view! {
                                                    <li>
                                                        {c.source.clone()}
                                                        " ("
                                                        {format!("{:.0}%", c.score * 100.0)}
                                                        ")"
                                                    </li>
                                                }).collect::<Vec<_>>()}
                                            </ul>
                                        </details>
                                    </Show>
                                </div>
                            </li>
                        }
                    }
                />
                <Show when=move || pending.get()>
                    <li style="padding:8px 12px;color:#888;font-style:italic">"Thinking…"</li>
                </Show>
            </ul>

            <Show when=move || error_msg.get().is_some()>
                <p style="color:red;margin-bottom:8px">{move || error_msg.get().unwrap_or_default()}</p>
            </Show>

            // Input form — Enter submits, Shift+Enter inserts newline
            <form on:submit=on_submit style="display:flex;gap:8px">
                <textarea
                    name="question"
                    placeholder="Ask a question…"
                    rows="3"
                    style="flex:1;padding:8px;font-size:1rem;border:1px solid #ccc;border-radius:4px;resize:vertical"
                    prop:value=move || question.get()
                    on:input=move |ev| question.set(event_target_value(&ev))
                    on:keydown=move |ev: web_sys::KeyboardEvent| {
                        if ev.key() == "Enter" && !ev.shift_key() {
                            ev.prevent_default();
                            do_submit();
                        }
                    }
                />
                <button
                    type="submit"
                    disabled=move || pending.get()
                    style="padding:8px 16px;font-size:1rem"
                >
                    "Send"
                </button>
            </form>
            <p style="font-size:0.75em;color:#888;margin-top:4px">"Shift+Enter for newline, Enter to send."</p>
        </div>
    }
}
