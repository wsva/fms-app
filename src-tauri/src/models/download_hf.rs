//! HuggingFace catalog download for the non-STT models (OCR / TTS / split /
//! phoneme). `catalog::MODELS` entries are file-list downloads from a HF repo,
//! driven by the `ModelDownloader` in `download.rs`.
//!
//! Progress + cancellation reuse the same `ModelState.download_progress` /
//! `cancel_flags` plumbing (and the `model-download-progress` event) the STT
//! Models page already speaks, so the existing UI works for these too. Files
//! land under `{model_root}/candle/{folder}` (see [`hf_models_root`] for the one
//! exception), exactly where `index::scan_models` looks.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tauri::{AppHandle, Emitter};

use crate::models::catalog;
use crate::models::download;
use crate::models::index::{self, ModelIndexEntry, ModelIndexState};
use crate::models::state::{FileDownloadInfo, ModelState, ModelStatus};

/// Root a catalog model's folder is resolved against.
///
/// Delegates to [`index::catalog_models_root`], which also drives the index
/// scan, so a download and a later rebuild agree on where a model lives. The
/// read-aloud phoneme model is the one folder that is not under `candle/`.
fn hf_models_root(def: &catalog::HfModelDef) -> PathBuf {
    index::catalog_models_root(&index::model_root(), def)
}

/// Directory a `catalog::HfModelDef`'s files are stored in.
pub fn hf_model_dir(def: &catalog::HfModelDef) -> PathBuf {
    index::catalog_model_dir(&index::model_root(), def)
}

/// `save_dir` to hand `ModelDownloader` so its `save_dir + repo_id` layout
/// resolves back to `hf_model_dir(def)`.
fn hf_save_dir(def: &catalog::HfModelDef) -> PathBuf {
    let base = hf_models_root(def);
    match def.folder.strip_suffix(def.id) {
        Some(prefix) => base.join(prefix.trim_end_matches('/')),
        None => base,
    }
}

/// Whether a HF catalog model's files are present on disk. Requires every
/// listed file plus (when `tokenizer_repo` / `vocab_repo` are set) the
/// separately-fetched `tokenizer.json` / `vocab.json`, so a partial download
/// reads as "not downloaded" and is re-completable rather than getting stuck.
pub fn hf_downloaded_on_disk(def: &catalog::HfModelDef) -> bool {
    let dir = hf_model_dir(def);
    let files_ok = !def.files.is_empty() && def.files.iter().all(|f| dir.join(f.path).exists());
    let tokenizer_ok = match def.tokenizer_repo {
        Some(_) => dir.join("tokenizer.json").exists(),
        None => true,
    };
    let vocab_ok = match def.vocab_repo {
        Some(_) => dir.join("vocab.json").exists(),
        None => true,
    };
    files_ok && tokenizer_ok && vocab_ok
}

/// Fetch one file that a catalog entry needs but its own repo does not carry
/// (a backbone tokenizer, the full phone vocabulary) and move it into the
/// model directory.
///
/// `ModelDownloader` downloads one repo per call, so the file is fetched into a
/// throwaway directory first. Any failure clears that directory and fails the
/// whole download: a model missing its vocabulary would otherwise read as
/// complete right up to the first scoring attempt.
async fn fetch_extra_file(
    def: &catalog::HfModelDef,
    provider: download::HubProvider,
    repo: &str,
    file: &str,
    app: &AppHandle,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    let target = hf_model_dir(def);
    let tmp = target.join("_extra");
    let _ = std::fs::remove_dir_all(&tmp);

    let result = download::ModelDownloader::new(provider)?
        .download(
            download::DownloadOptions {
                repo_id: repo.to_string(),
                revision: None,
                save_dir: tmp.clone(),
                files: Some(vec![file.to_string()]),
            },
            Some(app),
            None,
            Some(cancel),
        )
        .await;
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = app.emit("model-status-changed", ());
        return Err(format!("Failed to fetch '{}' from '{}': {}", file, repo, e));
    }

    // `ModelDownloader` nests files under `save_dir/<repo segments>/…`.
    let landed = repo.split('/').fold(tmp.clone(), |p, c| p.join(c)).join(file);
    let dst = target.join(file);
    let moved = std::fs::rename(&landed, &dst)
        .or_else(|_| std::fs::copy(&landed, &dst).map(|_| ()));
    let _ = std::fs::remove_dir_all(&tmp);
    moved.map_err(|e| format!("Failed to place '{}': {}", file, e))
}

