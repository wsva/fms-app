//! Model lifecycle: which model is resident, as opposed to running one
//! (`transcribe`). Load, unload, delete, cancel, and the auto-load seam.

use std::sync::atomic::Ordering;

#[cfg(feature = "desktop")]
use transcribe_cpp::Model as CppModel;
use transcribe_rs::onnx::{
    canary::CanaryModel,
    cohere::CohereModel,
    gigaam::GigaAMModel,
    moonshine::{MoonshineModel, MoonshineVariant, StreamingModel},
    parakeet::ParakeetModel,
    sense_voice::SenseVoiceModel,
    Quantization,
};

use crate::models::catalog_stt::{self, EngineType};
use crate::models::index::{make_key, remove_entry, ModelIndexState};
use crate::models::state::{auto_load_version, ActiveModel, ModelState, ModelStatus};

pub fn load_model_core(state: &ModelState, version: &str) -> Result<String, String> {
    log::info!("Loading model '{}'", version);
    {
        let active = state.active_version.lock().unwrap();
        if active.is_some() {
            return Err("A model is already running".into());
        }
    }

    let def = catalog_stt::find_model(version)
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
        #[cfg(feature = "desktop")]
        EngineType::TranscribeCpp => {
            let model_path = model_dir.join(def.id);
            let model = CppModel::load(&model_path)
                .map_err(|e| format!("Failed to load Whisper model: {}", e))?;
            let session = model
                .session()
                .map_err(|e| format!("Failed to create session: {}", e))?;
            ActiveModel::TranscribeCpp(session)
        }
        #[cfg(not(feature = "desktop"))]
        EngineType::TranscribeCpp => {
            return Err("Whisper GGUF models are not available in this build (desktop-only). Use an ONNX model such as Parakeet or Moonshine.".to_string());
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

    // Wait for any in-progress transcription to finish before unloading.
    // This ensures clean state and prevents the transcription from putting
    // the model back after we've unloaded it.
    let _transcribe_guard = state.transcribe_lock.lock().unwrap();

    {
        let mut m = state.model.lock().unwrap();
        *m = None;
    }
    {
        let mut a = state.active_version.lock().unwrap();
        *a = None;
    }

    log::info!("Model '{}' unloaded successfully", active_name);
    Ok(())
}

pub fn delete_model_core(
    state: &ModelState,
    index: &ModelIndexState,
    version: &str,
) -> Result<(), String> {
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
    let key = make_key("stt", version);
    if let Err(e) = remove_entry(index, &key) {
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

/// Ensure a model is loaded, loading the user's default (or the currently
/// selected version when no default is set) if needed.
pub fn ensure_model_loaded(state: &ModelState) -> Result<(), String> {
    {
        let active = state.active_version.lock().unwrap();
        if active.is_some() {
            return Ok(());
        }
    }
    let version = auto_load_version(state);
    log::info!("ensure_model_loaded: auto-loading model '{}'", version);
    load_model_core(state, &version)?;
    Ok(())
}
