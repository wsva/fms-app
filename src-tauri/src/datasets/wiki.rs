//! Wiki datasets: syncable markdown directory trees (dataset type `wiki`).
//!
//! A wiki dataset is a folder with an `info.json` (`uuid`, `name`, `updated`)
//! plus an arbitrary nested tree of `.md` files, living under
//! `<datasets_dir>/wiki/` (or any directory linked in that root's `meta.json`).
//! Unlike the other dataset types there is no `data.sqlite3`: the `.md` files
//! *are* the rows, and mutations journal through
//! [`crate::sync::change_log::commit_change`] as `wiki_file_save` /
//! `wiki_file_delete` changes (last-write-wins on the relative path). The
//! per-dataset `fts5.sqlite3` full-text index is **derived** — excluded from
//! manifest/snapshot shipping and rebuilt locally on every copy.
//!
//! The module is cross-platform on purpose: the Android thin client edits its
//! downloaded snapshot directly and lets the writeback queue carry changes to
//! the hub, so nothing here may depend on the `desktop` feature.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use crate::settings::SettingsState;
use crate::wiki::WikiSearchResult;

use super::{dataset_roots, DatasetType};

/// The derived FTS index inside a wiki dataset dir. Never synced, never listed.
const FTS_DB: &str = "fts5.sqlite3";

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// Minimal `info.json` shape. Kept lenient (`find_dataset_dir_typed` only ever
/// contracts on `uuid`) but rich enough for the UI list.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WikiDatasetInfo {
    pub uuid: String,
    pub name: String,
    #[serde(default)]
    pub structure: String,
    #[serde(default)]
    pub updated: String,
}

#[derive(Clone, Serialize, Debug)]
pub struct WikiDatasetSummary {
    pub uuid: String,
    pub name: String,
    pub updated: String,
    pub file_count: usize,
    /// Absolute path of the dataset directory on *this* machine.
    pub path: String,
    /// Root location this dataset was found under.
    pub location: String,
}

/// One entry of a wiki dataset's file tree. `rel_path` is dataset-relative
/// (forward slashes) so the same value works on a follower copy and in the
/// hub's REST browse responses — absolute paths mean nothing across devices.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WikiFileEntry {
    pub name: String,
    pub rel_path: String,
    pub is_dir: bool,
    pub modified: Option<String>,
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Normalize + validate a dataset-relative path. Rejects absolute paths and
/// any `..` component so a request can never escape the dataset directory;
/// returns the cleaned forward-slash form.
fn sanitize_rel(rel: &str) -> Result<String, String> {
    let unified = rel.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for part in unified.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return Err(format!("Path must stay inside the dataset: {rel}"));
        }
        // Windows alternate data streams / drive letters smuggled in a part.
        if part.contains(':') {
            return Err(format!("Invalid path component: {part}"));
        }
        parts.push(part);
    }
    if parts.is_empty() {
        return Err("Empty path".to_string());
    }
    // Derived index + identity file are never addressable content.
    let last = parts[parts.len() - 1];
    if last == FTS_DB || last == "info.json" || last.starts_with('.') {
        return Err(format!("{last} is not editable wiki content"));
    }
    Ok(parts.join("/"))
}

pub(crate) fn is_markdown(name: &str) -> bool {
    name.rsplit('/').next().unwrap_or(name)
        .rsplit('.').next()
        .map(|ext| ext.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.') || name == FTS_DB || name == "info.json"
}

fn get_modified_time(path: &Path) -> Option<String> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()
        .map(|t| {
            let duration = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            chrono::DateTime::from_timestamp(duration.as_secs() as i64, 0)
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                .unwrap_or_default()
        })
}

fn read_info(dir: &Path) -> Option<WikiDatasetInfo> {
    let data = fs::read_to_string(dir.join("info.json")).ok()?;
    serde_json::from_str(data.trim_start_matches('\u{FEFF}')).ok()
}

fn write_info(dir: &Path, info: &WikiDatasetInfo) -> Result<(), String> {
    let data = serde_json::to_string_pretty(info).map_err(|e| e.to_string())?;
    fs::write(dir.join("info.json"), data).map_err(|e| e.to_string())
}

/// Stamp a fresh `updated` time onto the dataset's `info.json` (best-effort:
/// the file write already succeeded; a metadata hiccup must not fail it).
fn bump_updated(dir: &Path) {
    if let Some(mut info) = read_info(dir) {
        info.updated = Utc::now().to_rfc3339();
        let _ = write_info(dir, &info);
    }
}

