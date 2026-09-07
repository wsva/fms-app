use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::settings::SettingsState;
use crate::model::ModelState;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
pub struct DatasetInfo {
    pub name: String,
    pub uuid: String,
    pub description: String,
    pub parent_uuid: String,
    pub version: u32,
    pub structure: String,
    pub updated: String,
}

#[derive(Clone, Serialize)]
pub struct MediaFile {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub has_transcript: bool,
}

#[derive(Clone, Serialize)]
pub struct DatasetSummary {
    pub info: DatasetInfo,
    pub media_count: usize,
    pub path: String,
    pub status: String,
}

#[derive(Clone, Serialize)]
pub struct DatasetDetail {
    pub info: DatasetInfo,
    pub media: Vec<MediaFile>,
    pub has_subtitles: bool,
    pub has_waveforms: bool,
    pub has_database: bool,
    pub has_book: bool,
    pub status: String,
}

pub struct DatasetState;

impl DatasetState {
    pub fn new() -> Self {
        Self
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const AUDIO_EXTENSIONS: &[&str] = &["mp3", "wav", "flac", "ogg"];

fn is_audio_file(name: &str) -> bool {
    AUDIO_EXTENSIONS
        .iter()
        .any(|ext| name.to_lowercase().ends_with(&format!(".{}", ext)))
}

fn datasets_dir(settings: &SettingsState) -> PathBuf {
    PathBuf::from(settings.settings.lock().unwrap().datasets_dir.clone())
}

fn list_media_files(media_dir: &PathBuf) -> Vec<MediaFile> {
    if !media_dir.exists() {
        return Vec::new();
    }

    let transcript_dir = media_dir.parent().unwrap().join("transcript");

    let mut files: Vec<MediaFile> = fs::read_dir(media_dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| is_audio_file(n))
                .unwrap_or(false)
        })
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path().to_string_lossy().into_owned();
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);

            // Check for matching transcript
            let file_path = e.path();
            let stem = file_path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let has_transcript = transcript_dir.join(format!("{}.txt", stem)).exists();

            MediaFile {
                name,
                path,
                size,
                has_transcript,
            }
        })
        .collect();

    files.sort_by(|a, b| a.name.cmp(&b.name));
    files
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// List all datasets in the configured datasets directory.
#[tauri::command]
pub async fn dataset_list(
    state: State<'_, DatasetState>,
    settings: State<'_, SettingsState>,
) -> Result<Vec<DatasetSummary>, String> {
    let _ = state;
    let dir = datasets_dir(&settings);

    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut datasets = Vec::new();

    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let info_path = path.join("info.json");
        if info_path.exists() {
            let data = fs::read_to_string(&info_path).map_err(|e| e.to_string())?;
            let info: DatasetInfo = serde_json::from_str(&data).map_err(|e| e.to_string())?;
            let media_dir = path.join("media");
            let media_count = list_media_files(&media_dir).len();
            let status = if path.join("data.sqlite3").exists() { "ready" } else { "not_ready" };
            datasets.push(DatasetSummary { info, media_count, path: path.to_string_lossy().into_owned(), status: status.into() });
        } else {
            // No info.json -- check if it has media files (raw import)
            let media_dir = path.join("media");
            let media_files = list_media_files(&media_dir);
            if !media_files.is_empty() {
                // Report as only_media with a generated UUID
                let info = DatasetInfo {
                    name: path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    uuid: String::new(),
                    description: String::new(),
                    parent_uuid: String::new(),
                    version: 0,
                    structure: "dictation-v1".into(),
                    updated: String::new(),
                };
                datasets.push(DatasetSummary {
                    info,
                    media_count: media_files.len(),
                    path: path.to_string_lossy().into_owned(),
                    status: "not_ready".into(),
                });
            }
        }
    }

    datasets.sort_by(|a, b| a.info.name.cmp(&b.info.name));
    Ok(datasets)
}

