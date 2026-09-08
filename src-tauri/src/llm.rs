/// LLM commands — communicates with a local Ollama instance via its HTTP API.
/// Ollama handles model management, inference, and GPU acceleration.
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::llm_model_list::{self, LlmModelDef};

const OLLAMA_BASE_URL: &str = "http://localhost:11434";

// ---------------------------------------------------------------------------
// Request / response types for Ollama API
// ---------------------------------------------------------------------------

#[derive(Serialize)]
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
struct ChatOptions {
    temperature: f32,
    num_predict: u32,
}

#[derive(Deserialize)]
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
    pub recommended: Vec<LlmModelDef>,
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

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Check if Ollama is running and reachable.
#[tauri::command]
pub async fn llm_check_connection() -> Result<bool, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| e.to_string())?;
    match client.get(OLLAMA_BASE_URL).send().await {
        Ok(resp) => Ok(resp.status().is_success()),
        Err(_) => Ok(false),
    }
}

/// List models installed in Ollama together with our recommended catalog.
#[tauri::command]
pub async fn llm_list_models() -> Result<LlmInstalledModelsResponse, String> {
    let client = ollama_client();
    let resp = client
        .get(format!("{}/api/tags", OLLAMA_BASE_URL))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let tags: OllamaTagsResponse = resp.json().await.map_err(|e| e.to_string())?;

    Ok(LlmInstalledModelsResponse {
        installed: tags.models,
        recommended: llm_model_list::MODELS.to_vec(),
    })
}

/// Pull a model into Ollama.  Emits `llm-pull-progress` events with status updates.
#[tauri::command]
pub async fn llm_pull_model(
    app: AppHandle,
    model: String,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3600)) // pulls can take a while
        .build()
        .map_err(|e| e.to_string())?;

    let body = serde_json::json!({ "name": model, "stream": true });

    let resp = client
        .post(format!("{}/api/pull", OLLAMA_BASE_URL))
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
pub async fn llm_delete_model(model: String) -> Result<(), String> {
    let client = ollama_client();
    let body = serde_json::json!({ "name": model });
    let resp = client
        .delete(format!("{}/api/delete", OLLAMA_BASE_URL))
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

/// Send a chat completion request to Ollama (non-streaming).
#[tauri::command]
pub async fn llm_chat(
    model: String,
    messages: Vec<ChatMessage>,
) -> Result<LlmChatResponse, String> {
    let client = ollama_client();
    let req = ChatRequest {
        model: &model,
        messages,
        stream: false,
        options: ChatOptions {
            temperature: 0.7,
            num_predict: 2048,
        },
    };

    let resp = client
        .post(format!("{}/api/chat", OLLAMA_BASE_URL))
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
