//! The align cache's portable half: what a recording's bytes are, and what was
//! heard in them.
//!
//! A cache file is named after its recording's **bytes** (`<sha256>.json`), not
//! after the file that happened to hold them. The bytes are what the model
//! listened to, and they are the only fact that survives a copy to another
//! machine, a rename, or a folder re-pick — so they are the identity the cache
//! is built on. `sources.json` keeps the per-device binding (folder, order,
//! path, size, mtime), and [`Manifest`] keeps the small hash → file table that
//! lets a cache arriving without one describe itself.
//!
//! Nothing here decides anything. [`scan`] answers "do I already have these
//! bytes?", [`Manifest`] answers "what did I hear in them?", and [`promote`] is
//! the one write: the rename that turns a cache written before content hashes
//! existed into one that can travel.

use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::index::{align_dir, read_json, write_json, SourceEntry, SourceTranscript};

/// The portable hash → file table. Reserved name in `align/`, like
/// `sources.json` and `proposals.json`.
pub(super) const MANIFEST_FILE: &str = "manifest.json";

/// Bytes read per step while hashing. A two-hour chapter costs no more memory
/// than this, and a sequential 64 KiB read is what a filesystem likes anyway.
const CHUNK: usize = 64 * 1024;

// ============================================================
// Hashing
// ============================================================

/// Lowercase hex of a digest — the shape both the file-name hash and the
/// content hash are spelled in, so there is one convention to read.
pub(super) fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect()
}

/// sha256 of a file's bytes.
///
/// This is the expensive half of registration and the reason a folder is
/// hashed only when it is picked: one sequential read per file, memoized across
/// picks by `sources.json` (see `register`). A new machine pays it once; every
/// later pick of the same folder pays it only for what changed.
pub(super) fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path)
        .map_err(|e| format!("Cannot open {} to hash it: {}", path.display(), e))?;
    let mut hasher = Sha256::new();
    let mut chunk = vec![0u8; CHUNK];
    loop {
        let read = file
            .read(&mut chunk)
            .map_err(|e| format!("Cannot read {} to hash it: {}", path.display(), e))?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
    }
    Ok(hex_digest(hasher.finalize()))
}

// ============================================================
// Manifest
// ============================================================

/// One cached recording, as the manifest remembers it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct ManifestEntry {
    pub(super) content_hash: String,
    pub(super) file_name: String,
    #[serde(default)]
    pub(super) size: u64,
    #[serde(default)]
    pub(super) duration: f64,
    #[serde(default)]
    pub(super) words: usize,
}

/// The cache's own table of contents, keyed by content hash.
///
/// Keyed by bytes, so it stays true across a rename and merges from another
/// machine's copy without a conflict: two rows can only disagree about a file
/// name, and the name is the binding's business, not the cache's.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Manifest {
    /// Written as `2`; a missing or older value still parses, because the
    /// entries are the only thing read.
    #[serde(default = "manifest_version")]
    pub(super) version: u32,
    #[serde(default)]
    pub(super) dataset_uuid: String,
    #[serde(default)]
    pub(super) updated_at: String,
    #[serde(default)]
    pub(super) entries: Vec<ManifestEntry>,
}

fn manifest_version() -> u32 {
    2
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            version: manifest_version(),
            dataset_uuid: String::new(),
            updated_at: String::new(),
            entries: Vec::new(),
        }
    }
}

impl Manifest {
    pub(super) fn entry(&self, content_hash: &str) -> Option<&ManifestEntry> {
        self.entries.iter().find(|entry| entry.content_hash == content_hash)
    }

    /// Insert or replace by content hash — in memory only. The caller saves
    /// once, so a folder of thirty files is not thirty writes.
    pub(super) fn upsert(&mut self, entry: ManifestEntry) {
        match self
            .entries
            .iter_mut()
            .find(|existing| existing.content_hash == entry.content_hash)
        {
            Some(existing) => *existing = entry,
            None => self.entries.push(entry),
        }
    }
}

pub(super) fn manifest_path(dir: &Path) -> PathBuf {
    align_dir(dir).join(MANIFEST_FILE)
}