/// Resolve a wiki dataset directory by UUID across every configured wiki root.
pub(crate) fn find_dataset_dir(settings: &SettingsState, uuid: &str) -> Result<PathBuf, String> {
    for root in dataset_roots(settings, DatasetType::Wiki) {
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
            if read_info(&path).map(|i| i.uuid).as_deref() == Some(uuid) {
                return Ok(path);
            }
        }
    }
    Err(format!(
        "Wiki dataset not downloaded on this device (uuid {uuid}) — download it first"
    ))
}

/// Move a file or directory into the shared trash instead of destroying it
/// (the "backup before destroy" rule), returning the trash path.
fn move_to_trash(path: &Path) -> Result<PathBuf, String> {
    let trash_dir = crate::app_paths::data_subdir("trash");
    fs::create_dir_all(&trash_dir).map_err(|e| format!("Failed to create trash directory: {}", e))?;
    let file_name = path
        .file_name()
        .ok_or("Invalid file name")?
        .to_string_lossy();
    let trash_name = format!("{}_{}", Utc::now().format("%Y%m%d_%H%M%S"), file_name);
    let trash_path = trash_dir.join(trash_name);
    fs::rename(path, &trash_path).map_err(|e| format!("Failed to move to trash: {}", e))?;
    Ok(trash_path)
}

/// Folder name for a new dataset: filesystem-safe form of the display name.
fn folder_name_for(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect();
    let cleaned = cleaned.trim_matches('.').trim().to_string();
    if cleaned.is_empty() { "wiki".to_string() } else { cleaned }
}

// ---------------------------------------------------------------------------
// Inner operations (no Tauri `State` — shared by commands, MCP, REST, sync)
// ---------------------------------------------------------------------------

/// Recursively count the markdown files under `dir`.
fn count_markdown(dir: &Path) -> usize {
    let mut n = 0;
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if is_hidden(name) {
            continue;
        }
        if path.is_dir() {
            n += count_markdown(&path);
        } else if name.to_lowercase().ends_with(".md") {
            n += 1;
        }
    }
    n
}

pub(crate) fn list_datasets(settings: &SettingsState) -> Vec<WikiDatasetSummary> {
    let mut out = Vec::new();
    for root in dataset_roots(settings, DatasetType::Wiki) {
        if !root.exists() {
            continue;
        }
        let location = root.to_string_lossy().into_owned();
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(info) = read_info(&path) else {
                continue;
            };
            if info.uuid.is_empty() {
                continue;
            }
            out.push(WikiDatasetSummary {
                uuid: info.uuid,
                name: info.name,
                updated: info.updated,
                file_count: count_markdown(&path),
                path: path.to_string_lossy().into_owned(),
                location: location.clone(),
            });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Create a brand-new empty wiki dataset in the first configured root.
pub(crate) fn create_dataset(settings: &SettingsState, name: &str) -> Result<WikiDatasetSummary, String> {
    if name.trim().is_empty() {
        return Err("Dataset name must not be empty".into());
    }
    let roots = dataset_roots(settings, DatasetType::Wiki);
    let root = match roots.into_iter().next() {
        Some(r) => r,
        None => {
            let dir = settings.datasets_dir().join(DatasetType::Wiki.as_str());
            fs::create_dir_all(&dir).map_err(|e| format!("Failed to create wiki dir: {}", e))?;
            dir
        }
    };
    let mut folder = folder_name_for(name);
    if root.join(&folder).exists() {
        folder = format!("{}-{}", folder, &Uuid::new_v4().simple().to_string()[..8]);
    }
    let dir = root.join(&folder);
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create dataset dir: {}", e))?;

    let info = WikiDatasetInfo {
        uuid: Uuid::new_v4().to_string(),
        name: name.trim().to_string(),
        structure: "wiki-v1".to_string(),
        updated: Utc::now().to_rfc3339(),
    };
    let result = write_info(&dir, &info);
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&dir);
        return Err(format!("Failed to write info.json: {}", e));
    }
    log::info!("[WikiDataset] Created '{}' at {}", info.name, dir.display());
    Ok(WikiDatasetSummary {
        uuid: info.uuid,
        name: info.name,
        updated: info.updated,
        file_count: 0,
        path: dir.to_string_lossy().into_owned(),
        location: root.to_string_lossy().into_owned(),
    })
}

