//! Language-model and voice surface: Ollama chat + model management, Edge TTS,
//! and the cross-device chat thread.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use serde::Deserialize;
use tauri::Manager;

use crate::ai;
use crate::edge_tts;
use crate::settings::SettingsState;

use super::{DatasetMcpServer, JsonValue};

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct LlmModelParam {
    model: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct LlmChatParam {
    model: String,
    messages: Vec<JsonValue>,
    #[serde(default)]
    temperature: Option<f32>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct TtsSynthesizeParam {
    text: String,
    voice: String,
    output_path: String,
    #[serde(default)]
    rate: String,
    #[serde(default)]
    volume: String,
    #[serde(default)]
    pitch: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ChatMessagesParam {
    /// Return only messages newer than this `created_at` cursor. Empty returns
    /// the newest page.
    #[serde(default)]
    after: String,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ChatSendParam {
    text: String,
    /// Absolute paths of files to attach; they must be readable by the PC.
    #[serde(default)]
    file_paths: Vec<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ChatSaveAttachmentParam {
    /// Attachment uuid, as reported by `chat_messages`.
    uuid: String,
    /// Absolute destination path on the PC; its directory must already exist.
    dest: String,
}

#[tool_router(router = ai_router, vis = "pub(crate)")]
impl DatasetMcpServer {
    #[tool(name = "llm_check_connection", description = "Check if Ollama is running and reachable. Returns true/false.")]
    async fn llm_check_connection(&self) -> String {
        log::info!("[MCP] llm_check_connection");
        let settings = self.app.state::<SettingsState>();
        match ai::llm::llm_check_connection(settings).await {
            Ok(connected) => serde_json::json!({"connected": connected}).to_string(),
            Err(e) => serde_json::json!({"connected": false, "error": e}).to_string(),
        }
    }

    #[tool(name = "llm_list_models", description = "List LLM models installed in Ollama plus our recommended catalog. Use llm_pull_model to download a recommended model.")]
    async fn llm_list_models(&self) -> Result<String, String> {
        log::info!("[MCP] llm_list_models");
        let settings = self.app.state::<SettingsState>();
        let result = ai::llm::llm_list_models(settings).await?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }

    #[tool(name = "llm_pull_model", description = "Pull/download a model into Ollama. Use llm_list_models to see recommended model names. May take a while.")]
    async fn llm_pull_model(&self, Parameters(param): Parameters<LlmModelParam>) -> Result<String, String> {
        log::info!("[MCP] llm_pull_model: model={}", param.model);
        let settings = self.app.state::<SettingsState>();
        ai::llm::llm_pull_model(self.app.clone(), settings, param.model).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Model pulled successfully"}).to_string())
    }

    #[tool(name = "llm_delete_model", description = "Delete a model from Ollama to free disk space.")]
    async fn llm_delete_model(&self, Parameters(param): Parameters<LlmModelParam>) -> Result<String, String> {
        log::info!("[MCP] llm_delete_model: model={}", param.model);
        let settings = self.app.state::<SettingsState>();
        ai::llm::llm_delete_model(settings, param.model.clone()).await?;
        Ok(serde_json::json!({"status": "ok", "deleted": param.model}).to_string())
    }

    #[tool(name = "llm_chat", description = "Send a chat completion request to Ollama. Pass model name, messages array [{role:'user',content:'...'}], and optional temperature. Returns the assistant response.")]
    async fn llm_chat(&self, Parameters(param): Parameters<LlmChatParam>) -> Result<String, String> {
        log::info!("[MCP] llm_chat: model={}, msgs={}", param.model, param.messages.len());
        let settings = self.app.state::<SettingsState>();
        let messages: Vec<ai::llm::ChatMessage> = param.messages.into_iter().map(|m| {
            let m: serde_json::Value = m.into();
            ai::llm::ChatMessage {
                role: m["role"].as_str().unwrap_or("user").to_string(),
                content: m["content"].as_str().unwrap_or("").to_string(),
            }
        }).collect();
        let result = ai::llm::llm_chat(settings, param.model, messages, param.temperature).await?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }

    #[tool(name = "tts_list_voices", description = "List all available Edge TTS voices. Returns name, short_name, locale, and gender for each voice.")]
    async fn tts_list_voices(&self) -> Result<String, String> {
        log::info!("[MCP] tts_list_voices");
        let voices = edge_tts::edge_tts_list_voices().await?;
        Ok(serde_json::to_string_pretty(&voices).unwrap_or_default())
    }

    #[tool(name = "tts_synthesize", description = "Synthesize text to audio using Edge TTS and save to a file. Pass text, voice (e.g. 'en-US-AriaNeural'), output_path, and optional rate/volume/pitch adjustments.")]
    async fn tts_synthesize(&self, Parameters(param): Parameters<TtsSynthesizeParam>) -> Result<String, String> {
        log::info!("[MCP] tts_synthesize: voice={}, text_len={}", param.voice, param.text.len());
        let args = edge_tts::TtsSynthesizeArgs {
            text: param.text,
            voice: param.voice,
            rate: if param.rate.is_empty() { None } else { Some(param.rate) },
            volume: if param.volume.is_empty() { None } else { Some(param.volume) },
            pitch: if param.pitch.is_empty() { None } else { Some(param.pitch) },
            output_path: param.output_path,
        };
        let result = edge_tts::edge_tts_synthesize(args).await?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }

    #[tool(name = "chat_messages", description = "Read the cross-device chat thread the PC stores for the user's paired devices (phone <-> PC). Returns messages newest-last with their attachments. Pass 'after' (a created_at from an earlier answer) to poll for new messages only.")]
    async fn chat_messages(&self, Parameters(param): Parameters<ChatMessagesParam>) -> Result<String, String> {
        log::info!("[MCP] chat_messages: after={} limit={:?}", param.after, param.limit);
        let settings = self.app.state::<SettingsState>();
        let messages = crate::ai::chat::list_messages(&settings, &param.after, param.limit.unwrap_or(200))?;
        let total = messages.len();
        Ok(serde_json::json!({"messages": messages, "total": total}).to_string())
    }

    #[tool(name = "chat_send", description = "Post a message into the cross-device chat thread as the PC (optionally attaching files by absolute path). Use it to hand something to the user's phone: a reminder, a generated file, the result of a longer job. The message appears in the Device Chat page on every device.")]
    async fn chat_send(&self, Parameters(param): Parameters<ChatSendParam>) -> Result<String, String> {
        log::info!("[MCP] chat_send: text={} files={}", param.text.chars().take(60).collect::<String>(), param.file_paths.len());
        let settings = self.app.state::<SettingsState>();
        let msg = crate::ai::chat::send_pc_message(&self.app, &settings, param.text, param.file_paths, None)?;
        Ok(serde_json::to_string_pretty(&msg).unwrap_or_default())
    }

    #[tool(name = "chat_save_attachment", description = "Copy one chat attachment (uuid from chat_messages) to a path on the PC so a file the user sent from their phone can be processed further (import it, open it, archive it). Returns the written path and size. The destination directory must already exist.")]
    async fn chat_save_attachment(&self, Parameters(param): Parameters<ChatSaveAttachmentParam>) -> Result<String, String> {
        log::info!("[MCP] chat_save_attachment: uuid={} dest={}", param.uuid, param.dest);
        let settings = self.app.state::<SettingsState>();
        let (src, att) = crate::ai::chat::attachment_file(&settings, &param.uuid)?;
        std::fs::copy(&src, &param.dest)
            .map_err(|e| format!("cannot write {}: {e}", param.dest))?;
        Ok(serde_json::json!({
            "status": "ok",
            "path": param.dest,
            "filename": att.filename,
            "mime": att.mime,
            "size": att.size
        })
        .to_string())
    }
}
