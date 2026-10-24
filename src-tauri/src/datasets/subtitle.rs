//! Stage 2a: STT transcription of media files into `subtitle/*.vtt`, and the VTT
//! sentence shaping that decides cue boundaries.

use std::fs;

use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::settings::SettingsState;
use super::touch_info;

use super::find_dataset_dir_write;

// Everything below is reached from the desktop-only STT pass, so an ungated import
// would warn in the mobile build.
#[cfg(feature = "desktop")]
use std::path::{Path, PathBuf};
#[cfg(feature = "desktop")]
use chrono::Utc;
#[cfg(feature = "desktop")]
use tauri::Emitter;
#[cfg(feature = "desktop")]
use uuid::Uuid;
#[cfg(feature = "desktop")]
use crate::models::ModelState;
#[cfg(feature = "desktop")]
use super::{
    MediaFile, create_db_schema, list_media_files, parse_vtt, rel_source_string, sibling_path,
};

// ---------------------------------------------------------------------------
// Stage 2: Subtitle + Waveform generation
// ---------------------------------------------------------------------------
#[allow(dead_code)]
#[derive(Clone, Serialize)]
pub struct DatasetProgress {
    pub uuid: String,
    pub current_file: String,
    pub file_index: usize,
    pub total_files: usize,
    pub stage: String,
}

/// Format seconds as HH:MM:SS.mmm for VTT.
#[allow(dead_code)]
pub(crate) fn format_vtt_time(seconds: f32) -> String {
    let total_ms = (seconds * 1000.0).round() as u64;
    let h = total_ms / 3_600_000;
    let m = (total_ms % 3_600_000) / 60_000;
    let s = (total_ms % 60_000) / 1000;
    let ms = total_ms % 1000;
    format!("{:02}:{:02}:{:02}.{:03}", h, m, s, ms)
}

/// Check if a character is a sentence-ending punctuation.
#[allow(dead_code)]
pub(crate) fn is_sentence_end(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '\u{3002}' | '\u{ff01}' | '\u{ff1f}' | '\u{2026}') // 。！？！…
}

/// Merge word/token-level segments into sentence-level cues.
/// Splits on sentence-ending punctuation (. ! ? etc.).
/// Segments are concatenated directly (no extra spaces) because the model's
/// token output already encodes spacing (e.g. " Bekannt" has a leading space).
#[cfg(feature = "desktop")]
pub(crate) fn merge_to_sentences(segments: &[transcribe_rs::TranscriptionSegment]) -> Vec<transcribe_rs::TranscriptionSegment> {
    if segments.is_empty() {
        return Vec::new();
    }

    let mut sentences: Vec<transcribe_rs::TranscriptionSegment> = Vec::new();
    let mut current_text = String::new();
    let mut start: f32 = segments[0].start;
    let mut end: f32 = segments[0].end;

    for seg in segments {
        if current_text.is_empty() {
            start = seg.start;
        }
        end = seg.end;

        // Concatenate directly — model tokens already contain proper spacing
        current_text.push_str(&seg.text);

        // Check if this segment ends with sentence-ending punctuation
        let trimmed_end = seg.text.trim_end();
        if trimmed_end.ends_with(|c: char| is_sentence_end(c)) {
            // Trim leading/trailing whitespace for clean subtitle display
            let clean = current_text.trim().to_string();
            sentences.push(transcribe_rs::TranscriptionSegment {
                start,
                end,
                text: clean,
            });
            current_text.clear();
        }
    }

    // Flush remaining text as the last sentence
    if !current_text.is_empty() {
        let clean = current_text.trim().to_string();
        if !clean.is_empty() {
            sentences.push(transcribe_rs::TranscriptionSegment {
                start,
                end,
                text: clean,
            });
        }
    }

    sentences
}

/// Generate VTT content from timed segments, merging word-level into sentences.
#[cfg(feature = "desktop")]
pub(crate) fn segments_to_vtt(segments: &[transcribe_rs::TranscriptionSegment]) -> String {
    let sentences = merge_to_sentences(segments);
    let mut vtt = String::from("WEBVTT\n\n");
    for seg in &sentences {
        vtt.push_str(&format!(
            "{} --> {}\n{}\n\n",
            format_vtt_time(seg.start),
            format_vtt_time(seg.end),
            seg.text,
        ));
    }
    vtt
}

