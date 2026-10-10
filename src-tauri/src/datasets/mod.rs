//! Dataset infrastructure shared by every dataset type, plus the dictation
//! dataset pipeline (import, STT subtitles, waveforms, cue DB generation).
//!
//! This module tree mirrors the on-disk layout: every type resolves through
//! [`dataset_roots`] to `<datasets_dir>/{dictation,card,book,read_aloud,wiki}/` plus
//! the linked directories listed in that folder's `meta.json`.
//!
//! The unified `info.json` descriptor every type shares lives in [`info`]; the
//! remaining shared core ([`DatasetType`], [`dataset_roots`], the `meta.json`
//! read/write helpers, `find_dataset_dir*` and the `dataset_*_dir` commands)
//! still lives in this file next to the dictation pipeline — split it out as a
//! follow-up now that the descriptor has left.

pub(crate) mod book;
pub(crate) mod cards;
pub(crate) mod dictation;
pub(crate) mod info;
pub(crate) mod read_aloud;
pub(crate) mod textsim;
pub(crate) mod wiki;
#[cfg(feature = "desktop")]
pub(crate) mod tools;

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::Utc;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::settings::SettingsState;
#[cfg(feature = "desktop")]
use crate::models::ModelState;

// The shared descriptor and its helpers are addressed through this module root by
// every dataset type and by the sync layer.
pub(crate) use info::{
    assert_uuid_free, canonical_stamp, is_derived_index, now_row_stamp, now_stamp, read_info,
    read_info_opt, row_stamp_at, stamps_cmp, touch_info, write_info, DatasetInfo,
    FAVORITES_DATASET_UUID, FORMAT_BOOK, FORMAT_CARD, FORMAT_DICTATION, FORMAT_READ_ALOUD,
    FORMAT_WIKI, INFO_FILE,
};
// The hub's edit-time normalizer is the only caller outside this group; the row
// stamps every type writes come through the ungated names above.
#[cfg(feature = "desktop")]
pub(crate) use info::parse_stamp;
// The cross-type enumerator's only caller is the desktop MCP listing; an ungated
// re-export would warn in the mobile build.
#[cfg(feature = "desktop")]
pub(crate) use info::scan_datasets;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

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

/// Resolve the default datasets directory.
///
/// When a workspace is selected, datasets always live in `<workspace>/datasets`
/// (derived from the workspace directory, not from settings.json). Linked
/// directories are read from `<workspace>/datasets/meta.json`. Without a
/// workspace, fall back to the configured/global default.
fn datasets_dir(settings: &SettingsState) -> PathBuf {
    settings.datasets_dir()
}

// ---------------------------------------------------------------------------
// Dataset locations (stored in meta.json like wiki)
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct DatasetLinkedDir {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
struct DatasetMeta {
    #[serde(default)]
    linked_dirs: Vec<DatasetLinkedDir>,
}

#[derive(Clone, Serialize, Debug)]
pub struct DatasetDirEntry {
    pub name: String,
    pub path: String,
    pub is_linked: bool,
}

fn dataset_meta_path(datasets_dir: &str) -> PathBuf {
    PathBuf::from(datasets_dir).join("meta.json")
}

fn read_dataset_meta(datasets_dir: &str) -> DatasetMeta {
    let meta_path = dataset_meta_path(datasets_dir);
    if !meta_path.exists() {
        return DatasetMeta::default();
    }
    fs::read_to_string(&meta_path)
        .ok()
        .map(|data| data.trim_start_matches('\u{FEFF}').to_string())
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or_default()
}

fn write_dataset_meta(datasets_dir: &str, meta: &DatasetMeta) -> Result<(), String> {
    let meta_path = dataset_meta_path(datasets_dir);
    // Ensure the datasets directory exists
    fs::create_dir_all(datasets_dir).map_err(|e| e.to_string())?;
    let data = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
    fs::write(&meta_path, data).map_err(|e| e.to_string())
}

/// Resolve all dataset root directories: default datasets_dir + linked dirs from meta.json
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DatasetType {
    Card,
    Dictation,
    Book,
    Read,
    Wiki,
}

impl DatasetType {
    /// Every type the app can host, in scan order. `find_dataset_dir_typed` and the
    /// cross-type listings iterate this, so a new type is added in exactly one place
    /// — anything left out is silently unsyncable and unlistable.
    pub(crate) const ALL: [DatasetType; 5] = [
        DatasetType::Dictation,
        DatasetType::Card,
        DatasetType::Book,
        DatasetType::Read,
        DatasetType::Wiki,
    ];

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            DatasetType::Card => "card",
            DatasetType::Dictation => "dictation",
            DatasetType::Book => "book",
            DatasetType::Read => "read_aloud",
            DatasetType::Wiki => "wiki",
        }
    }
}