/// Import a dataset from a source directory. The directory must contain a `media/`
/// subdirectory with at least one audio file. The entire directory is copied into
/// the managed datasets directory and an `info.json` is generated.
#[tauri::command]
pub async fn dataset_import(
    settings: State<'_, SettingsState>,
    source_dir: String,
) -> Result<DatasetSummary, String> {
    let src = PathBuf::from(&source_dir);
    if !src.exists() || !src.is_dir() {
        return Err("Source directory does not exist".into());
    }

    let media_dir = src.join("media");
    if !media_dir.exists() {
        return Err("Source directory must contain a 'media' subdirectory".into());
    }

    let media_files = list_media_files(&media_dir);
    if media_files.is_empty() {
        return Err("No audio files found in 'media' directory".into());
    }

    let dir = datasets_dir(&settings);
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create datasets dir: {}", e))?;

    let uuid = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();

    // Use the source directory name as the dataset folder name
    let dir_name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| uuid.clone());

    let dst = dir.join(&dir_name);
    if dst.exists() {
        return Err(format!("Dataset '{}' already exists", dir_name));
    }

    // Copy the entire source directory
    copy_dir_recursive(&src, &dst)?;

    let name = dir_name.clone();
    let media_count = media_files.len();

    let info = DatasetInfo {
        name,
        uuid: uuid.clone(),
        description: String::new(),
        parent_uuid: String::new(),
        version: 1,
        structure: "dictation-v1".into(),
        updated: now,
    };

    // Write info.json
    let info_path = dst.join("info.json");
    let data = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
    fs::write(&info_path, data).map_err(|e| e.to_string())?;

    Ok(DatasetSummary { info, media_count, path: dst.to_string_lossy().into_owned(), status: "not_ready".into() })
}

/// Get detailed information about a specific dataset, including its media files.
#[tauri::command]
pub async fn dataset_get(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<DatasetDetail, String> {
    let dir = datasets_dir(&settings);

    if !dir.exists() {
        return Err("Datasets directory does not exist".into());
    }

    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let info_path = path.join("info.json");
        if !info_path.exists() {
            continue;
        }

        let data = fs::read_to_string(&info_path).map_err(|e| e.to_string())?;
        let info: DatasetInfo = serde_json::from_str(&data).map_err(|e| e.to_string())?;

        if info.uuid == uuid {
            let media_dir = path.join("media");
            let media = list_media_files(&media_dir);
            let subtitle_dir = path.join("subtitle");
            let has_subtitles = subtitle_dir.exists()
                && fs::read_dir(&subtitle_dir)
                    .map(|mut d| d.next().is_some())
                    .unwrap_or(false);
            let waveform_dir = path.join("waveform");
            let has_waveforms = waveform_dir.exists()
                && fs::read_dir(&waveform_dir)
                    .map(|mut d| d.next().is_some())
                    .unwrap_or(false);
            let has_database = path.join("data.sqlite3").exists();
            let has_book = path.join("book.txt").exists();
            let status = if has_database { "ready" } else { "not_ready" };
            return Ok(DatasetDetail { info, media, has_subtitles, has_waveforms, has_database, has_book, status: status.into() });
        }
    }

    Err(format!("Dataset with UUID {} not found", uuid))
}

/// Update mutable fields of a dataset's info.json (name, description).
#[tauri::command]
pub async fn dataset_update(
    settings: State<'_, SettingsState>,
    uuid: String,
    name: Option<String>,
    description: Option<String>,
) -> Result<(), String> {
    let dir = datasets_dir(&settings);

    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let info_path = path.join("info.json");
        if !info_path.exists() {
            continue;
        }

        let data = fs::read_to_string(&info_path).map_err(|e| e.to_string())?;
        let mut info: DatasetInfo = serde_json::from_str(&data).map_err(|e| e.to_string())?;

        if info.uuid == uuid {
            if let Some(n) = name {
                info.name = n;
            }
            if let Some(d) = description {
                info.description = d;
            }
            info.updated = Utc::now().to_rfc3339();

            let data = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
            fs::write(&info_path, data).map_err(|e| e.to_string())?;
            return Ok(());
        }
    }

    Err(format!("Dataset with UUID {} not found", uuid))
}

