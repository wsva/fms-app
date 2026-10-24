//! STT model download: one blob (or one directory blob) per model, from the
//! CDN listed in `catalog_stt`. The HuggingFace catalog path lives in
//! `download_hf`; both share the progress/cancel plumbing in `ModelState`.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tauri::AppHandle;

use crate::models::catalog_stt;
use crate::models::download;
use crate::models::index::{self, ModelIndexEntry, ModelIndexState};
use crate::models::state::{FileDownloadInfo, ModelState, ModelStatus};
use crate::settings::SettingsState;

pub async fn model_download_inner(
    app: AppHandle,
    state: &ModelState,
    _settings: &SettingsState,
    index: &ModelIndexState,
    version: String,
) -> Result<(), String> {
    let def = catalog_stt::find_model(&version)
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

    match download::download_blob_model(
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
    let root = index::model_root();
    let files = index::collect_files(&model_dir, &root);
    let key = index::make_key("stt", &version);
    let entry = ModelIndexEntry {
        model_type: "stt".to_string(),
        id: version.clone(),
        name: def.name.to_string(),
        downloaded_at: Some(chrono::Utc::now().to_rfc3339()),
        provider: Some("cdn".to_string()),
        files,
    };
    if let Err(e) = index::upsert_entry(index, key, entry) {
        log::warn!("Failed to update model index after download: {}", e);
    }

    log::info!("Model '{}' downloaded successfully", version);
    Ok(())
}