/// Convert an existing markdown folder into a wiki dataset *in place*: write
/// an `info.json` with a fresh uuid and link the folder into
/// `<datasets_dir>/wiki/meta.json`. No data is moved.
pub(crate) fn import_dir(settings: &SettingsState, name: &str, path: &str) -> Result<WikiDatasetSummary, String> {
    let dir = PathBuf::from(path);
    if !dir.exists() {
        return Err(format!("Directory does not exist: {}", path));
    }
    if !dir.is_dir() {
        return Err(format!("Path is not a directory: {}", path));
    }
    if let Some(existing) = read_info(&dir) {
        if !existing.uuid.is_empty() {
            return Err(format!("Directory is already a dataset (uuid {})", existing.uuid));
        }
    }
    let type_dir = settings.datasets_dir().join(DatasetType::Wiki.as_str());
    let type_dir_str = type_dir.to_string_lossy().to_string();
    let mut meta = super::read_dataset_meta(&type_dir_str);
    if meta.linked_dirs.iter().any(|d| d.path == path) {
        return Err(format!("Directory already linked: {}", path));
    }
    if list_datasets(settings).iter().any(|d| d.path == path) {
        return Err(format!("Directory already imported: {}", path));
    }

    let info = WikiDatasetInfo {
        uuid: Uuid::new_v4().to_string(),
        name: if name.trim().is_empty() {
            dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "wiki".into())
        } else {
            name.trim().to_string()
        },
        structure: "wiki-v1".to_string(),
        updated: Utc::now().to_rfc3339(),
    };
    write_info(&dir, &info).map_err(|e| format!("Failed to write info.json: {}", e))?;
    meta.linked_dirs.push(super::DatasetLinkedDir {
        name: info.name.clone(),
        path: dir.to_string_lossy().into_owned(),
    });
    super::write_dataset_meta(&type_dir_str, &meta)?;
    log::info!("[WikiDataset] Imported folder '{}' as dataset {}", path, info.uuid);
    Ok(WikiDatasetSummary {
        uuid: info.uuid.clone(),
        name: info.name.clone(),
        updated: info.updated.clone(),
        file_count: count_markdown(&dir),
        path: dir.to_string_lossy().into_owned(),
        location: type_dir_str,
    })
}

/// Delete a wiki dataset: drop its `meta.json` link when it lives in a linked
/// directory, then move the whole folder to the trash.
pub(crate) fn delete_dataset(settings: &SettingsState, uuid: &str) -> Result<(), String> {
    let dir = find_dataset_dir(settings, uuid)?;
    let type_dir = settings.datasets_dir().join(DatasetType::Wiki.as_str());
    let type_dir_str = type_dir.to_string_lossy().to_string();
    let mut meta = super::read_dataset_meta(&type_dir_str);
    let before = meta.linked_dirs.len();
    let dir_str = dir.to_string_lossy().into_owned();
    meta.linked_dirs.retain(|d| d.path != dir_str);
    if meta.linked_dirs.len() != before {
        super::write_dataset_meta(&type_dir_str, &meta)?;
    }
    let trash_path = move_to_trash(&dir)?;
    log::info!("[WikiDataset] Deleted dataset {} -> {}", uuid, trash_path.display());
    Ok(())
}

pub(crate) fn list_dir(settings: &SettingsState, uuid: &str, rel: &str) -> Result<Vec<WikiFileEntry>, String> {
    let ds_dir = find_dataset_dir(settings, uuid)?;
    let dir = if rel.trim().is_empty() || rel.trim() == "." || rel.trim() == "/" {
        ds_dir.clone()
    } else {
        let clean = sanitize_rel(rel)?;
        ds_dir.join(&clean)
    };
    if !dir.exists() {
        return Err(format!("Directory does not exist: {}", dir.display()));
    }
    if !dir.is_dir() {
        return Err(format!("Path is not a directory: {}", dir.display()));
    }
    let entries = fs::read_dir(&dir).map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_hidden(&name) {
            continue;
        }
        let rel_path = path
            .strip_prefix(&ds_dir)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        result.push(WikiFileEntry {
            name,
            rel_path,
            is_dir: path.is_dir(),
            modified: get_modified_time(&path),
        });
    }
    // Directories first, then files, both alphabetically (legacy wiki order).
    result.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(result)
}

pub(crate) fn read_file(settings: &SettingsState, uuid: &str, rel: &str) -> Result<String, String> {
    let ds_dir = find_dataset_dir(settings, uuid)?;
    let clean = sanitize_rel(rel)?;
    let path = ds_dir.join(&clean);
    if !path.is_file() {
        return Err(format!("File does not exist: {}", rel));
    }
    fs::read_to_string(&path).map_err(|e| e.to_string())
}

