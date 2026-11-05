//! The app's built-in content: a dataset shipped inside the binary instead of
//! imported by the user. Currently one member — the user manual.
//!
//! Three properties define it, and each is a deliberate departure from an
//! ordinary dataset:
//!
//! * **Baked in, not bundled.** `build.rs` packs `docs/manual` into
//!   `$OUT_DIR/manual.tar.gz` and this module embeds those bytes. So the manual
//!   a user reads is the manual of the version they run — an update replaces it
//!   by construction — with no `bundle.resources` entry and no runtime
//!   `resource_dir` resolution (which in this project points at `target/debug`
//!   under `tauri dev`, behaving differently in every build mode).
//! * **Invisible to discovery.** It lives under app data, not under any
//!   `dataset_roots` entry, so `scan_datasets`, `find_dataset_dir_typed`, the
//!   hub's REST catalog and the sync journal can never name it. That is the
//!   whole reason the shipped bytes are not a synced dataset: a phone on v0.1
//!   must not be handed v0.2 prose by its hub, and a file-level
//!   last-write-wins river would happily do it. Reads reach it through
//!   [`crate::datasets::wiki`]'s own resolver, which consults this directory by
//!   id (see [`crate::datasets::wiki::find_dataset_dir`]).
//! * **Its id is not a constant.** Release CI rewrites `info.json`'s `uuid`
//!   (see `docs` of `pack_user_manual` in `build.rs` and the "Stamp the
//!   built-in manual id" step in `.github/workflows/build.yml`), so the shipped
//!   id differs from the repo's. Everything here therefore *reads* the id back
//!   from the data — never compares against a literal.
//!
//! Seeding is not a wiki write: it copies files with plain `fs` calls, so no
//! `wiki_file_save` is ever journaled and the change log stays unaware of app
//! content.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;

use flate2::read::GzDecoder;

use crate::app_paths;
use crate::datasets::wiki::WikiDatasetSummary;
use crate::datasets::{read_info_opt, DatasetInfo, INFO_FILE};

/// The manual as packed at build time: one gzip stream of `info.json` + `*.md`,
/// with dataset-relative entry names.
const MANUAL_ARCHIVE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/manual.tar.gz"));

/// The one refusal every mutation of the built-in manual answers with, kept here
/// so the wording is shared by the command road and the MCP road. Actionable by
/// design: it names the road the user actually wants rather than ending at "no".
pub(crate) const READ_ONLY: &str = "The built-in user manual is read-only: it ships with the app \
and the next update replaces it. To keep your own notes, create a wiki dataset \
(`wiki_dataset_create`) and write them there.";

/// The reserved parent of every built-in dataset: `<data_root>/builtin`.
///
/// Named for what it is rather than reusing the per-workspace download root: this
/// one is shared by every workspace on the machine, which is exactly why the
/// multi-workspace question never arises — there is one copy to seed and replace.
pub(crate) fn builtin_root() -> std::path::PathBuf {
    app_paths::data_root().join("builtin")
}

/// The built-in user manual's dataset directory.
pub(crate) fn manual_dir() -> std::path::PathBuf {
    builtin_root().join("manual")
}

/// The descriptor baked into this binary, parsed once.
fn embedded_info() -> Option<&'static DatasetInfo> {
    static CACHE: OnceLock<Option<DatasetInfo>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let mut archive = tar::Archive::new(GzDecoder::new(MANUAL_ARCHIVE));
            for entry in archive.entries().ok()? {
                let mut entry = entry.ok()?;
                let path = entry.path().ok()?;
                if path.file_name().and_then(|n| n.to_str()) != Some(INFO_FILE) {
                    continue;
                }
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf).ok()?;
                return serde_json::from_slice(&buf).ok();
            }
            None
        })
        .as_ref()
}

/// The manual's id as it exists on this machine, or as this binary means it to
/// be when nothing is seeded yet. The only source of the id at runtime.
pub(crate) fn manual_uuid() -> Option<String> {
    read_info_opt(&manual_dir())
        .map(|i| i.uuid)
        .or_else(|| embedded_info().map(|i| i.uuid.clone()))
}

/// True when `uuid` addresses the built-in manual — either the copy on disk or
/// the copy this build would seed, so a mutation is refused before the first
/// launch has seeded anything too.
pub(crate) fn is_builtin(uuid: &str) -> bool {
    if embedded_info().map(|i| i.uuid.as_str()) == Some(uuid) {
        return true;
    }
    let seeded = read_info_opt(&manual_dir());
    seeded.as_ref().map(|i| i.uuid.as_str()) == Some(uuid)
}

