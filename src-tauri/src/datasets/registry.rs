//! Dataset identity: the summary/detail DTOs, discovery of every dataset under
//! the configured locations, import/update/delete, and the base/overlay layer
//! handover.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::State;
use uuid::Uuid;

use crate::settings::SettingsState;

use super::{
    DatasetInfo, FAVORITES_DATASET_UUID, FORMAT_DICTATION, INFO_FILE, LAYER_ROLE_FLAT,
    LAYER_ROLE_OVERLAY, assert_uuid_free, family_of, is_valid_key, mint_into, now_stamp, read_info,
    read_info_opt, resolve_family, write_info,
};

use super::{
    DatasetType, copy_dir_recursive, dataset_roots, datasets_dir, find_dataset_dir_read,
    find_dataset_dir_typed, is_under_download_root, list_media_files,
    move_to_trash,
};

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
    /// True when this dataset lives under the reserved read-only download root,
    /// so every mutation is refused (dataset-layers.md §6).
    #[serde(default)]
    pub is_read_only: bool,
    /// True when this entry is a downloaded base. Equal to `is_read_only` in
    /// Phase 1 (no overlay growth yet).
    #[serde(default)]
    pub downloaded: bool,
    /// True when a writable overlay has diverged from this base. Always false in
    /// Phase 1 — overlay growth lands with the `merged/` cache (Phase 2).
    #[serde(default)]
    pub has_local_changes: bool,
    /// `info.layer.role`, or `"flat"` when the dataset carries no layer block.
    #[serde(default)]
    pub layer_role: String,
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

/// One dataset of a family. `dataset_type` is the slug the dataset was *found*
/// under, which is authoritative; the prefix in `uuid` is only a claim (see
/// [`super::ids`]).
#[derive(Clone, Serialize)]
pub struct FamilyMember {
    pub uuid: String,
    pub dataset_type: String,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub is_read_only: bool,
}

/// Everything knowable about a family in one read: its members, and the two
/// derived facts a UI needs — the book to link back to and a name to show.
///
/// A family is a relation, not an entity, so this report is computed per call
/// from the ids and never stored. `family` is `None` for a dataset whose id does
/// not follow the grammar, in which case `members` holds that one dataset: an
/// unaffiliated dataset is a family of one, which lets callers skip the special
/// case.
#[derive(Clone, Serialize)]
pub struct FamilyReport {
    pub family: Option<String>,
    pub members: Vec<FamilyMember>,
    /// The book member's uuid, when the family has one — what a derived page's
    /// breadcrumb links back to.
    pub anchor: Option<String>,
    /// The book member's name, else the alphabetically-first member's.
    pub display_name: Option<String>,
}

pub struct DatasetState;

impl DatasetState {
    pub fn new() -> Self {
        Self
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------
/// Scan a single directory and, if it looks like a dataset, build a summary.
/// `location` is the root directory the dataset was found under.
pub(crate) fn scan_dataset_dir(
    settings: &SettingsState,
    path: &Path,
    location: &str,
) -> Option<DatasetSummary> {
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
        let read_only = is_under_download_root(settings, path);
        let layer_role = info
            .layer
            .as_ref()
            .map(|l| l.role.clone())
            .unwrap_or_else(|| LAYER_ROLE_FLAT.to_string());
        // An overlay lives in the writable area (so it is not read-only) but
        // *has* a downloaded base and, by definition, local changes (§4.4).
        let is_overlay = layer_role == LAYER_ROLE_OVERLAY;
        return Some(DatasetSummary {
            info,
            media_count,
            path: path_str,
            location: location.to_string(),
            status: status.into(),
            is_read_only: read_only,
            downloaded: read_only || is_overlay,
            has_local_changes: is_overlay,
            layer_role,
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
        is_read_only: false,
        downloaded: false,
        has_local_changes: false,
        layer_role: LAYER_ROLE_FLAT.to_string(),
    })
}

/// Deduplicate a listing by dataset uuid, keeping the **first** occurrence.
///
/// `dataset_roots` appends the read-only download root last, so a writable local
/// dataset (or a future overlay) for a uuid is always encountered before its
/// downloaded base — keeping the first therefore prefers the writable copy
/// (dataset-layers.md §4.4 Case 1). Entries with an empty uuid (raw, not-yet
/// imported folders) are never deduped.
pub(crate) fn dedup_by_uuid_first<T, F>(items: Vec<T>, uuid_of: F) -> Vec<T>
where
    F: Fn(&T) -> &str,
{
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let uuid = uuid_of(&item);
        if uuid.is_empty() || seen.insert(uuid.to_string()) {
            out.push(item);
        }
    }
    out
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
            if let Some(summary) = scan_dataset_dir(&settings, &entry.path(), &location) {
                datasets.push(summary);
            }
        }
    }

    datasets = dedup_by_uuid_first(datasets, |s| s.info.uuid.as_str());
    datasets.sort_by(|a, b| a.info.name.cmp(&b.info.name));
    log::info!("Found {} dataset(s)", datasets.len());
    datasets
}

