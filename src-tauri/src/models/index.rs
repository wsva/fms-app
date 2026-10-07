//! Unified model index for tracking all downloaded models.
//!
//! Maintains `{model_root}/index.json` with metadata about every downloaded model
//! (STT, HuggingFace OCR/TTS, etc.). Replaces ad-hoc filesystem checks and
//! scattered persistence.
//!
//! ## Key format
//! `{type}:{id}` — e.g. `stt:parakeet-v3`, `candle-ocr:microsoft/trocr-base-printed`
//!
//! ## Integrity
//! Uses file-size-only checks (no hashing) for speed. Sufficient to catch
//! incomplete downloads and most common failures.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::models::{catalog, catalog_stt};


// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Top-level index stored as `index.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelIndex {
    /// Schema version for future migrations.
    pub version: u32,
    /// Keyed by `"{type}:{id}"`.
    pub models: HashMap<String, ModelIndexEntry>,
}

/// A single model entry in the index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelIndexEntry {
    /// Model category: "stt", "candle-ocr", "candle-tts", etc.
    pub model_type: String,
    /// Model-specific identifier (e.g. "parakeet-v3", "microsoft/trocr-base-printed").
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// ISO-8601 timestamp of when the model was downloaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloaded_at: Option<String>,
    /// Download source: "huggingface", "hf-mirror", "modelscope", "cdn".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Files belonging to this model, keyed by relative path from model root.
    /// Value is file size in bytes.
    pub files: HashMap<String, u64>,
}

/// Tauri-managed state wrapping the index.
pub struct ModelIndexState {
    pub index: Mutex<ModelIndex>,
    pub index_path: PathBuf,
}

// ---------------------------------------------------------------------------
// Index persistence
// ---------------------------------------------------------------------------

impl ModelIndexState {
    /// Load index from disk (or create empty if missing).
    pub fn new(model_root: &Path) -> Self {
        let index_path = model_root.join("index.json");
        let index = if index_path.exists() {
            match std::fs::read_to_string(&index_path) {
                Ok(data) => match serde_json::from_str::<ModelIndex>(&data) {
                    Ok(idx) => {
                        log::info!("[ModelIndex] Loaded {} entries from {:?}", idx.models.len(), index_path);
                        idx
                    }
                    Err(e) => {
                        log::warn!("[ModelIndex] Failed to parse index.json: {}, starting fresh", e);
                        ModelIndex::default()
                    }
                },
                Err(e) => {
                    log::warn!("[ModelIndex] Failed to read index.json: {}, starting fresh", e);
                    ModelIndex::default()
                }
            }
        } else {
            log::info!("[ModelIndex] No index.json found at {:?}, starting fresh", index_path);
            ModelIndex::default()
        };

        Self {
            index: Mutex::new(index),
            index_path,
        }
    }

    /// Persist index to disk atomically (write .tmp then rename).
    pub fn save(index: &ModelIndex, path: &Path) -> Result<(), String> {
        let data = serde_json::to_string_pretty(index).map_err(|e| e.to_string())?;
        let tmp_path = path.with_extension("json.tmp");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&tmp_path, &data).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp_path, path).map_err(|e| e.to_string())?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Index key helpers
// ---------------------------------------------------------------------------

/// Build an index key from type and id.
pub fn make_key(model_type: &str, id: &str) -> String {
    format!("{}:{}", model_type, id)
}

/// Resolve the model root directory.
pub fn model_root() -> PathBuf {
    crate::app_paths::data_subdir("models")
}

// ---------------------------------------------------------------------------
// Filesystem walking helper
// ---------------------------------------------------------------------------

/// Recursively collect all files under `dir`, returning paths relative to `root`.
pub fn collect_files(dir: &Path, root: &Path) -> HashMap<String, u64> {
    let mut files = HashMap::new();
    collect_files_recursive(dir, root, &mut files);
    files
}

fn collect_files_recursive(dir: &Path, root: &Path, out: &mut HashMap<String, u64>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(&path, root, out);
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                if let Ok(meta) = path.metadata() {
                    out.insert(rel.to_string_lossy().replace('\\', "/"), meta.len());
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Scan filesystem to rebuild index
// ---------------------------------------------------------------------------

/// Scan the model directory and rebuild the index from what's on disk.
pub fn scan_models(root: &Path) -> ModelIndex {
    let mut index = ModelIndex {
        version: 1,
        models: HashMap::new(),
    };

    if !root.exists() {
        return index;
    }

    // 1. Scan STT models (known IDs from model_list_stt)
    for def in catalog_stt::MODELS {
        let model_dir = root.join(def.id);
        if !model_dir.exists() {
            continue;
        }

        let files = collect_files(&model_dir, root);

        if !files.is_empty() {
            let key = make_key("stt", def.id);
            index.models.insert(key, ModelIndexEntry {
                model_type: "stt".to_string(),
                id: def.id.to_string(),
                name: def.name.to_string(),
                downloaded_at: None,
                provider: Some("cdn".to_string()),
                files,
            });
        }
    }

    // 2. Scan HuggingFace models from catalog (candle-ocr, candle-tts, etc.)
    for def in catalog::MODELS {
        let model_dir = root.join("candle").join(def.folder);
        if !model_dir.exists() {
            continue;
        }

        // Check that at least the primary file exists (first file in the list).
        let has_primary = def.files.first().map_or(false, |f| {
            model_dir.join(f.path).exists()
        });
        if !has_primary {
            continue;
        }

        let files = collect_files(&model_dir, root);
        let key = make_key(def.model_type, def.id);
        index.models.insert(key, ModelIndexEntry {
            model_type: def.model_type.to_string(),
            id: def.id.to_string(),
            name: def.name.to_string(),
            downloaded_at: None,
            provider: Some("huggingface".to_string()),
            files,
        });
    }

    log::info!("[ModelIndex] Scan complete: found {} models", index.models.len());
    index
}

// ---------------------------------------------------------------------------
// Mutation helpers (called from download/delete flows)
// ---------------------------------------------------------------------------

/// Add or update a model entry in the index, then persist.
pub fn upsert_entry(
    state: &ModelIndexState,
    key: String,
    entry: ModelIndexEntry,
) -> Result<(), String> {
    let mut idx = state.index.lock().unwrap();
    idx.models.insert(key, entry);
    ModelIndexState::save(&idx, &state.index_path)
}

/// Remove a model entry from the index, then persist.
#[allow(dead_code)]
pub fn remove_entry(state: &ModelIndexState, key: &str) -> Result<(), String> {
    let mut idx = state.index.lock().unwrap();
    idx.models.remove(key);
    ModelIndexState::save(&idx, &state.index_path)
}

/// Check if a model is in the index (downloaded).
#[allow(dead_code)]
pub fn is_downloaded(state: &ModelIndexState, key: &str) -> bool {
    let idx = state.index.lock().unwrap();
    idx.models.contains_key(key)
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Get the full model index.
#[tauri::command]
pub fn model_index_get(state: State<'_, ModelIndexState>) -> Result<ModelIndex, String> {
    let idx = state.index.lock().unwrap();
    Ok(idx.clone())
}

/// Scan the models directory and rebuild the index from disk.
#[tauri::command]
pub fn model_index_refresh(state: State<'_, ModelIndexState>) -> Result<ModelIndex, String> {
    let root = model_root();
    log::info!("[ModelIndex] Refreshing index by scanning {:?}", root);
    let scanned = scan_models(&root);

    let mut idx = state.index.lock().unwrap();
    *idx = scanned;
    ModelIndexState::save(&*idx, &state.index_path)?;
    Ok(idx.clone())
}
