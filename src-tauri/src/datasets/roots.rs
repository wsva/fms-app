//! Where datasets live: the dataset-type enum, the type-to-directory mapping,
//! the `meta.json` list of linked locations, and the `dataset_*_dir` commands
//! that manage those links.

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::settings::SettingsState;

/// Resolve the default datasets directory.
///
/// When a workspace is selected, datasets always live in `<workspace>/datasets`
/// (derived from the workspace directory, not from settings.json). Linked
/// directories are read from `<workspace>/datasets/meta.json`. Without a
/// workspace, fall back to the configured/global default.
pub(crate) fn datasets_dir(settings: &SettingsState) -> PathBuf {
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
pub(crate) struct DatasetMeta {
    #[serde(default)]
    pub(crate) linked_dirs: Vec<DatasetLinkedDir>,
}

#[derive(Clone, Serialize, Debug)]
pub struct DatasetDirEntry {
    pub name: String,
    pub path: String,
    pub is_linked: bool,
    /// True for the app-maintained download root, whose datasets are read-only
    /// bases (dataset-layers.md §4.1). The UI renders it locked (no remove).
    #[serde(default)]
    pub is_read_only: bool,
}

pub(crate) fn dataset_meta_path(datasets_dir: &str) -> PathBuf {
    PathBuf::from(datasets_dir).join("meta.json")
}

pub(crate) fn read_dataset_meta(datasets_dir: &str) -> DatasetMeta {
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

pub(crate) fn write_dataset_meta(datasets_dir: &str, meta: &DatasetMeta) -> Result<(), String> {
    let meta_path = dataset_meta_path(datasets_dir);
    // Ensure the datasets directory exists
    fs::create_dir_all(datasets_dir).map_err(|e| e.to_string())?;
    let data = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
    fs::write(&meta_path, data).map_err(|e| e.to_string())
}

/// The `meta.json` that owns one type's linked locations: `<datasets>/<type>/meta.json`.
///
/// Linked directories are per type (a card location is not also a dictation
/// location), and both readers — `dataset_roots` and `dataset_list_dirs` — resolve
/// them from here. Every writer must target the same file: a root-level
/// `<datasets>/meta.json` is never consulted, so writing there makes an add/remove
/// silently invisible to the UI.
pub(crate) fn type_meta_dir(settings: &SettingsState, dataset_type: DatasetType) -> String {
    datasets_dir(settings)
        .join(dataset_type.as_str())
        .to_string_lossy()
        .into_owned()
}

/// Compare two location paths as *directories*, not as strings: the same folder
/// reached through a different separator, trailing slash or drive-letter case must
/// still match the entry it was linked with. Canonicalizes when the directory still
/// exists (which also folds Windows case) and falls back to the raw path otherwise
/// (e.g. an unplugged external drive).
pub(crate) fn same_location(a: &str, b: &str) -> bool {
    let canon = |p: &str| fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p));
    let (a, b) = (canon(a), canon(b));
    if cfg!(windows) {
        a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
    } else {
        a == b
    }
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

/// The app-maintained read-only download root for a type:
/// `<datasets_dir>/download/<type>/`. Every dataset here is a base pulled from
/// the online server, read-only by location (dataset-layers.md §4.1). It is a
/// *sibling* of the type dirs — never `<type>/download` — so it cannot collide
/// with a user dataset named "download".
pub(crate) fn download_root(settings: &SettingsState, dataset_type: DatasetType) -> PathBuf {
    datasets_dir(settings).join("download").join(dataset_type.as_str())
}

/// The reserved parent of every type's download root: `<datasets_dir>/download`.
pub(crate) fn download_base(settings: &SettingsState) -> PathBuf {
    datasets_dir(settings).join("download")
}

/// True when `path` lives under the app-maintained download root, i.e. it is a
/// read-only base. This is the single read-only predicate the write path uses.
///
/// Canonicalizes both sides when possible so a symlinked or relative path still
/// matches; falls back to the raw path when the target does not exist yet.
pub(crate) fn is_under_download_root(settings: &SettingsState, path: &Path) -> bool {
    let base = download_base(settings);
    let base_canon = fs::canonicalize(&base).unwrap_or(base);
    let path_canon = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    path_canon.starts_with(&base_canon)
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
    
    // Append the read-only download root LAST. `find_dataset_dir_typed` returns
    // the first match by root order, so a writable local dataset (or, later, an
    // overlay) for a uuid is always resolved before its downloaded base
    // (dataset-layers.md §4.4).
    let dl = download_root(settings, dataset_type);
    if dl.exists() && dl.is_dir() {
        roots.push(dl);
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
            is_read_only: false,
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
                is_read_only: false,
            });
        }
    }
    
    // Surface the app-maintained download root as a locked (read-only) entry so
    // the UI can show where downloaded bases live without offering to remove it.
    let dl = download_root(&settings, ds_type);
    if dl.exists() && dl.is_dir() {
        entries.push(DatasetDirEntry {
            name: "Downloaded (read-only)".to_string(),
            path: dl.to_string_lossy().into_owned(),
            is_linked: false,
            is_read_only: true,
        });
    }
    
    // Sort alphabetically
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    
    log::info!("[Dataset] Listed {} {} dataset directories", entries.len(), ds_type.as_str());
    Ok(entries)
}

