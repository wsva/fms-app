//! Stage 2b: symphonia peak extraction into `waveform/*.json`, which the
//! dictation page draws without decoding the audio again.

use std::fs;

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::settings::SettingsState;

use super::find_dataset_dir_write;

// The two generation commands are desktop-only, so their imports stay behind the
// same gate rather than warning in the mobile build.
#[cfg(feature = "desktop")]
use std::path::Path;
#[cfg(feature = "desktop")]
use rusqlite::Connection;
#[cfg(feature = "desktop")]
use tauri::Emitter;
#[cfg(feature = "desktop")]
use super::{DatasetProgress, list_media_files, sibling_path};

/// Delete waveform JSON files for a dataset.
#[allow(dead_code)]
#[tauri::command]
pub async fn dataset_delete_waveforms(
    _app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<(), String> {
    let dataset_dir = find_dataset_dir_write(&settings, &uuid)?;
    let waveform_dir = dataset_dir.join("waveform");
    log::info!("Deleting waveforms for dataset '{}'", uuid);

    if waveform_dir.exists() {
        fs::remove_dir_all(&waveform_dir).map_err(|e| format!("Failed to remove waveforms: {}", e))?;
    }

    Ok(())
}

/// audiowaveform-compatible waveform JSON (v2 layout). Serialised from the
/// peaks computed in `audio::generate_waveform`; consumed by the frontend
/// `WaveformCanvas` and by `datasets::dictation::adjust::load_waveform`.
#[allow(dead_code)]
#[derive(Serialize)]
pub(crate) struct WaveformJson<'a> {
    version: u8,
    channels: u8,
    sample_rate: u32,
    samples_per_pixel: u32,
    bits: u8,
    length: usize,
    data: &'a [i8],
}

/// Generate waveform JSON files for all media files in a dataset.
///
/// Peaks are computed in pure Rust via symphonia (see `audio::generate_waveform`),
/// so no external `audiowaveform` binary is required and video containers are
/// supported (their audio track is decoded). The peaks are written to `waveform/*.json`,
/// which is the sole waveform read path (`listen_get_waveform`).
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn dataset_generate_waveform(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<usize, String> {
    let dataset_dir = find_dataset_dir_write(&settings, &uuid)?;
    let media_dir = dataset_dir.join("media");
    let waveform_dir = dataset_dir.join("waveform");
    fs::create_dir_all(&waveform_dir).map_err(|e| e.to_string())?;

    let media_files = list_media_files(&media_dir);
    let total = media_files.len();

    for (i, mf) in media_files.iter().enumerate() {
        log::info!("[waveform] ({}/{}) Processing: {}", i + 1, total, mf.name);
        let _ = app.emit(
            "dataset-progress",
            DatasetProgress {
                uuid: uuid.clone(),
                current_file: mf.name.clone(),
                file_index: i + 1,
                total_files: total,
                stage: "waveform".into(),
            },
        );

        let peaks = crate::audio::generate_waveform(Path::new(&mf.path), 100)
            .map_err(|e| format!("Waveform generation failed for {}: {}", mf.name, e))?;

        let wf = WaveformJson {
            version: peaks.version,
            channels: peaks.channels,
            sample_rate: peaks.sample_rate,
            samples_per_pixel: peaks.samples_per_pixel,
            bits: peaks.bits,
            length: peaks.data.len() / 2,
            data: &peaks.data,
        };

        // Mirror the media sub-directory under waveform/ before writing.
        let output_path = sibling_path(&media_dir, &waveform_dir, Path::new(&mf.path), "json");
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let file = fs::File::create(&output_path)
            .map_err(|e| format!("Failed to create {}: {}", output_path.display(), e))?;
        serde_json::to_writer(file, &wf).map_err(|e| e.to_string())?;
    }

    log::info!("[waveform] Complete: {} files processed", total);
    Ok(total)
}

/// Generate waveform for a single media file (by media_uuid).
/// Returns the source path of the processed media.
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn dataset_generate_waveform_single(
    settings: State<'_, SettingsState>,
    uuid: String,
    media_uuid: String,
) -> Result<String, String> {
    let dataset_dir = find_dataset_dir_write(&settings, &uuid)?;
    let db_path = dataset_dir.join("data.sqlite3");
    if !db_path.exists() {
        return Err("Database not found. Please build the database first.".into());
    }

    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;

    // Look up the media source path
    let source: String = conn
        .query_row(
            "SELECT source FROM listen_media WHERE uuid = ?1",
            rusqlite::params![media_uuid],
            |row| row.get(0),
        )
        .map_err(|e| format!("Media not found: {}", e))?;

    let media_dir = dataset_dir.join("media");
    let waveform_dir = dataset_dir.join("waveform");
    let media_path = media_dir.join(&source);

    if !media_path.exists() {
        return Err(format!("Media file not found: {}", source));
    }

    log::info!("[waveform] Processing: {}", source);

    let peaks = crate::audio::generate_waveform(&media_path, 100)
        .map_err(|e| format!("Waveform generation failed for {}: {}", source, e))?;

    let wf = WaveformJson {
        version: peaks.version,
        channels: peaks.channels,
        sample_rate: peaks.sample_rate,
        samples_per_pixel: peaks.samples_per_pixel,
        bits: peaks.bits,
        length: peaks.data.len() / 2,
        data: &peaks.data,
    };

    // Mirror the media sub-directory under waveform/
    let output_path = sibling_path(&media_dir, &waveform_dir, Path::new(&source), "json");
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let file = fs::File::create(&output_path)
        .map_err(|e| format!("Failed to create {}: {}", output_path.display(), e))?;
    serde_json::to_writer(file, &wf).map_err(|e| e.to_string())?;

    log::info!("[waveform] Done: {}", source);
    Ok(source)
}