/// Import a dataset from a source directory. The directory must contain a `media/`
/// subdirectory with at least one audio file. The entire directory is copied into
/// the managed datasets directory and an `info.json` is generated.
///
/// `family` is consulted only when the folder carries no usable `info.json`: a
/// dataset exported by another copy of the app keeps the identity it arrived
/// with, exactly as before.
#[allow(dead_code)]
#[tauri::command]
pub async fn dataset_import(
    settings: State<'_, SettingsState>,
    source_dir: String,
    family: Option<String>,
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

    // Use the source directory name as the dataset folder name. A path with no
    // file name (a filesystem root) falls back to a throwaway id: the dataset's
    // own uuid is only decided below, once we know whether the folder brought one.
    let dir_name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

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
            let uuid = match mint_into(&settings, DatasetType::Dictation, family.as_deref())
                .and_then(|uuid| assert_uuid_free(&settings, &uuid).map(|()| uuid))
            {
                Ok(uuid) => uuid,
                Err(e) => {
                    // The copy already happened; leaving it behind would register
                    // a half-imported dataset with no identity.
                    let _ = fs::remove_dir_all(&dst);
                    return Err(e);
                }
            };
            DatasetInfo::new(uuid, DatasetType::Dictation, FORMAT_DICTATION, name)
        }
    };

    write_info(&dst, &info)?;

    log::info!("Imported dataset '{}' with {} media file(s)", dir_name, media_count);
    Ok(DatasetSummary { info, media_count, path: dst.to_string_lossy().into_owned(), location: dir.to_string_lossy().into_owned(), status: "not_ready".into(), is_read_only: false, downloaded: false, has_local_changes: false, layer_role: LAYER_ROLE_FLAT.to_string() })
}