/// Ensure the on-disk manual matches the one embedded in this binary.
///
/// The stamp is `info.json`'s `updated_at`, which the pack step guarantees is
/// present: equal stamps, an equal id and a non-empty directory mean a no-op
/// return, so the steady-state cost of a launch is one small file read — no
/// hashing, no walk. Anything else (first run, a new app version, a directory
/// the user edited or half-deleted by hand) replaces the whole tree.
///
/// The id is compared *too*, not just the stamp: CI stamps only the `uuid` of
/// the same manual content, so a dev-built copy (repo uuid) upgrading to a CI
/// package (built-in id) would otherwise look current and keep publishing the
/// dev id forever. There is no trash and no journal: this is app content, and
/// trashing it on every upgrade would ship the manual to the user's disk once
/// per version.
pub(crate) fn seed_manual() -> Result<(), String> {
    let dir = manual_dir();
    let want = embedded_info().ok_or_else(|| {
        "the embedded manual archive carries no readable info.json — rebuild to repack"
            .to_string()
    })?;

    let seeded = read_info_opt(&dir);
    let current = seeded
        .as_ref()
        .map(|i| i.uuid == want.uuid && i.updated_at == want.updated_at && has_markdown(&dir))
        .unwrap_or(false);
    if current {
        return Ok(());
    }

    if dir.exists() {
        fs::remove_dir_all(&dir)
            .map_err(|e| format!("could not replace the built-in manual at {}: {e}", dir.display()))?;
    }
    fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    // A partial unpack leaves no `info.json`, so the next launch takes this
    // branch again and repairs it — no separate rollback needed.
    tar::Archive::new(GzDecoder::new(MANUAL_ARCHIVE))
        .unpack(&dir)
        .map_err(|e| format!("could not unpack the built-in manual into {}: {e}", dir.display()))?;

    log::info!(
        "[Builtin] seeded user manual '{}' ({}) at {}",
        want.name,
        want.updated_at,
        dir.display()
    );
    Ok(())
}

/// The manual as the Wiki page lists it: an ordinary summary, marked read-only.
/// `None` when the seed never landed, so the page simply omits it rather than
/// showing an empty dataset.
pub(crate) fn manual_summary() -> Option<WikiDatasetSummary> {
    let dir = manual_dir();
    let info = read_info_opt(&dir)?;
    Some(WikiDatasetSummary {
        uuid: info.uuid,
        name: info.name,
        description: info.description,
        updated: info.updated_at,
        file_count: crate::datasets::wiki::count_markdown(&dir),
        path: dir.to_string_lossy().into_owned(),
        location: builtin_root().to_string_lossy().into_owned(),
        is_read_only: true,
    })
}

/// The dataset directory behind a built-in id, if `uuid` names one. Called by
/// the wiki resolver, which is what lets every existing read command and MCP
/// tool serve the manual without a second implementation.
pub(crate) fn dir_for(uuid: &str) -> Option<std::path::PathBuf> {
    if !is_builtin(uuid) {
        return None;
    }
    let dir = manual_dir();
    dir.exists().then_some(dir)
}

/// The manual's file tree, dataset-relative and markdown-only: the table of
/// contents the `fms://docs/user-manual` resource serves, derived at read time
/// from the seeded directory so it can never disagree with what the page shows.
pub(crate) fn toc() -> Vec<String> {
    let dir = manual_dir();
    if !dir.exists() {
        return Vec::new();
    }
    let mut files = Vec::new();
    collect_markdown(&dir, &dir, &mut files);
    files.sort();
    files
}