/// Generate subtitles (VTT files) for the media files in a dataset.
///
/// Incremental: media that already have a subtitle (mirrored under `subtitle/`,
/// e.g. `media/a/b.mp3` -> `subtitle/a/b.vtt`) are skipped, so re-running only
/// transcribes newly added media. Use `dataset_delete_subtitles` to force a full
/// regeneration. After transcription, every media that has a VTT is also imported
/// into `data.sqlite3` — an active `listen_subtitle` plus cues (version 1), matched
/// to its `listen_media` row by source and written idempotently (an existing active
/// subtitle is left untouched; a missing media row is registered on the fly). This
/// makes generating subtitles alone enough to produce a practice-ready DB, so no
/// separate build-database pass is required; `dataset_sync_media` stays the
/// dedicated reconciler for media removals. Returns a short summary for the UI log.
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn dataset_generate_subtitles(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    model_state: State<'_, ModelState>,
    uuid: String,
) -> Result<String, String> {
    let dataset_dir = find_dataset_dir_write(&settings, &uuid)?;
    let media_dir = dataset_dir.join("media");
    let subtitle_dir = dataset_dir.join("subtitle");

    // Keep any existing subtitles so already-transcribed media can be skipped.
    fs::create_dir_all(&subtitle_dir).map_err(|e| e.to_string())?;

    let media_files = list_media_files(&media_dir);
    log::info!("Generating subtitles for dataset: {} media file(s) found", media_files.len());

    // Partition into media still needing a VTT and those that already have one.
    let mut skipped = 0usize;
    let mut pending: Vec<(&MediaFile, PathBuf)> = Vec::new();
    for mf in media_files.iter() {
        let vtt_path = sibling_path(&media_dir, &subtitle_dir, Path::new(&mf.path), "vtt");
        if vtt_path.exists() {
            skipped += 1;
        } else {
            pending.push((mf, vtt_path));
        }
    }

    let total = pending.len();
    for (i, (mf, vtt_path)) in pending.iter().enumerate() {
        let file_path = Path::new(&mf.path);

        let _ = app.emit(
            "dataset-progress",
            DatasetProgress {
                uuid: uuid.clone(),
                current_file: mf.name.clone(),
                file_index: i + 1,
                total_files: total,
                stage: "subtitles".into(),
            },
        );

        let result = crate::models::transcribe::transcribe_file(&model_state, file_path)?;

        // Mirror the media sub-directory under subtitle/ before writing.
        if let Some(parent) = vtt_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let vtt_content = segments_to_vtt(result.segments.as_deref().unwrap_or_default());
        fs::write(vtt_path, vtt_content).map_err(|e| e.to_string())?;
    }

    // Persist subtitles into the dataset DB so the pipeline needs no separate
    // "build database" pass: every media that now has a VTT gets an active
    // listen_subtitle + cues (version 1) keyed by source, idempotently. A missing
    // listen_media row is registered on the fly, so the standalone "Generate
    // Subtitles" action still yields a practice-ready DB; dataset_sync_media stays
    // the dedicated reconciler for removals.
    let db_path = dataset_dir.join("data.sqlite3");
    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
    create_db_schema(&conn)?;

    // source (media-relative) -> media_uuid, from the current inventory.
    let mut stmt = conn
        .prepare("SELECT uuid, source FROM listen_media")
        .map_err(|e| e.to_string())?;
    let mut media_map: std::collections::HashMap<String, String> = stmt
        .query_map([], |row| {
            let u: String = row.get(0)?;
            let s: String = row.get(1)?;
            Ok((s, u))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    let now = Utc::now().to_rfc3339();
    let mut cues_written = 0usize;
    for mf in media_files.iter() {
        let file_path = Path::new(&mf.path);
        let vtt_path = sibling_path(&media_dir, &subtitle_dir, file_path, "vtt");
        if !vtt_path.exists() {
            continue;
        }
        let source = rel_source_string(&media_dir, file_path);
        let media_uuid = match media_map.get(&source) {
            Some(u) => u.clone(),
            None => {
                let u = Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO listen_media (uuid, source, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![u, source, now, now],
                )
                .map_err(|e| e.to_string())?;
                media_map.insert(source.clone(), u.clone());
                u
            }
        };

        // Idempotent: leave an existing active subtitle untouched.
        let existing: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM listen_subtitle WHERE media_uuid = ?1 AND is_active = 1",
                [media_uuid.as_str()],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if existing > 0 {
            continue;
        }

        let cues = parse_vtt(&vtt_path)?;
        let name = file_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let subtitle_uuid = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO listen_subtitle (uuid, media_uuid, name, track_type, version, is_active, created_at, updated_at) VALUES (?1, ?2, ?3, 'stt', 1, 1, ?4, ?5)",
            rusqlite::params![subtitle_uuid, media_uuid, name, now, now],
        )
        .map_err(|e| e.to_string())?;
        let version_uuid = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO listen_subtitle_version (uuid, subtitle_uuid, version, change_type, description, cues_added, created_by) VALUES (?1, ?2, 1, 'initial', 'Initial STT transcription', ?3, 'system')",
            rusqlite::params![version_uuid, subtitle_uuid, cues.len() as i64],
        )
        .map_err(|e| e.to_string())?;
        for (order, cue) in cues.iter().enumerate() {
            let cue_uuid = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO listen_subtitle_cue (uuid, subtitle_uuid, order_num, start_ms, end_ms, content, version_created) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)",
                rusqlite::params![cue_uuid, subtitle_uuid, order as i64, cue.start_ms, cue.end_ms, cue.content],
            )
            .map_err(|e| e.to_string())?;
        }
        cues_written += 1;
    }
    drop(conn);

    // Update timestamp when either VTT files or DB subtitle rows changed.
    if total > 0 || cues_written > 0 {
        touch_info(&dataset_dir)?;
    }

    Ok(format!(
        "Generated {} subtitle(s), skipped {} already present; wrote {} subtitle row(s) to the database.",
        total, skipped, cues_written
    ))
}