/// Write a markdown file and journal the change. Every copy of the dataset
/// (hub or follower) routes through here, so `commit_change`'s role gate
/// decides sync_log-append vs writeback-queue; under an `ApplyGuard` (rows
/// arriving *from* the hub) it is a no-op and cannot echo.
pub(crate) fn write_file(settings: &SettingsState, uuid: &str, rel: &str, content: &str) -> Result<(), String> {
    let ds_dir = find_dataset_dir(settings, uuid)?;
    let clean = sanitize_rel(rel)?;
    if !is_markdown(&clean) {
        return Err(format!("Only markdown (.md) files can be written: {}", rel));
    }
    let path = ds_dir.join(&clean);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {}", e))?;
    }
    fs::write(&path, content).map_err(|e| format!("Failed to write file: {}", e))?;
    bump_updated(&ds_dir);
    let payload = serde_json::json!({ "dataset_uuid": uuid, "rel_path": clean, "content": content });
    let _ = crate::sync::change_log::commit_change(settings, "wiki_file_save", uuid, &payload);
    log::info!("[WikiDataset] Wrote {}/{}", uuid, clean);
    Ok(())
}

pub(crate) fn create_file(settings: &SettingsState, uuid: &str, rel: &str) -> Result<(), String> {
    let ds_dir = find_dataset_dir(settings, uuid)?;
    let clean = sanitize_rel(rel)?;
    if ds_dir.join(&clean).exists() {
        return Err(format!("File already exists: {}", rel));
    }
    write_file(settings, uuid, &clean, "")
}

/// Create a directory inside the local copy only. Empty dirs are not
/// journaled (git-style): the dir materializes on other devices as soon as
/// the first file inside it syncs (`write_file` creates parent dirs).
pub(crate) fn create_dir(settings: &SettingsState, uuid: &str, rel: &str) -> Result<(), String> {
    let ds_dir = find_dataset_dir(settings, uuid)?;
    let clean = sanitize_rel(rel)?;
    let path = ds_dir.join(&clean);
    fs::create_dir_all(&path).map_err(|e| format!("Failed to create directory: {}", e))?;
    log::info!("[WikiDataset] Created dir {}/{}", uuid, clean);
    Ok(())
}