/// Add a linked directory for one dataset type
#[tauri::command]
pub async fn dataset_add_dir(
    settings: State<'_, SettingsState>,
    dataset_type: String,
    name: String,
    path: String,
) -> Result<(), String> {
    let ds_type = DatasetType::from_str(&dataset_type)?;
    let path = path.trim().to_string();
    
    // Verify the path exists and is a directory
    let target_path = PathBuf::from(&path);
    if !target_path.exists() {
        return Err(format!("Directory does not exist: {}", path));
    }
    if !target_path.is_dir() {
        return Err(format!("Path is not a directory: {}", path));
    }
    
    // The download root is app-managed (read-only bases); it is never a manual
    // linked location (dataset-layers.md §4.2).
    if is_under_download_root(&settings, &target_path) {
        return Err("The download location is app-managed and cannot be added manually".into());
    }
    
    // Links live in `<datasets>/<type>/meta.json` — the exact file `dataset_roots`
    // and `dataset_list_dirs` read back.
    let meta_dir = type_meta_dir(&settings, ds_type);
    let mut meta = read_dataset_meta(&meta_dir);
    
    // Check if already linked
    if meta.linked_dirs.iter().any(|d| same_location(&d.path, &path)) {
        return Err(format!("Directory already linked: {}", path));
    }
    
    meta.linked_dirs.push(DatasetLinkedDir {
        name: name.clone(),
        path: path.clone(),
    });
    
    write_dataset_meta(&meta_dir, &meta)?;
    log::info!("[Dataset] {}: linked directory '{}' -> {}", ds_type.as_str(), name, path);
    Ok(())
}

/// Remove a linked directory from one dataset type's locations (unlink only — the
/// directory and the datasets inside it are left untouched on disk)
#[tauri::command]
pub async fn dataset_remove_dir(
    settings: State<'_, SettingsState>,
    dataset_type: String,
    path: String,
) -> Result<(), String> {
    let ds_type = DatasetType::from_str(&dataset_type)?;
    
    // The download root is app-managed; refuse to unlink it (dataset-layers.md §4.2).
    if is_under_download_root(&settings, &PathBuf::from(&path)) {
        return Err("The download location is app-managed and cannot be removed".into());
    }
    
    let meta_dir = type_meta_dir(&settings, ds_type);
    let mut meta = read_dataset_meta(&meta_dir);
    let initial_len = meta.linked_dirs.len();
    meta.linked_dirs.retain(|d| !same_location(&d.path, &path));
    
    if meta.linked_dirs.len() == initial_len {
        return Err(format!("Directory not found in {} locations: {}", ds_type.as_str(), path));
    }
    
    write_dataset_meta(&meta_dir, &meta)?;
    log::info!("[Dataset] {}: unlinked directory: {}", ds_type.as_str(), path);
    Ok(())
}
