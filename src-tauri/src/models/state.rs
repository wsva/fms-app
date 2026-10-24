//! What a model is in this build ([`ActiveModel`]), the managed [`ModelState`],
//! the DTOs the frontend and MCP read, and the persisted default/selection
//! preferences every auto-load path consults.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use serde::Serialize;
// Whisper GGUF (transcribe-cpp) is desktop-only; ONNX builds (e.g. the Android
// `stt` experiment) compile the engine out and reject its models at load time.
#[cfg(feature = "desktop")]
use transcribe_cpp::Session as CppSession;
use transcribe_rs::onnx::{
    canary::CanaryModel,
    cohere::CohereModel,
    gigaam::GigaAMModel,
    moonshine::{MoonshineModel, StreamingModel},
    parakeet::ParakeetModel,
    sense_voice::SenseVoiceModel,
};

use crate::models::catalog_stt::{self, EngineType};
use crate::settings::SettingsState;

#[derive(Clone, Serialize, PartialEq)]
pub enum ModelStatus {
    NotDownloaded,
    Downloading,
    Downloaded,
    Running,
    Stopped,
    Error(String),
}

/// Active model wrapper for all supported engine types
pub(crate) enum ActiveModel {
    #[cfg(feature = "desktop")]
    TranscribeCpp(CppSession),
    Parakeet(ParakeetModel),
    Moonshine(MoonshineModel),
    MoonshineStreaming(StreamingModel),
    SenseVoice(SenseVoiceModel),
    GigaAM(GigaAMModel),
    Canary(CanaryModel),
    Cohere(CohereModel),
}

pub struct ModelState {
    pub download_status: Mutex<HashMap<String, ModelStatus>>,
    pub download_progress: Arc<Mutex<DownloadProgress>>,
    pub selected_version: Mutex<String>,
    /// User's preferred default model, persisted in settings (`selected_model`).
    /// This is what the app loads when it needs a model and none is loaded yet.
    /// Empty until the user picks one via "Set as default".
    pub default_version: Mutex<String>,
    /// Empty while a transcription holds the model out (see
    /// `transcribe::ModelGuard`); `active_version` still names what it is.
    pub(crate) model: Mutex<Option<ActiveModel>>,
    pub active_version: Mutex<Option<String>>,
    pub cancel_flags: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Semaphore to serialize transcription requests and prevent race conditions
    /// where the model is temporarily unavailable during an ongoing transcription.
    pub(crate) transcribe_lock: Mutex<()>,
}

#[derive(Clone, Serialize)]
pub struct FileDownloadInfo {
    pub file: String,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub speed: u64,
    pub eta_seconds: Option<u64>,
}

#[derive(Clone, Serialize)]
pub struct DownloadProgress {
    pub files: Vec<FileDownloadInfo>,
    pub overall_bytes_downloaded: u64,
    pub overall_total_bytes: u64,
    pub speed: u64,
    pub eta_seconds: Option<u64>,
}

#[derive(Clone, Serialize)]
pub struct ModelVersionInfo {
    pub id: String,
    pub label: String,
    pub description: String,
    pub downloaded: bool,
    pub languages: Vec<String>,
    pub size_mb: u64,
    pub engine_type: String,
    pub blob_url: String,
    pub hf_repo_url: String,
    pub accuracy_score: f32,
    pub speed_score: f32,
    pub supports_translation: bool,
}

#[derive(Clone, Serialize)]
pub struct ModelStatusResponse {
    pub models: Vec<ModelVersionInfo>,
    pub selected_version: String,
    pub default_version: String,
    pub active_version: Option<String>,
    pub active_status: ModelStatus,
    pub download_progress: Option<DownloadProgress>,
    pub downloaded_versions: Vec<String>,
    pub hint: String,
}

impl ModelState {
    pub fn new() -> Self {
        let mut download_status = HashMap::new();
        for model_def in Self::available_models() {
            let status = if Self::downloaded_on_disk(model_def) {
                ModelStatus::Downloaded
            } else {
                ModelStatus::NotDownloaded
            };
            download_status.insert(model_def.id.to_string(), status);
        }

        let selected = catalog_stt::MODELS
            .iter()
            .find(|m| cfg!(feature = "desktop") || m.engine != EngineType::TranscribeCpp)
            .map(|m| m.id.to_string())
            .unwrap_or_else(|| catalog_stt::MODELS[0].id.to_string());

        Self {
            download_status: Mutex::new(download_status),
            download_progress: Arc::new(Mutex::new(DownloadProgress {
                files: Vec::new(),
                overall_bytes_downloaded: 0,
                overall_total_bytes: 0,
                speed: 0,
                eta_seconds: None,
            })),
            selected_version: Mutex::new(selected),
            default_version: Mutex::new(String::new()),
            model: Mutex::new(None),
            active_version: Mutex::new(None),
            cancel_flags: Mutex::new(HashMap::new()),
            transcribe_lock: Mutex::new(()),
        }
    }

