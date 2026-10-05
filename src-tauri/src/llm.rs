/// LLM commands — communicates with a local Ollama instance via its HTTP API.
/// Ollama handles model management, inference, and GPU acceleration.
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::settings::SettingsState;

// ---------------------------------------------------------------------------
// Request / response types for Ollama API
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[cfg(not(feature = "desktop"))]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage>,
    stream: bool,
    options: ChatOptions,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Serialize)]
#[cfg(not(feature = "desktop"))]
struct ChatOptions {
    temperature: f32,
    num_predict: u32,
}

#[derive(Deserialize)]
#[cfg(not(feature = "desktop"))]
struct ChatResponse {
    message: Option<ChatMessage>,
    prompt_eval_count: Option<u32>,
    eval_count: Option<u32>,
}

#[derive(Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModelInfo>,
}

#[derive(Deserialize, Clone, Serialize)]
pub struct OllamaModelInfo {
    pub name: String,
    pub size: Option<u64>,
    pub digest: Option<String>,
    pub details: Option<OllamaModelDetails>,
}

#[derive(Deserialize, Clone, Serialize)]
pub struct OllamaModelDetails {
    pub family: Option<String>,
    pub parameter_size: Option<String>,
    pub quantization_level: Option<String>,
}

#[derive(Deserialize)]
struct PullStatusLine {
    status: Option<String>,
    total: Option<u64>,
    completed: Option<u64>,
}

#[derive(Serialize, Clone)]
pub struct PullProgressPayload {
    pub status: String,
    pub total: Option<u64>,
    pub completed: Option<u64>,
}

#[derive(Serialize, Clone)]
pub struct LlmChatResponse {
    pub content: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

#[derive(Serialize, Clone)]
pub struct LlmInstalledModelsResponse {
    pub installed: Vec<OllamaModelInfo>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn ollama_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .expect("failed to build HTTP client")
}

/// Get the Ollama base URL from settings.
fn get_ollama_url(state: &State<'_, SettingsState>) -> String {
    state.settings.lock().unwrap().ollama_url.clone()
}

/// Read the LLM inference configuration: (provider, api_key, base_url).
/// `base_url` is the Ollama URL for the `ollama` provider, and the Databricks
/// host for `databricks`; unused by the other cloud providers.
fn get_llm_config(state: &State<'_, SettingsState>) -> (String, String, String) {
    let s = state.settings.lock().unwrap();
    (
        s.llm_provider.clone(),
        s.llm_api_key.clone(),
        s.ollama_url.clone(),
    )
}

/// True when the configured provider is a cloud provider (not local Ollama).
fn is_cloud_provider(provider: &str) -> bool {
    !provider.is_empty() && provider != "ollama"
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Check if the configured LLM provider is reachable.
///
/// For cloud providers this reports whether an API key is configured. For local
/// Ollama it pings the HTTP endpoint.
#[tauri::command]
pub async fn llm_check_connection(state: State<'_, SettingsState>) -> Result<bool, String> {
    let (provider, api_key, base_url) = get_llm_config(&state);
    if is_cloud_provider(&provider) {
        return Ok(!api_key.trim().is_empty());
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| e.to_string())?;
    match client.get(&base_url).send().await {
        Ok(resp) => Ok(resp.status().is_success()),
        Err(_) => Ok(false),
    }
}

/// List models installed in Ollama together with our recommended catalog.
#[tauri::command]
pub async fn llm_list_models(state: State<'_, SettingsState>) -> Result<LlmInstalledModelsResponse, String> {
    let base_url = get_ollama_url(&state);
    let client = ollama_client();
    let resp = client
        .get(format!("{}/api/tags", base_url))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let tags: OllamaTagsResponse = resp.json().await.map_err(|e| e.to_string())?;

    Ok(LlmInstalledModelsResponse {
        installed: tags.models,
    })
}

/// Pull a model into Ollama.  Emits `llm-pull-progress` events with status updates.
#[tauri::command]
pub async fn llm_pull_model(
    app: AppHandle,
    state: State<'_, SettingsState>,
    model: String,
) -> Result<(), String> {
    let base_url = get_ollama_url(&state);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3600)) // pulls can take a while
        .build()
        .map_err(|e| e.to_string())?;

    let body = serde_json::json!({ "name": model, "stream": true });

    let resp = client
        .post(format!("{}/api/pull", base_url))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Failed to connect to Ollama: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Ollama pull failed ({}): {}", status, text));
    }

    // Read the newline-delimited JSON stream.
    use futures_util::StreamExt;

    let mut stream = resp.bytes_stream();
    let mut line_buf = Vec::new();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| e.to_string())?;
        line_buf.extend_from_slice(&chunk);

        // Process every complete line in the buffer.
        while let Some(pos) = line_buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = line_buf.drain(..=pos).collect();
            let line_str = String::from_utf8_lossy(&line).trim().to_string();
            if line_str.is_empty() {
                continue;
            }

            if let Ok(status) = serde_json::from_str::<PullStatusLine>(&line_str) {
                let payload = PullProgressPayload {
                    status: status.status.unwrap_or_else(|| "pulling".to_string()),
                    total: status.total,
                    completed: status.completed,
                };
                let _ = app.emit("llm-pull-progress", &payload);
            }
        }
    }

    Ok(())
}

