//! SaT (Segment any Text) subword sentence segmentation.
//!
//! A faithful port of the SaT path from <https://github.com/19h/wtsplit-rs>,
//! vendored against the statically-linked `ort` already in the tree (via
//! transcribe-rs) rather than depending on the `wtpsplit` crate. Exposed as the
//! desktop "Split Text" page, its `text_split` MCP twin, and the dataset
//! "Split Book" stage (`crate::datasets::tools::dataset_parse_book`).
//!
//! Only the subword (XLM-RoBERTa) sentence path is ported; all WtP / character
//! hashing, paragraph segmentation, and language-adapter code is dropped.

#[path = "const.rs"]
mod consts;
mod config;
mod extract;
mod model;
mod utils;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::State;
use tokenizers::Tokenizer;

use crate::models::catalog;
use crate::models::index as model_index;

use config::ModelConfig;
use extract::{extract_sat, logits_to_probs, ExtractConfig, ExtractionResult, Weighting};
use model::OnnxModel;
use utils::{indices_to_sentences, reinsert_space_probs, remove_spaces};

/// Local result alias: this codebase surfaces errors to Tauri as `String`.
pub type Result<T> = std::result::Result<T, String>;

/// The single SaT model shipped in the catalog. Used as the default when the
/// caller does not name a model.
pub const DEFAULT_MODEL_ID: &str = "segment-any-text/sat-3l-sm";

/// The model the "Split Book" stage (4a) runs: the full 3L, whose boundaries are
/// more accurate than `-sm` on literary prose and whose cost is paid once per
/// book rather than interactively.
pub const BOOK_MODEL_ID: &str = "segment-any-text/sat-3l";

/// Display name of [`BOOK_MODEL_ID`], so the Split Book stage's summary string
/// cannot drift from the catalog entry it names.
pub fn book_model_name() -> &'static str {
    catalog_def(BOOK_MODEL_ID)
        .map(|def| def.name)
        .unwrap_or(BOOK_MODEL_ID)
}

/// The catalog `model_type` these models are registered under.
const TEXTSPLIT_TYPE: &str = "textsplit";

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// Configuration for a SaT split, mirroring the wtsplit-rs `SaTOptions` public
/// subset that the Split Text page exposes.
#[derive(Debug, Clone)]
pub struct SaTOptions {
    /// Probability threshold for sentence boundaries (None → model default).
    pub threshold: Option<f32>,
    /// Stride for overlapping chunks.
    pub stride: usize,
    /// Maximum block size.
    pub block_size: usize,
    /// Batch size for inference.
    pub batch_size: usize,
    /// Weighting scheme for overlapping predictions.
    pub weighting: Weighting,
    /// Remove whitespace before inference (used for some no-space languages).
    pub remove_whitespace_before_inference: bool,
    /// Strip surrounding whitespace from each sentence.
    pub strip_whitespace: bool,
    /// Also split on the input's own newlines.
    pub split_on_input_newlines: bool,
}

impl Default for SaTOptions {
    fn default() -> Self {
        Self {
            threshold: None,
            stride: 64,
            block_size: 512,
            batch_size: 32,
            weighting: Weighting::Uniform,
            remove_whitespace_before_inference: false,
            strip_whitespace: false,
            split_on_input_newlines: true,
        }
    }
}

// ---------------------------------------------------------------------------
// SaT engine
// ---------------------------------------------------------------------------

/// SaT sentence segmentation model (ONNX session + tokenizer).
pub struct SaT {
    model: OnnxModel,
    tokenizer: Tokenizer,
}

impl SaT {
    /// Load a SaT model from a local directory containing `config.json`,
    /// `model_optimized.onnx` (or `model.onnx`), and `tokenizer.json`.
    pub fn from_dir(dir: &Path) -> Result<Self> {
        let config_path = dir.join("config.json");
        if !config_path.exists() {
            return Err(format!("config.json not found in {}", dir.display()));
        }
        let config = ModelConfig::from_file(&config_path)?;

        let onnx_path = pick_onnx(dir)?;
        let model = OnnxModel::new(&onnx_path, config)?;

        let tokenizer_path = dir.join("tokenizer.json");
        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| format!("Failed to load tokenizer {:?}: {}", tokenizer_path, e))?;

