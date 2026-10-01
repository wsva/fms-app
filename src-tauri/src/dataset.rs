use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::settings::SettingsState;
use crate::model::ModelState;
use crate::dictation::open_app_db;

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
    /// Root location directory this dataset was found under.
    pub location: String,
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

/// File extensions recognised as dataset media.
///
/// Audio containers are decoded directly. Video containers are also supported:
/// symphonia (built with the `all` feature) reads the container and decodes its
/// audio track, and `audio::decode_to_pcm` skips the unsupported video track
/// (its codec is reported as NULL) and picks the audio one. This covers the
/// common cases such as MP4/MOV (AAC), and MKV/WebM (Opus/Vorbis).
const MEDIA_EXTENSIONS: &[&str] = &[
    // audio
    "mp3", "wav", "flac", "ogg", "m4a", "m4b", "aac", "opus",
    // video (only the audio track is used)
    "mp4", "m4v", "mov", "mkv", "webm",
];

fn is_media_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    MEDIA_EXTENSIONS
        .iter()
        .any(|ext| lower.ends_with(&format!(".{}", ext)))
}

fn datasets_dir(settings: &SettingsState) -> PathBuf {
    PathBuf::from(settings.settings.lock().unwrap().datasets_dir.clone())
}

// ---------------------------------------------------------------------------
// Dataset locations (stored in the app-level database)
// ---------------------------------------------------------------------------

