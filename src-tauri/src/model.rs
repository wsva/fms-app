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
    parakeet::ParakeetModel,
    sense_voice::SenseVoiceModel,
    Quantization,
};
use transcribe_rs::{SpeechModel, TranscribeOptions};

use crate::model_list::{self, EngineType};
use crate::settings::SettingsState;

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
}

impl ModelState {
    pub fn new() -> Self {
        let mut download_status = HashMap::new();
        for model_def in model_list::MODELS {
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

        let selected = model_list::MODELS
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
    let models: Vec<ModelVersionInfo> = model_list::MODELS
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
    let is_downloading = state
        .download_status
        .lock()
        .unwrap()
        .values()
        .any(|s| *s == ModelStatus::Downloading);

    let active_status = if is_downloading {
        ModelStatus::Downloading
    } else if active.is_some() {
        ModelStatus::Running
    } else {
        ModelStatus::Stopped
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
    })
}

#[tauri::command]
pub async fn model_select_version(
    state: State<'_, ModelState>,
    version: String,
) -> Result<(), String> {
    if model_list::find_model(&version).is_none() {
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
    version: String,
) -> Result<(), String> {
    let def = model_list::find_model(&version)
        .ok_or_else(|| format!("Unknown model version: {}", version))?;

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
            let mut flags = state.cancel_flags.lock().unwrap();
            flags.remove(&version);
            let mut s = state.download_status.lock().unwrap();
            s.insert(version.clone(), ModelStatus::Error(e.clone()));
            return Err(e);
        }
    }

    {
        let mut s = state.download_status.lock().unwrap();
        s.insert(version, ModelStatus::Downloaded);
    }

    Ok(())
}

#[tauri::command]
pub async fn model_download(
    app: AppHandle,
    state: State<'_, ModelState>,
    settings: State<'_, SettingsState>,
    version: String,
) -> Result<(), String> {
    model_download_inner(app, &state, &settings, version).await
}

// ---------------------------------------------------------------------------
// Core logic (callable from managers without Tauri State)
// ---------------------------------------------------------------------------

pub fn load_model_core(state: &ModelState, version: &str) -> Result<String, String> {
    {
        let active = state.active_version.lock().unwrap();
        if active.is_some() {
            return Err("A model is already running".into());
        }
    }

    let def = model_list::find_model(version)
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

    Ok(version.to_string())
}

pub fn unload_model_core(state: &ModelState) -> Result<(), String> {
    {
        let active = state.active_version.lock().unwrap();
        if active.is_none() {
            return Err("No model is running".into());
        }
    }

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

pub fn delete_model_core(state: &ModelState, version: &str) -> Result<(), String> {
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
    version: String,
) -> Result<(), String> {
    delete_model_core(&state, &version)
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

pub(crate) fn transcribe_file(
    state: &ModelState,
    file_path: &Path,
) -> Result<transcribe_rs::TranscriptionResult, String> {
    let samples = crate::audio::decode_to_pcm(file_path)?;

    let mut model = {
        let mut m = state.model.lock().unwrap();
        m.take().ok_or_else(|| "No model is loaded".to_string())?
    };

    let result = match &mut model {
        ActiveModel::TranscribeCpp(session) => {
            let transcript = session
                .run(&samples, &CppRunOptions::default())
                .map_err(|e| format!("Whisper transcription failed: {}", e))?;
            let segments = if transcript.segments.is_empty() {
                None
            } else {
                Some(
                    transcript
                        .segments
                        .iter()
                        .map(|s| transcribe_rs::TranscriptionSegment {
                            start: s.t0_ms as f32 / 1000.0,
                            end: s.t1_ms as f32 / 1000.0,
                            text: s.text.clone(),
                        })
                        .collect(),
                )
            };
            transcribe_rs::TranscriptionResult {
                text: transcript.text,
                segments,
            }
        }
        ActiveModel::Parakeet(m) => m
            .transcribe(&samples, &TranscribeOptions::default())
            .map_err(|e| format!("Parakeet transcription failed: {}", e))?,
        ActiveModel::Moonshine(m) => m
            .transcribe(&samples, &TranscribeOptions::default())
            .map_err(|e| format!("Moonshine transcription failed: {}", e))?,
        ActiveModel::MoonshineStreaming(m) => m
            .transcribe(&samples, &TranscribeOptions::default())
            .map_err(|e| format!("Moonshine Streaming transcription failed: {}", e))?,
        ActiveModel::SenseVoice(m) => m
            .transcribe(&samples, &TranscribeOptions::default())
            .map_err(|e| format!("SenseVoice transcription failed: {}", e))?,
        ActiveModel::GigaAM(m) => m
            .transcribe(&samples, &TranscribeOptions::default())
            .map_err(|e| format!("GigaAM transcription failed: {}", e))?,
        ActiveModel::Canary(m) => m
            .transcribe(&samples, &TranscribeOptions::default())
            .map_err(|e| format!("Canary transcription failed: {}", e))?,
        ActiveModel::Cohere(m) => m
            .transcribe(&samples, &TranscribeOptions::default())
            .map_err(|e| format!("Cohere transcription failed: {}", e))?,
    };

    {
        let mut m = state.model.lock().unwrap();
        *m = Some(model);
    }

    Ok(result)
}