impl FromStr for DatasetType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "card" => Ok(DatasetType::Card),
            "dictation" => Ok(DatasetType::Dictation),
            "book" => Ok(DatasetType::Book),
            "read_aloud" => Ok(DatasetType::Read),
            "wiki" => Ok(DatasetType::Wiki),
            other => Err(format!(
                "Invalid dataset type: {} (expected one of card, dictation, book, read_aloud, wiki)",
                other
            )),
        }
    }
}

/// Wire `DatasetType` to its slug without duplicating the mapping: `as_str()` is
/// the single source of truth for both the directory name and the `type` field in
/// `info.json`. Deliberately not `#[derive(Serialize, Deserialize)]` with
/// `rename_all` — that would make `DatasetType::Read` serialize as `read` while
/// its directory is `read_aloud`, silently splitting the two spellings.
impl Serialize for DatasetType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for DatasetType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        DatasetType::from_str(&raw).map_err(serde::de::Error::custom)
    }
}

pub(crate) fn dataset_roots(settings: &SettingsState, dataset_type: DatasetType) -> Vec<PathBuf> {
    let base_dir = datasets_dir(settings);
    let type_dir = base_dir.join(dataset_type.as_str());
    let type_dir_str = type_dir.to_string_lossy().to_string();
    
    let mut roots = Vec::new();
    
    // Add type-specific subdirectory if it exists
    if type_dir.exists() && type_dir.is_dir() {
        roots.push(type_dir.clone());
    }
    
    // Add linked directories from meta.json in the type-specific directory
    let meta = read_dataset_meta(&type_dir_str);
    for linked in &meta.linked_dirs {
        let linked_path = PathBuf::from(&linked.path);
        if linked_path.exists() && linked_path.is_dir() {
            roots.push(linked_path);
        }
    }
    
    roots
}

/// List all dataset directories (default + linked) for a specific dataset type
#[tauri::command]
pub async fn dataset_list_dirs(
    settings: State<'_, SettingsState>,
    dataset_type: String,
) -> Result<Vec<DatasetDirEntry>, String> {
    let ds_type = DatasetType::from_str(&dataset_type)?;
    
    let base_dir = datasets_dir(&settings);
    let type_dir = base_dir.join(ds_type.as_str());
    let type_dir_str = type_dir.to_string_lossy().to_string();
    
    let mut entries = Vec::new();
    
    // Add type-specific subdirectory as default entry
    if type_dir.exists() && type_dir.is_dir() {
        entries.push(DatasetDirEntry {
            name: format!("{} datasets", ds_type.as_str()),
            path: type_dir_str.clone(),
            is_linked: false,
        });
    }
    
    // Add linked directories from meta.json in the type-specific directory
    let meta = read_dataset_meta(&type_dir_str);
    for linked in &meta.linked_dirs {
        let linked_path = PathBuf::from(&linked.path);
        if linked_path.exists() && linked_path.is_dir() {
            entries.push(DatasetDirEntry {
                name: linked.name.clone(),
                path: linked.path.clone(),
                is_linked: true,
            });
        }
    }
    
    // Sort alphabetically
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    
    log::info!("[Dataset] Listed {} {} dataset directories", entries.len(), ds_type.as_str());
    Ok(entries)
}

/// Add a linked directory for datasets
#[tauri::command]
pub async fn dataset_add_dir(
    settings: State<'_, SettingsState>,
    name: String,
    path: String,
) -> Result<(), String> {
    let default_dir = datasets_dir(&settings);
    let default_str = default_dir.to_string_lossy().to_string();
    
    // Verify the path exists and is a directory
    let target_path = PathBuf::from(&path);
    if !target_path.exists() {
        return Err(format!("Directory does not exist: {}", path));
    }
    if !target_path.is_dir() {
        return Err(format!("Path is not a directory: {}", path));
    }
    
    let mut meta = read_dataset_meta(&default_str);
    
    // Check if already linked
    if meta.linked_dirs.iter().any(|d| d.path == path) {
        return Err(format!("Directory already linked: {}", path));
    }
    
    meta.linked_dirs.push(DatasetLinkedDir {
        name: name.clone(),
        path: path.clone(),
    });
    
    write_dataset_meta(&default_str, &meta)?;
    log::info!("[Dataset] Linked directory '{}' -> {}", name, path);
    Ok(())
}

