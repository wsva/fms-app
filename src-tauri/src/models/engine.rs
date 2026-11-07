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
use crate::models::gguf_meta::GgufProbe;
use crate::models::index::{make_key, remove_entry, ModelIndexState};
use crate::models::state::{auto_load_version, ActiveModel, ModelState, ModelStatus};

/// ggml's CPU backend is compiled for a fixed instruction set (see
/// `src-tauri/.cargo/config.toml`: SSE4.2 + AVX + AVX2 + FMA + F16C + BMI2).
/// Loading a model on a CPU without those is not a recoverable error — the first
/// unsupported kernel raises `STATUS_ILLEGAL_INSTRUCTION` and the process is
/// simply gone, which is what v0.1.8's Whisper crash looked like from the
/// outside. Refuse the load instead, with something a user or an agent can act
/// on.
#[cfg(all(
    feature = "desktop",
    any(target_arch = "x86", target_arch = "x86_64")
))]
fn cpp_isa_supported() -> Result<(), String> {
    let checks = [
        ("AVX2", std::is_x86_feature_detected!("avx2")),
        ("FMA", std::is_x86_feature_detected!("fma")),
        ("F16C", std::is_x86_feature_detected!("f16c")),
        ("BMI2", std::is_x86_feature_detected!("bmi2")),
        ("SSE4.2", std::is_x86_feature_detected!("sse4.2")),
    ];
    let missing: Vec<&str> = checks
        .iter()
        .filter(|(_, present)| !present)
        .map(|(name, _)| *name)
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "This CPU does not support {}, which the Whisper (GGML) backend is \
             compiled for. Use an ONNX model instead — Parakeet, Moonshine or \
             SenseVoice run on every CPU.",
            missing.join(" / ")
        ))
    }
}

/// Non-x86 desktops (Apple Silicon, Linux on ARM) never see the x86 build path.
#[cfg(all(
    feature = "desktop",
    not(any(target_arch = "x86", target_arch = "x86_64"))
))]
fn cpp_isa_supported() -> Result<(), String> {
    Ok(())
}

/// The word a reader recognises for a compute device, so the log says GPU or
/// CPU instead of naming an enum.
#[cfg(feature = "desktop")]
fn device_type_word(kind: transcribe_cpp::DeviceType) -> &'static str {
    use transcribe_cpp::DeviceType as D;
    match kind {
        D::Cpu => "the CPU",
        D::Gpu => "a GPU",
        D::Igpu => "the integrated GPU",
        D::Accel => "a host accelerator",
        // `#[non_exhaustive]`: a runtime newer than this binding can report a
        // type we have never seen. Say so rather than guessing which side of the
        // CPU/GPU line it falls on.
        _ => "an unrecognised device",
    }
}

/// Say what the model just loaded is *actually* running on.
///
/// `Backend::Auto` is a request, not an answer: the runtime binds whatever the
/// machine gave it and CPU is always the floor, so the only honest placement is
/// the one read back after the load. A machine with two GPUs makes the question
/// sharper — which one, and why not the other — so the registered device list
/// and the compiled-in GPU backends go to the log with it. A silent fallback to
/// the CPU is exactly what made the last slow transcription impossible to
/// explain from the outside.
fn log_compute_device(model: &ActiveModel, version: &str) {
    match model {
        #[cfg(feature = "desktop")]
        ActiveModel::TranscribeCpp(session) => {
            let loaded = session.model();
            // The *effective* backend after degradation. "cpu" here means a GPU
            // was wanted and not obtained, which is the whole point of asking.
            let backend = loaded.backend();
            let chosen = loaded.device();
            match chosen.as_ref() {
                Ok(device) => {
                    const MIB: u64 = 1024 * 1024;
                    let name = if device.description.trim().is_empty() {
                        device.name.as_str()
                    } else {
                        device.description.as_str()
                    };
                    let mut line = format!(
                        "Model '{}' is running on {} '{}' via the '{}' backend",
                        version,
                        device_type_word(device.device_type),
                        name,
                        backend
                    );
                    // A PCI bus id is what tells two identical cards apart; only
                    // PCI devices report one (Metal reports none).
                    if let Some(id) = device.device_id.as_deref() {
                        line.push_str(&format!(" (id {})", id));
                    }
                    if device.memory_total > 0 {
                        line.push_str(&format!(
                            " - {} MiB of {} MiB free",
                            device.memory_free / MIB,
                            device.memory_total / MIB
                        ));
                    }
                    log::info!("{}", line);
                }
                // No resolved device is a real answer, not a missing one: the
                // backend string still says what the run is on.
                Err(e) => log::warn!(
                    "Model '{}' is running on the '{}' backend, which reported no device: {}",
                    version,
                    backend,
                    e
                ),
            }
            log_registered_devices(chosen.as_ref().ok());
        }
        // An ONNX model's placement is `ort`'s choice and transcribe-rs keeps it:
        // it builds the provider list from a global preference and reports no
        // device back. So this says what cannot be known instead of claiming the
        // CPU — the preference is `Auto`, which deliberately excludes the GPU
        // providers that need sequential execution, but that is a request.
        _ => log::info!(
            "Model '{}' is an ONNX model: its runtime chooses the execution providers itself and reports no device back, so its placement is not measurable from here",
            version
        ),
    }
}

