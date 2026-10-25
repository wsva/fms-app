//! The app trash: the read-and-undo side of `datasets::media::move_to_trash`.
//!
//! Every delete that follows `docs/agent_friendly_design.md` §3 moves its
//! directory here instead of destroying it, and returns the path it landed at.
//! That is only a real undo while something can move it back out — `app_restore`
//! is that something, and `app_trash_list` is how an agent finds out what is in
//! there before offering to. Together they are what makes a permissive policy
//! defensible: an agent that deletes can be answered by an agent that restores.
//!
//! One flat directory, `<data_root>/trash/`, entries named
//! `<YYYYMMDD_HHMMSS>_<original name>` (plus `_N` on the second deletion of a
//! same-named folder inside one second — see `move_to_trash`). A `.partial`
//! suffix means a cross-volume copy that never finished publishing, so it is not
//! offered for restore. Nothing here deletes: emptying the trash stays a manual
//! act in the OS file manager, because that is the one step nothing can undo.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::settings::SettingsState;

/// `<data_root>/trash/` — the same directory `move_to_trash` writes to.
pub(crate) fn trash_dir() -> PathBuf {
    crate::app_paths::data_subdir("trash")
}

/// A trash entry is addressed by its bare name, never by a path: both tools take
/// one string straight from `app_trash_list` and must not be talked into
/// escaping the trash directory (`..`, a drive letter, a UNC share).
pub(crate) fn entry_component(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(
            "`name` is empty — pass the `name` field from app_trash_list.".to_string(),
        );
    }
    let looks_like_path = name.contains('/')
        || name.contains('\\')
        || name.contains(':')
        || name == "."
        || name == "..";
    if looks_like_path {
        return Err(format!(
            "`{}` is a path, not a trash entry name. Pass the bare `name` app_trash_list \
             returned (entries are named `<timestamp>_<original name>`); arbitrary paths are \
             not addressable here.",
            name
        ));
    }
    Ok(name)
}

/// Split `<YYYYMMDD_HHMMSS>_<original name>` into the deletion time
/// (`2026-10-25 10:15:00`) and the name the folder had before. Anything that
/// does not match that shape — a folder the user dragged in by hand — keeps its
/// name and reports no timestamp, so restoring it still works.
pub(crate) fn split_entry_name(name: &str) -> (Option<String>, String) {
    let b = name.as_bytes();
    let stamped = b.len() > 16
        && b[8] == b'_'
        && b[15] == b'_'
        && b[..8].iter().all(|c| c.is_ascii_digit())
        && b[9..15].iter().all(|c| c.is_ascii_digit());
    if !stamped {
        return (None, name.to_string());
    }
    let s = &name[..15];
    let stamp = format!(
        "{}-{}-{} {}:{}:{}",
        &s[0..4],
        &s[4..6],
        &s[6..8],
        &s[9..11],
        &s[11..13],
        &s[13..15]
    );
    (Some(stamp), name[16..].to_string())
}

/// Recursive byte total. Best-effort: a directory that cannot be read counts as
/// zero rather than failing the listing — the size is orientation, not the point.
fn dir_bytes(path: &Path) -> u64 {
    let Ok(read) = fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0;
    for entry in read.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        total += if meta.is_dir() {
            dir_bytes(&entry.path())
        } else {
            meta.len()
        };
    }
    total
}

/// One JSON object per trash entry, newest deletion first. A missing trash
/// directory is an empty list, not an error: most installs never delete
/// anything.
pub(crate) fn list() -> Result<Vec<Value>, String> {
    let dir = trash_dir();
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {}", dir.display(), e))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".partial") {
            continue;
        }
        let path = entry.path();
        let is_dir = path.is_dir();
        let bytes = if is_dir {
            dir_bytes(&path)
        } else {
            fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        };
        let (trashed_at, original_name) = split_entry_name(&name);
        // A dataset/book/read-aloud directory carries info.json, and that is what
        // makes the entry identifiable: the trash name alone says nothing about
        // which uuid the user lost.
        let dataset = crate::datasets::info::read_info_opt(&path).map(|info| {
            json!({
                "uuid": info.uuid,
                "type": info.dataset_type.as_str(),
                "name": info.name,
                "format": info.format,
                "language": info.language,
            })
        });
        out.push(json!({
            "name": name,
            "path": path.to_string_lossy(),
            "kind": if is_dir { "dir" } else { "file" },
            "trashed_at": trashed_at,
            "original_name": original_name,
            "bytes": bytes,
            "dataset": dataset,
        }));
    }
    // Newest first. An entry without a timestamp prefix sorts last, which is
    // where an unexplained hand-placed folder belongs.
    out.sort_by(|a, b| {
        let key = |v: &Value| v["trashed_at"].as_str().unwrap_or("").to_string();
        key(b).cmp(&key(a))
    });
    Ok(out)
}