pub(super) fn load_manifest(dir: &Path) -> Manifest {
    read_json(&manifest_path(dir)).unwrap_or_default()
}

pub(super) fn save_manifest(dir: &Path, manifest: &Manifest) -> Result<(), String> {
    write_json(&manifest_path(dir), manifest)
}

// ============================================================
// What is already cached
// ============================================================

/// The cache files already in `align/`, by their stem.
///
/// One set rather than two: a content hash and a file-name hash are both 64 hex
/// characters, so a name cannot say which question is being asked. The caller
/// knows — `has(content_hash)` is "these bytes are cached", `has(uuid)` is "a
/// cache from before content hashes is here".
#[derive(Default)]
pub(super) struct CacheScan {
    names: HashSet<String>,
}

impl CacheScan {
    pub(super) fn has(&self, stem: &str) -> bool {
        !stem.is_empty() && self.names.contains(stem)
    }

    /// Cache files no registered recording claims: a recording left behind in
    /// the old folder, a chapter that was deleted, or the bytes of a file that
    /// was re-ripped. Reported, never deleted — keeping them is what makes
    /// switching folders back free.
    pub(super) fn orphaned(&self, keep: &HashSet<String>) -> usize {
        self.names
            .iter()
            .filter(|name| !keep.contains(name.as_str()))
            .count()
    }
}

pub(super) fn scan(dir: &Path) -> CacheScan {
    let mut scan = CacheScan::default();
    let Ok(entries) = fs::read_dir(align_dir(dir)) else {
        return scan;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".json") else {
            continue;
        };
        if matches!(stem, "sources" | "proposals" | "manifest") {
            continue;
        }
        scan.names.insert(stem.to_string());
    }
    scan
}

/// Rename a cache written under the file-name hash to its content hash.
///
/// The payload is untouched: the words and peaks describe the same bytes either
/// way, so a legacy cache is adopted with a rename, not a rewrite. A target
/// that already exists wins — the content-addressed file is the one the rest of
/// the page looks for, and the second copy describes the same audio.
pub(super) fn promote(dir: &Path, uuid: &str, content_hash: &str) -> Result<(), String> {
    if uuid.is_empty() || content_hash.is_empty() || uuid == content_hash {
        return Ok(());
    }
    let from = align_dir(dir).join(format!("{}.json", uuid));
    let to = align_dir(dir).join(format!("{}.json", content_hash));
    if !from.is_file() {
        return Ok(());
    }
    if to.is_file() {
        let _ = fs::remove_file(&from);
        return Ok(());
    }
    fs::rename(&from, &to)
        .map_err(|e| format!("Cannot rename {} to {}: {}", from.display(), to.display(), e))
}

// ============================================================
// What a cached recording amounts to
// ============================================================

/// The seconds and the word count of a cached recording.
///
/// Asked in order of cost: the manifest, then the previous binding, then the
/// cache file itself. Only a cache that arrived with neither (an `align/`
/// copied in on its own) pays for the last read, and it pays once — the answer
/// goes into the manifest and the binding it is written to.
pub(super) fn facts(
    dir: &Path,
    content_hash: &str,
    uuid: &str,
    previous: Option<&SourceEntry>,
    manifest: &Manifest,
) -> Option<(f64, usize)> {
    if let Some(entry) = manifest.entry(content_hash) {
        if entry.duration > 0.0 && entry.words > 0 {
            return Some((entry.duration, entry.words));
        }
    }
    if let Some(previous) = previous {
        if let (Some(duration), Some(words)) = (previous.duration, previous.words) {
            return Some((duration, words));
        }
    }
    for path in [
        align_dir(dir).join(format!("{}.json", content_hash)),
        align_dir(dir).join(format!("{}.json", uuid)),
    ] {
        if let Some(transcript) = read_json::<SourceTranscript>(&path) {
            if transcript.duration > 0.0 && !transcript.words.is_empty() {
                return Some((transcript.duration, transcript.words.len()));
            }
        }
    }
    None
}