/// Generate subtitle for a single media file using the loaded STT model.
/// Looks up the media source path from the database, transcribes, and writes VTT.
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn dataset_generate_subtitle_single(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    model_state: State<'_, ModelState>,
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
    let subtitle_dir = dataset_dir.join("subtitle");
    let media_path = media_dir.join(&source);

    if !media_path.exists() {
        return Err(format!("Media file not found: {}", source));
    }

    log::info!("[subtitle_single] Processing: {}", source);

    let _ = app.emit(
        "dataset-progress",
        DatasetProgress {
            uuid: uuid.clone(),
            current_file: Path::new(&source)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| source.clone()),
            file_index: 1,
            total_files: 1,
            stage: "subtitles".into(),
        },
    );

    let result = crate::models::transcribe::transcribe_file(&model_state, &media_path)?;

    // Mirror the media sub-directory under subtitle/
    let vtt_path = sibling_path(&media_dir, &subtitle_dir, Path::new(&source), "vtt");
    if let Some(parent) = vtt_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let vtt_content = segments_to_vtt(result.segments.as_deref().unwrap_or_default());
    fs::write(&vtt_path, vtt_content).map_err(|e| e.to_string())?;

    log::info!("[subtitle_single] Done: {}", source);
    Ok(source)
}

/// Delete subtitles (VTT files) for a dataset and reset status to only_media.
/// Also removes subtitle data from the database if it exists.
#[allow(dead_code)]
#[tauri::command]
pub async fn dataset_delete_subtitles(
    _app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<(), String> {
    let dataset_dir = find_dataset_dir_write(&settings, &uuid)?;
    let subtitle_dir = dataset_dir.join("subtitle");

    if subtitle_dir.exists() {
        log::info!("Deleting subtitles for dataset '{}'", uuid);
        fs::remove_dir_all(&subtitle_dir).map_err(|e| format!("Failed to remove subtitles: {}", e))?;
    }

    // Also delete subtitle data from database if it exists
    let db_path = dataset_dir.join("data.sqlite3");
    if db_path.exists() {
        let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM listen_subtitle_cue", [])
            .map_err(|e| format!("Failed to clear subtitle cues: {}", e))?;
        conn.execute("DELETE FROM listen_subtitle", [])
            .map_err(|e| format!("Failed to clear subtitles: {}", e))?;
        drop(conn);
    }

    // Update timestamp
    touch_info(&dataset_dir)?;

    Ok(())
}
