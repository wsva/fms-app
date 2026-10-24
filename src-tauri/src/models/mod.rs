//! STT model management: download, lifecycle, and transcription via
//! `transcribe-rs`, plus the download catalogs and the unified index of every
//! model on disk.
//!
//! The whole group is `stt`-gated (see `lib.rs`); `desktop` implies `stt`.
//!
//! `state` owns what a model is and the preferences that pick one, `engine` owns
//! load/unload/delete, `transcribe` owns running one, `download_stt` /
//! `download_hf` own the two download pipelines. This file keeps the
//! `#[tauri::command]` wrappers: they cannot live in a submodule, because
//! `generate_handler!` resolves each command's generated `__cmd__` macro at the
//! path it is given, and a `pub use` re-export carries only the function.

pub(crate) mod catalog;
pub(crate) mod catalog_stt;
pub(crate) mod download;
pub(crate) mod download_hf;
pub(crate) mod download_stt;
pub(crate) mod engine;
pub(crate) mod index;
pub(crate) mod state;
pub(crate) mod transcribe;

use base64::Engine;
use tauri::{AppHandle, Emitter, State};

use crate::models::index::ModelIndexState;
use crate::settings::SettingsState;

// Re-exports: the group root stays the facade every caller outside `models`
// names, even though the bodies live one level down.
pub use download_hf::{hf_downloaded_on_disk, hf_download_inner, hf_model_dir};
pub use download_stt::model_download_inner;
pub use engine::{cancel_download_core, delete_model_core, load_model_core, unload_model_core};
pub use state::{
    auto_load_version, clear_default_if_matches, set_default_version_core, DownloadProgress,
    FileDownloadInfo, ModelState, ModelStatus, ModelStatusResponse, ModelVersionInfo,
};
pub use transcribe::{
    transcribe_samples_detailed, transcribe_samples_text, transcribe_samples_word_level,
    DetailedTranscript,
};
// `transcribe_file`, `transcribe_file_word_level` and `transcribe_wav_bytes` are
// deliberately not re-exported: their callers (dataset pipeline, cue adjust, web
// service) are desktop-only, so an ONNX-only build would see the import as
// unused. Those three are named as `models::transcribe::<fn>`.

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn model_get_status(state: State<'_, ModelState>) -> Result<ModelStatusResponse, String> {
    let models: Vec<ModelVersionInfo> = ModelState::available_models()
        .map(|def| {
            let downloaded = state
                .download_status
                .lock()
                .unwrap()
                .get(def.id)
                .map(|s| *s == ModelStatus::Downloaded)
                .unwrap_or(false);
            ModelVersionInfo {
                id: def.id.to_string(),
                label: def.name.to_string(),
                description: def.description.to_string(),
                downloaded,
                languages: def.languages.iter().map(|s| s.to_string()).collect(),
                size_mb: def.size_mb,
                engine_type: def.engine.to_string(),
                blob_url: def.blob_url.to_string(),
                hf_repo_url: def.hf_repo_url.to_string(),
                accuracy_score: def.accuracy_score,
                speed_score: def.speed_score,
                supports_translation: def.supports_translation,
            }
        })
        .collect();

    let selected = state.selected_version.lock().unwrap().clone();
    let default_version = state.default_version.lock().unwrap().clone();
    let active = state.active_version.lock().unwrap().clone();

    let progress = state.download_progress.lock().unwrap().clone();
    let statuses = state.download_status.lock().unwrap();
    let is_downloading = statuses.values().any(|s| *s == ModelStatus::Downloading);

    // Collect downloaded model versions
    let downloaded_versions: Vec<String> = statuses
        .iter()
        .filter(|(_, s)| **s == ModelStatus::Downloaded)
        .map(|(v, _)| v.clone())
        .collect();
    drop(statuses);

    let active_status = if is_downloading {
        ModelStatus::Downloading
    } else if active.is_some() {
        ModelStatus::Running
    } else {
        ModelStatus::Stopped
    };

    // Generate actionable hint for AI agents
    let hint = if active.is_some() {
        format!("Model '{}' is loaded and ready for transcription.", active.as_ref().unwrap())
    } else if !downloaded_versions.is_empty() {
        let preferred = auto_load_version(&state);
        format!(
            "No model loaded. {} model(s) downloaded: {}. Call model_load with version='{}' (or empty for auto-select) to load one.",
            downloaded_versions.len(),
            downloaded_versions.join(", "),
            preferred
        )
    } else {
        "No models downloaded. Call model_download with a version ID first. Use model_list to see available models.".to_string()
    };

    Ok(ModelStatusResponse {
        models,
        selected_version: selected,
        default_version,
        active_version: active,
        active_status,
        download_progress: if is_downloading {
            Some(progress)
        } else {
            None
        },
        downloaded_versions,
        hint,
    })
}