/// What this binary could have run on, and what the machine offered. Two
/// different absences, worth telling apart in the log: a backend that is not
/// compiled in is a build fact, a device that is not registered is a driver or
/// hardware fact.
#[cfg(feature = "desktop")]
fn log_registered_devices(chosen: Option<&transcribe_cpp::Device>) {
    use transcribe_cpp::{backend_available, devices, Backend};

    let compiled: Vec<&str> = [
        (Backend::Metal, "metal"),
        (Backend::Vulkan, "vulkan"),
        (Backend::Cuda, "cuda"),
        (Backend::Rocm, "rocm"),
    ]
    .into_iter()
    .filter(|(backend, _)| backend_available(*backend))
    .map(|(_, name)| name)
    .collect();
    let registered = devices();
    let names = if compiled.is_empty() { "none".to_string() } else { compiled.join(", ") };
    log::info!(
        "GPU backends compiled into this build: {}; {} compute device(s) registered",
        names,
        registered.len()
    );
    for device in registered.iter() {
        // `Device` compares by handle, so this marks the one the model got.
        let mark = if Some(device) == chosen { " [in use]" } else { "" };
        log::info!(
            "  device: {} - {} (kind {}, type {}){}",
            device.name,
            device.description,
            device.kind,
            device_type_word(device.device_type),
            mark
        );
    }
}

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
            cpp_isa_supported()?;
            let model_path = model_dir.join(def.artifact_name());
            let model = CppModel::load(&model_path)
                .map_err(|e| format!("Failed to load {} via transcribe-cpp: {}", version, e))?;
            let session = model
                .session()
                .map_err(|e| format!("Failed to create session: {}", e))?;
            ActiveModel::TranscribeCpp(session)
        }
        #[cfg(not(feature = "desktop"))]
        EngineType::TranscribeCpp => {
            // The GGML/GGUF rows are named after their runtimes, so the refusal
            // has to name the alternative that actually loads here — the ONNX row
            // of the same weights, which is a different id, not the same one.
            return Err(format!(
                "Model '{version}' needs transcribe-cpp, which is desktop-only in this build. Use an ONNX model such as parakeet-v2 or moonshine-base instead."
            ));
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

    // Measured while the model is still in hand: a transcription takes it out of
    // its slot for the whole run, so nothing later can ask it anything. This is
    // the answer every capability gate in `transcribe` reads.
    let measured = active_model.engine_capabilities(&def.capabilities);
    log_compute_device(&active_model, version);

    {
        let mut m = state.model.lock().unwrap();
        *m = Some(active_model);
    }
    {
        let mut a = state.active_capabilities.lock().unwrap();
        *a = Some(measured);
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
    {
        // Whatever was measured belongs to the model that is gone: `None` means
        // "not measured", and must never be read as an engine's "no".
        *state.active_capabilities.lock().unwrap() = None;
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
    // Whatever the header of a file at this path claimed is gone with it; a
    // later download of the same id must be re-read, not believed from cache.
    if let Some(def) = catalog_stt::find_model(version) {
        GgufProbe::forget(&model_dir.join(def.artifact_name()));
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