/// Delete a model from Ollama.
#[tauri::command]
pub async fn llm_delete_model(state: State<'_, SettingsState>, model: String) -> Result<(), String> {
    let base_url = get_ollama_url(&state);
    let client = ollama_client();
    let body = serde_json::json!({ "name": model });
    let resp = client
        .delete(format!("{}/api/delete", base_url))
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Ollama delete failed ({}): {}", status, text));
    }
    Ok(())
}

/// Send a chat completion request (non-streaming).
///
/// On desktop this is backed by the goose-sdk provider layer (multi-provider +
/// local Ollama via a declarative provider). On other platforms it falls back to
/// the Ollama HTTP API. The signature and response shape are unchanged, so all
/// callers (chat page, OCR fix, word generation) keep working.
#[tauri::command]
pub async fn llm_chat(
    state: State<'_, SettingsState>,
    model: String,
    messages: Vec<ChatMessage>,
    temperature: Option<f32>,
) -> Result<LlmChatResponse, String> {
    let temperature = temperature.unwrap_or(0.7);
    #[cfg(feature = "desktop")]
    {
        let (provider, api_key, base_url) = get_llm_config(&state);
        crate::goose_llm::chat_once(
            &provider,
            &api_key,
            &base_url,
            &model,
            messages,
            None,
            temperature,
        )
        .await
    }
    #[cfg(not(feature = "desktop"))]
    {
        let base_url = get_ollama_url(&state);
        ollama_chat(&base_url, &model, messages, temperature).await
    }
}

/// Streaming chat completion. Emits `llm-chat-chunk` events (text deltas) as the
/// model responds, then a single `llm-chat-done` event with token usage. Errors
/// mid-stream are emitted as `llm-chat-error`. Every event carries `id` so the
/// frontend can correlate chunks with the request that produced them.
#[tauri::command]
pub async fn llm_chat_stream(
    app: AppHandle,
    state: State<'_, SettingsState>,
    id: String,
    model: String,
    messages: Vec<ChatMessage>,
    system: Option<String>,
    temperature: Option<f32>,
) -> Result<(), String> {
    let temperature = temperature.unwrap_or(0.7);
    #[cfg(feature = "desktop")]
    {
        let (provider, api_key, base_url) = get_llm_config(&state);
        crate::goose_llm::chat_stream(
            &app,
            &id,
            &provider,
            &api_key,
            &base_url,
            &model,
            messages,
            system,
            temperature,
        )
        .await
    }
    #[cfg(not(feature = "desktop"))]
    {
        use tauri::Emitter;
        // Fallback: no goose-sdk on this platform. Run the Ollama HTTP path once
        // and replay it as a single chunk + done event so the frontend streaming
        // contract still holds.
        let mut msgs = messages;
        if let Some(sys) = system {
            if !sys.trim().is_empty() {
                msgs.insert(0, ChatMessage { role: "system".to_string(), content: sys });
            }
        }
        let base_url = get_ollama_url(&state);
        let resp = ollama_chat(&base_url, &model, msgs, temperature).await?;
        let _ = app.emit("llm-chat-chunk", serde_json::json!({ "id": id, "text": resp.content }));
        let _ = app.emit(
            "llm-chat-done",
            serde_json::json!({ "id": id, "prompt_tokens": resp.prompt_tokens, "completion_tokens": resp.completion_tokens }),
        );
        Ok(())
    }
}

/// Non-streaming Ollama HTTP chat completion (non-desktop fallback path).
#[cfg(not(feature = "desktop"))]
async fn ollama_chat(
    base_url: &str,
    model: &str,
    messages: Vec<ChatMessage>,
    temperature: f32,
) -> Result<LlmChatResponse, String> {
    let client = ollama_client();
    let req = ChatRequest {
        model,
        messages,
        stream: false,
        options: ChatOptions {
            temperature,
            num_predict: 8192,
        },
    };

    let resp = client
        .post(format!("{}/api/chat", base_url))
        .json(&req)
        .send()
        .await
        .map_err(|e| format!("Failed to connect to Ollama: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Chat request failed ({}): {}", status, text));
    }

    let chat_resp: ChatResponse = resp.json().await.map_err(|e| e.to_string())?;

    Ok(LlmChatResponse {
        content: chat_resp
            .message
            .map(|m| m.content)
            .unwrap_or_default(),
        prompt_tokens: chat_resp.prompt_eval_count.unwrap_or(0),
        completion_tokens: chat_resp.eval_count.unwrap_or(0),
    })
}