pub(crate) fn delete_file(settings: &SettingsState, uuid: &str, rel: &str) -> Result<(), String> {
    let ds_dir = find_dataset_dir(settings, uuid)?;
    let clean = sanitize_rel(rel)?;
    if !is_markdown(&clean) {
        return Err(format!("Only markdown (.md) files can be deleted: {}", rel));
    }
    let path = ds_dir.join(&clean);
    if !path.is_file() {
        return Err(format!("File does not exist: {}", rel));
    }
    let trash_path = move_to_trash(&path)?;
    bump_updated(&ds_dir);
    let payload = serde_json::json!({ "dataset_uuid": uuid, "rel_path": clean });
    let _ = crate::sync::change_log::commit_change(settings, "wiki_file_delete", uuid, &payload);
    log::info!("[WikiDataset] Deleted {}/{} -> {}", uuid, clean, trash_path.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Full-text search (derived `fts5.sqlite3`, rebuilt per copy)
// ---------------------------------------------------------------------------

fn open_fts_db(ds_dir: &Path) -> Result<Connection, String> {
    let conn = Connection::open(ds_dir.join(FTS_DB)).map_err(|e| e.to_string())?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS wiki_files (
            path TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            content TEXT,
            modified INTEGER,
            indexed_at INTEGER
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS wiki_fts USING fts5(
            path,
            name,
            content,
            content='wiki_files',
            content_rowid='rowid'
        );
        CREATE TRIGGER IF NOT EXISTS wiki_fts_update AFTER UPDATE ON wiki_files BEGIN
            DELETE FROM wiki_fts WHERE rowid = old.rowid;
            INSERT INTO wiki_fts (rowid, path, name, content) VALUES (new.rowid, new.path, new.name, new.content);
        END;
        CREATE TRIGGER IF NOT EXISTS wiki_fts_insert AFTER INSERT ON wiki_files BEGIN
            INSERT INTO wiki_fts (rowid, path, name, content) VALUES (new.rowid, new.path, new.name, new.content);
        END;
        CREATE TRIGGER IF NOT EXISTS wiki_fts_delete AFTER DELETE ON wiki_files BEGIN
            DELETE FROM wiki_fts WHERE rowid = old.rowid;
        END;
        "#,
    )
    .map_err(|e| e.to_string())?;
    Ok(conn)
}

fn collect_markdown_files(dir: &Path, ds_root: &Path, out: &mut Vec<(PathBuf, String)>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if is_hidden(name) {
            continue;
        }
        if path.is_dir() {
            collect_markdown_files(&path, ds_root, out)?;
        } else if name.to_lowercase().ends_with(".md") {
            let rel = path
                .strip_prefix(ds_root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            out.push((path, rel));
        }
    }
    Ok(())
}

/// Rebuild the dataset's local FTS index. Returns the number of files indexed.
pub(crate) fn index_dataset(settings: &SettingsState, uuid: &str) -> Result<u32, String> {
    let ds_dir = find_dataset_dir(settings, uuid)?;
    let conn = open_fts_db(&ds_dir)?;

    let mut files = Vec::new();
    collect_markdown_files(&ds_dir, &ds_dir, &mut files)?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let mut count = 0u32;
    let mut indexed_paths: Vec<String> = Vec::new();
    for (path, rel) in &files {
        let content = fs::read_to_string(path).unwrap_or_default();
        let modified = path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        tx.execute(
            "INSERT INTO wiki_files (path, name, content, modified, indexed_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET name = ?2, content = ?3, modified = ?4, indexed_at = ?5",
            params![rel, path.file_name().unwrap_or_default().to_string_lossy().as_ref(), content, modified, now],
        )
        .map_err(|e| e.to_string())?;
        indexed_paths.push(rel.clone());
        count += 1;
    }
    // Prune rows for files that no longer exist on this copy.
    if indexed_paths.is_empty() {
        tx.execute("DELETE FROM wiki_files", []).map_err(|e| e.to_string())?;
    } else {
        let placeholders: Vec<String> = indexed_paths.iter().map(|_| "?".to_string()).collect();
        let query = format!("DELETE FROM wiki_files WHERE path NOT IN ({})", placeholders.join(","));
        let params_refs: Vec<&dyn rusqlite::ToSql> =
            indexed_paths.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        tx.execute(&query, params_refs.as_slice()).map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    log::info!("[WikiDataset] Indexed {} files for dataset {}", count, uuid);
    Ok(count)
}

/// Build an FTS5 MATCH expression from a raw keyword, same rules as the
/// legacy wiki search: quoted phrases pass through, bare words get prefix
/// matching, FTS operators survive, dangling operators are stripped.
fn build_fts_query(keyword: &str) -> Option<String> {
    let trimmed = keyword.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains('"') {
        return Some(trimmed.to_string());
    }
    let operators = ["OR", "AND", "NOT", "NEAR", "or", "and", "not", "near"];
    let words: Vec<String> = trimmed
        .split_whitespace()
        .map(|w| {
            if operators.contains(&w) {
                w.to_uppercase()
            } else {
                format!("{}*", w)
            }
        })
        .collect();
    let mut cleaned: Vec<String> = Vec::new();
    for word in &words {
        if operators.contains(&word.as_str()) {
            if !cleaned.is_empty() && !operators.contains(&cleaned.last().unwrap().as_str()) {
                cleaned.push(word.clone());
            }
        } else {
            cleaned.push(word.clone());
        }
    }
    while let Some(last) = cleaned.last() {
        if operators.contains(&last.as_str()) {
            cleaned.pop();
        } else {
            break;
        }
    }
    if cleaned.is_empty() {
        return None;
    }
    Some(cleaned.join(" "))
}

/// Full-text search over one dataset's markdown. Indexes on first use, like
/// `wiki_search_local` does for the legacy wiki.
pub(crate) fn search_dataset(settings: &SettingsState, uuid: &str, keyword: &str) -> Result<Vec<WikiSearchResult>, String> {
    let ds_dir = find_dataset_dir(settings, uuid)?;
    let fts_query = match build_fts_query(keyword) {
        Some(q) => q,
        None => return Ok(Vec::new()),
    };
    let conn = open_fts_db(&ds_dir)?;
    let file_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM wiki_files", [], |row| row.get(0))
        .unwrap_or(0);
    drop(conn);
    if file_count == 0 {
        index_dataset(settings, uuid)?;
    }
    let conn = open_fts_db(&ds_dir)?;
    let mut stmt = conn
        .prepare(
            r#"
            SELECT
                wf.path,
                wf.name,
                snippet(wiki_fts, 2, '<mark>', '</mark>', '...', 20) as snippet,
                bm25(wiki_fts) as rank
            FROM wiki_fts
            JOIN wiki_files wf ON wiki_fts.path = wf.path
            WHERE wiki_fts MATCH ?1
            ORDER BY bm25(wiki_fts)
            LIMIT 50
            "#,
        )
        .map_err(|e| e.to_string())?;
    let results = stmt
        .query_map(params![fts_query], |row| {
            let rel: String = row.get(0)?;
            Ok(WikiSearchResult {
                file_path: rel.clone(),
                file_name: row.get(1)?,
                relative_path: rel,
                snippet: row.get(2)?,
                rank: row.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(results)
}

// ---------------------------------------------------------------------------
// Tauri commands (cross-platform: hub reads/journals, follower edits queue)
// ---------------------------------------------------------------------------

/// List all locally available wiki datasets.
#[tauri::command]
pub async fn wiki_dataset_list(settings: State<'_, SettingsState>) -> Result<Vec<WikiDatasetSummary>, String> {
    Ok(list_datasets(settings.inner()))
}

/// Create a new empty wiki dataset.
#[tauri::command]
pub async fn wiki_dataset_create(
    settings: State<'_, SettingsState>,
    name: String,
) -> Result<WikiDatasetSummary, String> {
    create_dataset(settings.inner(), &name)
}

/// Convert an existing markdown folder into a wiki dataset in place.
#[tauri::command]
pub async fn wiki_dataset_import_dir(
    settings: State<'_, SettingsState>,
    name: String,
    path: String,
) -> Result<WikiDatasetSummary, String> {
    import_dir(settings.inner(), &name, &path)
}

/// Delete a wiki dataset (moved to trash).
#[tauri::command]
pub async fn wiki_dataset_delete(settings: State<'_, SettingsState>, uuid: String) -> Result<(), String> {
    delete_dataset(settings.inner(), &uuid)
}

/// List one directory inside a wiki dataset (relative paths, '' = root).
#[tauri::command]
pub async fn wiki_dataset_list_dir(
    settings: State<'_, SettingsState>,
    uuid: String,
    rel: String,
) -> Result<Vec<WikiFileEntry>, String> {
    list_dir(settings.inner(), &uuid, &rel)
}

/// Read a markdown file from a wiki dataset.
#[tauri::command]
pub async fn wiki_dataset_read_file(
    settings: State<'_, SettingsState>,
    uuid: String,
    rel: String,
) -> Result<String, String> {
    read_file(settings.inner(), &uuid, &rel)
}

/// Write a markdown file in a wiki dataset (journals the change).
#[tauri::command]
pub async fn wiki_dataset_write_file(
    settings: State<'_, SettingsState>,
    uuid: String,
    rel: String,
    content: String,
) -> Result<(), String> {
    write_file(settings.inner(), &uuid, &rel, &content)
}

/// Create a new empty markdown file in a wiki dataset (journals the change).
#[tauri::command]
pub async fn wiki_dataset_create_file(
    settings: State<'_, SettingsState>,
    uuid: String,
    rel: String,
) -> Result<(), String> {
    create_file(settings.inner(), &uuid, &rel)
}

/// Create a directory inside a locally available wiki dataset (local-only;
/// the dir syncs implicitly with the first file written into it).
#[tauri::command]
pub async fn wiki_dataset_create_dir(
    settings: State<'_, SettingsState>,
    uuid: String,
    rel: String,
) -> Result<(), String> {
    create_dir(settings.inner(), &uuid, &rel)
}

/// Delete a markdown file from a wiki dataset (moved to trash, journals).
#[tauri::command]
pub async fn wiki_dataset_delete_file(
    settings: State<'_, SettingsState>,
    uuid: String,
    rel: String,
) -> Result<(), String> {
    delete_file(settings.inner(), &uuid, &rel)
}

/// Rebuild a wiki dataset's local FTS index. Returns the indexed file count.
#[tauri::command]
pub async fn wiki_dataset_index(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<u32, String> {
    index_dataset(settings.inner(), &uuid)
}

/// Full-text search inside one wiki dataset.
#[tauri::command]
pub async fn wiki_dataset_search(
    settings: State<'_, SettingsState>,
    uuid: String,
    keyword: String,
) -> Result<Vec<WikiSearchResult>, String> {
    search_dataset(settings.inner(), &uuid, &keyword)
}