/// Get detailed information about a specific dataset, including its media files.
#[tauri::command]
pub async fn dataset_get(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<DatasetDetail, String> {
    let path = find_dataset_dir_read(&settings, &uuid)?;
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

/// The datasets that share a family key — the `<type>_<key>` relation defined in
/// [`super::ids`]. `uuid_or_key` may be a member's uuid or a bare family key.
///
/// Nothing here is stored: the report is rebuilt from the ids on every call, so
/// it cannot drift the way a family registry would.
#[tauri::command]
pub async fn dataset_family(
    settings: State<'_, SettingsState>,
    uuid_or_key: String,
) -> Result<FamilyReport, String> {
    family_report(&settings, &uuid_or_key)
}

/// Body of [`dataset_family`], split out so the MCP tool and the command share it.
///
/// The dataset lookup comes first, and that order is load-bearing: a legacy
/// 32-hex uuid *is* a well-formed family key, so testing [`is_valid_key`] first
/// would send every pre-scheme dataset down the family path and answer "no
/// dataset belongs to family …" for an id that plainly exists. A bare key can
/// never be a registered dataset id, so dataset-first is unambiguous both ways.
pub(crate) fn family_report(
    settings: &SettingsState,
    uuid_or_key: &str,
) -> Result<FamilyReport, String> {
    let arg = uuid_or_key.trim();
    if arg.is_empty() {
        return Err("dataset_family needs a dataset uuid or a family key".into());
    }

    if let Ok((path, ty)) = find_dataset_dir_typed(settings, arg) {
        let info = read_info(&path)?;
        let Some(family) = family_of(&info.uuid) else {
            // Unaffiliated: reported as a family of one, so a caller never needs
            // a special case for a legacy dataset.
            return Ok(report_from(None, vec![member_of(settings, &info, ty, &path)]));
        };
        return Ok(report_from(Some(family.to_string()), members_of(settings, family)));
    }

    if is_valid_key(arg) {
        super::family_resolves(settings, arg)?;
        return Ok(report_from(Some(arg.to_string()), members_of(settings, arg)));
    }

    Err(format!(
        "No dataset has id '{}', and it is not a family key either. Pass a uuid from \
         `dataset_list`, or the key part of one (an id looks like '{}').",
        arg,
        super::ids::mint(DatasetType::Book, "a1b2c3d4e5f6")
    ))
}

/// Read every dataset carrying `family`, in `DatasetType::ALL` order so the UI
/// shows siblings in a stable sequence.
fn members_of(settings: &SettingsState, family: &str) -> Vec<FamilyMember> {
    resolve_family(settings, family)
        .into_iter()
        .filter_map(|(ty, uuid, path)| match read_info(&path) {
            Ok(info) => Some(member_of(settings, &info, ty, &path)),
            // A member we cannot read is not one we can report; a broken
            // `info.json` is the audit tools' business, not this one's.
            Err(e) => {
                log::warn!("Skipping family member '{}': {}", uuid, e);
                None
            }
        })
        .collect()
}

fn member_of(
    settings: &SettingsState,
    info: &DatasetInfo,
    ty: DatasetType,
    path: &Path,
) -> FamilyMember {
    FamilyMember {
        uuid: info.uuid.clone(),
        // The type it was *found* under, not the one its prefix claims.
        dataset_type: ty.as_str().to_string(),
        name: info.name.clone(),
        path: path.to_string_lossy().into_owned(),
        is_read_only: is_under_download_root(settings, path),
    }
}

/// Derive the two facts a UI asks of a family: which member to link back to, and
/// what to call the family. Both come from the members — a family owns no name.
fn report_from(family: Option<String>, members: Vec<FamilyMember>) -> FamilyReport {
    let anchor = members
        .iter()
        .find(|m| m.dataset_type == DatasetType::Book.as_str())
        .map(|m| m.uuid.clone());
    let display_name = anchor
        .as_deref()
        .and_then(|a| members.iter().find(|m| m.uuid == a))
        .map(|m| m.name.clone())
        .or_else(|| members.iter().map(|m| m.name.clone()).min());
    FamilyReport { family, members, anchor, display_name }
}

/// One family a dataset could join, as the "Join family" picker asks for it:
/// the key, what the family already holds, and one addressable member to name
/// the row by.
#[derive(Clone, Serialize)]
pub struct FamilyCandidate {
    pub family: String,
    /// The member the row is labelled by — the book when there is one, else the
    /// alphabetically-first member, the same rule [`report_from`] names a family
    /// with.
    pub uuid: String,
    pub name: String,
    pub dataset_type: String,
    /// Every type the family holds, in `DatasetType::ALL` order.
    pub member_types: Vec<String>,
}

/// Every dataset on this machine, deduped by uuid.
///
/// Not [`list_datasets`]: that one scans the dictation root alone and surfaces
/// raw media folders for the UI, neither of which can be joined to anything.
/// A family spans types by definition, so this walks every type's roots. The
/// directory stays the routing authority, exactly as in
/// [`find_dataset_dir_typed`]: a dataset declaring a type other than the tree it
/// sits in is skipped rather than trusted.
fn every_dataset(settings: &SettingsState) -> Vec<(DatasetInfo, DatasetType)> {
    let mut out = Vec::new();
    for ty in DatasetType::ALL {
        for root in dataset_roots(settings, ty) {
            let Ok(entries) = fs::read_dir(&root) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(info) = read_info_opt(&path) else { continue };
                if info.uuid.is_empty() || info.dataset_type != ty {
                    continue;
                }
                out.push((info, ty));
            }
        }
    }
    // Root order puts a writable copy before its downloaded base, so keeping the
    // first hides the base behind the copy it was forked into.
    dedup_by_uuid_first(out, |(info, _)| info.uuid.as_str())
}

/// The families `uuid`'s dataset could join.
///
/// One row per family, because joining is a relation: two members of one family
/// would be two names for the same outcome. A family that already holds a
/// dataset of this one's type is left out — the grammar mints one dataset per
/// type per family, and a second `card_<key>` would be shadowed by the first on
/// every lookup (see [`super::ids`]).
pub(crate) fn family_candidates(
    settings: &SettingsState,
    uuid: &str,
) -> Result<Vec<FamilyCandidate>, String> {
    let (_dir, ty) = find_dataset_dir_typed(settings, uuid)?;
    let own_key = family_of(uuid).map(str::to_string);

    let mut groups: BTreeMap<String, Vec<(DatasetType, DatasetInfo)>> = BTreeMap::new();
    for (info, found) in every_dataset(settings) {
        if info.uuid == uuid {
            continue;
        }
        let Some(key) = family_of(&info.uuid) else { continue };
        // The family it is already in would be a no-op join.
        if own_key.as_deref() == Some(key) {
            continue;
        }
        groups.entry(key.to_string()).or_default().push((found, info));
    }

    let mut out = Vec::new();
    for (family, mut members) in groups {
        if members.iter().any(|(found, _)| *found == ty) {
            continue;
        }
        members.sort_by(|a, b| {
            (a.0 != DatasetType::Book, a.1.name.to_lowercase())
                .cmp(&(b.0 != DatasetType::Book, b.1.name.to_lowercase()))
        });
        let (rep_ty, rep) = &members[0];
        out.push(FamilyCandidate {
            family,
            uuid: rep.uuid.clone(),
            name: rep.name.clone(),
            dataset_type: rep_ty.as_str().to_string(),
            member_types: members
                .iter()
                .map(|(found, _)| found.as_str().to_string())
                .collect(),
        });
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

/// Re-key `uuid`'s dataset into `target_uuid`'s family: its id becomes
/// `<its type>_<the target's key>`, which *is* the whole of what a family is
/// (see [`super::ids`]). This is the one manual way to build a family, for the
/// pairs the ids alone cannot prove — the book and the deck someone made from it
/// separately, with nothing in either name to say so.
///
/// Only `info.json`'s uuid changes. The directory keeps its name (the id, not the
/// folder, is what the app and the sync protocol address) and no content is
/// touched. Practice state that hangs off a card or a user — review schedules,
/// XP totals — is keyed by those, not by this dataset, so it survives. What does
/// not follow is everything that *addresses* the dataset by id: its sync state,
/// dictation progress, the card search index, and any copy outside the app still
/// holding the old id. The UI warns about exactly that before calling this.
///
/// Refused where re-keying back could not undo it: a read-only base, an overlay
/// (its id is its base's), a target that carries no key, a family that already
/// holds this type, and a target id something else answers to.
pub(crate) fn join_family(
    settings: &SettingsState,
    uuid: &str,
    target_uuid: &str,
) -> Result<DatasetInfo, String> {
    let (dir, ty) = find_dataset_dir_typed(settings, uuid)?;
    let mut info = read_info(&dir)?;

    if uuid == target_uuid {
        return Err(format!(
            "'{}' is this dataset itself — pick another one to join",
            info.name
        ));
    }
    let (_target_dir, target_ty) = find_dataset_dir_typed(settings, target_uuid)?;

    if is_under_download_root(settings, &dir) {
        return Err(format!(
            "'{}' is a downloaded copy and stays read-only — join a family from one of your own datasets",
            info.name
        ));
    }
    if info.layer.is_some() {
        return Err(format!(
            "'{}' is an overlay of a downloaded copy, so its id is its base's — flatten it first, then join a family",
            info.name
        ));
    }

    let Some(key) = family_of(target_uuid) else {
        return Err(format!(
            "'{}' (the {} you picked) has an id from before the family scheme, so it has no key to share",
            target_uuid,
            target_ty.as_str()
        ));
    };
    if family_of(&info.uuid) == Some(key) {
        return Err(format!(
            "'{}' already belongs to family '{}'",
            info.name, key
        ));
    }
    // Same rule `mint_into` enforces on creation, from the other side: a family
    // holds one dataset per type, or lookups would only ever find the first.
    if let Some((_, existing, path)) = resolve_family(settings, key)
        .into_iter()
        .find(|(found, ..)| *found == ty)
    {
        return Err(format!(
            "Family '{}' already holds a {} dataset ('{}' at '{}') — one dataset per type per family",
            key,
            ty.as_str(),
            existing,
            path.display()
        ));
    }

    let new_id = super::ids::mint(ty, key);
    assert_uuid_free(settings, &new_id)?;

    let old_id = std::mem::replace(&mut info.uuid, new_id);
    info.updated_at = now_stamp();
    write_info(&dir, &info)?;
    log::info!(
        "[Dataset] Joined family '{}': '{}' -> '{}' at '{}'",
        key,
        old_id,
        info.uuid,
        dir.display()
    );
    Ok(info)
}

/// The families a dataset could join, for the edit form's picker.
#[tauri::command]
pub async fn dataset_family_candidates(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<Vec<FamilyCandidate>, String> {
    family_candidates(&settings, &uuid)
}

/// Re-key a dataset into another dataset's family. Destructive to the *id* — the
/// sync state, dictation progress and the search index address the dataset by
/// it — so the caller asks first (the Cards page's Join family dialog carries the
/// warning).
#[tauri::command]
pub async fn dataset_join_family(
    settings: State<'_, SettingsState>,
    uuid: String,
    target_uuid: String,
) -> Result<DatasetInfo, String> {
    join_family(&settings, &uuid, &target_uuid)
}

/// Layer / read-only status of a dataset, for status-before-acting checks
/// (dataset-layers.md §6). `role` comes from `info.layer`; `is_read_only` /
/// `downloaded` come from the dataset's *location* under the reserved download
/// root, not from `info.json`.
#[derive(Clone, Serialize)]
pub struct DatasetLayerStatus {
    pub uuid: String,
    pub role: String,
    pub is_read_only: bool,
    pub downloaded: bool,
    /// True when a writable overlay has been forked on top of a downloaded base
    /// (i.e. `role == "overlay"`): there are local changes to detach or flatten.
    pub has_overlay: bool,
    pub base_uuid: Option<String>,
    pub base_version: Option<String>,
    pub path: String,
}

/// Core layer-status resolution, reusable without Tauri `State` (MCP + command).
pub(crate) fn layer_status(
    settings: &SettingsState,
    uuid: &str,
) -> Result<DatasetLayerStatus, String> {
    let (dir, _ty) = find_dataset_dir_typed(settings, uuid)?;
    let info = read_info(&dir)?;
    let read_only = is_under_download_root(settings, &dir);
    let layer = info.layer.as_ref();
    let is_overlay = layer.map(|l| l.role.as_str()) == Some(LAYER_ROLE_OVERLAY);
    Ok(DatasetLayerStatus {
        uuid: info.uuid,
        role: layer
            .map(|l| l.role.clone())
            .unwrap_or_else(|| LAYER_ROLE_FLAT.to_string()),
        is_read_only: read_only,
        downloaded: read_only || is_overlay,
        has_overlay: is_overlay,
        base_uuid: layer.and_then(|l| l.base_uuid.clone()),
        base_version: layer.and_then(|l| l.base_version.clone()),
        path: dir.to_string_lossy().into_owned(),
    })
}

/// Report a dataset's layer / read-only status before attempting a mutation.
#[tauri::command]
pub async fn dataset_layer_status(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<DatasetLayerStatus, String> {
    layer_status(&settings, &uuid)
}

/// Detach a downloaded dataset's local overlay: the overlay directory is moved
/// to the trash, reverting the uuid to its pristine read-only base
/// (dataset-layers.md §11). Non-destructive to the base; undoable via the trash.
pub(crate) fn layer_detach(settings: &SettingsState, uuid: &str) -> Result<String, String> {
    let (dir, _ty) = find_dataset_dir_typed(settings, uuid)?;
    let info = read_info(&dir)?;
    if info.layer.as_ref().map(|l| l.role.as_str()) != Some(LAYER_ROLE_OVERLAY) {
        return Err(format!(
            "nothing to detach: '{}' has no local overlay (it is a {}); edit it first to grow an overlay, or delete it to remove the download",
            info.name,
            if is_under_download_root(settings, &dir) { "read-only base" } else { "flat dataset" }
        ));
    }
    let trashed = move_to_trash(&dir)?;
    log::info!("[Layer] Detached overlay for '{}' -> trash '{}'", uuid, trashed.display());
    Ok(trashed.to_string_lossy().into_owned())
}

/// Detach a downloaded dataset's local overlay, reverting to the pristine base.
#[tauri::command]
pub async fn dataset_layer_detach(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<String, String> {
    layer_detach(&settings, &uuid)
}

/// Flatten an overlay into a standalone writable dataset (todo item 9): strip the
/// `layer` block so it no longer depends on `download/` (dataset-layers.md §11).
/// Its media are hardlinks into the base, which survive the base's deletion (a
/// hardlink is an independent directory entry), so nothing else needs copying.
pub(crate) fn layer_flatten(
    settings: &SettingsState,
    uuid: &str,
) -> Result<DatasetLayerStatus, String> {
    let (dir, _ty) = find_dataset_dir_typed(settings, uuid)?;
    let mut info = read_info(&dir)?;
    if info.layer.as_ref().map(|l| l.role.as_str()) != Some(LAYER_ROLE_OVERLAY) {
        return Err(format!(
            "cannot flatten: '{}' is not an overlay (role '{}'); only a downloaded dataset's local overlay can be flattened into a standalone dataset",
            info.name,
            info.layer.as_ref().map(|l| l.role.as_str()).unwrap_or(LAYER_ROLE_FLAT)
        ));
    }
    info.layer = None;
    info.updated_at = now_stamp();
    write_info(&dir, &info)?;
    log::info!("[Layer] Flattened overlay '{}' into a standalone dataset at '{}'", uuid, dir.display());
    layer_status(settings, uuid)
}

/// Flatten an overlay into a standalone writable dataset (drop the base link).
#[tauri::command]
pub async fn dataset_layer_flatten(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<DatasetLayerStatus, String> {
    layer_flatten(&settings, &uuid)
}

/// Update the mutable descriptive fields of a dataset's `info.json`.
///
/// Identity (`uuid`), layout (`type`/`format`) and history (`created_at`) are not
/// settable here: changing any of them is a different dataset, not an edit. The
/// body lives in [`crate::datasets::update_info_fields`] so the MCP tool
/// `dataset_info_update` and this command cannot diverge over which fields are
/// settable or over what happens to a downloaded (read-only) base.
pub(crate) fn update_dataset(
    settings: &SettingsState,
    uuid: &str,
    name: Option<&str>,
    description: Option<&str>,
    language: Option<&str>,
    visibility: Option<&str>,
) -> Result<DatasetInfo, String> {
    crate::datasets::update_info_fields(
        settings,
        uuid,
        name,
        description,
        language,
        visibility,
        None,
    )
}

/// Update a dataset's descriptive fields, returning the descriptor actually
/// written — the dataset card's flip-to-edit back face binds straight to this.
#[tauri::command]
pub async fn dataset_update(
    settings: State<'_, SettingsState>,
    uuid: String,
    name: Option<String>,
    description: Option<String>,
    language: Option<String>,
    visibility: Option<String>,
) -> Result<DatasetInfo, String> {
    let info = update_dataset(
        &settings,
        &uuid,
        name.as_deref(),
        description.as_deref(),
        language.as_deref(),
        visibility.as_deref(),
    )?;
    log::info!("Updated info.json metadata for dataset '{}'", info.uuid);
    Ok(info)
}

/// Delete a dataset: its whole directory moves into the shared trash rather than
/// being destroyed, so the operation stays undoable (see [`move_to_trash`]).
/// Returns the trash path.
///
/// Layered delete (dataset-layers.md §6): resolves the **top** layer and never
/// forks. When an overlay exists it is the top layer, so deleting *detaches* the
/// local edits and leaves the read-only base intact; deleting again removes the
/// base (the download). A flat dataset deletes outright.
#[tauri::command]
pub async fn dataset_delete(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<String, String> {
    let path = find_dataset_dir_read(&settings, &uuid)?;
    log::info!("Deleting dataset at '{}'", path.display());
    let trashed = move_to_trash(&path)?;
    log::info!("Dataset {} moved to trash '{}'", uuid, trashed.display());
    Ok(trashed.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_settings(tag: &str) -> (SettingsState, PathBuf) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("fms-family-test-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        let settings = SettingsState::new();
        settings.set_workspace_dir(Some(dir.clone()));
        (settings, dir)
    }

    /// A dataset is a directory with an `info.json`; the family lookups read
    /// nothing else, so nothing else is created.
    fn put(
        settings: &SettingsState,
        ty: DatasetType,
        dir_name: &str,
        uuid: &str,
        name: &str,
    ) -> PathBuf {
        let dir = datasets_dir(settings).join(ty.as_str()).join(dir_name);
        fs::create_dir_all(&dir).unwrap();
        let info = DatasetInfo::new(uuid.to_string(), ty, FORMAT_DICTATION, name.to_string());
        write_info(&dir, &info).unwrap();
        dir
    }

    /// One row per family, the family it already belongs to left out, and every
    /// family that could not take another dataset of this type left out with it.
    #[test]
    fn candidates_offer_one_row_per_family_that_can_take_this_type() {
        let (settings, _dir) = temp_settings("candidates");
        put(&settings, DatasetType::Card, "alphas", "card_111111111111", "Alpha deck");
        put(&settings, DatasetType::Book, "alpha", "book_111111111111", "Alpha book");
        put(&settings, DatasetType::Book, "beta", "book_222222222222", "Beta book");
        put(&settings, DatasetType::Card, "gamma deck", "card_333333333333", "Gamma deck");
        put(&settings, DatasetType::Book, "gamma", "book_333333333333", "Gamma book");
        // A legacy id carries no key to share, so nothing can join onto it.
        put(
            &settings,
            DatasetType::Book,
            "legacy",
            "b76a0a93-20e0-4d1a-bd16-ab6f18197952",
            "Legacy book",
        );

        let got = family_candidates(&settings, "card_111111111111").unwrap();
        let names: Vec<&str> = got.iter().map(|c| c.name.as_str()).collect();
        // Its own family (`1111…`) is a no-op, and `3333…` already holds a card.
        assert_eq!(names, vec!["Beta book"], "only Beta can take a card");
        assert_eq!(got[0].family, "222222222222");
        assert_eq!(got[0].member_types, vec![DatasetType::Book.as_str().to_string()]);
    }

    /// The re-key is the descriptor alone: the folder keeps its name, the new id
    /// resolves, the old one stops, and the family then refuses a second dataset
    /// of the type that just joined — the same rule `mint_into` enforces from the
    /// creation side.
    #[test]
    fn joining_rekeys_the_descriptor_and_keeps_the_directory() {
        let (settings, _dir) = temp_settings("join");
        let legacy = "b76a0a93-20e0-4d1a-bd16-ab6f18197952";
        let dir = put(&settings, DatasetType::Card, "my deck", legacy, "My deck");
        put(&settings, DatasetType::Book, "the book", "book_abcd1234ef01", "The book");

        let joined = join_family(&settings, legacy, "book_abcd1234ef01").unwrap();
        assert_eq!(joined.uuid, "card_abcd1234ef01");
        assert_eq!(
            joined.dataset_type,
            DatasetType::Card,
            "the prefix follows the directory that holds it, not the target"
        );
        assert!(dir.exists(), "the folder is not renamed — only the id is an address");
        assert_eq!(read_info(&dir).unwrap().uuid, "card_abcd1234ef01", "written to disk");
        assert!(find_dataset_dir_typed(&settings, "card_abcd1234ef01").is_ok());
        assert!(find_dataset_dir_typed(&settings, legacy).is_err(), "the old id is gone");

        put(&settings, DatasetType::Card, "other deck", "f00d", "Other deck");
        let err = join_family(&settings, "f00d", "book_abcd1234ef01").unwrap_err();
        assert!(err.contains("one dataset per type per family"), "{err}");
        assert!(
            family_candidates(&settings, "f00d").unwrap().is_empty(),
            "a family that cannot take the dataset is not offered"
        );
    }
}