/// Remove a linked directory from datasets
#[tauri::command]
pub async fn dataset_remove_dir(
    settings: State<'_, SettingsState>,
    path: String,
) -> Result<(), String> {
    let default_dir = datasets_dir(&settings);
    let default_str = default_dir.to_string_lossy().to_string();
    
    let mut meta = read_dataset_meta(&default_str);
    let initial_len = meta.linked_dirs.len();
    meta.linked_dirs.retain(|d| d.path != path);
    
    if meta.linked_dirs.len() == initial_len {
        return Err(format!("Directory not found in linked directories: {}", path));
    }
    
    write_dataset_meta(&default_str, &meta)?;
    log::info!("[Dataset] Unlinked directory: {}", path);
    Ok(())
}

pub(crate) fn list_media_files(media_dir: &PathBuf) -> Vec<MediaFile> {
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
pub(crate) fn sibling_path(media_dir: &Path, base_dir: &Path, media_path: &Path, ext: &str) -> PathBuf {
    base_dir.join(media_rel_path(media_dir, media_path).with_extension(ext))
}

/// The relative media path as a portable forward-slash string, stored in the
/// database `source` column so sibling files and playback URLs resolve for
/// nested media (e.g. `a/b.mp3`).
pub(crate) fn rel_source_string(media_dir: &Path, media_path: &Path) -> String {
    media_rel_path(media_dir, media_path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Copy a directory tree. Only reachable through [`move_to_trash`], whose
/// cross-volume fallback needs it; the fast path is a rename.
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

/// Move a file or directory into the shared trash instead of destroying it — the
/// "backup before destroy" rule in `docs/agent_friendly_design.md`, which is what
/// lets a delete be issued without a confirmation prompt and still be undone.
/// Returns the trash path: a delete that reports only success leaves nobody the
/// wiser about where the data went, so callers surface it.
///
/// A rename is the fast path, but a linked dataset dir can live on another volume
/// than the app-data dir, and Windows refuses to rename across that boundary — so
/// a failed rename copies instead. The copy lands under a `.partial` name and is
/// published only once the whole tree has been written, and only then is the
/// original removed: a half-copied backup can never be mistaken for a real one,
/// and a failed copy leaves the dataset exactly where it was.
///
/// The timestamp has second precision, so two deletions of same-named folders
/// inside one second would otherwise land on the same path and the second rename
/// would fail on Windows (or silently replace on Unix). The counter keeps each
/// deletion recoverable.
pub(crate) fn move_to_trash(path: &Path) -> Result<PathBuf, String> {
    let trash_dir = crate::app_paths::data_subdir("trash");
    fs::create_dir_all(&trash_dir).map_err(|e| format!("Failed to create trash directory: {}", e))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("Cannot trash {}: it has no file name", path.display()))?
        .to_string_lossy()
        .into_owned();
    let stem = format!("{}_{}", Utc::now().format("%Y%m%d_%H%M%S"), file_name);
    let mut trash_path = trash_dir.join(&stem);
    let mut n = 1;
    while trash_path.exists() {
        trash_path = trash_dir.join(format!("{}_{}", stem, n));
        n += 1;
    }

    if fs::rename(path, &trash_path).is_ok() {
        return Ok(trash_path);
    }
    log::warn!(
        "[Trash] rename {} -> {} failed, copying instead (probably another volume)",
        path.display(),
        trash_path.display()
    );

    let partial = trash_dir.join(format!("{}.partial", stem));
    let copied: Result<(), String> = if path.is_dir() {
        copy_dir_recursive(path, &partial)
    } else {
        fs::copy(path, &partial).map(|_| ()).map_err(|e| e.to_string())
    };
    if let Err(e) = copied {
        // Nothing was published and nothing was removed: the original is intact.
        if partial.is_dir() {
            let _ = fs::remove_dir_all(&partial);
        } else {
            let _ = fs::remove_file(&partial);
        }
        return Err(format!("Failed to copy {} into trash: {}", path.display(), e));
    }
    fs::rename(&partial, &trash_path).map_err(|e| {
        format!(
            "Copied {} into trash as {} but could not publish it: {}",
            path.display(),
            partial.display(),
            e
        )
    })?;
    let removed = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    if let Err(e) = removed {
        // Recoverable, but say so: the trash copy exists *and* the original is
        // still in place, which is the safe direction to have failed in.
        return Err(format!(
            "Backed up {} to {} but could not remove the original: {}",
            path.display(),
            trash_path.display(),
            e
        ));
    }
    Ok(trash_path)
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
    let info_path = path.join(INFO_FILE);

    if info_path.exists() {
        let info = read_info_opt(path)?;
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
    // A raw folder is not yet a dataset: the empty uuid is what tells the UI and
    // the sync catalog to skip it (see `useDictationData` and `sync::rest`).
    let mut info = DatasetInfo::new(
        String::new(),
        DatasetType::Dictation,
        FORMAT_DICTATION,
        path.file_name()?.to_string_lossy().into_owned(),
    );
    info.created_at = String::new();
    info.updated_at = String::new();
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
    let roots = dataset_roots(settings, DatasetType::Dictation);
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

/// Import a dataset from a source directory. The directory must contain a `media/`
/// subdirectory with at least one audio file. The entire directory is copied into
/// the managed datasets directory and an `info.json` is generated.
#[allow(dead_code)]
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

    let dir = dataset_roots(&settings, DatasetType::Dictation)
        .into_iter()
        .next()
        .unwrap_or_else(|| datasets_dir(&settings));
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create datasets dir: {}", e))?;

    let uuid = Uuid::new_v4().to_string();

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

    // An imported folder may already carry an `info.json` (a dataset exported by
    // another copy of the app). Keep its identity when it is ours to keep, and
    // reject it when another dataset already claims that id.
    let imported_info_path = dst.join(INFO_FILE);
    let info = match read_info_opt(&dst) {
        Some(mut existing) => {
            if existing.uuid == FAVORITES_DATASET_UUID || find_dataset_dir_typed(&settings, &existing.uuid).is_ok() {
                let _ = fs::remove_dir_all(&dst);
                return Err(format!(
                    "Imported dataset id {} is already used by a dataset on this machine; re-export it with a new id",
                    existing.uuid
                ));
            }
            existing.updated_at = now_stamp();
            existing
        }
        None => {
            if imported_info_path.exists() {
                log::warn!("Imported '{}' has an info.json we cannot read; replacing it", dir_name);
            }
            DatasetInfo::new(uuid, DatasetType::Dictation, FORMAT_DICTATION, name)
        }
    };

    write_info(&dst, &info)?;

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
    let info = read_info(&path)?;

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

/// Update the mutable descriptive fields of a dataset's `info.json`.
///
/// Identity (`uuid`), layout (`type`/`format`) and history (`created_at`) are not
/// settable here: changing any of them is a different dataset, not an edit.
#[tauri::command]
pub async fn dataset_update(
    settings: State<'_, SettingsState>,
    uuid: String,
    name: Option<String>,
    description: Option<String>,
    language: Option<String>,
) -> Result<(), String> {
    let path = find_dataset_dir(&settings, &uuid)?;
    let mut info = read_info(&path)?;

    if let Some(n) = name {
        info.name = n;
    }
    if let Some(d) = description {
        info.description = d;
    }
    if let Some(l) = language {
        info.language = l;
    }
    info.updated_at = now_stamp();

    write_info(&path, &info)
}

/// Delete a dataset: its whole directory moves into the shared trash rather than
/// being destroyed, so the operation stays undoable (see [`move_to_trash`]).
/// Returns the trash path.
#[tauri::command]
pub async fn dataset_delete(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<String, String> {
    let path = find_dataset_dir(&settings, &uuid)?;
    log::info!("Deleting dataset at '{}'", path.display());
    let trashed = move_to_trash(&path)?;
    log::info!("Dataset {} moved to trash '{}'", uuid, trashed.display());
    Ok(trashed.to_string_lossy().into_owned())
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
#[allow(dead_code)]
fn symlink_file(src: &Path, dst: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(src, dst).map_err(|e| format!("Failed to symlink: {}", e))
}

/// Bootstrap a dictation dataset's `data.sqlite3` so it exists (empty) from
/// creation: open the file and run the schema. **No-op when the DB already
/// exists**, so it never clobbers a fully generated database and is safe on the
/// idempotent init path. Media registration is deliberately NOT done here —
/// [`dataset_sync_media`] owns reconciling `listen_media` with the files on disk,
/// and `dataset_generate_database` rebuilds everything from the VTT subtitles.
fn ensure_dataset_db(dataset_dir: &Path) -> Result<(), String> {
    let db_path = dataset_dir.join("data.sqlite3");
    if db_path.exists() {
        return Ok(());
    }
    let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
    create_db_schema(&conn)?;
    log::info!("Created empty database at '{}'", db_path.display());
    Ok(())
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

    let roots = dataset_roots(&settings, DatasetType::Dictation);
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
    assert_uuid_free(&settings, &uuid)?;
    let dir_name = sanitize_dir_name(trimmed).unwrap_or_else(|| uuid.clone());
    let dst = root.join(&dir_name);
    if dst.exists() {
        return Err(format!("Dataset '{}' already exists", dir_name));
    }

    for sub in ["media", "subtitle", "waveform", "transcript"] {
        fs::create_dir_all(dst.join(sub)).map_err(|e| e.to_string())?;
    }

    let mut info = DatasetInfo::new(
        uuid.clone(),
        DatasetType::Dictation,
        FORMAT_DICTATION,
        trimmed.to_string(),
    );
    info.description = description.unwrap_or_default();
    write_info(&dst, &info)?;
    ensure_dataset_db(&dst)?;

    log::info!("Created dataset '{}' at '{}'", trimmed, dst.display());
    Ok(DatasetSummary {
        info,
        media_count: 0,
        path: dst.to_string_lossy().into_owned(),
        location: root.to_string_lossy().into_owned(),
        status: "not_ready".into(),
    })
}

/// Initialize a raw media folder **in place** as a dictation dataset.
///
/// Unlike `dataset_import` (which copies the folder into the managed datasets
/// directory), this writes a default `info.json` — with a freshly minted,
/// server-assigned uuid — into the folder exactly where it already sits. The
/// folder must live directly under a configured dictation root and contain at
/// least one media file, mirroring what `scan_dataset_dir` surfaces as a
/// no-uuid candidate. Idempotent: if a readable `info.json` already exists, its
/// summary is returned unchanged, so re-running is safe.
#[tauri::command]
pub async fn dataset_init_dir(
    settings: State<'_, SettingsState>,
    path: String,
) -> Result<DatasetSummary, String> {
    let dir = PathBuf::from(path.trim());
    if !dir.is_dir() {
        return Err(format!("Not a directory: {}", dir.display()));
    }

    // Only touch folders the app already treats as dataset candidates: they sit
    // directly inside a configured dictation root (default dir or a linked dir).
    let location = dataset_roots(&settings, DatasetType::Dictation)
        .into_iter()
        .find(|root| dir.parent() == Some(root.as_path()))
        .ok_or_else(|| "Folder is not directly inside a configured dataset location".to_string())?;

    let media_dir = dir.join("media");
    let media_files = list_media_files(&media_dir);
    if media_files.is_empty() {
        return Err(format!("No media files found in '{}'", media_dir.display()));
    }

    let info = if dir.join(INFO_FILE).exists() {
        read_info(&dir)?
    } else {
        let uuid = Uuid::new_v4().to_string();
        assert_uuid_free(&settings, &uuid)?;
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| uuid.clone());
        let fresh = DatasetInfo::new(uuid, DatasetType::Dictation, FORMAT_DICTATION, name);
        write_info(&dir, &fresh)?;
        log::info!(
            "Initialized dataset '{}' (uuid {}) in place at '{}'",
            fresh.name, fresh.uuid, dir.display()
        );
        fresh
    };

    // Ensure the dataset has its (empty) SQLite database from the start; media
    // registration is left to `dataset_sync_media` / `dataset_generate_database`.
    ensure_dataset_db(&dir)?;

    Ok(DatasetSummary {
        info,
        media_count: media_files.len(),
        path: dir.to_string_lossy().into_owned(),
        location: location.to_string_lossy().into_owned(),
        status: "not_ready".into(),
    })
}

/// Result of [`dataset_sync_media`]: how many `listen_media` rows were added /
/// removed to bring the DB in line with the `media/` folder.
#[derive(Serialize)]
pub struct MediaSyncResult {
    pub added_count: usize,
    pub removed_count: usize,
}

/// Reconcile a dataset's `listen_media` table with the files actually under
/// `media/` **in place**: INSERT a row for every media file not yet registered
/// and DELETE rows whose file has disappeared from disk, dropping each removed
/// media's subtitle / cue / version / transcript dependents so no orphan rows
/// survive. Idempotent — a no-op when the DB already matches the folder — and,
/// unlike [`dataset_generate_database`], it never rebuilds the DB or re-keys the
/// media that stay, so cues and (app-level) practice history for surviving files
/// are left untouched. Requires an existing `data.sqlite3` (created empty at
/// `dataset_create` / `dataset_init_dir`).
#[tauri::command]
pub async fn dataset_sync_media(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<MediaSyncResult, String> {
    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
    let db_path = dataset_dir.join("data.sqlite3");
    if !db_path.exists() {
        return Err("Database file not found. Create or initialize the dataset first.".into());
    }
    let media_dir = dataset_dir.join("media");

    let mut conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
    create_db_schema(&conn)?;

    // On-disk inventory, keyed by the DB's `source` form (media-relative, forward slash).
    let on_disk: std::collections::HashSet<String> = list_media_files(&media_dir)
        .iter()
        .map(|mf| rel_source_string(&media_dir, Path::new(&mf.path)))
        .collect();

    // Current DB inventory as (uuid, source) rows.
    let mut stmt = conn
        .prepare("SELECT uuid, source FROM listen_media")
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |row| {
            let media_uuid: String = row.get(0)?;
            let source: String = row.get(1)?;
            Ok((media_uuid, source))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);
    let in_db: std::collections::HashSet<String> =
        rows.iter().map(|(_, s)| s.clone()).collect();

    let now = Utc::now().to_rfc3339();
    let tx = conn.transaction().map_err(|e| e.to_string())?;

    // Register media present on disk but missing from the DB.
    let mut added_count = 0usize;
    for src in &on_disk {
        if in_db.contains(src) {
            continue;
        }
        tx.execute(
            "INSERT INTO listen_media (uuid, source, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![Uuid::new_v4().to_string(), src, now, now],
        )
        .map_err(|e| e.to_string())?;
        added_count += 1;
    }

    // Remove DB rows whose file is gone, cascading to their dependents via
    // subqueries so no orphan subtitle / cue / version / transcript rows survive.
    let mut removed_count = 0usize;
    for (media_uuid, src) in &rows {
        if on_disk.contains(src) {
            continue;
        }
        tx.execute(
            "DELETE FROM listen_subtitle_cue WHERE subtitle_uuid IN (SELECT uuid FROM listen_subtitle WHERE media_uuid = ?1)",
            [media_uuid],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM listen_subtitle_version WHERE subtitle_uuid IN (SELECT uuid FROM listen_subtitle WHERE media_uuid = ?1)",
            [media_uuid],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM listen_subtitle WHERE media_uuid = ?1", [media_uuid])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM listen_transcript WHERE media_uuid = ?1", [media_uuid])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM listen_media WHERE uuid = ?1", [media_uuid])
            .map_err(|e| e.to_string())?;
        removed_count += 1;
    }

    tx.commit().map_err(|e| e.to_string())?;

    if added_count > 0 || removed_count > 0 {
        touch_info(&dataset_dir)?;
    }
    log::info!(
        "Synced media for dataset '{}': +{} registered, -{} removed",
        uuid,
        added_count,
        removed_count
    );
    Ok(MediaSyncResult {
        added_count,
        removed_count,
    })
}

/// Overwrite a dataset's `info.json` from a full JSON document edited as text
/// (e.g. the Workflow page's editor). The text must parse as a `DatasetInfo`;
/// identity fields (`uuid`, `type`, `format`, `created_at`) are always taken from
/// what is already on disk — an edit can never re-key or re-type a dataset — and
/// `updated_at` is refreshed. The authoritative descriptor is returned so the
/// caller sees exactly what was written.
#[tauri::command]
pub async fn dataset_info_save(
    settings: State<'_, SettingsState>,
    uuid: String,
    content: String,
) -> Result<DatasetInfo, String> {
    let dir = find_dataset_dir(&settings, &uuid)?;
    let existing = read_info(&dir)?;

    let trimmed = content.trim_start_matches('\u{FEFF}');
    let mut parsed: DatasetInfo =
        serde_json::from_str(trimmed).map_err(|e| format!("Invalid info.json: {}", e))?;

    parsed.uuid = existing.uuid;
    parsed.dataset_type = existing.dataset_type;
    parsed.format = existing.format;
    parsed.created_at = existing.created_at;
    parsed.updated_at = now_stamp();

    write_info(&dir, &parsed)?;
    log::info!("Saved info.json for dataset '{}'", parsed.uuid);
    Ok(parsed)
}

/// Locate the built-in "Favorites" dataset WITHOUT creating it.
///
/// It is an ordinary dataset addressed by the reserved id [`FAVORITES_DATASET_UUID`]
/// rather than by a flag, so this is a plain uuid lookup instead of a scan of every
/// `info.json`. Returns `None` when it has not been created yet — read-only callers
/// (de-duplication, listing) must not cause a dataset to appear.
pub(crate) fn find_favorites_dataset(settings: &SettingsState) -> Option<(PathBuf, String)> {
    find_dataset_dir(settings, FAVORITES_DATASET_UUID)
        .ok()
        .map(|dir| (dir, FAVORITES_DATASET_UUID.to_string()))
}

/// Resolve the built-in "Favorites" dataset, creating it on first use.
///
/// The id is reserved and constant, so a Favorites dataset on the PC and one on the
/// phone are the *same* dataset: their clips merge under sync instead of forking per
/// device. `assert_uuid_free` is deliberately not called here — it would reject the
/// very id this function exists to mint, and any dataset it finds is by definition
/// the one it was looking for.
///
/// A pre-existing Favorites dataset carrying a random uuid (the shape before the
/// reserved id) is not adopted: it stays an ordinary dictation dataset until it is
/// migrated by hand.
pub(crate) fn ensure_favorites_dataset(settings: &SettingsState) -> Result<(PathBuf, String), String> {
    if let Some(found) = find_favorites_dataset(settings) {
        log::debug!("Found Favorites dataset at '{}'", found.0.display());
        return Ok(found);
    }

    // None found -- create one under the first configured root.
    let root = dataset_roots(settings, DatasetType::Dictation)
        .into_iter()
        .next()
        .unwrap_or_else(|| datasets_dir(settings));
    fs::create_dir_all(&root).map_err(|e| format!("Failed to create datasets dir: {}", e))?;

    let dir_name = sanitize_dir_name("Favorites").unwrap_or_else(|| FAVORITES_DATASET_UUID.to_string());
    // Avoid clobbering an existing unrelated "Favorites" directory.
    let mut dst = root.join(&dir_name);
    let mut n = 2;
    while dst.exists() {
        dst = root.join(format!("{}-{}", dir_name, n));
        n += 1;
    }

    for sub in ["media", "subtitle", "waveform", "transcript"] {
        fs::create_dir_all(dst.join(sub)).map_err(|e| e.to_string())?;
    }

    let mut info = DatasetInfo::new(
        FAVORITES_DATASET_UUID.to_string(),
        DatasetType::Dictation,
        FORMAT_DICTATION,
        "Favorites".to_string(),
    );
    info.description = "Clips cut from favorited cues".to_string();
    write_info(&dst, &info)?;

    log::info!("Created Favorites dataset '{}' at '{}'", FAVORITES_DATASET_UUID, dst.display());
    Ok((dst, FAVORITES_DATASET_UUID.to_string()))
}

/// Copy (or symlink) audio/video files from a source directory into a dataset's
/// media/ folder. Returns the number of files imported.
#[allow(dead_code)]
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
fn format_vtt_time(seconds: f32) -> String {
    let total_ms = (seconds * 1000.0).round() as u64;
    let h = total_ms / 3_600_000;
    let m = (total_ms % 3_600_000) / 60_000;
    let s = (total_ms % 60_000) / 1000;
    let ms = total_ms % 1000;
    format!("{:02}:{:02}:{:02}.{:03}", h, m, s, ms)
}

/// Check if a character is a sentence-ending punctuation.
#[allow(dead_code)]
fn is_sentence_end(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '\u{3002}' | '\u{ff01}' | '\u{ff1f}' | '\u{2026}') // 。！？！…
}

/// Merge word/token-level segments into sentence-level cues.
/// Splits on sentence-ending punctuation (. ! ? etc.).
/// Segments are concatenated directly (no extra spaces) because the model's
/// token output already encodes spacing (e.g. " Bekannt" has a leading space).
#[cfg(feature = "desktop")]
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
#[cfg(feature = "desktop")]
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
    for root in dataset_roots(settings, DatasetType::Dictation) {
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
            let info_path = path.join(INFO_FILE);
            if !info_path.exists() {
                continue;
            }
            let info = match read_info_opt(&path) {
                Some(i) => i,
                None => continue,
            };
            if info.uuid == uuid {
                return Ok(path);
            }
        }
    }
    Err(format!("Dataset with UUID {} not found", uuid))
}

/// Find a dataset directory by UUID across every dataset type (dictation, card,
/// book, read-aloud, wiki). Returns the resolved path together with its type so
/// callers — the REST manifest/snapshot endpoints and the sync client — know
/// both where the dataset lives and which local root it belongs to.
///
/// Every type now shares one `info.json` shape, so this is a typed parse. The
/// directory remains the routing authority: a dataset that declares a different
/// `type` than the tree it sits in is reported and skipped, since honouring it
/// would let a file edit move a dataset between types behind the UI's back.
pub(crate) fn find_dataset_dir_typed(
    settings: &SettingsState,
    uuid: &str,
) -> Result<(PathBuf, DatasetType), String> {
    // Every type the sync transport can serve. A type missing here cannot be
    // resolved by `/manifest`, `/snapshot`, `/changes` or `/file`, so it is
    // effectively unsyncable even when the catalog advertises it.
    for ty in DatasetType::ALL {
        for root in dataset_roots(settings, ty) {
            if !root.exists() {
                continue;
            }
            let entries = match fs::read_dir(&root) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() || !path.join(INFO_FILE).exists() {
                    continue;
                }
                let info = match read_info_opt(&path) {
                    Some(i) => i,
                    None => continue,
                };
                if info.uuid != uuid {
                    continue;
                }
                if info.dataset_type != ty {
                    log::warn!(
                        "[Dataset] '{}' declares type '{}' but lives under '{}'; ignoring it",
                        uuid,
                        info.dataset_type.as_str(),
                        ty.as_str()
                    );
                    continue;
                }
                return Ok((path, ty));
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

        let result = crate::models::transcribe_file(&model_state, file_path)?;

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

    let result = crate::models::transcribe_file(&model_state, &media_path)?;

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
    touch_info(&dataset_dir)?;

    Ok(())
}

/// Delete waveform JSON files for a dataset.
#[allow(dead_code)]
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
/// `WaveformCanvas` and by `datasets::dictation::adjust::load_waveform`.
#[allow(dead_code)]
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
    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
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

    log::info!("[waveform] Done: {}", source);
    Ok(source)
}

/// Advance a dataset to Stage 2: generate subtitles + waveforms.
#[cfg(feature = "desktop")]
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
#[allow(dead_code)]
pub(crate) struct VttCue {
    pub(crate) start_ms: i64,
    pub(crate) end_ms: i64,
    pub(crate) content: String,
}

/// Parse a VTT file into a list of cues.
#[allow(dead_code)]
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
#[allow(dead_code)]
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
pub(crate) fn create_db_schema(conn: &rusqlite::Connection) -> Result<(), String> {
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

    // §3.4: a per-dataset tombstone table (travels with snapshots) so a delete
    // is durable and can collide with a resurrected offline edit.
    crate::sync::change_log::ensure_tombstones(conn)?;

    Ok(())
}

/// Delete the SQLite database for a dataset and reset status to with_subtitle.
#[allow(dead_code)]
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
    touch_info(&dataset_dir)?;

    Ok(())
}

/// Generate the SQLite database for a dataset (Stage 2 → Stage 3).
#[allow(dead_code)]
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
    touch_info(&dataset_dir)?;

    log::info!("Database generated for dataset '{}': {} media, {} total", uuid, total, total);
    Ok(())
}

/// Write VTT subtitles to the existing database (without rebuilding the entire database).
/// Clears existing subtitle data and re-imports from VTT files by walking the
/// subtitle/ directory recursively.
#[allow(dead_code)]
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
    touch_info(&dataset_dir)?;

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
#[allow(dead_code)]
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
