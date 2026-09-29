use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use base64::Engine;
use serde::Serialize;
use tauri::{AppHandle, State};
use transcribe_cpp::{Model as CppModel, RunOptions as CppRunOptions, Session as CppSession};
use transcribe_rs::onnx::{
    canary::CanaryModel,
    cohere::CohereModel,
    gigaam::GigaAMModel,
    moonshine::{MoonshineModel, MoonshineVariant, StreamingModel},
    parakeet::{ParakeetModel, ParakeetParams, TimestampGranularity},
    sense_voice::SenseVoiceModel,
    Quantization,
};
use transcribe_rs::{SpeechModel, TranscribeOptions, TranscriptionResult, TranscriptionSegment};

use crate::model_list_stt::{self, EngineType};
use crate::model_index::{self, ModelIndexEntry, ModelIndexState};
use crate::settings::SettingsState;

// ---------------------------------------------------------------------------
// Constants for chunked transcription of long audio files
// ---------------------------------------------------------------------------

/// Maximum audio duration (seconds) per STT chunk. Parakeet v3 struggles with
/// audio longer than ~4-5 minutes, so we split long files into overlapping chunks.
const MAX_CHUNK_DURATION_SECS: f64 = 240.0; // 4 minutes

/// Overlap between consecutive chunks (seconds). Words near chunk boundaries
/// may be missed, so overlap ensures they're captured in at least one chunk.
const CHUNK_OVERLAP_SECS: f64 = 15.0;

/// Audio sample rate used by decode_to_pcm (16 kHz).
const STT_SAMPLE_RATE: u32 = 16000;

// ---------------------------------------------------------------------------
// State types
// ---------------------------------------------------------------------------

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
enum ActiveModel {
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
    model: Mutex<Option<ActiveModel>>,
    pub active_version: Mutex<Option<String>>,
    pub cancel_flags: Mutex<HashMap<String, Arc<AtomicBool>>>,
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
    pub active_version: Option<String>,
    pub active_status: ModelStatus,
    pub download_progress: Option<DownloadProgress>,
    pub downloaded_versions: Vec<String>,
    pub hint: String,
}