/// Map a frontend provider key (from the model-card selector) to a
/// `HubProvider`. Absent or unrecognized keys default to Hugging Face.
fn hub_provider_from_key(key: Option<&str>) -> download::HubProvider {
    match key {
        Some("hf-mirror") => download::HubProvider::HfMirror { token: None },
        Some("modelscope") => download::HubProvider::ModelScope { token: None },
        _ => download::HubProvider::HuggingFace { token: None },
    }
}

/// Canonical provider label recorded in the model index for a given key.
fn provider_label(key: Option<&str>) -> &'static str {
    match key {
        Some("hf-mirror") => "hf-mirror",
        Some("modelscope") => "modelscope",
        _ => "huggingface",
    }
}

/// Body of the `model_download_hf` command, shared with the MCP tool of the
/// same name — the agent path holds an `AppHandle` and the managed states, not
/// command parameters.
pub async fn hf_download_inner(
    app: AppHandle,
    state: &ModelState,
    index: &ModelIndexState,
    id: &str,
    provider: Option<&str>,
) -> Result<(), String> {
    let def = catalog::find_model(id).ok_or_else(|| format!("Unknown model: {}", id))?;

    if hf_downloaded_on_disk(def) {
        return Err("Model already downloaded".into());
    }
    {
        let statuses = state.download_status.lock().unwrap();
        if statuses.values().any(|s| *s == ModelStatus::Downloading) {
            return Err("A download is already in progress".into());
        }
    }

    log::info!("Starting HF download of model '{}' ({} MB)", id, def.size_mb);
    {
        let mut p = state.download_progress.lock().unwrap();
        p.files = def
            .files
            .iter()
            .map(|f| FileDownloadInfo {
                file: f.path.to_string(),
                bytes_downloaded: 0,
                total_bytes: (f.size_bytes > 0).then_some(f.size_bytes),
                speed: 0,
                eta_seconds: None,
            })
            .collect();
        p.overall_bytes_downloaded = 0;
        p.overall_total_bytes = def.size_mb * 1024 * 1024;
    }

    let cancel_flag = Arc::new(AtomicBool::new(false));
    state.cancel_flags.lock().unwrap().insert(id.to_string(), cancel_flag.clone());

    let provider_enum = hub_provider_from_key(provider);
    log::info!(
        "HF download of '{}' requested via provider '{}'",
        id,
        provider_label(provider)
    );
    let downloader = download::ModelDownloader::new(provider_enum.clone())?;
    let options = download::DownloadOptions {
        repo_id: id.to_string(),
        revision: None,
        save_dir: hf_save_dir(def),
        files: Some(def.files.iter().map(|f| f.path.to_string()).collect()),
    };

    let result = downloader
        .download(options, Some(&app), Some(&state.download_progress), Some(&cancel_flag))
        .await;

    state.cancel_flags.lock().unwrap().remove(id);

    if let Err(e) = result {
        log::error!("HF download failed for '{}': {}", id, e);
        let _ = app.emit("model-status-changed", ());
        return Err(e);
    }

    // Some ONNX repos (e.g. segment-any-text/sat-3l-sm) ship weights only; the
    // tokenizer lives in the backbone repo. Fetch it into the same directory so
    // the model is complete and `is_downloaded_dir` / `SaT::from_dir` succeed.
    if let Some(tok_repo) = def.tokenizer_repo {
        fetch_extra_file(def, provider_enum.clone(), tok_repo, "tokenizer.json", &app, &cancel_flag)
            .await?;
    }
    // The phoneme repo carries per-language, pruned phone maps; the scorer needs
    // the full inventory so every id it emits has a label.
    if let Some(vocab_repo) = def.vocab_repo {
        fetch_extra_file(def, provider_enum, vocab_repo, "vocab.json", &app, &cancel_flag).await?;
    }

    // Register in the unified index so it is reported as downloaded everywhere.
    let root = index::model_root();
    let files = index::collect_files(&hf_model_dir(def), &root);
    let key = index::make_key(def.model_type, id);
    let entry = ModelIndexEntry {
        model_type: def.model_type.to_string(),
        id: id.to_string(),
        name: def.name.to_string(),
        downloaded_at: Some(chrono::Utc::now().to_rfc3339()),
        provider: Some(provider_label(provider).to_string()),
        files,
    };
    if let Err(e) = index::upsert_entry(index, key, entry) {
        log::warn!("Failed to update model index after HF download: {}", e);
    }

    log::info!("Model '{}' downloaded successfully", id);
    let _ = app.emit("model-status-changed", ());
    Ok(())
}