    pub fn model_dir(version: &str) -> Option<std::path::PathBuf> {
        Some(crate::app_paths::data_subdir("models").join(version))
    }

    /// Whether this model's files are present in the (current) model directory.
    fn downloaded_on_disk(model_def: &catalog_stt::ModelDef) -> bool {
        let Some(dir) = Self::model_dir(model_def.id) else {
            return false;
        };
        if model_def.is_directory {
            dir.exists()
        } else {
            dir.join(model_def.id).exists()
        }
    }

    /// Re-check the model directory against the storage base established by
    /// [`crate::app_paths::init`]. [`Self::new`] runs at builder time, before that
    /// base is known, so its pre-init fallback path sees nothing — on Android/iOS
    /// every model would stay reported `NotDownloaded` (and `load_model_core`
    /// would refuse to load it) even though the files are on disk. Models with a
    /// download in flight are left untouched.
    pub fn reload(&self) {
        let mut statuses = self.download_status.lock().unwrap();
        for model_def in Self::available_models() {
            if statuses.get(model_def.id) == Some(&ModelStatus::Downloading) {
                continue;
            }
            let status = if Self::downloaded_on_disk(model_def) {
                ModelStatus::Downloaded
            } else {
                ModelStatus::NotDownloaded
            };
            statuses.insert(model_def.id.to_string(), status);
        }
    }

    /// Models loadable in this build. ONNX-only builds (e.g. the Android `stt`
    /// experiment) hide Whisper GGUF entries since transcribe-cpp is desktop-only.
    pub fn available_models() -> impl Iterator<Item = &'static catalog_stt::ModelDef> {
        catalog_stt::MODELS
            .iter()
            .filter(|def| cfg!(feature = "desktop") || def.engine != EngineType::TranscribeCpp)
    }

    /// Whether a model is present in the model directory (per our cached status).
    pub(crate) fn is_downloaded(&self, version: &str) -> bool {
        self.download_status
            .lock()
            .unwrap()
            .get(version)
            .map(|s| *s == ModelStatus::Downloaded)
            .unwrap_or(false)
    }

    /// Seed the persisted default model preference at startup. Values that no
    /// longer exist in this build's catalog (e.g. a desktop-only model id saved
    /// before switching to an ONNX-only build) are ignored, keeping the
    /// built-in fallback selection.
    pub fn apply_persisted_default(&self, version: &str) {
        if version.is_empty() {
            return;
        }
        if !Self::available_models().any(|def| def.id == version) {
            log::warn!("Ignoring unknown default model '{}' from settings", version);
            return;
        }
        log::info!("Restoring default model '{}' from settings", version);
        *self.default_version.lock().unwrap() = version.to_string();
        *self.selected_version.lock().unwrap() = version.to_string();
    }
}

/// Version to load when the app needs a model but none is loaded: the user's
/// default (if still downloaded), otherwise the session selection.
pub fn auto_load_version(state: &ModelState) -> String {
    let default = state.default_version.lock().unwrap().clone();
    if !default.is_empty() && state.is_downloaded(&default) {
        return default;
    }
    state.selected_version.lock().unwrap().clone()
}

/// Mark `version` as the user's preferred default model — in memory and in the
/// persisted settings, so it survives restarts and drives every auto-load path
/// (voice input, subtitle generation, MCP transcription).
///
/// Passing an empty string clears the preference and restores the built-in
/// fallback (first model loadable in this build).
pub fn set_default_version_core(
    state: &ModelState,
    settings: &SettingsState,
    version: &str,
) -> Result<(), String> {
    if version.is_empty() {
        let fallback = ModelState::available_models()
            .next()
            .map(|def| def.id.to_string())
            .unwrap_or_default();
        log::info!("Clearing default model, falling back to '{}'", fallback);
        *state.default_version.lock().unwrap() = String::new();
        *state.selected_version.lock().unwrap() = fallback;
        return settings.set_selected_model("");
    }

    let def = catalog_stt::find_model(version)
        .ok_or_else(|| format!("Unknown model version: {}", version))?;
    if !(cfg!(feature = "desktop") || def.engine != EngineType::TranscribeCpp) {
        return Err(format!("Model '{}' cannot be loaded in this build", version));
    }
    if !state.is_downloaded(version) {
        return Err(format!(
            "Model '{}' is not downloaded. Download it before setting it as the default.",
            version
        ));
    }

    log::info!("Default model set to '{}'", version);
    *state.default_version.lock().unwrap() = version.to_string();
    *state.selected_version.lock().unwrap() = version.to_string();
    settings.set_selected_model(version)
}

/// Forget the default preference when the model it pointed at was deleted.
pub fn clear_default_if_matches(state: &ModelState, settings: &SettingsState, version: &str) {
    let matches = state.default_version.lock().unwrap().as_str() == version;
    if matches {
        log::info!("Default model '{}' deleted, clearing the preference", version);
        *state.default_version.lock().unwrap() = String::new();
        let _ = settings.set_selected_model("");
    }
}