impl ModelState {
    pub fn new() -> Self {
        let mut download_status = HashMap::new();
        for model_def in model_list_stt::MODELS {
            let dir = Self::model_dir(model_def.id);
            let downloaded = if model_def.is_directory {
                dir.as_ref().map(|d| d.exists()).unwrap_or(false)
            } else {
                dir.as_ref()
                    .map(|d| {
                        let file_path = d.join(model_def.id);
                        file_path.exists()
                    })
                    .unwrap_or(false)
            };
            let status = if downloaded {
                ModelStatus::Downloaded
            } else {
                ModelStatus::NotDownloaded
            };
            download_status.insert(model_def.id.to_string(), status);
        }

        let selected = model_list_stt::MODELS
            .first()
            .map(|m| m.id.to_string())
            .unwrap();

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
            model: Mutex::new(None),
            active_version: Mutex::new(None),
            cancel_flags: Mutex::new(HashMap::new()),
        }
    }

    pub fn model_dir(version: &str) -> Option<std::path::PathBuf> {
        dirs::data_dir().map(|d| d.join("fms-app").join("models").join(version))
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn model_get_status(state: State<'_, ModelState>) -> Result<ModelStatusResponse, String> {
    let models: Vec<ModelVersionInfo> = model_list_stt::MODELS
        .iter()
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
        let preferred = if downloaded_versions.contains(&"parakeet-v3".to_string()) {
            "parakeet-v3"
        } else {
            &downloaded_versions[0]
        };
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
    if model_list_stt::find_model(&version).is_none() {
        return Err(format!("Unknown model version: {}", version));
    }
    let mut sel = state.selected_version.lock().unwrap();
    *sel = version;
    Ok(())
}

// ---------------------------------------------------------------------------
// Download logic
// ---------------------------------------------------------------------------

pub async fn model_download_inner(
    app: AppHandle,
    state: &ModelState,
    _settings: &SettingsState,
    index: &ModelIndexState,
    version: String,
) -> Result<(), String> {
    let def = model_list_stt::find_model(&version)
        .ok_or_else(|| format!("Unknown model version: {}", version))?;

    log::info!("Starting download of model '{}' ({} MB)", version, def.size_mb);
    {
        let statuses = state.download_status.lock().unwrap();
        if statuses.values().any(|s| *s == ModelStatus::Downloading) {
            return Err("A download is already in progress".into());
        }
        if statuses.get(&version) == Some(&ModelStatus::Downloaded) {
            return Err("Model already downloaded".into());
        }
    }

    {
        let mut s = state.download_status.lock().unwrap();
        s.insert(version.clone(), ModelStatus::Downloading);
    }

    let model_dir = ModelState::model_dir(&version)
        .ok_or_else(|| "Could not determine model directory".to_string())?;
    std::fs::create_dir_all(&model_dir).map_err(|e| e.to_string())?;

    // Initialize download progress
    {
        let mut p = state.download_progress.lock().unwrap();
        p.files = vec![FileDownloadInfo {
            file: def.name.to_string(),
            bytes_downloaded: 0,
            total_bytes: Some(def.size_mb * 1024 * 1024),
            speed: 0,
            eta_seconds: None,
        }];
        p.overall_total_bytes = def.size_mb * 1024 * 1024;
        p.overall_bytes_downloaded = 0;
    }

    let cancel_flag = Arc::new(AtomicBool::new(false));
    {
        let mut flags = state.cancel_flags.lock().unwrap();
        flags.insert(version.clone(), cancel_flag.clone());
    }

    match crate::model_download::download_blob_model(
        &app,
        &state.download_progress,
        &model_dir,
        def.blob_url,
        def.sha256,
        def.is_directory,
        &cancel_flag,
        &version,
    )
    .await
    {
        Ok(()) => {}
        Err(e) => {
            log::error!("Model download failed for '{}': {}", version, e);
            let mut flags = state.cancel_flags.lock().unwrap();
            flags.remove(&version);
            let mut s = state.download_status.lock().unwrap();
            s.insert(version.clone(), ModelStatus::Error(e.clone()));
            return Err(e);
        }
    }

    {
        let mut s = state.download_status.lock().unwrap();
        s.insert(version.clone(), ModelStatus::Downloaded);
    }

    // Update unified model index
    let root = model_index::model_root();
    let files = model_index::collect_files(&model_dir, &root);
    let key = model_index::make_key("stt", &version);
    let entry = ModelIndexEntry {
        model_type: "stt".to_string(),
        id: version.clone(),
        name: def.name.to_string(),
        downloaded_at: Some(chrono::Utc::now().to_rfc3339()),
        provider: Some("cdn".to_string()),
        files,
    };
    if let Err(e) = model_index::upsert_entry(index, key, entry) {
        log::warn!("Failed to update model index after download: {}", e);
    }

    log::info!("Model '{}' downloaded successfully", version);
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

// ---------------------------------------------------------------------------
// Core logic (callable from managers without Tauri State)
// ---------------------------------------------------------------------------

pub fn load_model_core(state: &ModelState, version: &str) -> Result<String, String> {
    log::info!("Loading model '{}'", version);
    {
        let active = state.active_version.lock().unwrap();
        if active.is_some() {
            return Err("A model is already running".into());
        }
    }

    let def = model_list_stt::find_model(version)
        .ok_or_else(|| format!("Unknown model version: {}", version))?;

    {
        let statuses = state.download_status.lock().unwrap();
        if statuses.get(version) != Some(&ModelStatus::Downloaded) {
            return Err("Selected model is not downloaded yet".into());
        }
    }

    let model_dir = ModelState::model_dir(version)
        .ok_or_else(|| "Could not determine model directory".to_string())?;

    let active_model = match def.engine {
        EngineType::TranscribeCpp => {
            let model_path = model_dir.join(def.id);
            let model = CppModel::load(&model_path)
                .map_err(|e| format!("Failed to load Whisper model: {}", e))?;
            let session = model
                .session()
                .map_err(|e| format!("Failed to create session: {}", e))?;
            ActiveModel::TranscribeCpp(session)
        }
        EngineType::Parakeet => {
            let m = ParakeetModel::load(&model_dir, &Quantization::Int8)
                .map_err(|e| format!("Failed to load Parakeet model: {}", e))?;
            ActiveModel::Parakeet(m)
        }
        EngineType::Moonshine => {
            let m = MoonshineModel::load(&model_dir, MoonshineVariant::Base, &Quantization::default())
                .map_err(|e| format!("Failed to load Moonshine model: {}", e))?;
            ActiveModel::Moonshine(m)
        }
        EngineType::MoonshineStreaming => {
            let m = StreamingModel::load(&model_dir, 0, &Quantization::default())
                .map_err(|e| format!("Failed to load Moonshine Streaming model: {}", e))?;
            ActiveModel::MoonshineStreaming(m)
        }
        EngineType::SenseVoice => {
            let m = SenseVoiceModel::load(&model_dir, &Quantization::Int8)
                .map_err(|e| format!("Failed to load SenseVoice model: {}", e))?;
            ActiveModel::SenseVoice(m)
        }
        EngineType::GigaAM => {
            let m = GigaAMModel::load(&model_dir, &Quantization::Int8)
                .map_err(|e| format!("Failed to load GigaAM model: {}", e))?;
            ActiveModel::GigaAM(m)
        }
        EngineType::Canary => {
            let m = CanaryModel::load(&model_dir, &Quantization::Int8)
                .map_err(|e| format!("Failed to load Canary model: {}", e))?;
            ActiveModel::Canary(m)
        }
        EngineType::Cohere => {
            let m = CohereModel::load(&model_dir, &Quantization::Int8)
                .map_err(|e| format!("Failed to load Cohere model: {}", e))?;
            ActiveModel::Cohere(m)
        }
    };

    {
        let mut m = state.model.lock().unwrap();
        *m = Some(active_model);
    }
    {
        let mut a = state.active_version.lock().unwrap();
        *a = Some(version.to_string());
    }

    log::info!("Model '{}' loaded successfully", version);
    Ok(version.to_string())
}

pub fn unload_model_core(state: &ModelState) -> Result<(), String> {
    let active_name = {
        let active = state.active_version.lock().unwrap();
        match active.as_deref() {
            Some(name) => name.to_string(),
            None => return Err("No model is running".into()),
        }
    };
    log::info!("Unloading model '{}'", active_name);

    {
        let mut m = state.model.lock().unwrap();
        *m = None;
    }
    {
        let mut a = state.active_version.lock().unwrap();
        *a = None;
    }

    Ok(())
}

pub fn delete_model_core(state: &ModelState, index: &ModelIndexState, version: &str) -> Result<(), String> {
    log::info!("Deleting model '{}'", version);
    {
        let active = state.active_version.lock().unwrap();
        if active.as_deref() == Some(version) {
            return Err("Cannot delete a model that is currently running. Stop it first.".into());
        }
    }

    let model_dir = ModelState::model_dir(version)
        .ok_or_else(|| "Could not determine model directory".to_string())?;

    if model_dir.exists() {
        std::fs::remove_dir_all(&model_dir)
            .map_err(|e| format!("Failed to delete model directory: {}", e))?;
    }

    {
        let mut s = state.download_status.lock().unwrap();
        s.insert(version.to_string(), ModelStatus::NotDownloaded);
    }

    // Remove from unified model index
    let key = model_index::make_key("stt", version);
    if let Err(e) = model_index::remove_entry(index, &key) {
        log::warn!("Failed to update model index after delete: {}", e);
    }

    Ok(())
}

pub fn cancel_download_core(state: &ModelState, version: &str) -> Result<(), String> {
    let flag = {
        let flags = state.cancel_flags.lock().unwrap();
        flags.get(version).cloned()
    };
    if let Some(flag) = flag {
        flag.store(true, Ordering::Relaxed);
        Ok(())
    } else {
        Err("No active download to cancel".into())
    }
}

// ---------------------------------------------------------------------------
// Tauri commands (thin wrappers around core functions)
// ---------------------------------------------------------------------------

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
    version: String,
) -> Result<(), String> {
    delete_model_core(&state, &index, &version)
}

#[tauri::command]
pub async fn model_cancel_download(
    state: State<'_, ModelState>,
    version: String,
) -> Result<(), String> {
    cancel_download_core(&state, &version)
}

#[tauri::command]
pub async fn model_transcribe(
    state: State<'_, ModelState>,
    wav_base64: String,
) -> Result<String, String> {
    log::debug!("model_transcribe: decoding WAV ({} bytes base64)", wav_base64.len());
    let wav_bytes = base64::engine::general_purpose::STANDARD
        .decode(&wav_base64)
        .map_err(|e| format!("Invalid base64: {}", e))?;

    let samples = parse_wav_pcm(&wav_bytes)?;

    let mut model = {
        let mut m = state.model.lock().unwrap();
        m.take().ok_or_else(|| "No model is loaded".to_string())?
    };

    let result = match &mut model {
        ActiveModel::TranscribeCpp(session) => {
            let transcript = session
                .run(&samples, &CppRunOptions::default())
                .map_err(|e| format!("Whisper transcription failed: {}", e))?;
            transcript.text
        }
        ActiveModel::Parakeet(m) => m
            .transcribe(&samples, &TranscribeOptions::default())
            .map_err(|e| format!("Parakeet transcription failed: {}", e))?
            .text,
        ActiveModel::Moonshine(m) => m
            .transcribe(&samples, &TranscribeOptions::default())
            .map_err(|e| format!("Moonshine transcription failed: {}", e))?
            .text,
        ActiveModel::MoonshineStreaming(m) => m
            .transcribe(&samples, &TranscribeOptions::default())
            .map_err(|e| format!("Moonshine Streaming transcription failed: {}", e))?
            .text,
        ActiveModel::SenseVoice(m) => m
            .transcribe(&samples, &TranscribeOptions::default())
            .map_err(|e| format!("SenseVoice transcription failed: {}", e))?
            .text,
        ActiveModel::GigaAM(m) => m
            .transcribe(&samples, &TranscribeOptions::default())
            .map_err(|e| format!("GigaAM transcription failed: {}", e))?
            .text,
        ActiveModel::Canary(m) => m
            .transcribe(&samples, &TranscribeOptions::default())
            .map_err(|e| format!("Canary transcription failed: {}", e))?
            .text,
        ActiveModel::Cohere(m) => m
            .transcribe(&samples, &TranscribeOptions::default())
            .map_err(|e| format!("Cohere transcription failed: {}", e))?
            .text,
    };

    {
        let mut m = state.model.lock().unwrap();
        *m = Some(model);
    }

    log::debug!("model_transcribe: result length={} chars", result.len());
    Ok(result)
}

// ---------------------------------------------------------------------------
// WAV parser (16-bit PCM, 16kHz mono)
// ---------------------------------------------------------------------------

fn parse_wav_pcm(data: &[u8]) -> Result<Vec<f32>, String> {
    if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err("Invalid WAV file".into());
    }

    let mut pos = 12;
    let mut channels: u16 = 0;
    let mut sample_rate: u32 = 0;
    let mut bits_per_sample: u16 = 0;
    let mut pcm_data: Option<&[u8]> = None;

    while pos + 8 <= data.len() {
        let chunk_id = &data[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([
            data[pos + 4],
            data[pos + 5],
            data[pos + 6],
            data[pos + 7],
        ]) as usize;
        pos += 8;

        if chunk_id == b"fmt " {
            if chunk_size < 16 {
                return Err("Invalid fmt chunk".into());
            }
            let format_tag = u16::from_le_bytes([data[pos], data[pos + 1]]);
            if format_tag != 1 {
                return Err("Only PCM WAV files are supported".into());
            }
            channels = u16::from_le_bytes([data[pos + 2], data[pos + 3]]);
            sample_rate = u32::from_le_bytes([
                data[pos + 4],
                data[pos + 5],
                data[pos + 6],
                data[pos + 7],
            ]);
            bits_per_sample = u16::from_le_bytes([data[pos + 14], data[pos + 15]]);
        } else if chunk_id == b"data" {
            let end = (pos + chunk_size).min(data.len());
            pcm_data = Some(&data[pos..end]);
        }

        pos += chunk_size;
    }

    if sample_rate != 16000 {
        return Err(format!("Expected 16kHz audio, got {}Hz", sample_rate));
    }
    if channels != 1 {
        return Err(format!("Expected mono audio, got {} channels", channels));
    }

    let raw = pcm_data.ok_or("No data chunk in WAV file")?;

    match bits_per_sample {
        16 => Ok(raw
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect()),
        32 => Ok(raw
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()),
        other => Err(format!("Unsupported bit depth: {}", other)),
    }
}

// ---------------------------------------------------------------------------
// File transcription (used by dataset module)
// ---------------------------------------------------------------------------

/// Transcribe an audio file for subtitle generation.
///
/// ## Long Audio Handling (Parakeet only)
/// Parakeet models have a ~4-5 minute limit. For Parakeet, long audio is
/// automatically split into overlapping chunks and results are combined.
/// Other models (Whisper, Moonshine, etc.) can handle longer audio natively.
pub(crate) fn transcribe_file(
    state: &ModelState,
    file_path: &Path,
) -> Result<transcribe_rs::TranscriptionResult, String> {
    log::info!("transcribe_file: {}", file_path.display());
    let samples = crate::audio::decode_to_pcm(file_path)?;

    let total_duration_secs = samples.len() as f64 / STT_SAMPLE_RATE as f64;
    log::info!("Audio duration: {:.1}s ({:.1} min)", total_duration_secs, total_duration_secs / 60.0);

    // Check if model is loaded
    {
        let m = state.model.lock().unwrap();
        if m.is_none() {
            return Err("No model is loaded".to_string());
        }
    }

    // For Parakeet with long audio, use chunked transcription
    let is_parakeet = {
        let m = state.model.lock().unwrap();
        matches!(&*m, Some(ActiveModel::Parakeet(_)))
    };

    if is_parakeet && total_duration_secs > MAX_CHUNK_DURATION_SECS {
        log::info!(
            "Parakeet: audio too long (>{:.0}s), splitting into chunks",
            MAX_CHUNK_DURATION_SECS
        );
        return transcribe_file_chunked(state, &samples, total_duration_secs);
    }

    // Single-pass transcription for short audio or non-Parakeet models
    transcribe_file_pass(state, &samples, 0.0)
}

/// Run a single pass of text transcription on a sample slice.
/// `time_offset_secs` is added to all segment timestamps (for chunked processing).
fn transcribe_file_pass(
    state: &ModelState,
    samples: &[f32],
    time_offset_secs: f64,
) -> Result<TranscriptionResult, String> {
    let mut model = {
        let mut m = state.model.lock().unwrap();
        m.take().ok_or_else(|| "No model is loaded".to_string())?
    };

    let result = match &mut model {
        ActiveModel::TranscribeCpp(session) => {
            let transcript = session
                .run(samples, &CppRunOptions::default())
                .map_err(|e| format!("Whisper transcription failed: {}", e))?;
            let segments: Vec<TranscriptionSegment> = if transcript.segments.is_empty() {
                Vec::new()
            } else {
                transcript
                    .segments
                    .iter()
                    .map(|s| TranscriptionSegment {
                        start: s.t0_ms as f32 / 1000.0 + time_offset_secs as f32,
                        end: s.t1_ms as f32 / 1000.0 + time_offset_secs as f32,
                        text: s.text.clone(),
                    })
                    .collect()
            };
            TranscriptionResult {
                text: transcript.text,
                segments: if segments.is_empty() { None } else { Some(segments) },
            }
        }
        ActiveModel::Parakeet(m) => {
            let mut r = m
                .transcribe(samples, &TranscribeOptions::default())
                .map_err(|e| format!("Parakeet transcription failed: {}", e))?;
            if time_offset_secs > 0.0 {
                if let Some(ref mut segs) = r.segments {
                    for seg in segs.iter_mut() {
                        seg.start += time_offset_secs as f32;
                        seg.end += time_offset_secs as f32;
                    }
                }
            }
            r
        }
        ActiveModel::Moonshine(m) => m
            .transcribe(samples, &TranscribeOptions::default())
            .map_err(|e| format!("Moonshine transcription failed: {}", e))?,
        ActiveModel::MoonshineStreaming(m) => m
            .transcribe(samples, &TranscribeOptions::default())
            .map_err(|e| format!("Moonshine Streaming transcription failed: {}", e))?,
        ActiveModel::SenseVoice(m) => m
            .transcribe(samples, &TranscribeOptions::default())
            .map_err(|e| format!("SenseVoice transcription failed: {}", e))?,
        ActiveModel::GigaAM(m) => m
            .transcribe(samples, &TranscribeOptions::default())
            .map_err(|e| format!("GigaAM transcription failed: {}", e))?,
        ActiveModel::Canary(m) => m
            .transcribe(samples, &TranscribeOptions::default())
            .map_err(|e| format!("Canary transcription failed: {}", e))?,
        ActiveModel::Cohere(m) => m
            .transcribe(samples, &TranscribeOptions::default())
            .map_err(|e| format!("Cohere transcription failed: {}", e))?,
    };

    {
        let mut m = state.model.lock().unwrap();
        *m = Some(model);
    }

    Ok(result)
}

/// Chunked transcription for long audio files (Parakeet only).
/// Splits audio into overlapping chunks, transcribes each, and combines results.
fn transcribe_file_chunked(
    state: &ModelState,
    samples: &[f32],
    total_duration_secs: f64,
) -> Result<TranscriptionResult, String> {
    log::info!(
        "Splitting into chunks of {:.0}s with {:.0}s overlap",
        MAX_CHUNK_DURATION_SECS, CHUNK_OVERLAP_SECS
    );

    let chunk_samples = (MAX_CHUNK_DURATION_SECS * STT_SAMPLE_RATE as f64) as usize;
    let step_samples = ((MAX_CHUNK_DURATION_SECS - CHUNK_OVERLAP_SECS) * STT_SAMPLE_RATE as f64) as usize;

    let mut all_segments: Vec<TranscriptionSegment> = Vec::new();
    let mut combined_text = String::new();
    let mut covered_until_secs: f64 = 0.0;
    let mut chunk_index = 0;

    loop {
        let start_sample = chunk_index * step_samples;
        if start_sample >= samples.len() {
            break;
        }
        let end_sample = (start_sample + chunk_samples).min(samples.len());
        let chunk = &samples[start_sample..end_sample];
        let chunk_duration = chunk.len() as f64 / STT_SAMPLE_RATE as f64;
        let offset_secs = start_sample as f64 / STT_SAMPLE_RATE as f64;

        log::info!(
            "Chunk {}: {:.1}s - {:.1}s ({:.1}s of audio)",
            chunk_index, offset_secs, offset_secs + chunk_duration, chunk_duration
        );

        // Transcribe this chunk
        let chunk_result = transcribe_file_pass(state, chunk, offset_secs)?;

        // Add text with space separator
        if !combined_text.is_empty() && !chunk_result.text.is_empty() {
            combined_text.push(' ');
        }
        combined_text.push_str(&chunk_result.text);

        // Add segments, deduplicating in overlap regions
        if let Some(segments) = chunk_result.segments {
            for seg in segments {
                // Skip segments that fall in the already-covered region
                let tolerance = 1.0_f32; // 1 second tolerance for segment boundaries
                if seg.start < (covered_until_secs as f32) - tolerance {
                    continue;
                }
                all_segments.push(seg);
            }
        }

        // Update covered region
        covered_until_secs = (offset_secs + chunk_duration - CHUNK_OVERLAP_SECS).max(covered_until_secs);

        log::info!(
            "Chunk {} done: {} total segments, covered_until={:.1}s",
            chunk_index, all_segments.len(), covered_until_secs
        );

        chunk_index += 1;

        // If this chunk reached the end of the audio, stop
        if end_sample >= samples.len() {
            break;
        }
    }

    let result = TranscriptionResult {
        text: combined_text,
        segments: if all_segments.is_empty() { None } else { Some(all_segments) },
    };

    log::info!(
        "transcribe_file complete (chunked): {} chunks, {} segments, {:.1}s audio",
        chunk_index,
        result.segments.as_ref().map(|s| s.len()).unwrap_or(0),
        total_duration_secs
    );
    Ok(result)
}

/// Transcribe a file with word-level timestamps (Parakeet only).
///
/// Each segment in the result represents a single word with precise start/end times.
/// Returns an error if the active model is not Parakeet.
///
/// ## Backend word-level timestamp support (transcribe-rs v0.3.11)
///
/// | Backend | Word-level | Notes |
/// |---------|-----------|-------|
/// | Parakeet (ONNX) | Yes | `TimestampGranularity::Word` via `transcribe_with()` |
/// | Whisper (whisper.cpp/GGML) | Not exposed | whisper.cpp supports DTW word timestamps, but transcribe-rs does not expose the API |
/// | Canary, SenseVoice, Moonshine (ONNX) | No | Only segment-level timestamps |
/// | OpenAI Remote | Yes | `TimestampGranularity::Word` via API (cloud-only) |
///
/// To add Whisper word-level support, transcribe-rs needs to expose the DTW
/// parameters from whisper.cpp, or we need to call whisper-rs directly.
///
/// ## Long Audio Handling
/// Parakeet v3 cannot process very long audio files (>4-5 min). This function
/// automatically splits long audio into overlapping chunks, transcribes each
/// chunk separately, and combines the results with deduplication.
pub(crate) fn transcribe_file_word_level(
    state: &ModelState,
    file_path: &Path,
) -> Result<transcribe_rs::TranscriptionResult, String> {
    log::info!("transcribe_file_word_level: {}", file_path.display());
    let samples = crate::audio::decode_to_pcm(file_path)?;

    let total_duration_secs = samples.len() as f64 / STT_SAMPLE_RATE as f64;
    log::info!("Audio duration: {:.1}s ({:.1} min)", total_duration_secs, total_duration_secs / 60.0);

    // Check if model is Parakeet before proceeding
    {
        let m = state.model.lock().unwrap();
        if m.is_none() {
            return Err("No model is loaded".to_string());
        }
    }

    // If audio is short enough, run single-pass (original behavior)
    if total_duration_secs <= MAX_CHUNK_DURATION_SECS {
        log::info!("Audio fits in single chunk, running single-pass transcription");
        return transcribe_word_level_pass(state, &samples, 0.0);
    }

    // Long audio: split into overlapping chunks
    log::info!(
        "Audio too long (>{:.0}s), splitting into chunks of {:.0}s with {:.0}s overlap",
        MAX_CHUNK_DURATION_SECS, MAX_CHUNK_DURATION_SECS, CHUNK_OVERLAP_SECS
    );

    let chunk_samples = (MAX_CHUNK_DURATION_SECS * STT_SAMPLE_RATE as f64) as usize;
    let step_samples = ((MAX_CHUNK_DURATION_SECS - CHUNK_OVERLAP_SECS) * STT_SAMPLE_RATE as f64) as usize;

    let mut all_words: Vec<TranscriptionSegment> = Vec::new();
    let mut covered_until_secs: f64 = 0.0;
    let mut chunk_index = 0;
    let mut offset_secs: f64 = 0.0;

    loop {
        let start_sample = chunk_index * step_samples;
        if start_sample >= samples.len() {
            break;
        }
        let end_sample = (start_sample + chunk_samples).min(samples.len());
        let chunk = &samples[start_sample..end_sample];
        let chunk_duration = chunk.len() as f64 / STT_SAMPLE_RATE as f64;

        log::info!(
            "Chunk {}: {:.1}s - {:.1}s ({:.1}s of audio, {} samples)",
            chunk_index, offset_secs, offset_secs + chunk_duration, chunk_duration, chunk.len()
        );

        // Transcribe this chunk
        let chunk_result = transcribe_word_level_pass(state, chunk, offset_secs)?;

        if let Some(words) = chunk_result.segments {
            for word in words {
                // Skip words that fall in the already-covered region (deduplication).
                // Allow a small tolerance to avoid cutting words at the boundary.
                let tolerance = 0.5_f32; // seconds
                if word.start < (covered_until_secs as f32) - tolerance {
                    continue;
                }
                all_words.push(word);
            }
        }

        // Update covered region: everything up to (chunk_end - overlap) is covered
        covered_until_secs = (offset_secs + chunk_duration - CHUNK_OVERLAP_SECS).max(covered_until_secs);

        log::info!("Chunk {} done: {} total words, covered_until={:.1}s", chunk_index, all_words.len(), covered_until_secs);

        // Move to next chunk
        chunk_index += 1;
        offset_secs = (chunk_index * step_samples) as f64 / STT_SAMPLE_RATE as f64;

        // If this chunk reached the end of the audio, stop
        if end_sample >= samples.len() {
            break;
        }
    }

    // Build combined result
    let combined_text: String = all_words.iter().map(|w| w.text.as_str()).collect::<Vec<&str>>().join(" ");
    let result = TranscriptionResult {
        text: combined_text,
        segments: if all_words.is_empty() { None } else { Some(all_words) },
    };

    log::info!(
        "transcribe_file_word_level complete: {} chunks, {} words, {:.1}s audio",
        chunk_index + 1,
        result.segments.as_ref().map(|s| s.len()).unwrap_or(0),
        total_duration_secs
    );
    Ok(result)
}

/// Run a single pass of word-level transcription on a sample slice.
/// `time_offset_secs` is added to all timestamps (for chunked processing).
fn transcribe_word_level_pass(
    state: &ModelState,
    samples: &[f32],
    time_offset_secs: f64,
) -> Result<TranscriptionResult, String> {
    let mut model = {
        let mut m = state.model.lock().unwrap();
        m.take().ok_or_else(|| "No model is loaded".to_string())?
    };

    let result = match &mut model {
        ActiveModel::Parakeet(m) => {
            let params = ParakeetParams {
                language: None,
                timestamp_granularity: Some(TimestampGranularity::Word),
            };
            let mut r = m.transcribe_with(samples, &params)
                .map_err(|e| format!("Parakeet word-level transcription failed: {}", e))?;
            // Apply time offset if chunked
            if time_offset_secs > 0.0 {
                if let Some(ref mut segs) = r.segments {
                    for seg in segs.iter_mut() {
                        seg.start += time_offset_secs as f32;
                        seg.end += time_offset_secs as f32;
                    }
                }
            }
            r
        }
        _ => {
            // Put model back before returning error
            {
                let mut m = state.model.lock().unwrap();
                *m = Some(model);
            }
            return Err("Word-level timestamps are only supported by Parakeet models".to_string());
        }
    };

    {
        let mut m = state.model.lock().unwrap();
        *m = Some(model);
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Reusable helpers (used by the web service module)
// ---------------------------------------------------------------------------

/// Run text transcription against an already-loaded active model.
fn run_transcribe_text(model: &mut ActiveModel, samples: &[f32]) -> Result<String, String> {
    match model {
        ActiveModel::TranscribeCpp(session) => {
            let transcript = session
                .run(samples, &CppRunOptions::default())
                .map_err(|e| format!("Whisper transcription failed: {}", e))?;
            Ok(transcript.text)
        }
        ActiveModel::Parakeet(m) => m
            .transcribe(samples, &TranscribeOptions::default())
            .map_err(|e| format!("Parakeet transcription failed: {}", e))
            .map(|r| r.text),
        ActiveModel::Moonshine(m) => m
            .transcribe(samples, &TranscribeOptions::default())
            .map_err(|e| format!("Moonshine transcription failed: {}", e))
            .map(|r| r.text),
        ActiveModel::MoonshineStreaming(m) => m
            .transcribe(samples, &TranscribeOptions::default())
            .map_err(|e| format!("Moonshine Streaming transcription failed: {}", e))
            .map(|r| r.text),
        ActiveModel::SenseVoice(m) => m
            .transcribe(samples, &TranscribeOptions::default())
            .map_err(|e| format!("SenseVoice transcription failed: {}", e))
            .map(|r| r.text),
        ActiveModel::GigaAM(m) => m
            .transcribe(samples, &TranscribeOptions::default())
            .map_err(|e| format!("GigaAM transcription failed: {}", e))
            .map(|r| r.text),
        ActiveModel::Canary(m) => m
            .transcribe(samples, &TranscribeOptions::default())
            .map_err(|e| format!("Canary transcription failed: {}", e))
            .map(|r| r.text),
        ActiveModel::Cohere(m) => m
            .transcribe(samples, &TranscribeOptions::default())
            .map_err(|e| format!("Cohere transcription failed: {}", e))
            .map(|r| r.text),
    }
}

/// Ensure a model is loaded, loading the currently selected version if needed.
pub(crate) fn ensure_model_loaded(state: &ModelState) -> Result<(), String> {
    {
        let active = state.active_version.lock().unwrap();
        if active.is_some() {
            return Ok(());
        }
    }
    let version = state.selected_version.lock().unwrap().clone();
    log::info!("ensure_model_loaded: auto-loading selected model '{}'", version);
    load_model_core(state, &version)?;
    Ok(())
}

/// Transcribe raw 16kHz mono PCM WAV bytes, loading the model first if needed.
pub(crate) fn transcribe_wav_bytes(state: &ModelState, wav: &[u8]) -> Result<String, String> {
    ensure_model_loaded(state)?;
    let samples = parse_wav_pcm(wav)?;

    let mut model = {
        let mut m = state.model.lock().unwrap();
        m.take().ok_or_else(|| "No model is loaded".to_string())?
    };

    let result = run_transcribe_text(&mut model, &samples);

    {
        let mut m = state.model.lock().unwrap();
        *m = Some(model);
    }

    result
}
