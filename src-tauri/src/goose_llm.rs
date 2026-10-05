/// goose-sdk backed inference layer (desktop only).
///
/// Replaces the hand-rolled Ollama HTTP client for the *inference* path with
/// goose-sdk's provider layer, giving multi-provider access (local Ollama via a
/// declarative provider, plus OpenAI / Anthropic / Groq / Databricks via API
/// key) and token-by-token streaming.
///
/// Model *management* (list/pull/delete) stays on the Ollama HTTP API in
/// `llm.rs` — goose-sdk provides model access, not model downloads.
use std::sync::Arc;

use goose_sdk::bindings::{
    anthropic_provider, databricks_provider, declarative_provider_from_json, groq_provider,
    openai_provider, MessageContent, MessageRole, Provider, ProviderMessage, ProviderModelConfig,
    StreamChunk,
};
use tauri::{AppHandle, Emitter};

use crate::llm::{ChatMessage, LlmChatResponse};

/// Build a goose provider from the app's LLM settings.
///
/// * `ollama` (default) — declarative provider against the local Ollama native
///   API at `base_url` (no auth).
/// * `openai` / `anthropic` / `groq` — built-in providers using `api_key`.
/// * `databricks` — `base_url` is treated as the Databricks host, `api_key` as
///   the token.
pub fn build_provider(
    provider: &str,
    api_key: &str,
    base_url: &str,
) -> Result<Arc<Provider>, String> {
    match provider {
        "openai" => openai_provider(api_key.to_string(), None).map_err(|e| e.to_string()),
        "anthropic" => {
            anthropic_provider(api_key.to_string(), None, Vec::new()).map_err(|e| e.to_string())
        }
        "groq" => groq_provider(api_key.to_string()).map_err(|e| e.to_string()),
        "databricks" => {
            databricks_provider(base_url.to_string(), api_key.to_string()).map_err(|e| e.to_string())
        }
        // Default: local Ollama via an OpenAI-less declarative provider.
        _ => {
            let json = serde_json::json!({
                "name": "ollama",
                "engine": "ollama",
                "display_name": "Ollama",
                "base_url": base_url,
                "models": [],
                "requires_auth": false,
                "dynamic_models": true,
            })
            .to_string();
            declarative_provider_from_json(json).map_err(|e| e.to_string())
        }
    }
}

fn model_config(model: &str, temperature: f32) -> ProviderModelConfig {
    ProviderModelConfig {
        model_name: model.to_string(),
        context_limit: None,
        temperature: Some(temperature),
        max_tokens: None,
        toolshim: false,
        toolshim_model: None,
        request_params_json: None,
        provider_params_json: None,
        reasoning: None,
        timeout_ms: None,
        request_headers: None,
    }
}

/// Split our flat `{role, content}` messages into the goose shape: a single
/// system-prompt string (goose has no `System` message role — it takes the
/// system prompt as a separate `stream`/`complete` argument) plus the ordered
/// user/assistant turns.
pub fn to_goose_messages(messages: Vec<ChatMessage>) -> (String, Vec<ProviderMessage>) {
    let mut system = String::new();
    let mut out = Vec::new();
    for m in messages {
        match m.role.as_str() {
            "system" => {
                if system.is_empty() {
                    system = m.content;
                } else {
                    system = format!("{}\n\n{}", system, m.content);
                }
            }
            "assistant" => out.push(ProviderMessage {
                role: MessageRole::Assistant,
                content: vec![MessageContent::Text { text: m.content }],
            }),
            // Everything else (user, tool, unknown) is sent as a user turn.
            _ => out.push(ProviderMessage {
                role: MessageRole::User,
                content: vec![MessageContent::Text { text: m.content }],
            }),
        }
    }
    (system, out)
}

/// Resolve the effective system prompt: an explicit override wins, otherwise the
/// system message extracted from the conversation history is used.
fn resolve_system(extracted: String, system_override: Option<String>) -> String {
    match system_override {
        Some(s) if !s.trim().is_empty() => s,
        _ => extracted,
    }
}

fn usage_tokens(usage: Option<goose_sdk::bindings::Usage>) -> (u32, u32) {
    match usage {
        Some(u) => (
            u.input_tokens.unwrap_or(0).max(0) as u32,
            u.output_tokens.unwrap_or(0).max(0) as u32,
        ),
        None => (0, 0),
    }
}

/// Non-streaming completion. Drives the goose provider to completion and
/// returns the same `LlmChatResponse` shape the Ollama path produced, so all
/// existing callers (OCR fix, word generation) keep working unchanged.
pub async fn chat_once(
    provider: &str,
    api_key: &str,
    base_url: &str,
    model: &str,
    messages: Vec<ChatMessage>,
    system_override: Option<String>,
    temperature: f32,
) -> Result<LlmChatResponse, String> {
    let provider_obj = build_provider(provider, api_key, base_url)?;
    let (extracted, goose_messages) = to_goose_messages(messages);
    let system = resolve_system(extracted, system_override);

    let completion = provider_obj
        .complete(
            model_config(model, temperature),
            system,
            goose_messages,
            Vec::new(),
        )
        .await
        .map_err(|e| e.to_string())?;

    let mut content = String::new();
    for c in completion.content {
        if let MessageContent::Text { text } = c {
            content.push_str(&text);
        }
    }
    let (prompt_tokens, completion_tokens) = usage_tokens(completion.usage);

    Ok(LlmChatResponse {
        content,
        prompt_tokens,
        completion_tokens,
    })
}

/// Streaming completion. Emits `llm-chat-chunk` per text delta, then a single
/// `llm-chat-done` with token usage. Mid-stream failures emit `llm-chat-error`.
/// All events carry the caller-supplied `id` so the frontend can correlate.
pub async fn chat_stream(
    app: &AppHandle,
    id: &str,
    provider: &str,
    api_key: &str,
    base_url: &str,
    model: &str,
    messages: Vec<ChatMessage>,
    system_override: Option<String>,
    temperature: f32,
) -> Result<(), String> {
    let provider_obj = build_provider(provider, api_key, base_url)?;
    let (extracted, goose_messages) = to_goose_messages(messages);
    let system = resolve_system(extracted, system_override);

    let stream = provider_obj
        .stream(
            model_config(model, temperature),
            system,
            goose_messages,
            Vec::new(),
        )
        .await
        .map_err(|e| e.to_string())?;

    loop {
        match stream.next_chunk().await.map_err(|e| e.to_string())? {
            Some(StreamChunk::TextChunk { text }) => {
                let _ = app.emit("llm-chat-chunk", serde_json::json!({ "id": id, "text": text }));
            }
            Some(StreamChunk::EndChunk { usage }) => {
                let (prompt_tokens, completion_tokens) = usage_tokens(usage);
                let _ = app.emit(
                    "llm-chat-done",
                    serde_json::json!({ "id": id, "prompt_tokens": prompt_tokens, "completion_tokens": completion_tokens }),
                );
                break;
            }
            Some(StreamChunk::ErrorChunk { error }) => {
                let _ = app.emit(
                    "llm-chat-error",
                    serde_json::json!({ "id": id, "message": error.message }),
                );
                break;
            }
            // Thinking / tool chunks are not surfaced in the plain chat UI.
            Some(_) => {}
            None => {
                let _ = app.emit(
                    "llm-chat-done",
                    serde_json::json!({ "id": id, "prompt_tokens": 0, "completion_tokens": 0 }),
                );
                break;
            }
        }
    }

    Ok(())
}