/// Delete a dataset by removing its entire directory.
#[tauri::command]
pub async fn dataset_delete(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<(), String> {
    let dir = datasets_dir(&settings);

    if !dir.exists() {
        return Err("Datasets directory does not exist".into());
    }

    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let info_path = path.join("info.json");
        if !info_path.exists() {
            continue;
        }

        let data = fs::read_to_string(&info_path).map_err(|e| e.to_string())?;
        let info: DatasetInfo = serde_json::from_str(&data).map_err(|e| e.to_string())?;

        if info.uuid == uuid {
            fs::remove_dir_all(&path).map_err(|e| e.to_string())?;
            return Ok(());
        }
    }

    Err(format!("Dataset with UUID {} not found", uuid))
}

// ---------------------------------------------------------------------------
// Stage 2: Subtitle + Waveform generation
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
pub struct DatasetProgress {
    pub uuid: String,
    pub current_file: String,
    pub file_index: usize,
    pub total_files: usize,
    pub stage: String,
}

/// Format seconds as HH:MM:SS.mmm for VTT.
fn format_vtt_time(seconds: f32) -> String {
    let total_ms = (seconds * 1000.0).round() as u64;
    let h = total_ms / 3_600_000;
    let m = (total_ms % 3_600_000) / 60_000;
    let s = (total_ms % 60_000) / 1000;
    let ms = total_ms % 1000;
    format!("{:02}:{:02}:{:02}.{:03}", h, m, s, ms)
}

/// Check if a character is a sentence-ending punctuation.
fn is_sentence_end(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '\u{3002}' | '\u{ff01}' | '\u{ff1f}' | '\u{2026}') // 。！？！…
}

