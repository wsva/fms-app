//! Media files: the extension filter, on-disk enumeration, the paths that mirror
//! a media file into `subtitle/` and `waveform/`, the trash helper, the
//! create/init/sync commands, and the favourites dataset.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::Connection;
use serde::Serialize;
use tauri::State;
use uuid::Uuid;

use crate::settings::SettingsState;

use super::{
    DatasetInfo, FAVORITES_DATASET_UUID, FORMAT_DICTATION, INFO_FILE, LAYER_ROLE_FLAT,
    assert_uuid_free, now_stamp, read_info, touch_info, write_info,
};

use super::{
    DatasetSummary, DatasetType, MediaFile, create_db_schema, dataset_roots, datasets_dir,
    ensure_dataset_db, find_dataset_dir, find_dataset_dir_write, mint_into,
};

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
pub(crate) const MEDIA_EXTENSIONS: &[&str] = &[
    // audio
    "mp3", "wav", "flac", "ogg", "m4a", "m4b", "aac", "opus",
    // video (only the audio track is used)
    "mp4", "m4v", "mov", "mkv", "webm",
];

pub(crate) fn is_media_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    MEDIA_EXTENSIONS
        .iter()
        .any(|ext| lower.ends_with(&format!(".{}", ext)))
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
pub(crate) fn collect_media_files(root: &Path, dir: &Path, transcript_dir: &Path, out: &mut Vec<MediaFile>) {
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
pub(crate) fn media_rel_path(media_dir: &Path, media_path: &Path) -> PathBuf {
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
pub(crate) fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
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
// Studio: create dataset + import media
// ---------------------------------------------------------------------------
/// Sanitize a string for use as a directory name. Returns None when empty.
pub(crate) fn sanitize_dir_name(name: &str) -> Option<String> {
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
pub(crate) fn symlink_file(src: &Path, dst: &Path) -> Result<(), String> {
    std::os::windows::fs::symlink_file(src, dst).map_err(|e| format!("Failed to symlink: {}", e))
}

#[cfg(not(windows))]
#[allow(dead_code)]
pub(crate) fn symlink_file(src: &Path, dst: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(src, dst).map_err(|e| format!("Failed to symlink: {}", e))
}

/// Create a new empty dataset directory skeleton (media/, subtitle/, waveform/,
/// transcript/) plus an info.json under a configured dataset location.
///
/// `family` joins an existing family of datasets (the `<type>_<key>` id grammar
/// in [`super::ids`]) so this dataset becomes a sibling of the ones that already
/// carry that key; omit it to start a family of one.
#[tauri::command]
pub async fn dataset_create(
    settings: State<'_, SettingsState>,
    name: String,
    description: Option<String>,
    location: Option<String>,
    family: Option<String>,
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

    let uuid = mint_into(&settings, DatasetType::Dictation, family.as_deref())?;
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
        is_read_only: false,
        downloaded: false,
        has_local_changes: false,
        layer_role: LAYER_ROLE_FLAT.to_string(),
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
///
/// `family` is only consulted when the folder has no `info.json` yet — an
/// already-initialized dataset keeps the id it has.
#[tauri::command]
pub async fn dataset_init_dir(
    settings: State<'_, SettingsState>,
    path: String,
    family: Option<String>,
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
        let uuid = mint_into(&settings, DatasetType::Dictation, family.as_deref())?;
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
        is_read_only: false,
        downloaded: false,
        has_local_changes: false,
        layer_role: LAYER_ROLE_FLAT.to_string(),
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
    let dataset_dir = find_dataset_dir_write(&settings, &uuid)?;
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
    let dir = find_dataset_dir_write(&settings, &uuid)?;
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
    let dataset_dir = find_dataset_dir_write(&settings, &uuid)?;
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