        Ok(Self { model, tokenizer })
    }

    /// Default boundary threshold (matches Python wtpsplit / wtsplit-rs).
    fn get_default_threshold() -> f32 {
        consts::DEFAULT_THRESHOLD
    }

    /// Per-character sentence-boundary probabilities for a batch of texts.
    pub fn predict_proba_batch(&mut self, texts: &[&str], options: &SaTOptions) -> Result<Vec<Vec<f32>>> {
        let (input_texts, space_positions): (Vec<String>, Vec<Vec<usize>>) =
            if options.remove_whitespace_before_inference {
                texts.iter().map(|t| remove_spaces(t)).unzip()
            } else {
                (
                    texts.iter().map(|s| s.to_string()).collect(),
                    vec![vec![]; texts.len()],
                )
            };

        let non_empty_indices: Vec<usize> = input_texts
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.trim().is_empty())
            .map(|(i, _)| i)
            .collect();

        let non_empty_texts: Vec<&str> =
            non_empty_indices.iter().map(|&i| input_texts[i].as_str()).collect();

        let extract_config = ExtractConfig {
            block_size: options.block_size,
            stride: options.stride,
            batch_size: options.batch_size,
            pad_last_batch: false,
            weighting: options.weighting,
        };

        let extraction_result = if non_empty_texts.is_empty() {
            ExtractionResult { logits: vec![] }
        } else {
            extract_sat(&non_empty_texts, &mut self.model, &self.tokenizer, &extract_config)?
        };

        let mut all_probs: Vec<Vec<f32>> = vec![vec![]; texts.len()];
        for (result_idx, &text_idx) in non_empty_indices.iter().enumerate() {
            all_probs[text_idx] = logits_to_probs(&extraction_result.logits[result_idx]);
        }

        // Fill whitespace-only / empty texts deterministically.
        for i in 0..texts.len() {
            if all_probs[i].is_empty() && !texts[i].is_empty() {
                all_probs[i] = vec![0.0; texts[i].chars().count()];
            }
        }

        if options.remove_whitespace_before_inference {
            for (probs, positions) in all_probs.iter_mut().zip(space_positions.iter()) {
                if !positions.is_empty() {
                    *probs = reinsert_space_probs(probs, positions);
                }
            }
        }

        Ok(all_probs)
    }

    /// Split a batch of texts into sentences.
    pub fn split_batch(&mut self, texts: &[&str], options: &SaTOptions) -> Result<Vec<Vec<String>>> {
        let threshold = options.threshold.unwrap_or_else(Self::get_default_threshold);
        let all_probs = self.predict_proba_batch(texts, options)?;

        let results = texts
            .iter()
            .zip(all_probs.iter())
            .map(|(text, probs)| {
                let indices: Vec<usize> = probs
                    .iter()
                    .enumerate()
                    .filter(|(_, &p)| p > threshold)
                    .map(|(i, _)| i)
                    .collect();

                let mut sentences = indices_to_sentences(text, &indices, options.strip_whitespace);

                if options.split_on_input_newlines {
                    sentences = sentences
                        .into_iter()
                        .flat_map(|s| s.split('\n').map(String::from).collect::<Vec<_>>())
                        .filter(|s| !s.is_empty())
                        .collect();
                }

                sentences
            })
            .collect();

        Ok(results)
    }

    /// Split a single text into sentences.
    pub fn split(&mut self, text: &str, options: &SaTOptions) -> Result<Vec<String>> {
        let results = self.split_batch(&[text], options)?;
        Ok(results.into_iter().next().unwrap_or_default())
    }
}

// ---------------------------------------------------------------------------
// Model path helpers (shared by load, status, and download)
// ---------------------------------------------------------------------------

/// Look up a catalog entry by id, ensuring it is a text-split model.
fn catalog_def(id: &str) -> Result<&'static catalog::HfModelDef> {
    let def = catalog::find_model(id).ok_or_else(|| format!("Unknown text-split model: {}", id))?;
    if def.model_type != TEXTSPLIT_TYPE {
        return Err(format!("Model '{}' is not a text-split model", id));
    }
    Ok(def)
}