/// Merge word/token-level segments into sentence-level cues.
/// Splits on sentence-ending punctuation (. ! ? etc.).
/// Segments are concatenated directly (no extra spaces) because the model's
/// token output already encodes spacing (e.g. " Bekannt" has a leading space).
fn merge_to_sentences(segments: &[transcribe_rs::TranscriptionSegment]) -> Vec<transcribe_rs::TranscriptionSegment> {
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
fn segments_to_vtt(segments: &[transcribe_rs::TranscriptionSegment]) -> String {
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

/// Find the dataset directory by UUID.
pub(crate) fn find_dataset_dir(settings: &SettingsState, uuid: &str) -> Result<PathBuf, String> {
    let dir = datasets_dir(settings);
    if !dir.exists() {
        return Err("Datasets directory does not exist".into());
    }

    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let info_path = path.join("info.json");
        if !info_path.exists() {
            continue;
        }
        let data = fs::read_to_string(&info_path).map_err(|e| e.to_string())?;
        let info: DatasetInfo = serde_json::from_str(&data).map_err(|e| e.to_string())?;
        if info.uuid == uuid {
            return Ok(path);
        }
    }
    Err(format!("Dataset with UUID {} not found", uuid))
}

/// Generate subtitles (VTT files) for all media files in a dataset.
#[tauri::command]
pub async fn dataset_generate_subtitles(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    model_state: State<'_, ModelState>,
    uuid: String,
) -> Result<(), String> {
    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
    let media_dir = dataset_dir.join("media");
    let subtitle_dir = dataset_dir.join("subtitle");

    // Delete old subtitles directory if it exists, so we start fresh.
    if subtitle_dir.exists() {
        fs::remove_dir_all(&subtitle_dir).map_err(|e| format!("Failed to remove old subtitles: {}", e))?;
    }
    fs::create_dir_all(&subtitle_dir).map_err(|e| e.to_string())?;

    let media_files = list_media_files(&media_dir);
    let total = media_files.len();

    for (i, mf) in media_files.iter().enumerate() {
        let file_path = Path::new(&mf.path);
        let stem = file_path.file_stem().unwrap().to_string_lossy();

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

        let result = crate::model::transcribe_file(&model_state, file_path)?;

        let vtt_path = subtitle_dir.join(format!("{}.vtt", stem));
        let vtt_content = segments_to_vtt(result.segments.as_deref().unwrap_or_default());
        fs::write(&vtt_path, vtt_content).map_err(|e| e.to_string())?;
    }

    // Update timestamp
    let info_path = dataset_dir.join("info.json");
    let data = fs::read_to_string(&info_path).map_err(|e| e.to_string())?;
    let mut info: DatasetInfo = serde_json::from_str(&data).map_err(|e| e.to_string())?;
    info.updated = Utc::now().to_rfc3339();
    let data = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
    fs::write(&info_path, data).map_err(|e| e.to_string())?;

    Ok(())
}

/// Delete subtitles (VTT files) for a dataset and reset status to only_media.
#[tauri::command]
pub async fn dataset_delete_subtitles(
    _app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<(), String> {
    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
    let subtitle_dir = dataset_dir.join("subtitle");

    if subtitle_dir.exists() {
        fs::remove_dir_all(&subtitle_dir).map_err(|e| format!("Failed to remove subtitles: {}", e))?;
    }

    // Update timestamp
    let info_path = dataset_dir.join("info.json");
    let data = fs::read_to_string(&info_path).map_err(|e| e.to_string())?;
    let mut info: DatasetInfo = serde_json::from_str(&data).map_err(|e| e.to_string())?;
    info.updated = Utc::now().to_rfc3339();
    let data = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
    fs::write(&info_path, data).map_err(|e| e.to_string())?;

    Ok(())
}

/// Delete waveform JSON files for a dataset.
#[tauri::command]
pub async fn dataset_delete_waveforms(
    _app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<(), String> {
    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
    let waveform_dir = dataset_dir.join("waveform");

    if waveform_dir.exists() {
        fs::remove_dir_all(&waveform_dir).map_err(|e| format!("Failed to remove waveforms: {}", e))?;
    }

    Ok(())
}

/// Generate waveform JSON files for all media files in a dataset.
/// Resolves audiowaveform binary: system PATH first, then bundled sidecar.
#[tauri::command]
pub async fn dataset_generate_waveform(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<(), String> {
    // 1. Check system PATH
    let audiowaveform_bin = if std::process::Command::new("audiowaveform")
        .arg("--version")
        .output()
        .is_ok()
    {
        std::path::PathBuf::from("audiowaveform")
    } else {
        // 2. Check bundled sidecar binary
        let resource_dir = app
            .path()
            .resource_dir()
            .map_err(|e| format!("Failed to resolve resource directory: {}", e))?;

        let sidecar = if resource_dir.exists() {
            fs::read_dir(&resource_dir)
                .ok()
                .and_then(|entries| {
                    entries.filter_map(|e| e.ok()).find_map(|entry| {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.starts_with("audiowaveform-") {
                            Some(entry.path())
                        } else {
                            None
                        }
                    })
                })
        } else {
            None
        };

        sidecar.ok_or_else(|| {
            "audiowaveform not found on system PATH and no sidecar binary bundled. \
             Install audiowaveform or place the binary in src-tauri/binaries/"
                .to_string()
        })?
    };

    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
    let media_dir = dataset_dir.join("media");
    let waveform_dir = dataset_dir.join("waveform");
    fs::create_dir_all(&waveform_dir).map_err(|e| e.to_string())?;

    let media_files = list_media_files(&media_dir);
    let total = media_files.len();

    for (i, mf) in media_files.iter().enumerate() {
        let stem = Path::new(&mf.path)
            .file_stem()
            .unwrap()
            .to_string_lossy();

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

        let output_path = waveform_dir.join(format!("{}.json", stem));

        let status = std::process::Command::new(&audiowaveform_bin)
            .arg("-i")
            .arg(&mf.path)
            .arg("-o")
            .arg(&output_path)
            .arg("--pixels-per-second")
            .arg("100")
            .status()
            .map_err(|e| format!("Failed to run audiowaveform: {}", e))?;

        if !status.success() {
            return Err(format!("audiowaveform failed for {}", mf.name));
        }
    }

    Ok(())
}

/// Advance a dataset to Stage 2: generate subtitles + waveforms.
#[tauri::command]
pub async fn dataset_advance_to_stage2(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    model_state: State<'_, ModelState>,
    uuid: String,
) -> Result<(), String> {
    dataset_generate_subtitles(app.clone(), settings.clone(), model_state, uuid.clone()).await?;
    dataset_generate_waveform(app, settings, uuid).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Stage 3: SQLite database generation
// ---------------------------------------------------------------------------

/// A single cue parsed from a VTT file.
struct VttCue {
    start_ms: i64,
    end_ms: i64,
    content: String,
}

/// Parse a VTT file into a list of cues.
fn parse_vtt(path: &Path) -> Result<Vec<VttCue>, String> {
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut cues = Vec::new();

    // Skip the WEBVTT header line
    let mut lines = content.lines().peekable();
    while let Some(line) = lines.peek() {
        if line.starts_with("WEBVTT") || line.is_empty() {
            lines.next();
            continue;
        }
        break;
    }

    // Parse cue blocks
    loop {
        // Skip blank lines
        while let Some(line) = lines.peek() {
            if line.is_empty() {
                lines.next();
            } else {
                break;
            }
        }

        // Look for a timing line: HH:MM:SS.mmm --> HH:MM:SS.mmm
        let timing_line = match lines.next() {
            Some(l) => l,
            None => break,
        };

        if !timing_line.contains("-->") {
            continue; // skip non-timing lines (e.g. NOTE blocks)
        }

        let parts: Vec<&str> = timing_line.split("-->").collect();
        if parts.len() != 2 {
            continue;
        }

        let start_ms = match parse_vtt_timestamp(parts[0].trim()) {
            Some(ms) => ms,
            None => continue,
        };
        let end_ms = match parse_vtt_timestamp(parts[1].trim()) {
            Some(ms) => ms,
            None => continue,
        };

        // Collect text lines until blank line or EOF
        let mut text_lines = Vec::new();
        while let Some(line) = lines.peek() {
            if line.is_empty() {
                break;
            }
            text_lines.push(lines.next().unwrap());
        }

        let content = text_lines.join("\n");
        if !content.is_empty() {
            cues.push(VttCue {
                start_ms,
                end_ms,
                content,
            });
        }
    }

    Ok(cues)
}

/// Parse a VTT timestamp like "00:01:23.456" into milliseconds.
fn parse_vtt_timestamp(s: &str) -> Option<i64> {
    let s = s.split_whitespace().next()?; // ignore any trailing position info
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let hours: i64 = parts[0].parse().ok()?;
    let minutes: i64 = parts[1].parse().ok()?;
    // Seconds may have a decimal point
    let sec_parts: Vec<&str> = parts[2].split('.').collect();
    let seconds: i64 = sec_parts[0].parse().ok()?;
    let millis: i64 = if sec_parts.len() == 2 {
        let ms_str = sec_parts[1];
        // Pad or truncate to 3 digits
        let padded = format!("{:0<3}", ms_str);
        padded[..3].parse().ok()?
    } else {
        0
    };

    Some(hours * 3_600_000 + minutes * 60_000 + seconds * 1_000 + millis)
}

/// Hardcoded default user ID until auth is implemented.
const DEFAULT_USER_ID: &str = "default";

/// Create the SQLite database schema.
fn create_db_schema(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS listen_media (
            uuid       TEXT PRIMARY KEY,
            user_id    TEXT NOT NULL,
            title      TEXT NOT NULL,
            source     TEXT NOT NULL,
            note       TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS listen_transcript (
            uuid       TEXT PRIMARY KEY,
            user_id    TEXT NOT NULL,
            media_uuid TEXT NOT NULL,
            transcript TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS listen_subtitle (
            uuid       TEXT PRIMARY KEY,
            user_id    TEXT NOT NULL,
            media_uuid TEXT NOT NULL,
            name       TEXT NOT NULL,
            note       TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS listen_subtitle_cue (
            uuid          TEXT PRIMARY KEY,
            subtitle_uuid TEXT NOT NULL,
            order_num     INTEGER NOT NULL,
            start_ms      INTEGER NOT NULL,
            end_ms        INTEGER NOT NULL,
            content       TEXT NOT NULL,
            reference     TEXT
        );

        CREATE TABLE IF NOT EXISTS listen_subtitle_reference (
            uuid       TEXT PRIMARY KEY,
            chunk_uuid TEXT NOT NULL,
            order_num  INTEGER NOT NULL,
            content    TEXT NOT NULL,
            cue_uuid   TEXT
        );

        CREATE TABLE IF NOT EXISTS listen_dictation (
            uuid          TEXT PRIMARY KEY,
            user_id       TEXT NOT NULL,
            media_uuid    TEXT NOT NULL,
            subtitle_uuid TEXT NOT NULL,
            status        TEXT NOT NULL DEFAULT '',
            completed     TEXT NOT NULL DEFAULT '',
            created_at    TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at    TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, media_uuid, subtitle_uuid)
        );

        CREATE TABLE IF NOT EXISTS listen_note (
            uuid       TEXT PRIMARY KEY,
            user_id    TEXT NOT NULL,
            media_uuid TEXT NOT NULL,
            note       TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        ",
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Delete the SQLite database for a dataset and reset status to with_subtitle.
#[tauri::command]
pub async fn dataset_delete_database(
    _app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<(), String> {
    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
    let db_path = dataset_dir.join("data.sqlite3");

    if db_path.exists() {
        fs::remove_file(&db_path).map_err(|e| format!("Failed to remove database: {}", e))?;
    }

    // Update timestamp
    let info_path = dataset_dir.join("info.json");
    let data = fs::read_to_string(&info_path).map_err(|e| e.to_string())?;
    let mut info: DatasetInfo = serde_json::from_str(&data).map_err(|e| e.to_string())?;
    info.updated = Utc::now().to_rfc3339();
    let data = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
    fs::write(&info_path, data).map_err(|e| e.to_string())?;

    Ok(())
}

/// Generate the SQLite database for a dataset (Stage 2 → Stage 3).
#[tauri::command]
pub async fn dataset_generate_database(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<(), String> {
    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
    let media_dir = dataset_dir.join("media");
    let subtitle_dir = dataset_dir.join("subtitle");
    let transcript_dir = dataset_dir.join("transcript");
    let db_path = dataset_dir.join("data.sqlite3");

    // Remove existing DB if present
    if db_path.exists() {
        fs::remove_file(&db_path).map_err(|e| e.to_string())?;
    }

    let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
    create_db_schema(&conn)?;

    let media_files = list_media_files(&media_dir);
    let total = media_files.len();
    let now = Utc::now().to_rfc3339();

    for (i, mf) in media_files.iter().enumerate() {
        let _ = app.emit(
            "dataset-progress",
            DatasetProgress {
                uuid: uuid.clone(),
                current_file: mf.name.clone(),
                file_index: i + 1,
                total_files: total,
                stage: "database".into(),
            },
        );

        let file_path = Path::new(&mf.path);
        let stem = file_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();

        let media_uuid = Uuid::new_v4().to_string();

        // Insert listen_media
        conn.execute(
            "INSERT INTO listen_media (uuid, user_id, title, source, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![media_uuid, DEFAULT_USER_ID, stem.clone(), mf.name, now, now],
        )
        .map_err(|e| e.to_string())?;

        // Insert listen_transcript if transcript exists
        let transcript_path = transcript_dir.join(format!("{}.txt", stem));
        if transcript_path.exists() {
            let transcript_text = fs::read_to_string(&transcript_path).map_err(|e| e.to_string())?;
            let transcript_uuid = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO listen_transcript (uuid, user_id, media_uuid, transcript, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![transcript_uuid, DEFAULT_USER_ID, media_uuid, transcript_text, now, now],
            )
            .map_err(|e| e.to_string())?;
        }

        // Parse VTT and insert subtitle + cues
        let vtt_path = subtitle_dir.join(format!("{}.vtt", stem));
        if vtt_path.exists() {
            let cues = parse_vtt(&vtt_path)?;
            let subtitle_uuid = Uuid::new_v4().to_string();

            // Insert listen_subtitle
            conn.execute(
                "INSERT INTO listen_subtitle (uuid, user_id, media_uuid, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![subtitle_uuid, DEFAULT_USER_ID, media_uuid, stem, now, now],
            )
            .map_err(|e| e.to_string())?;

            // Insert each cue
            for (order, cue) in cues.iter().enumerate() {
                let cue_uuid = Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO listen_subtitle_cue (uuid, subtitle_uuid, order_num, start_ms, end_ms, content) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![cue_uuid, subtitle_uuid, order as i64, cue.start_ms, cue.end_ms, cue.content],
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }

    // Close connection before updating info.json
    drop(conn);

    // Update timestamp
    let info_path = dataset_dir.join("info.json");
    let data = fs::read_to_string(&info_path).map_err(|e| e.to_string())?;
    let mut info: DatasetInfo = serde_json::from_str(&data).map_err(|e| e.to_string())?;
    info.updated = now;
    let data = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
    fs::write(&info_path, data).map_err(|e| e.to_string())?;

    Ok(())
}
