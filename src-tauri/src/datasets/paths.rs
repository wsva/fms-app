//! Resolving a dataset uuid to a directory: base (read-only download root) vs
//! writable overlay, plus the fork that grows an overlay out of a base.

use std::fs;
use std::path::{Path, PathBuf};

use crate::settings::SettingsState;

use super::{
    INFO_FILE, LAYER_ROLE_OVERLAY, LayerBlock, now_stamp, read_info, read_info_opt, write_info,
};

use super::{
    DatasetType, dataset_roots, datasets_dir, is_media_file, is_under_download_root,
    sanitize_dir_name,
};

/// Read-resolution: the effective directory a reader opens for `uuid`.
///
/// Scans every type's roots (including the read-only download root) via
/// [`find_dataset_dir_typed`] and returns the first match — a writable local
/// dataset before a downloaded base of the same uuid (root order, §4.4).
///
/// With the fork model an edited base resolves to its overlay — a self-contained
/// writable dataset in the `<type>/` area — so reads stay single-directory with
/// no `merged/` cache (dataset-layers.md §5 alternative).
pub(crate) fn find_dataset_dir_read(settings: &SettingsState, uuid: &str) -> Result<PathBuf, String> {
    find_dataset_dir_typed(settings, uuid).map(|(path, _ty)| path)
}

/// Write-resolution: where a mutation to `uuid` lands.
///
/// A dataset under the app-maintained download root is a read-only base and is
/// never a write target (dataset-layers.md §6): the first write transparently
/// **forks** it into a writable overlay in the `<type>/` area and returns that.
/// A flat dataset, or an overlay that already exists, is returned as-is, so the
/// fork happens once and is idempotent (the overlay then resolves first).
pub(crate) fn find_dataset_dir_write(settings: &SettingsState, uuid: &str) -> Result<PathBuf, String> {
    let (path, ty) = find_dataset_dir_typed(settings, uuid)?;
    if is_under_download_root(settings, &path) {
        return fork_base_to_overlay(settings, &path, ty);
    }
    Ok(path)
}

/// Fork a read-only base under the download root into a self-contained writable
/// overlay (dataset-layers.md §6).
///
/// The overlay is an ordinary dataset directory in the writable `<type>/` area:
/// the SQLite DB is **copied** (never hardlinked — row writes would otherwise
/// reach through the link and corrupt the base), immutable media are
/// **hardlinked** (falling back to a copy across volumes or on filesystems
/// without hardlinks), and small mutable files (`.vtt`, waveform `.json`,
/// `book.txt`, transcripts) are **copied** so an in-place edit can never reach
/// the base inode. `info.json` is rewritten with `layer.role = "overlay"` so
/// reads union and a later base re-pull can detect staleness (§4.3).
///
/// Exempt from `assert_uuid_free`: like [`ensure_favorites_dataset`], it
/// legitimately writes a second directory carrying the base's uuid (§4.4).
pub(crate) fn fork_base_to_overlay(
    settings: &SettingsState,
    base_dir: &Path,
    dataset_type: DatasetType,
) -> Result<PathBuf, String> {
    let info = read_info(base_dir)?;

    // Target: the first writable root for this type (never the download root).
    let target_root = dataset_roots(settings, dataset_type)
        .into_iter()
        .find(|r| !is_under_download_root(settings, r))
        .unwrap_or_else(|| datasets_dir(settings).join(dataset_type.as_str()));
    fs::create_dir_all(&target_root).map_err(|e| format!("Failed to create datasets dir: {}", e))?;

    // Collision-avoided dir name from the display name (same pattern as
    // `ensure_favorites_dataset`); the overlay keeps the base's uuid inside.
    let dir_name = sanitize_dir_name(&info.name).unwrap_or_else(|| info.uuid.clone());
    let mut dst = target_root.join(&dir_name);
    let mut n = 2;
    while dst.exists() {
        dst = target_root.join(format!("{}-{}", dir_name, n));
        n += 1;
    }
    fs::create_dir_all(&dst).map_err(|e| e.to_string())?;

    // Walk the base tree, forking each file by kind (info.json written below).
    fork_tree(base_dir, &dst)?;

    let mut overlay_info = info.clone();
    overlay_info.layer = Some(LayerBlock {
        role: LAYER_ROLE_OVERLAY.to_string(),
        base_uuid: Some(info.uuid.clone()),
        base_version: Some(info.updated_at.clone()),
    });
    overlay_info.updated_at = now_stamp();
    write_info(&dst, &overlay_info)?;

    log::info!(
        "[Layer] Forked read-only base '{}' into writable overlay at '{}'",
        info.uuid,
        dst.display()
    );
    Ok(dst)
}

/// Recursively fork `src` into `dst`: media hardlinked (copy fallback), the DB
/// and every other file copied, `info.json` and any `overlay/`/`merged/` skipped
/// (the caller writes `info.json` fresh; a pristine base has no overlay dirs).
pub(crate) fn fork_tree(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let src_path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if src_path.is_dir() {
            if name == "overlay" || name == "merged" {
                continue;
            }
            fork_tree(&src_path, &dst.join(&*file_name))?;
            continue;
        }
        if name == INFO_FILE {
            continue; // written fresh by the caller
        }
        let dst_path = dst.join(&file_name);
        // Media (large, immutable) → hardlink, falling back to copy. Everything
        // else (DB + sidecars, .vtt, waveform .json, book.txt, transcripts) →
        // copy, so an in-place edit in the overlay can never reach the base.
        if is_media_file(&name) && fs::hard_link(&src_path, &dst_path).is_ok() {
            continue;
        }
        fs::copy(&src_path, &dst_path)
            .map_err(|e| format!("Failed to fork {}: {}", src_path.display(), e))?;
    }
    Ok(())
}

/// Find the dataset directory by UUID across all configured locations.
///
/// Thin alias for [`find_dataset_dir_read`], kept so existing call sites compile
/// while they are classified read vs write (dataset-layers.md §12.3).
pub(crate) fn find_dataset_dir(settings: &SettingsState, uuid: &str) -> Result<PathBuf, String> {
    find_dataset_dir_read(settings, uuid)
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