/// Resolve the on-disk directory for a model id: `{model_root}/candle/{folder}`.
/// Kept identical to what `index::scan_models` checks and what
/// `models::model_download_hf` writes, so the paths cannot drift.
pub fn resolve_model_dir(id: &str) -> Result<PathBuf> {
    let def = catalog_def(id)?;
    Ok(model_index::model_root().join("candle").join(def.folder))
}

/// Pick the ONNX weights: prefer the optimized export, fall back to the base.
fn pick_onnx(dir: &Path) -> Result<PathBuf> {
    let optimized = dir.join("model_optimized.onnx");
    if optimized.exists() {
        return Ok(optimized);
    }
    let base = dir.join("model.onnx");
    if base.exists() {
        return Ok(base);
    }
    Err(format!("No ONNX model (model_optimized.onnx / model.onnx) found in {}", dir.display()))
}

/// Whether the model's required files are all present on disk.
fn is_downloaded_dir(dir: &Path) -> bool {
    dir.join("config.json").exists()
        && dir.join("tokenizer.json").exists()
        && (dir.join("model_optimized.onnx").exists() || dir.join("model.onnx").exists())
}

// ---------------------------------------------------------------------------
// Tauri-managed state
// ---------------------------------------------------------------------------

struct Inner {
    sat: Option<SaT>,
    loaded_id: Option<String>,
}

struct Shared {
    inner: Mutex<Inner>,
    /// Serialize splits so a re-load never races an in-flight inference.
    split_lock: Mutex<()>,
}

/// Cached SaT session, managed by Tauri. The `Arc<Shared>` is cheap to clone
/// into `spawn_blocking` closures.
pub struct TextSplitState {
    shared: Arc<Shared>,
}

impl TextSplitState {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                inner: Mutex::new(Inner { sat: None, loaded_id: None }),
                split_lock: Mutex::new(()),
            }),
        }
    }

    /// Load `id` if not already loaded. Assumes `split_lock` is held and
    /// `inner` is locked (both callers keep them in one critical section).
    fn ensure_loaded_locked(inner: &mut Inner, id: &str) -> Result<()> {
        if inner.sat.is_some() && inner.loaded_id.as_deref() == Some(id) {
            return Ok(());
        }
        let dir = resolve_model_dir(id)?;
        if !is_downloaded_dir(&dir) {
            let name = catalog_def(id).map(|d| d.name).unwrap_or(id);
            return Err(format!(
                "Model '{}' ({}) is not downloaded. Download it on the Models page first.",
                name, id
            ));
        }
        log::info!("[textsplit] Loading SaT model '{}' from {:?}", id, dir);
        let sat = SaT::from_dir(&dir)?;
        inner.sat = Some(sat);
        inner.loaded_id = Some(id.to_string());
        Ok(())
    }

    /// Split `text` with the cached engine, loading `model_id` on first use.
    /// The inference runs on a blocking thread so the async runtime stays free.
    /// Shared by the [`text_split`] command and the "Split Book" stage so both
    /// go down one locking and loading path.
    pub async fn split_text(
        &self,
        text: String,
        model_id: &str,
        options: SaTOptions,
    ) -> Result<Vec<String>> {
        let shared = self.shared.clone();
        let id = model_id.to_string();
        tauri::async_runtime::spawn_blocking(move || {
            let _split_guard = shared.split_lock.lock().unwrap();
            let mut inner = shared.inner.lock().unwrap();
            TextSplitState::ensure_loaded_locked(&mut inner, &id)?;
            let sat = inner.sat.as_mut().ok_or_else(|| "Model not loaded".to_string())?;
            sat.split(&text, &options)
        })
        .await
        .map_err(|e| format!("Split task failed: {}", e))?
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Per-model info for the status payload.
#[derive(Serialize)]
pub struct TextSplitModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub downloaded: bool,
    pub loaded: bool,
    pub size_mb: u64,
}

