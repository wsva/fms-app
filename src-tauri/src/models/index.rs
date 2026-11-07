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

/// Root a catalog model's `folder` is resolved against, under `root`.
///
/// Nearly every entry lives under `{root}/candle/`. The read-aloud phoneme model
/// is the exception: its consumer
/// (`datasets::read_aloud::phoneme::find_model_dir`) scans `{root}/phoneme/`, the
/// same place a hand-installed model goes — so a downloaded model and a
/// dropped-in one are indistinguishable to the scorer.
pub fn catalog_models_root(root: &Path, def: &catalog::HfModelDef) -> PathBuf {
    if def.folder.starts_with("phoneme/") {
        root.to_path_buf()
    } else {
        root.join("candle")
    }
}

/// Directory a catalog model's files are stored in, under `root`.
pub fn catalog_model_dir(root: &Path, def: &catalog::HfModelDef) -> PathBuf {
    catalog_models_root(root, def).join(def.folder)
}

/// Scan the model directory and rebuild the index from what's on disk.
pub fn scan_models(root: &Path) -> ModelIndex {
    let mut index = ModelIndex {
        version: 1,
        models: HashMap::new(),
    };

    if !root.exists() {
        return index;
    }

    // 1. Scan STT models (known IDs from the generated catalog)
    for def in catalog_stt::all() {
        let model_dir = root.join(&def.id);
        if !model_dir.exists() {
            continue;
        }

        let files = collect_files(&model_dir, root);

        if !files.is_empty() {
            let key = make_key("stt", &def.id);
            index.models.insert(key, ModelIndexEntry {
                model_type: "stt".to_string(),
                id: def.id.clone(),
                name: def.name.clone(),
                downloaded_at: None,
                provider: Some("cdn".to_string()),
                files,
            });
        }
    }

    // 1b. The outer tier: artifacts no catalog row claims. Additive — a row that
    //     already exists is never shadowed — and reporting-only, since loading
    //     names a curated row. See [`discover_stt`].
    for (id, artifact) in discover_stt(root) {
        let key = make_key("stt", &id);
        if index.models.contains_key(&key) {
            continue;
        }
        // Ask the bytes what they are; a `.bin`, a corrupt header and a file we
        // cannot open all fall back to the name on disk rather than a guess.
        let probed = crate::models::gguf_meta::GgufProbe::read(&artifact);
        let name = probed
            .as_ref()
            .and_then(|p| p.display_name.clone())
            .filter(|n| !n.trim().is_empty())
            .or_else(|| probed.as_ref().and_then(|p| p.architecture.clone()))
            .unwrap_or_else(|| artifact.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(id.clone()));
        let files = discovered_files(&artifact, root);
        if files.is_empty() {
            continue;
        }
        index.models.insert(key, ModelIndexEntry {
            model_type: "stt".to_string(),
            id,
            name,
            downloaded_at: None,
            provider: Some("local".to_string()),
            files,
        });
    }

    // 2. Scan HuggingFace models from catalog (candle-ocr, candle-tts, etc.)
    for def in catalog::MODELS {
        let model_dir = catalog_model_dir(root, def);
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

/// Extensions a hand-dropped STT artifact can carry. `.gguf` is transcribe-cpp's
/// current container and `.bin` its GGML one; nothing else on disk is an STT
/// model recognisable by name alone (ONNX exports are directories of `.onnx`
/// files and are catalog rows by construction).
const STT_ARTIFACT_EXTS: &[&str] = &["gguf", "bin"];

/// Directories that hold other catalogs' models. `candle/` is the HuggingFace
/// layout and `phoneme/` the forced-aligner's, so a `.bin` in either is a
/// vocabulary or a weight of some other kind, not an STT model.
const NON_STT_DIRS: &[&str] = &["candle", "phoneme"];

/// Name an STT artifact that no catalog row claims, plus the file it was found
/// in. `root`'s own layout is ours — `<model_root>/<id>/…` — so a directory's
/// name is the id and a loose file's stem is the id.
///
/// This is the outer tier of discovery and it is deliberately only a report:
/// loading goes through `catalog_stt::find_model`, and a row is a curated
/// decision made in `scripts/gen_catalog.py` (with a pinned revision and a hash),
/// not something a stray file can earn. That is the whole point of listing it
/// here — a family shows up in the index, somebody runs it, and only then
/// becomes a row.
fn discover_stt(root: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return found,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().map(|n| n.to_string_lossy().to_string()) {
            Some(n) => n,
            None => continue,
        };
        if name.starts_with('.') || NON_STT_DIRS.contains(&name.as_str()) {
            continue;
        }
        // A catalog row's own directory is step 1's business, whether or not the
        // files inside it passed the integrity check.
        if catalog_stt::find_model(&name).is_some() {
            continue;
        }
        if path.is_dir() {
            if let Some(artifact) = first_artifact(&path) {
                found.push((name, artifact));
            }
        } else if is_stt_artifact(&path) {
            let id = match path.file_stem() {
                Some(s) => s.to_string_lossy().to_string(),
                None => continue,
            };
            found.push((id, path));
        }
    }
    found
}

fn is_stt_artifact(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| STT_ARTIFACT_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// The first artifact inside a hand-made directory, in name order so the answer
/// does not depend on the filesystem's mood.
fn first_artifact(dir: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_stt_artifact(p))
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}

/// The files to record for a discovered artifact: the whole directory it sits
/// in, or the loose file itself when it sits at the root.
fn discovered_files(artifact: &Path, root: &Path) -> HashMap<String, u64> {
    let mut map = HashMap::new();
    if artifact.is_file() && artifact.parent() == Some(root) {
        if let Ok(rel) = artifact.strip_prefix(root) {
            if let Ok(meta) = artifact.metadata() {
                map.insert(rel.to_string_lossy().replace('\\', "/"), meta.len());
            }
        }
        return map;
    }
    match artifact.parent() {
        Some(dir) => collect_files(dir, root),
        None => map,
    }
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