/// Create the dataset_locations table if it does not exist.
fn ensure_locations_table(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS dataset_locations (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            path       TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .map_err(|e| e.to_string())
}

/// Load all stored dataset location paths, ordered by insertion.
fn load_locations(settings: &SettingsState) -> Vec<String> {
    let Ok(conn) = open_app_db(settings) else { return Vec::new(); };
    if ensure_locations_table(&conn).is_err() {
        return Vec::new();
    }
    let mut stmt = match conn.prepare("SELECT path FROM dataset_locations ORDER BY id") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = match stmt.query_map([], |row| row.get::<_, String>(0)) {
        Ok(rows) => rows,
        Err(_) => return Vec::new(),
    };
    rows.filter_map(|r| r.ok()).collect()
}

/// Persist a new dataset location path (ignores duplicates).
fn insert_location(settings: &SettingsState, path: &str) -> Result<(), String> {
    let conn = open_app_db(settings).map_err(|e| e.to_string())?;
    ensure_locations_table(&conn)?;
    conn.execute(
        "INSERT OR IGNORE INTO dataset_locations (path) VALUES (?1)",
        rusqlite::params![path],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Remove a stored dataset location path.
fn delete_location(settings: &SettingsState, path: &str) -> Result<(), String> {
    let conn = open_app_db(settings).map_err(|e| e.to_string())?;
    ensure_locations_table(&conn)?;
    conn.execute(
        "DELETE FROM dataset_locations WHERE path = ?1",
        rusqlite::params![path],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Whether the dataset_locations table already exists in the app database.
fn locations_table_exists(conn: &rusqlite::Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='dataset_locations'",
        [],
        |_| Ok(()),
    )
    .is_ok()
}

/// Resolve all dataset root directories. On first run (table absent) the
/// configured default `datasets_dir` is seeded so existing setups keep working.
/// Afterwards the stored list is authoritative, even when empty.
fn dataset_roots(settings: &SettingsState) -> Vec<PathBuf> {
    if let Ok(conn) = open_app_db(settings) {
        if !locations_table_exists(&conn) && ensure_locations_table(&conn).is_ok() {
            let default_dir = datasets_dir(settings).to_string_lossy().into_owned();
            let _ = conn.execute(
                "INSERT OR IGNORE INTO dataset_locations (path) VALUES (?1)",
                rusqlite::params![default_dir],
            );
        }
    }
    load_locations(settings).into_iter().map(PathBuf::from).collect()
}

fn list_media_files(media_dir: &PathBuf) -> Vec<MediaFile> {
    if !media_dir.exists() {
        return Vec::new();
    }

    let transcript_dir = media_dir.parent().unwrap().join("transcript");

    let mut files: Vec<MediaFile> = Vec::new();
    collect_media_files(media_dir, media_dir, &transcript_dir, &mut files);

    files.sort_by(|a, b| a.name.cmp(&b.name));
    files
}

/// Recursively walk `dir`, collecting every media file into `out`. Media may be
/// organised in nested sub-folders, so the whole tree under `media/` is scanned
/// rather than just its top level. `root` is the `media/` directory used to
/// compute each file's relative path; `transcript_dir` flags files that already
/// have a transcript, mirroring the sub-directory (`media/a/b.mp3` ->
/// `transcript/a/b.txt`).
fn collect_media_files(root: &Path, dir: &Path, transcript_dir: &Path, out: &mut Vec<MediaFile>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_media_files(root, &path, transcript_dir, out);
            continue;
        }

        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_media_file(&name) {
            continue;
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);

        // Transcript mirrors the media sub-directory: media/a/b.mp3 -> transcript/a/b.txt
        let has_transcript = transcript_dir
            .join(media_rel_path(root, &path).with_extension("txt"))
            .exists();

        out.push(MediaFile {
            name,
            path: path.to_string_lossy().into_owned(),
            size,
            has_transcript,
        });
    }
}

/// A media file's path relative to `media_dir`, preserving sub-directories and
/// the original extension (e.g. `a/b.mp3`). Media may be organised in nested
/// folders (see `collect_media_files`), so every derived artefact mirrors that
/// layout: `media/a/b.mp3` -> `subtitle/a/b.vtt`, `waveform/a/b.json`,
/// `transcript/a/b.txt`.
fn media_rel_path(media_dir: &Path, media_path: &Path) -> PathBuf {
    media_path
        .strip_prefix(media_dir)
        .unwrap_or(media_path)
        .to_path_buf()
}

/// Sibling artefact for a media file under `base_dir` with extension `ext`,
/// mirroring the media sub-directory (see `media_rel_path`). Shared by subtitle,
/// waveform and transcript resolution so all stages agree on nested media.
fn sibling_path(media_dir: &Path, base_dir: &Path, media_path: &Path, ext: &str) -> PathBuf {
    base_dir.join(media_rel_path(media_dir, media_path).with_extension(ext))
}

/// The relative media path as a portable forward-slash string, stored in the
/// database `source` column so sibling files and playback URLs resolve for
/// nested media (e.g. `a/b.mp3`).
fn rel_source_string(media_dir: &Path, media_path: &Path) -> String {
    media_rel_path(media_dir, media_path)
        .to_string_lossy()
        .replace('\\', "/")
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

/// Scan a single directory and, if it looks like a dataset, build a summary.
/// `location` is the root directory the dataset was found under.
fn scan_dataset_dir(path: &Path, location: &str) -> Option<DatasetSummary> {
    if !path.is_dir() {
        return None;
    }

    let path_str = path.to_string_lossy().into_owned();
    let info_path = path.join("info.json");

    if info_path.exists() {
        let data = fs::read_to_string(&info_path).ok()?;
        let info: DatasetInfo = serde_json::from_str(&data).ok()?;
        let media_dir = path.join("media");
        let media_count = list_media_files(&media_dir).len();
        let status = if path.join("data.sqlite3").exists() { "ready" } else { "not_ready" };
        return Some(DatasetSummary {
            info,
            media_count,
            path: path_str,
            location: location.to_string(),
            status: status.into(),
        });
    }

    // No info.json -- check if it has media files (raw import)
    let media_dir = path.join("media");
    let media_files = list_media_files(&media_dir);
    if media_files.is_empty() {
        return None;
    }
    let info = DatasetInfo {
        name: path.file_name()?.to_string_lossy().into_owned(),
        uuid: String::new(),
        description: String::new(),
        parent_uuid: String::new(),
        version: 0,
        structure: "dictation-v1".into(),
        updated: String::new(),
    };
    Some(DatasetSummary {
        info,
        media_count: media_files.len(),
        path: path_str,
        location: location.to_string(),
        status: "not_ready".into(),
    })
}

/// List all datasets across every configured dataset location.
#[tauri::command]
pub async fn dataset_list(
    state: State<'_, DatasetState>,
    settings: State<'_, SettingsState>,
) -> Result<Vec<DatasetSummary>, String> {
    let _ = state;
    Ok(list_datasets(&settings))
}

/// Core dataset listing logic, reusable without Tauri `State` (e.g. web service).
pub(crate) fn list_datasets(settings: &SettingsState) -> Vec<DatasetSummary> {
    let roots = dataset_roots(settings);
    log::debug!("Scanning {} dataset location(s)", roots.len());

    let mut datasets = Vec::new();

    for root in roots {
        if !root.exists() {
            continue;
        }
        let location = root.to_string_lossy().into_owned();
        let entries = match fs::read_dir(&root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if let Some(summary) = scan_dataset_dir(&entry.path(), &location) {
                datasets.push(summary);
            }
        }
    }

    datasets.sort_by(|a, b| a.info.name.cmp(&b.info.name));
    log::info!("Found {} dataset(s)", datasets.len());
    datasets
}

/// List all configured dataset location paths.
#[tauri::command]
pub async fn dataset_list_locations(
    settings: State<'_, SettingsState>,
) -> Result<Vec<String>, String> {
    Ok(dataset_roots(&settings)
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect())
}

/// Add a new dataset location and return the updated list of locations.
#[tauri::command]
pub async fn dataset_add_location(
    settings: State<'_, SettingsState>,
    path: String,
) -> Result<Vec<String>, String> {
    let p = PathBuf::from(&path);
    if !p.exists() || !p.is_dir() {
        return Err("Selected path is not a valid directory".into());
    }
    insert_location(&settings, &path)?;
    log::info!("Added dataset location: {}", path);
    dataset_list_locations(settings).await
}

/// Remove a dataset location and return the updated list of locations.
#[tauri::command]
pub async fn dataset_remove_location(
    settings: State<'_, SettingsState>,
    path: String,
) -> Result<Vec<String>, String> {
    delete_location(&settings, &path)?;
    log::info!("Removed dataset location: {}", path);
    dataset_list_locations(settings).await
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

    let dir = dataset_roots(&settings)
        .into_iter()
        .next()
        .unwrap_or_else(|| datasets_dir(&settings));
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
    log::info!("Importing dataset from '{}' to '{}'", source_dir, dst.display());
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

    log::info!("Imported dataset '{}' with {} media file(s)", dir_name, media_count);
    Ok(DatasetSummary { info, media_count, path: dst.to_string_lossy().into_owned(), location: dir.to_string_lossy().into_owned(), status: "not_ready".into() })
}

/// Get detailed information about a specific dataset, including its media files.
#[tauri::command]
pub async fn dataset_get(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<DatasetDetail, String> {
    let path = find_dataset_dir(&settings, &uuid)?;

    let info_path = path.join("info.json");
    let data = fs::read_to_string(&info_path).map_err(|e| e.to_string())?;
    let info: DatasetInfo = serde_json::from_str(&data).map_err(|e| e.to_string())?;

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
    Ok(DatasetDetail { info, media, has_subtitles, has_waveforms, has_database, has_book, status: status.into() })
}

/// Update mutable fields of a dataset's info.json (name, description).
#[tauri::command]
pub async fn dataset_update(
    settings: State<'_, SettingsState>,
    uuid: String,
    name: Option<String>,
    description: Option<String>,
) -> Result<(), String> {
    let path = find_dataset_dir(&settings, &uuid)?;
    let info_path = path.join("info.json");

    let data = fs::read_to_string(&info_path).map_err(|e| e.to_string())?;
    let mut info: DatasetInfo = serde_json::from_str(&data).map_err(|e| e.to_string())?;

    if let Some(n) = name {
        info.name = n;
    }
    if let Some(d) = description {
        info.description = d;
    }
    info.updated = Utc::now().to_rfc3339();

    let data = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
    fs::write(&info_path, data).map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a dataset by removing its entire directory.
#[tauri::command]
pub async fn dataset_delete(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<(), String> {
    let path = find_dataset_dir(&settings, &uuid)?;
    log::info!("Deleting dataset at '{}'", path.display());
    fs::remove_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Studio: create dataset + import media
// ---------------------------------------------------------------------------

/// Sanitize a string for use as a directory name. Returns None when empty.
fn sanitize_dir_name(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(windows)]
fn symlink_file(src: &Path, dst: &Path) -> Result<(), String> {
    std::os::windows::fs::symlink_file(src, dst).map_err(|e| format!("Failed to symlink: {}", e))
}

#[cfg(not(windows))]
fn symlink_file(src: &Path, dst: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(src, dst).map_err(|e| format!("Failed to symlink: {}", e))
}

/// Create a new empty dataset directory skeleton (media/, subtitle/, waveform/,
/// transcript/) plus an info.json under a configured dataset location.
#[tauri::command]
pub async fn dataset_create(
    settings: State<'_, SettingsState>,
    name: String,
    description: Option<String>,
    location: Option<String>,
) -> Result<DatasetSummary, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Dataset name must not be empty".into());
    }

    let roots = dataset_roots(&settings);
    let root = match location {
        Some(loc) if !loc.trim().is_empty() => {
            let p = PathBuf::from(loc.trim());
            if !roots.iter().any(|r| *r == p) {
                return Err("Selected location is not a configured dataset location".into());
            }
            p
        }
        _ => roots
            .into_iter()
            .next()
            .unwrap_or_else(|| datasets_dir(&settings)),
    };
    fs::create_dir_all(&root).map_err(|e| format!("Failed to create datasets dir: {}", e))?;

    let uuid = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let dir_name = sanitize_dir_name(trimmed).unwrap_or_else(|| uuid.clone());
    let dst = root.join(&dir_name);
    if dst.exists() {
        return Err(format!("Dataset '{}' already exists", dir_name));
    }

    for sub in ["media", "subtitle", "waveform", "transcript"] {
        fs::create_dir_all(dst.join(sub)).map_err(|e| e.to_string())?;
    }

    let info = DatasetInfo {
        name: trimmed.to_string(),
        uuid: uuid.clone(),
        description: description.unwrap_or_default(),
        parent_uuid: String::new(),
        version: 1,
        structure: "dictation-v1".into(),
        updated: now,
    };
    let data = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
    fs::write(dst.join("info.json"), data).map_err(|e| e.to_string())?;

    log::info!("Created dataset '{}' at '{}'", trimmed, dst.display());
    Ok(DatasetSummary {
        info,
        media_count: 0,
        path: dst.to_string_lossy().into_owned(),
        location: root.to_string_lossy().into_owned(),
        status: "not_ready".into(),
    })
}

/// Copy (or symlink) audio/video files from a source directory into a dataset's
/// media/ folder. Returns the number of files imported.
#[tauri::command]
pub async fn dataset_import_media(
    settings: State<'_, SettingsState>,
    uuid: String,
    source_dir: String,
    link: bool,
) -> Result<usize, String> {
    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
    let src = PathBuf::from(&source_dir);
    if !src.exists() || !src.is_dir() {
        return Err("Source directory does not exist".into());
    }

    let media_dir = dataset_dir.join("media");
    fs::create_dir_all(&media_dir).map_err(|e| e.to_string())?;

    let mut imported = 0usize;
    for entry in fs::read_dir(&src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if !is_media_file(&file_name) {
            continue;
        }
        let dst = media_dir.join(&file_name);
        if dst.exists() {
            continue;
        }
        if link {
            symlink_file(&path, &dst)?;
        } else {
            fs::copy(&path, &dst).map_err(|e| format!("Failed to copy {}: {}", file_name, e))?;
        }
        imported += 1;
    }
    log::info!("Imported {} media file(s) into dataset", imported);
    Ok(imported)
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

/// Find the dataset directory by UUID across all configured locations.
pub(crate) fn find_dataset_dir(settings: &SettingsState, uuid: &str) -> Result<PathBuf, String> {
    for root in dataset_roots(settings) {
        if !root.exists() {
            continue;
        }
        let entries = match fs::read_dir(&root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let info_path = path.join("info.json");
            if !info_path.exists() {
                continue;
            }
            let data = match fs::read_to_string(&info_path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let info: DatasetInfo = match serde_json::from_str(&data) {
                Ok(i) => i,
                Err(_) => continue,
            };
            if info.uuid == uuid {
                return Ok(path);
            }
        }
    }
    Err(format!("Dataset with UUID {} not found", uuid))
}

/// Generate subtitles (VTT files) for the media files in a dataset.
///
/// Incremental: media that already have a subtitle (mirrored under `subtitle/`,
/// e.g. `media/a/b.mp3` -> `subtitle/a/b.vtt`) are skipped, so re-running only
/// transcribes newly added media. Use `dataset_delete_subtitles` to force a full
/// regeneration. Returns a short summary for the UI log.
#[tauri::command]
pub async fn dataset_generate_subtitles(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    model_state: State<'_, ModelState>,
    uuid: String,
) -> Result<String, String> {
    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
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

        let result = crate::model::transcribe_file(&model_state, file_path)?;

        // Mirror the media sub-directory under subtitle/ before writing.
        if let Some(parent) = vtt_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let vtt_content = segments_to_vtt(result.segments.as_deref().unwrap_or_default());
        fs::write(vtt_path, vtt_content).map_err(|e| e.to_string())?;
    }

    // Update timestamp only when something was actually generated.
    if total > 0 {
        let info_path = dataset_dir.join("info.json");
        let data = fs::read_to_string(&info_path).map_err(|e| e.to_string())?;
        let mut info: DatasetInfo = serde_json::from_str(&data).map_err(|e| e.to_string())?;
        info.updated = Utc::now().to_rfc3339();
        let data = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
        fs::write(&info_path, data).map_err(|e| e.to_string())?;
    }

    Ok(format!(
        "Generated {} subtitle(s), skipped {} already present.",
        total, skipped
    ))
}

/// Generate subtitle for a single media file using the loaded STT model.
/// Looks up the media source path from the database, transcribes, and writes VTT.
#[tauri::command]
pub async fn dataset_generate_subtitle_single(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    model_state: State<'_, ModelState>,
    uuid: String,
    media_uuid: String,
) -> Result<String, String> {
    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
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

    let result = crate::model::transcribe_file(&model_state, &media_path)?;

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
#[tauri::command]
pub async fn dataset_delete_subtitles(
    _app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<(), String> {
    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
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
    log::info!("Deleting waveforms for dataset '{}'", uuid);

    if waveform_dir.exists() {
        fs::remove_dir_all(&waveform_dir).map_err(|e| format!("Failed to remove waveforms: {}", e))?;
    }

    Ok(())
}

/// audiowaveform-compatible waveform JSON (v2 layout). Serialised from the
/// peaks computed in `audio::generate_waveform`; consumed by the frontend
/// `WaveformCanvas` and by `adjust::load_waveform`.
#[derive(Serialize)]
struct WaveformJson<'a> {
    version: u8,
    channels: u8,
    sample_rate: u32,
    samples_per_pixel: u32,
    bits: u8,
    length: usize,
    data: &'a [i8],
}

/// Write waveform peaks data into the `listen_waveform` table of the dataset DB.
/// Looks up the media_uuid by matching the relative source path.
fn write_waveform_to_db(
    conn: &Connection,
    media_dir: &Path,
    media_path: &Path,
    peaks_json: &str,
    sample_rate: u32,
) -> Result<(), String> {
    let rel_source = rel_source_string(media_dir, media_path);
    let media_uuid: String = conn
        .query_row(
            "SELECT uuid FROM listen_media WHERE source = ?1",
            rusqlite::params![rel_source],
            |row| row.get(0),
        )
        .map_err(|e| format!("Media not found in DB for '{}': {}", rel_source, e))?;

    let wf_uuid = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT OR REPLACE INTO listen_waveform (uuid, media_uuid, peaks_data, sample_rate) \
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![wf_uuid, media_uuid, peaks_json, sample_rate as i64],
    )
    .map_err(|e| format!("Failed to write waveform to DB: {}", e))?;
    Ok(())
}

/// Generate waveform JSON files for all media files in a dataset.
///
/// Peaks are computed in pure Rust via symphonia (see `audio::generate_waveform`),
/// so no external `audiowaveform` binary is required and video containers are
/// supported (their audio track is decoded).
///
/// If the dataset database exists, waveform data is also written to the
/// `listen_waveform` table alongside the JSON files.
#[tauri::command]
pub async fn dataset_generate_waveform(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<usize, String> {
    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
    let media_dir = dataset_dir.join("media");
    let waveform_dir = dataset_dir.join("waveform");
    fs::create_dir_all(&waveform_dir).map_err(|e| e.to_string())?;

    // Open DB if it exists (for writing waveform data alongside JSON files).
    let db_path = dataset_dir.join("data.sqlite3");
    let db_conn = if db_path.exists() {
        Connection::open(&db_path).ok()
    } else {
        None
    };

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

        // Also write waveform data to database if available.
        if let Some(ref conn) = db_conn {
            let peaks_json = serde_json::to_string(&wf).map_err(|e| e.to_string())?;
            if let Err(e) = write_waveform_to_db(conn, &media_dir, Path::new(&mf.path), &peaks_json, peaks.sample_rate) {
                log::warn!("[waveform] DB write skipped for {}: {}", mf.name, e);
            }
        }
    }

    log::info!("[waveform] Complete: {} files processed", total);
    Ok(total)
}

/// Generate waveform for a single media file (by media_uuid).
/// Returns the source path of the processed media.
#[tauri::command]
pub async fn dataset_generate_waveform_single(
    settings: State<'_, SettingsState>,
    uuid: String,
    media_uuid: String,
) -> Result<String, String> {
    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
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

    // Also write waveform data to database.
    let peaks_json = serde_json::to_string(&wf).map_err(|e| e.to_string())?;
    write_waveform_to_db(&conn, &media_dir, &media_path, &peaks_json, peaks.sample_rate)?;

    log::info!("[waveform] Done: {}", source);
    Ok(source)
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
pub(crate) struct VttCue {
    pub(crate) start_ms: i64,
    pub(crate) end_ms: i64,
    pub(crate) content: String,
}

/// Parse a VTT file into a list of cues.
pub(crate) fn parse_vtt(path: &Path) -> Result<Vec<VttCue>, String> {
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

/// Create the SQLite database schema (v2 with versioning).
fn create_db_schema(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        -- Media files (audio/video)
        CREATE TABLE IF NOT EXISTS listen_media (
            uuid        TEXT PRIMARY KEY,
            source      TEXT NOT NULL,
            duration_ms INTEGER,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- Subtitle tracks with versioning
        CREATE TABLE IF NOT EXISTS listen_subtitle (
            uuid        TEXT PRIMARY KEY,
            media_uuid  TEXT NOT NULL,
            name        TEXT NOT NULL,
            track_type  TEXT,
            model_uuid  TEXT,
            version     INTEGER NOT NULL DEFAULT 1,
            is_active   INTEGER NOT NULL DEFAULT 1,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
            note        TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_subtitle_media ON listen_subtitle(media_uuid);
        CREATE INDEX IF NOT EXISTS idx_subtitle_active ON listen_subtitle(media_uuid, is_active);

        -- Cues with efficient versioning (effective range pattern)
        CREATE TABLE IF NOT EXISTS listen_subtitle_cue (
            uuid               TEXT PRIMARY KEY,
            subtitle_uuid      TEXT NOT NULL,
            order_num          INTEGER NOT NULL,
            start_ms           INTEGER NOT NULL,
            end_ms             INTEGER NOT NULL,
            content            TEXT NOT NULL,
            reference          TEXT,
            confidence         REAL,
            version_created    INTEGER NOT NULL,
            version_superseded INTEGER,
            created_at         TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_cue_subtitle ON listen_subtitle_cue(subtitle_uuid);
        CREATE INDEX IF NOT EXISTS idx_cue_current ON listen_subtitle_cue(subtitle_uuid, version_superseded);
        CREATE INDEX IF NOT EXISTS idx_cue_version ON listen_subtitle_cue(subtitle_uuid, version_created);

        -- Version metadata
        CREATE TABLE IF NOT EXISTS listen_subtitle_version (
            uuid           TEXT PRIMARY KEY,
            subtitle_uuid  TEXT NOT NULL,
            version        INTEGER NOT NULL,
            change_type    TEXT,
            description    TEXT,
            cues_added     INTEGER NOT NULL DEFAULT 0,
            cues_modified  INTEGER NOT NULL DEFAULT 0,
            cues_deleted   INTEGER NOT NULL DEFAULT 0,
            created_at     TEXT NOT NULL DEFAULT (datetime('now')),
            created_by     TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_version_subtitle ON listen_subtitle_version(subtitle_uuid);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_version_unique ON listen_subtitle_version(subtitle_uuid, version);

        -- Waveform data (cached)
        CREATE TABLE IF NOT EXISTS listen_waveform (
            uuid          TEXT PRIMARY KEY,
            media_uuid    TEXT NOT NULL,
            peaks_data    TEXT,
            sample_rate   INTEGER,
            created_at    TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_waveform_media ON listen_waveform(media_uuid);

        -- Transcript (per-media)
        CREATE TABLE IF NOT EXISTS listen_transcript (
            uuid       TEXT PRIMARY KEY,
            media_uuid TEXT NOT NULL,
            transcript TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- Notes
        CREATE TABLE IF NOT EXISTS listen_note (
            uuid       TEXT PRIMARY KEY,
            media_uuid TEXT NOT NULL,
            note       TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        "
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
        log::info!("Deleting database for dataset '{}'", uuid);
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

    log::info!("Generating database for dataset '{}'", uuid);
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

        // Insert listen_media. `source` stores the media-relative path so nested
        // media resolve for playback and sibling (subtitle/waveform/transcript) files.
        conn.execute(
            "INSERT INTO listen_media (uuid, source, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![media_uuid, rel_source_string(&media_dir, file_path), now, now],
        )
        .map_err(|e| e.to_string())?;

        // Insert listen_transcript if transcript exists (mirrors the media sub-directory).
        let transcript_path = sibling_path(&media_dir, &transcript_dir, file_path, "txt");
        if transcript_path.exists() {
            let transcript_text = fs::read_to_string(&transcript_path).map_err(|e| e.to_string())?;
            let transcript_uuid = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO listen_transcript (uuid, media_uuid, transcript, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![transcript_uuid, media_uuid, transcript_text, now, now],
            )
            .map_err(|e| e.to_string())?;
        }

        // Parse VTT and insert subtitle + cues (mirrors the media sub-directory).
        let vtt_path = sibling_path(&media_dir, &subtitle_dir, file_path, "vtt");
        if vtt_path.exists() {
            let cues = parse_vtt(&vtt_path)?;
            let subtitle_uuid = Uuid::new_v4().to_string();

            // Insert listen_subtitle (version 1, initial STT)
            conn.execute(
                "INSERT INTO listen_subtitle (uuid, media_uuid, name, track_type, version, is_active, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 1, 1, ?5, ?6)",
                rusqlite::params![subtitle_uuid, media_uuid, stem, "stt", now, now],
            )
            .map_err(|e| e.to_string())?;

            // Insert version log entry
            let version_uuid = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO listen_subtitle_version (uuid, subtitle_uuid, version, change_type, description, cues_added, created_by) VALUES (?1, ?2, 1, 'initial', 'Initial STT transcription', ?3, 'system')",
                rusqlite::params![version_uuid, subtitle_uuid, cues.len() as i64],
            )
            .map_err(|e| e.to_string())?;

            // Insert each cue (version 1)
            for (order, cue) in cues.iter().enumerate() {
                let cue_uuid = Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO listen_subtitle_cue (uuid, subtitle_uuid, order_num, start_ms, end_ms, content, version_created) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)",
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

    log::info!("Database generated for dataset '{}': {} media, {} total", uuid, total, total);
    Ok(())
}

/// Write VTT subtitles to the existing database (without rebuilding the entire database).
/// Clears existing subtitle data and re-imports from VTT files by walking the
/// subtitle/ directory recursively.
#[tauri::command]
pub async fn dataset_write_subtitles_to_db(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<String, String> {
    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
    let subtitle_dir = dataset_dir.join("subtitle");
    let db_path = dataset_dir.join("data.sqlite3");

    if !db_path.exists() {
        return Err("Database file not found. Please build the database first.".into());
    }
    if !subtitle_dir.exists() {
        return Err("No subtitle directory found. Generate subtitles first.".into());
    }

    log::info!("Writing subtitles to database for dataset '{}'", uuid);
    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;

    // Clear existing subtitle data
    conn.execute("DELETE FROM listen_subtitle_cue", [])
        .map_err(|e| format!("Failed to clear subtitle cues: {}", e))?;
    conn.execute("DELETE FROM listen_subtitle", [])
        .map_err(|e| format!("Failed to clear subtitles: {}", e))?;

    // Build a lookup: media source stem -> media_uuid  (e.g. "a/b" -> uuid)
    let mut stmt = conn
        .prepare("SELECT uuid, source FROM listen_media")
        .map_err(|e| e.to_string())?;
    let media_map: std::collections::HashMap<String, String> = stmt
        .query_map([], |row| {
            let media_uuid: String = row.get(0)?;
            let source: String = row.get(1)?;
            Ok((media_uuid, source))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .map(|(media_uuid, source)| {
            let stem = Path::new(&source)
                .with_extension("")
                .to_string_lossy()
                .into_owned();
            (stem, media_uuid)
        })
        .collect();
    drop(stmt);

    // Walk the subtitle directory recursively, collecting .vtt files
    let mut vtt_files: Vec<(String, std::path::PathBuf)> = Vec::new();
    collect_vtt_files(&subtitle_dir, &subtitle_dir, &mut vtt_files);
    vtt_files.sort_by(|a, b| a.0.cmp(&b.0));

    let now = Utc::now().to_rfc3339();
    let mut written = 0usize;
    let mut skipped = 0usize;
    let total = vtt_files.len();
    let mut log_lines: Vec<String> = Vec::new();

    log_lines.push(format!("Media entries in database ({}):", media_map.len()));
    for (stem, media_uuid) in &media_map {
        log_lines.push(format!("  stem='{}'  media_uuid={}", stem, media_uuid));
    }
    log_lines.push(format!("VTT files found on disk ({}):", total));
    for (rel_stem, vtt_path) in &vtt_files {
        log_lines.push(format!("  stem='{}'  path={}", rel_stem, vtt_path.display()));
    }
    log_lines.push(String::new());

    for (rel_stem, vtt_path) in &vtt_files {
        let _ = app.emit(
            "dataset-progress",
            DatasetProgress {
                uuid: uuid.clone(),
                current_file: rel_stem.clone(),
                file_index: written + skipped + 1,
                total_files: total,
                stage: "write_subtitles".into(),
            },
        );

        // Match VTT to a media entry by source stem
        let Some(media_uuid) = media_map.get(rel_stem) else {
            log_lines.push(format!("SKIP '{}' — no matching media entry (stem not found in database)", rel_stem));
            skipped += 1;
            continue;
        };

        let cues = parse_vtt(vtt_path)?;
        let subtitle_uuid = Uuid::new_v4().to_string();
        let name = Path::new(rel_stem)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();

        log_lines.push(format!("WRITE '{}' -> media_uuid={}, {} cue(s)", rel_stem, media_uuid, cues.len()));

        // Insert listen_subtitle (version 1, initial import)
        conn.execute(
            "INSERT INTO listen_subtitle (uuid, media_uuid, name, track_type, version, is_active, created_at, updated_at) VALUES (?1, ?2, ?3, 'stt', 1, 1, ?4, ?5)",
            rusqlite::params![subtitle_uuid, media_uuid, name, now, now],
        )
        .map_err(|e| e.to_string())?;

        // Insert version log entry
        let version_uuid = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO listen_subtitle_version (uuid, subtitle_uuid, version, change_type, description, cues_added, created_by) VALUES (?1, ?2, 1, 'initial', 'Imported from VTT', ?3, 'system')",
            rusqlite::params![version_uuid, subtitle_uuid, cues.len() as i64],
        )
        .map_err(|e| e.to_string())?;

        // Insert each cue (version 1)
        for (order, cue) in cues.iter().enumerate() {
            let cue_uuid = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO listen_subtitle_cue (uuid, subtitle_uuid, order_num, start_ms, end_ms, content, version_created) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)",
                rusqlite::params![cue_uuid, subtitle_uuid, order as i64, cue.start_ms, cue.end_ms, cue.content],
            )
            .map_err(|e| e.to_string())?;
        }

        written += 1;
    }

    drop(conn);

    // Update timestamp
    let info_path = dataset_dir.join("info.json");
    let data = fs::read_to_string(&info_path).map_err(|e| e.to_string())?;
    let mut info: DatasetInfo = serde_json::from_str(&data).map_err(|e| e.to_string())?;
    info.updated = now;
    let data = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
    fs::write(&info_path, data).map_err(|e| e.to_string())?;

    log_lines.push(String::new());
    log_lines.push(format!("Wrote {} subtitle(s) to database.", written));
    if skipped > 0 {
        log_lines.push(format!("Skipped {} VTT file(s) without a matching media entry.", skipped));
    }

    log::info!("Wrote {} subtitle(s) to database for dataset '{}', skipped {}", written, uuid, skipped);
    Ok(log_lines.join("\n"))
}

/// Recursively walk `dir` collecting every `.vtt` file. `root` is the subtitle/
/// directory used to compute each file's relative stem (e.g. `a/b.vtt` → `a/b`).
/// Path separators are normalised to `/` to match the `source` field in the database.
fn collect_vtt_files(root: &Path, dir: &Path, out: &mut Vec<(String, std::path::PathBuf)>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_vtt_files(root, &path, out);
        } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("vtt")) {
            let rel_stem = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .with_extension("")
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel_stem, path));
        }
    }
}