/// Overall status: the primary model's downloaded/loaded flags plus the full
/// text-split catalog (for the UI badge and the agent).
#[derive(Serialize)]
pub struct TextSplitStatus {
    pub downloaded: bool,
    pub loaded: bool,
    pub model_id: String,
    pub models: Vec<TextSplitModelInfo>,
}

#[tauri::command]
pub async fn text_split_status(state: State<'_, TextSplitState>) -> Result<TextSplitStatus> {
    let loaded_id = state.shared.inner.lock().unwrap().loaded_id.clone();

    let models: Vec<TextSplitModelInfo> = catalog::models_by_type(TEXTSPLIT_TYPE)
        .into_iter()
        .map(|def| {
            let dir = model_index::model_root().join("candle").join(def.folder);
            TextSplitModelInfo {
                id: def.id.to_string(),
                name: def.name.to_string(),
                description: def.description.to_string(),
                downloaded: is_downloaded_dir(&dir),
                loaded: loaded_id.as_deref() == Some(def.id),
                size_mb: def.size_mb,
            }
        })
        .collect();

    // Report against the loaded model if any, else the default.
    let model_id = loaded_id.clone().unwrap_or_else(|| DEFAULT_MODEL_ID.to_string());
    let downloaded = resolve_model_dir(&model_id).map(|d| is_downloaded_dir(&d)).unwrap_or(false);
    let loaded = loaded_id.as_deref() == Some(model_id.as_str());

    let hint = if loaded {
        format!("Model '{}' is loaded and ready.", model_id)
    } else if downloaded {
        format!("Model '{}' is downloaded; it loads on the first text_split call.", model_id)
    } else {
        "No text-split model downloaded. Use model_download_hf with segment-any-text/sat-3l-sm.".to_string()
    };
    log::info!("[textsplit] status: {}", hint);

    Ok(TextSplitStatus { downloaded, loaded, model_id, models })
}

/// Delete a downloaded text-split model and drop any cached session for it, so
/// the Models page can fully manage it (download / delete) alongside STT models.
#[tauri::command]
pub async fn text_split_delete(
    state: State<'_, TextSplitState>,
    index: State<'_, model_index::ModelIndexState>,
    id: String,
) -> Result<()> {
    catalog_def(&id)?;
    let dir = resolve_model_dir(&id)?;

    // Unload first: the cached session must never outlive its files.
    {
        let _split_guard = state.shared.split_lock.lock().unwrap();
        let mut inner = state.shared.inner.lock().unwrap();
        if inner.loaded_id.as_deref() == Some(id.as_str()) {
            inner.sat = None;
            inner.loaded_id = None;
        }
    }

    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| format!("Failed to delete model directory {:?}: {}", dir, e))?;
    }

    let key = model_index::make_key(TEXTSPLIT_TYPE, &id);
    if let Err(e) = model_index::remove_entry(&index, &key) {
        log::warn!("[textsplit] failed to update model index after delete: {}", e);
    }

    log::info!("[textsplit] Deleted model '{}'", id);
    Ok(())
}

/// Split `text` into sentences. Auto-loads (and caches) the model on first use.
/// Runs the ONNX inference on a blocking thread so the async runtime stays free.
#[tauri::command]
pub async fn text_split(
    state: State<'_, TextSplitState>,
    text: String,
    model: Option<String>,
    threshold: Option<f32>,
    stride: Option<usize>,
    block_size: Option<usize>,
    batch_size: Option<usize>,
    strip_whitespace: Option<bool>,
    split_on_input_newlines: Option<bool>,
) -> Result<Vec<String>> {
    let id = model.unwrap_or_else(|| DEFAULT_MODEL_ID.to_string());

    // Defaults chosen for clean page output: strip whitespace, split on newlines.
    let options = SaTOptions {
        threshold,
        stride: stride.unwrap_or(64),
        block_size: block_size.unwrap_or(512),
        batch_size: batch_size.unwrap_or(32),
        weighting: Weighting::Uniform,
        remove_whitespace_before_inference: false,
        strip_whitespace: strip_whitespace.unwrap_or(true),
        split_on_input_newlines: split_on_input_newlines.unwrap_or(true),
    };

    state.split_text(text, &id, options).await
}
