//! STT model lifecycle: status, download, load/unload, set default, delete,
//! transcribe.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use serde::Deserialize;
use tauri::{Emitter, Manager};

use crate::models::ModelState;
use crate::settings::SettingsState;

use super::{DatasetMcpServer, ReadFileParam};

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ModelVersionParam {
    version: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ModelLoadParam {
    #[serde(default)]
    version: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ModelStatusParam {
    /// Ask for the catalogue as the Models page renders it — descriptions, blob
    /// and HuggingFace URLs, accuracy/speed scores included. Costs a few thousand
    /// context tokens and answers a question no pipeline asks.
    #[serde(default)]
    full: bool,
}

/// The keys of a catalogue entry a caller chooses a model with: what it is, whether
/// it is on disk, what it costs and which languages it knows. Everything else on
/// `ModelVersionInfo` is page furniture.
const CHOOSABLE_KEYS: [&str; 5] = ["id", "label", "downloaded", "size_mb", "languages"];

#[tool_router(router = models_router, vis = "pub(crate)")]
impl DatasetMcpServer {
    #[tool(name = "model_status", description = "Get STT model status: downloaded models, loaded model, the user's preferred default (default_version), and an actionable 'hint' field telling you exactly what to do next. If hint says 'Call model_load', do NOT call model_download — the model is already downloaded. Always check model_status before model_download. Each model is trimmed to id/label/downloaded/size_mb/languages unless you pass full=true. Driving a dataset pipeline? Do not call this at all: dataset_full_report already reports the loaded model under stt_model.")]
    async fn model_status(&self, Parameters(param): Parameters<ModelStatusParam>) -> String {
        log::info!("[MCP] model_status: full={}", param.full);
        let state = self.app.state::<ModelState>();
        match crate::models::model_get_status(state).await {
            Ok(resp) => {
                let mut value =
                    serde_json::to_value(&resp).unwrap_or_else(|_| serde_json::json!({}));
                if !param.full {
                    if let Some(models) = value.get_mut("models").and_then(|v| v.as_array_mut()) {
                        for model in models.iter_mut() {
                            if let Some(obj) = model.as_object_mut() {
                                obj.retain(|k, _| CHOOSABLE_KEYS.contains(&k.as_str()));
                            }
                        }
                    }
                }
                serde_json::to_string_pretty(&value).unwrap_or_default()
            }
            Err(e) => serde_json::json!({"error": e}).to_string(),
        }
    }

    #[tool(name = "model_download", description = "Download an STT model by version ID. Use model_status first to see available versions. Download may take a while.")]
    async fn model_download(&self, Parameters(param): Parameters<ModelVersionParam>) -> Result<String, String> {
        log::info!("[MCP] model_download: version={}", param.version);
        let app = self.app.clone();
        let state = self.app.state::<ModelState>();
        let settings = self.app.state::<SettingsState>();
        let index = self.app.state::<crate::models::index::ModelIndexState>();
        crate::models::model_download_inner(app.clone(), &state, &settings, &index, param.version).await?;
        let _ = app.emit("model-status-changed", ());
        Ok(serde_json::json!({"status": "ok", "message": "Model downloaded successfully"}).to_string())
    }

    #[tool(name = "model_load", description = "Load a downloaded STT model for transcription. If version is empty, auto-selects the user's default model (see default_version in model_status; set it with model_set_default), preferring parakeet-v3 otherwise. Use model_status to see available versions.")]
    async fn model_load(&self, Parameters(param): Parameters<ModelLoadParam>) -> Result<String, String> {
        log::info!("[MCP] model_load: version={}", param.version);
        let state = self.app.state::<ModelState>();
        let version = if param.version.is_empty() {
            // Auto-select: the user's default first, then parakeet-v3, then the
            // first downloaded model.
            let default = state.default_version.lock().unwrap().clone();
            let statuses = state.download_status.lock().unwrap();
            let downloaded: Vec<&String> = statuses
                .iter()
                .filter(|(_, s)| **s == crate::models::ModelStatus::Downloaded)
                .map(|(v, _)| v)
                .collect();
            if downloaded.is_empty() {
                return Err("No models are downloaded. Use model_download first.".into());
            }
            if downloaded.contains(&&default) {
                default
            } else if downloaded.contains(&&"parakeet-v3".to_string()) {
                "parakeet-v3".to_string()
            } else {
                downloaded[0].clone()
            }
        } else {
            param.version
        };
        crate::models::load_model_core(&state, &version)?;
        let _ = self.app.emit("model-status-changed", ());
        Ok(serde_json::json!({"status": "ok", "loaded_version": version}).to_string())
    }

    #[tool(name = "model_set_default", description = "Set the user's preferred default STT model — the one auto-loaded when nothing is loaded (must be downloaded first). Pass an empty version to clear the preference and restore the built-in fallback.")]
    async fn model_set_default(&self, Parameters(param): Parameters<ModelVersionParam>) -> Result<String, String> {
        log::info!("[MCP] model_set_default: version={}", param.version);
        let state = self.app.state::<ModelState>();
        let settings = self.app.state::<SettingsState>();
        crate::models::set_default_version_core(&state, &settings, &param.version)?;
        let _ = self.app.emit("model-status-changed", ());
        Ok(serde_json::json!({"status": "ok", "default_version": param.version}).to_string())
    }

    #[tool(name = "model_unload", description = "Unload the currently active STT model, freeing memory.")]
    async fn model_unload(&self) -> Result<String, String> {
        log::info!("[MCP] model_unload");
        let state = self.app.state::<ModelState>();
        crate::models::unload_model_core(&state)?;
        let _ = self.app.emit("model-status-changed", ());
        Ok(serde_json::json!({"status": "ok", "message": "Model unloaded"}).to_string())
    }

    #[tool(name = "model_delete", description = "Delete a downloaded STT model to free disk space. Cannot delete a model that is currently loaded — unload it first. Deleting the default model clears that preference.")]
    async fn model_delete(&self, Parameters(param): Parameters<ModelVersionParam>) -> Result<String, String> {
        log::info!("[MCP] model_delete: version={}", param.version);
        let state = self.app.state::<ModelState>();
        let index = self.app.state::<crate::models::index::ModelIndexState>();
        let settings = self.app.state::<SettingsState>();
        crate::models::delete_model_core(&state, &index, &param.version)?;
        crate::models::clear_default_if_matches(&state, &settings, &param.version);
        let _ = self.app.emit("model-status-changed", ());
        Ok(serde_json::json!({"status": "ok", "deleted": param.version}).to_string())
    }

    #[tool(name = "model_transcribe", description = "Transcribe base64-encoded 16kHz mono WAV audio using the loaded STT model. Returns the transcript text. Requires a model to be loaded first (use model_load).")]
    async fn model_transcribe(&self, Parameters(param): Parameters<ReadFileParam>) -> Result<String, String> {
        // Reuse ReadFileParam but we only need the path field as wav_base64
        // Actually let's use a dedicated approach: treat 'path' as wav_base64
        log::info!("[MCP] model_transcribe: base64_len={}", param.path.len());
        let state = self.app.state::<ModelState>();
        let result = crate::models::model_transcribe(state, param.path).await?;
        Ok(serde_json::json!({"status": "ok", "transcript": result}).to_string())
    }
}