/// Move one trash entry back out. `dest` is the absolute path the entry should
/// end up at (parent + folder name); without it the entry is restored into the
/// default root of the dataset type its `info.json` declares.
///
/// Refuses to write over anything: a half-merged dataset is worse than a failed
/// restore, and the caller can always pass `dest`.
pub(crate) fn restore(
    settings: &SettingsState,
    name: &str,
    dest: Option<&str>,
) -> Result<Value, String> {
    let name = entry_component(name)?;
    let source = trash_dir().join(name);
    if !source.exists() {
        return Err(format!(
            "no trash entry named '{}'. app_trash_list lists what is there — entries written by a \
             delete are named `<timestamp>_<original name>`, and a hand-placed folder keeps its own \
             name.",
            name
        ));
    }

    let (_, original_name) = split_entry_name(name);
    let info = crate::datasets::info::read_info_opt(&source);
    let dataset = info.as_ref().map(|info| {
        json!({
            "uuid": info.uuid,
            "type": info.dataset_type.as_str(),
            "name": info.name,
        })
    });

    let dest = dest.map(str::trim).filter(|d| !d.is_empty());
    let target = match dest {
        Some(dest) => {
            let path = PathBuf::from(dest);
            if !path.is_absolute() {
                return Err(format!(
                    "`dest` must be an absolute path; '{}' is relative. Pass the full path the \
                     entry should be restored to, ending in its folder name.",
                    dest
                ));
            }
            path
        }
        None => {
            let info = info.as_ref().ok_or_else(|| {
                format!(
                    "cannot tell where '{}' belongs: it has no readable info.json, and books, wiki \
                     folders and plain files land in the trash without one. Pass `dest` — the \
                     absolute path to restore it to.",
                    name
                )
            })?;
            // The canonical writable location for the type (`<datasets_dir>/<type>/`),
            // not `dataset_roots`, which would also offer a linked directory: a
            // restore should land where new datasets of that type are created.
            PathBuf::from(crate::datasets::roots::type_meta_dir(settings, info.dataset_type))
                .join(&original_name)
        }
    };

    let trash = trash_dir();
    let trash_canon = fs::canonicalize(&trash).unwrap_or_else(|_| trash.clone());
    let target_canon = fs::canonicalize(&target).unwrap_or_else(|_| target.clone());
    if target_canon.starts_with(&trash_canon) {
        return Err(format!(
            "`dest` resolves back into the trash ({}), which would restore nothing. Pass the \
             directory the entry should live in.",
            target.display()
        ));
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
    } else {
        return Err(format!(
            "`dest` {} has no parent directory to restore into.",
            target.display()
        ));
    }
    if target.exists() {
        return Err(format!(
            "{} already exists, so nothing was restored — writing over it would merge two \
             datasets. Move it aside, or pass `dest` naming a free path.",
            target.display()
        ));
    }

    match fs::rename(&source, &target) {
        Ok(()) => {}
        Err(_) => {
            // Cross-volume: the trash and a linked dataset root can sit on
            // different drives, which Windows refuses to rename across. Same
            // fallback `move_to_trash` takes in the other direction.
            let copied = if source.is_dir() {
                crate::datasets::media::copy_dir_recursive(&source, &target)
            } else {
                fs::copy(&source, &target).map(|_| ()).map_err(|e| e.to_string())
            };
            if let Err(e) = copied {
                if target.is_dir() {
                    let _ = fs::remove_dir_all(&target);
                } else {
                    let _ = fs::remove_file(&target);
                }
                return Err(format!(
                    "could not move {} back out of the trash: {}",
                    source.display(),
                    e
                ));
            }
            let removed = if source.is_dir() {
                fs::remove_dir_all(&source)
            } else {
                fs::remove_file(&source)
            };
            if let Err(e) = removed {
                // The restore itself worked; say what is left behind rather than
                // failing and making the caller retry into an existing path.
                log::warn!("[Trash] restored {} but could not clear the trash copy: {}", target.display(), e);
            }
        }
    }

    log::info!("[Trash] restored {} -> {}", source.display(), target.display());
    Ok(json!({
        "status": "ok",
        "restored_to": target.to_string_lossy(),
        "from": source.to_string_lossy(),
        "original_name": original_name,
        "dataset": dataset,
        "note": "The entry is back on disk with its uuid and contents intact; the dataset, book or \
                 read-aloud listings pick it up on their next read. The trash copy is gone.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_the_timestamp_prefix_off_a_trash_entry() {
        assert_eq!(
            split_entry_name("20261025_101500_german_basics"),
            (Some("2026-10-25 10:15:00".to_string()), "german_basics".to_string())
        );
        // The `_N` collision suffix stays part of the name: only the first 16
        // characters are the stamp, and guessing at the rest would rename data.
        assert_eq!(
            split_entry_name("20261025_101500_german_basics_1"),
            (Some("2026-10-25 10:15:00".to_string()), "german_basics_1".to_string())
        );
    }

    #[test]
    fn leaves_a_hand_placed_entry_alone() {
        assert_eq!(
            split_entry_name("my old notes"),
            (None, "my old notes".to_string())
        );
        assert_eq!(
            split_entry_name("20261025_10_notes"),
            (None, "20261025_10_notes".to_string())
        );
    }

    #[test]
    fn rejects_names_that_are_paths() {
        for bad in ["", "   ", "../secret", "a/b", "c:\\x", ".", "..", "C:evil"] {
            assert!(entry_component(bad).is_err(), "{:?} should be refused", bad);
        }
        assert_eq!(entry_component(" 20261025_101500_x ").unwrap(), "20261025_101500_x");
    }
}