#[tauri::command]
pub async fn model_select_version(
    state: State<'_, ModelState>,
    version: String,
) -> Result<(), String> {
    if catalog_stt::find_model(&version).is_none() {
        return Err(format!("Unknown model version: {}", version));
    }
    let mut sel = state.selected_version.lock().unwrap();
    *sel = version;
    Ok(())
}

#[tauri::command]
pub async fn model_set_default(
    app: AppHandle,
    state: State<'_, ModelState>,
    settings: State<'_, SettingsState>,
    version: String,
) -> Result<(), String> {
    set_default_version_core(&state, &settings, &version)?;
    let _ = app.emit("model-status-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn model_download(
    app: AppHandle,
    state: State<'_, ModelState>,
    settings: State<'_, SettingsState>,
    index: State<'_, ModelIndexState>,
    version: String,
) -> Result<(), String> {
    model_download_inner(app, &state, &settings, &index, version).await
}

#[tauri::command]
pub async fn model_download_hf(
    app: AppHandle,
    state: State<'_, ModelState>,
    index: State<'_, ModelIndexState>,
    id: String,
    provider: Option<String>,
) -> Result<(), String> {
    hf_download_inner(app, &state, &index, &id, provider.as_deref()).await
}

#[tauri::command]
pub async fn model_start(state: State<'_, ModelState>) -> Result<(), String> {
    load_model_core(&state, &state.selected_version.lock().unwrap().clone())?;
    Ok(())
}

#[tauri::command]
pub async fn model_stop(state: State<'_, ModelState>) -> Result<(), String> {
    unload_model_core(&state)
}

#[tauri::command]
pub async fn model_delete(
    state: State<'_, ModelState>,
    index: State<'_, ModelIndexState>,
    settings: State<'_, SettingsState>,
    version: String,
) -> Result<(), String> {
    delete_model_core(&state, &index, &version)?;
    clear_default_if_matches(&state, &settings, &version);
    Ok(())
}

#[tauri::command]
pub async fn model_cancel_download(
    state: State<'_, ModelState>,
    version: String,
) -> Result<(), String> {
    cancel_download_core(&state, &version)
}

/// Decode a base64 WAV payload into the 16 kHz mono samples every transcription
/// path below works on.
fn decode_wav_base64(wav_base64: &str) -> Result<Vec<f32>, String> {
    let wav_bytes = base64::engine::general_purpose::STANDARD
        .decode(wav_base64)
        .map_err(|e| format!("Invalid base64: {}", e))?;
    crate::audio::decode_wav_pcm(&wav_bytes)
}

#[tauri::command]
pub async fn model_transcribe(
    state: State<'_, ModelState>,
    wav_base64: String,
) -> Result<String, String> {
    log::debug!("model_transcribe: decoding WAV ({} bytes base64)", wav_base64.len());
    let samples = decode_wav_base64(&wav_base64)?;

    log::debug!("model_transcribe: starting transcription");
    let result = transcribe_samples_text(&state, &samples)?;
    log::debug!("model_transcribe: result length={} chars", result.len());
    Ok(result)
}

#[tauri::command]
pub async fn model_transcribe_detailed(
    state: State<'_, ModelState>,
    wav_base64: String,
) -> Result<DetailedTranscript, String> {
    let samples = decode_wav_base64(&wav_base64)?;
    transcribe_samples_detailed(&state, &samples)
}
