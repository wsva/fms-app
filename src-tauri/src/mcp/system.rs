//! App-level tools that act on the running app rather than on content: settings,
//! auth, logs, OCR, screenshot capture and UI control.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use serde::Deserialize;
use tauri::{Emitter, Manager};

use crate::auth;
use crate::capture;
use crate::logger::LogBuffer;
use crate::ocr::{self, OcrState};
use crate::settings::SettingsState;

use super::{DatasetMcpServer, JsonValue};

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct SettingsSetParam {
    settings: JsonValue,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct OcrRecognizeParam {
    image_base64: String,
    #[serde(default)]
    lang: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct LogHistoryParam {
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct AppNavigateParam {
    /// Target tab id, e.g. "cards", "dictation", "wiki", "llm-chat".
    tab: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct AppOpenReviewParam {
    /// Optional card dataset to preselect in the review/quiz panel.
    #[serde(default)]
    dataset_uuid: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct AppStartDictationParam {
    #[serde(default)]
    dataset_uuid: Option<String>,
    #[serde(default)]
    media_uuid: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct AppNotifyParam {
    message: String,
}

#[tool_router(router = system_router, vis = "pub(crate)")]
impl DatasetMcpServer {
    #[tool(name = "settings_get", description = "Get current app settings: model directory, datasets directory, books directory, recordings directory, and other configuration.")]
    async fn settings_get(&self) -> String {
        log::info!("[MCP] settings_get");
        let state = self.app.state::<SettingsState>();
        let settings = state.settings.lock().unwrap().clone();
        serde_json::to_string_pretty(&settings).unwrap_or_default()
    }

    #[tool(name = "settings_set", description = "Update app settings. Pass a JSON object with the settings fields to update. Use settings_get first to see current values. Fields: model_dir, recordings_dir, datasets_dir, books_dir, selected_model, model_unload_timeout, onboarding_completed.")]
    async fn settings_set(&self, Parameters(param): Parameters<SettingsSetParam>) -> Result<String, String> {
        log::info!("[MCP] settings_set");
        let state = self.app.state::<SettingsState>();
        let new_settings: crate::settings::AppSettings = serde_json::from_value(param.settings.into())
            .map_err(|e| format!("Invalid settings JSON: {}", e))?;
        let ws_dir = state.workspace_dir.lock().unwrap().clone();
        crate::settings::SettingsState::save(&new_settings, ws_dir.as_ref())?;
        {
            let mut s = state.settings.lock().unwrap();
            *s = new_settings;
        }
        let _ = self.app.emit("settings-changed", ());
        Ok(serde_json::json!({"status": "ok", "message": "Settings updated"}).to_string())
    }

    #[tool(name = "auth_status", description = "Get the currently logged-in user. Returns null if not logged in. Use auth_login to log in.")]
    async fn auth_status(&self) -> String {
        log::info!("[MCP] auth_status");
        let state = self.app.state::<SettingsState>();
        match auth::auth_get_user(state).await {
            Ok(Some(user)) => serde_json::json!({"logged_in": true, "user": user}).to_string(),
            Ok(None) => serde_json::json!({"logged_in": false}).to_string(),
            Err(e) => serde_json::json!({"logged_in": false, "error": e}).to_string(),
        }
    }

    #[tool(name = "ocr_recognize", description = "Recognize text in an image using Tesseract OCR. Pass image as base64 (with or without data URL prefix) and optional language code (default: 'eng'). Use ocr_list_languages to see available languages.")]
    async fn ocr_recognize(&self, Parameters(param): Parameters<OcrRecognizeParam>) -> Result<String, String> {
        log::info!("[MCP] ocr_recognize: lang={:?}", param.lang);
        let state = self.app.state::<OcrState>();
        let lang = if param.lang.is_empty() { None } else { Some(param.lang) };
        let text = ocr::ocr_recognize(state, param.image_base64, lang).await?;
        Ok(serde_json::json!({"status": "ok", "text": text}).to_string())
    }

    #[tool(name = "ocr_list_languages", description = "List available Tesseract OCR languages installed on the system. Returns language codes that can be used with ocr_recognize (e.g., 'eng', 'deu', 'eng+deu' for multi-language).")]
    async fn ocr_list_languages(&self) -> Result<String, String> {
        log::info!("[MCP] ocr_list_languages");
        let state = self.app.state::<OcrState>();
        let langs = ocr::ocr_list_languages(state).await?;
        Ok(serde_json::json!({"status": "ok", "languages": langs}).to_string())
    }

    #[tool(name = "capture_screenshot", description = "Minimize the app window and capture a screenshot using the native snipping tool (Windows) or full-screen capture (other platforms). Returns a data:image/png;base64 URL.")]
    async fn capture_screenshot(&self) -> Result<String, String> {
        log::info!("[MCP] capture_screenshot");
        let result = capture::capture_screenshot(self.app.clone()).await?;
        Ok(serde_json::json!({"status": "ok", "image_url": result}).to_string())
    }

    #[tool(name = "log_get_history", description = "Get recent application log entries. Useful for debugging or checking what happened during operations. Returns up to 1000 entries.")]
    async fn log_get_history(&self, Parameters(param): Parameters<LogHistoryParam>) -> String {
        let buffer = self.app.state::<LogBuffer>();
        let mut entries = buffer.get_history();
        if let Some(limit) = param.limit {
            if limit < entries.len() {
                entries = entries[entries.len() - limit..].to_vec();
            }
        }
        serde_json::to_string_pretty(&entries).unwrap_or_default()
    }

    #[tool(name = "log_clear", description = "Clear the application log buffer. The on-disk log file keeps the history and stays readable via log_read_file_history.")]
    async fn log_clear(&self) -> String {
        let buffer = self.app.state::<LogBuffer>();
        buffer.clear();
        serde_json::json!({"status": "ok", "message": "Logs cleared"}).to_string()
    }

    #[tool(name = "log_get_file_path", description = "Get the path of the persistent on-disk log file (with size-based rotation). The in-memory buffer behind log_get_history only keeps the latest 1000 entries and is lost on restart; this file survives both.")]
    async fn log_get_file_path(&self) -> String {
        log::info!("[MCP] log_get_file_path");
        let path = crate::logger::LogBuffer::file_path();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        serde_json::json!({"status": "ok", "path": path.to_string_lossy(), "size_bytes": size}).to_string()
    }

    #[tool(name = "log_read_file_history", description = "Read archived log entries from the persistent log file (oldest first, includes the rotated backup). Survives app restarts and log_clear, unlike log_get_history. Optional 'limit' returns only the newest N entries.")]
    async fn log_read_file_history(&self, Parameters(param): Parameters<LogHistoryParam>) -> String {
        log::info!("[MCP] log_read_file_history: limit={:?}", param.limit);
        let buffer = self.app.state::<LogBuffer>();
        let entries = buffer.read_file_history(param.limit);
        serde_json::to_string_pretty(&entries).unwrap_or_default()
    }

    #[tool(name = "app_navigate", description = "Switch the app to another page/tab. Valid tabs: dictation, read-book, read-aloud, cards, studio, workflow, datasets-sync, devices-hub, simple-words, models, edge-tts, llm-chat, chat, ocr, wiki, workspaces, logs, settings. Use this to bring the user to the right screen before/while acting. The agent itself has no tab — it lives in a dock always available on every screen.")]
    async fn app_navigate(&self, Parameters(param): Parameters<AppNavigateParam>) -> Result<String, String> {
        const VALID: [&str; 18] = [
            "dictation", "read-book", "read-aloud", "cards", "studio", "workflow", "datasets-sync", "devices-hub", "simple-words",
            "models", "edge-tts", "llm-chat", "chat", "ocr", "wiki", "workspaces", "logs", "settings",
        ];
        let tab = param.tab.trim();
        if tab == "agent" {
            return Err(
                "the agent has no page — it lives in a dock available on every screen; navigate to the page you want the user to see instead.".to_string(),
            );
        }
        if !VALID.contains(&tab) {
            return Err(format!(
                "unknown tab '{}'. Valid tabs: {}",
                tab,
                VALID.join(", ")
            ));
        }
        log::info!("[MCP] app_navigate: tab={}", tab);
        self.app
            .emit("agent-action", serde_json::json!({ "type": "navigate", "tab": tab }))
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "ok": true, "action": "navigate", "tab": tab }).to_string())
    }

    #[tool(name = "app_open_review", description = "Open the interactive Cards review/quiz panel. Optionally preselect a card dataset by uuid. Use this when the user wants the visual quiz UI (as opposed to answering inline in the chat).")]
    async fn app_open_review(&self, Parameters(param): Parameters<AppOpenReviewParam>) -> Result<String, String> {
        log::info!("[MCP] app_open_review: dataset={:?}", param.dataset_uuid);
        let payload = serde_json::json!({
            "type": "open-review",
            "dataset_uuid": param.dataset_uuid,
        });
        self.app.emit("agent-action", payload).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({
            "ok": true,
            "action": "open-review",
            "dataset_uuid": param.dataset_uuid,
        }).to_string())
    }

    #[tool(name = "app_start_dictation", description = "Open the Dictation page and optionally select a dataset/media item to start practicing.")]
    async fn app_start_dictation(&self, Parameters(param): Parameters<AppStartDictationParam>) -> Result<String, String> {
        log::info!("[MCP] app_start_dictation: dataset={:?} media={:?}", param.dataset_uuid, param.media_uuid);
        let payload = serde_json::json!({
            "type": "start-dictation",
            "dataset_uuid": param.dataset_uuid,
            "media_uuid": param.media_uuid,
        });
        self.app.emit("agent-action", payload).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({
            "ok": true,
            "action": "start-dictation",
            "dataset_uuid": param.dataset_uuid,
            "media_uuid": param.media_uuid,
        }).to_string())
    }

    #[tool(name = "app_notify", description = "Show a toast notification to the user in the app UI.")]
    async fn app_notify(&self, Parameters(param): Parameters<AppNotifyParam>) -> Result<String, String> {
        log::info!("[MCP] app_notify: {}", param.message);
        self.app
            .emit("agent-action", serde_json::json!({ "type": "notify", "message": param.message }))
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "ok": true, "action": "notify", "message": param.message }).to_string())
    }

    #[tool(name = "app_get_ui_state", description = "Read the app's current UI state (active tab, selected dataset). Call this before navigating or acting so you know where the user currently is.")]
    async fn app_get_ui_state(&self) -> String {
        let state = self.app.state::<crate::ai::agent_acp::AcpClientState>();
        let ui = state.ui_state();
        serde_json::json!({
            "ok": true,
            "connected": state.is_connected(),
            "active_tab": ui.get("active_tab").cloned().unwrap_or(serde_json::Value::Null),
            "active_dataset_uuid": ui.get("active_dataset_uuid").cloned().unwrap_or(serde_json::Value::Null),
            "updated_at": ui.get("updated_at").cloned().unwrap_or(serde_json::Value::Null),
        })
        .to_string()
    }
}