fn collect_markdown(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_markdown(root, &path, out);
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.to_ascii_lowercase().ends_with(".md"))
            .unwrap_or(false)
        {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

fn has_markdown(dir: &Path) -> bool {
    let mut files = Vec::new();
    collect_markdown(dir, dir, &mut files);
    !files.is_empty()
}

/// The manual's `{uuid, name, updated_at}` for the agent manual's
/// `capabilities.builtin_manual`. This is the one place an agent learns the id it
/// has to pass to the wiki read tools — the dataset is invisible to every
/// listing, so if this line is absent the manual is undiscoverable, not merely
/// unlisted. Derived from the seeded descriptor (falling back to the embedded
/// one before the first seed), never a hand-maintained constant.
pub(crate) fn manual_capability() -> Option<serde_json::Value> {
    let (uuid, name, updated_at) = match read_info_opt(&manual_dir()) {
        Some(info) => (info.uuid, info.name, info.updated_at),
        None => {
            let info = embedded_info()?;
            (info.uuid.clone(), info.name.clone(), info.updated_at.clone())
        }
    };
    Some(serde_json::json!({ "uuid": uuid, "name": name, "updated_at": updated_at }))
}

/// Report the built-in manual for the Wiki page — the one surface that shows it.
///
/// Reads then go through the ordinary `wiki_dataset_list_dir` /
/// `wiki_dataset_read_file` / `wiki_dataset_search` commands with this uuid;
/// there is no built-in read road, because there is no need for one.
#[tauri::command]
pub async fn builtin_manual_get() -> Option<WikiDatasetSummary> {
    manual_summary()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// Every seeding test writes the one real `<data_root>/fms-app/builtin/manual`
    /// directory, so they must not overlap — a parallel pair would have one
    /// deleting the markdown the other is asserting on. Held as a `Static` so
    /// poisoning from an earlier failure does not cascade.
    static SEED_LOCK: std::sync::OnceLock<Mutex<()>> = OnceLock::new();

    fn lock_seed() -> MutexGuard<'static, ()> {
        SEED_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// The archive `build.rs` packed for this test build is embedded in the
    /// binary, so these tests exercise the real payload rather than a fixture.
    #[test]
    fn embedded_archive_carries_a_stamped_descriptor() {
        let info = embedded_info().expect("the embedded manual must carry a readable info.json");
        assert!(!info.uuid.is_empty(), "manual uuid must be set");
        assert!(!info.updated_at.is_empty(), "manual must carry the stamp seed_manual compares");
        assert!(
            !info.name.is_empty(),
            "manual must be named for the Wiki page to list it"
        );
    }

    #[test]
    fn is_builtin_recognises_the_embedded_id() {
        let uuid = embedded_info().map(|i| i.uuid.clone()).unwrap_or_default();
        assert!(is_builtin(&uuid));
        assert!(!is_builtin("some-other-dataset"));
        assert!(!is_builtin(""));
    }

    #[test]
    fn seeding_is_idempotent_while_the_stamp_matches() {
        let _guard = lock_seed();
        let dir = manual_dir();
        seed_manual().expect("seeding the built-in manual must succeed");
        let info = read_info_opt(&dir).expect("the seeded dir must carry info.json");
        assert_eq!(
            info.updated_at,
            embedded_info().unwrap().updated_at,
            "the on-disk stamp must equal the embedded one"
        );

        // A sentinel outside the packed set proves the second call took the
        // no-op branch instead of replacing the directory.
        let sentinel = dir.join("seed-sentinel.txt");
        fs::write(&sentinel, b"x").expect("write sentinel next to the manual");
        seed_manual().expect("second seed must succeed");
        assert!(
            sentinel.exists(),
            "an up-to-date manual must not be re-extracted"
        );
        let _ = fs::remove_file(&sentinel);
    }

    #[test]
    fn an_id_change_under_the_same_stamp_is_replaced() {
        let _guard = lock_seed();
        let dir = manual_dir();
        seed_manual().expect("seed first");
        // The dev-install-upgrading-to-a-CI-package case: same content stamp,
        // different id (CI stamps only the uuid). The stamp alone would call
        // this current and keep publishing the stale id.
        let stale = dir.join(INFO_FILE);
        let mut info: DatasetInfo = serde_json::from_slice(
            &fs::read(&stale).expect("seeded info.json"),
        )
        .expect("parse seeded info.json");
        info.uuid = "wiki_000000000000".to_string();
        fs::write(&stale, serde_json::to_vec_pretty(&info).unwrap())
            .expect("write the stale id");
        seed_manual().expect("re-seed must take over the id change");
        assert_eq!(
            read_info_opt(&dir).expect("info.json after re-seed").uuid,
            embedded_info().unwrap().uuid,
            "a manual whose id differs from the embedded one must be replaced despite a matching stamp"
        );
    }

    #[test]
    fn a_broken_manual_is_repaired_not_skipped() {
        let _guard = lock_seed();
        let dir = manual_dir();
        seed_manual().expect("seed first");
        // The failure mode the design must survive: content deleted by hand
        // (or by an agent with filesystem access) while the descriptor still
        // claims to be current.
        for rel in toc() {
            let _ = fs::remove_file(dir.join(&rel));
        }
        seed_manual().expect("re-seed must repair");
        assert!(
            has_markdown(&dir),
            "a manual with no markdown must be re-extracted despite a matching stamp"
        );
    }
}
