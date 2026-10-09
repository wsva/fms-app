//! The unified dataset descriptor — the `info.json` every dataset type shares.
//!
//! One [`DatasetInfo`] shape describes dictation, card, book, read-aloud and wiki
//! datasets alike; the only per-type difference is the `type`/`format` pair. The
//! five per-type structs this replaces (`CardDatasetInfo`, `BookInfoFile`,
//! `ReadAloudInfoFile`, `WikiDatasetInfo` and the old dictation `DatasetInfo`)
//! disagreed on field *names* for the same concept (`updated` vs `updated_at`,
//! `name` vs `title`), which is what broke the sync manifest's timestamp and
//! forced every reader into a type-specific parse.
//!
//! Design rules, each with a reason:
//!
//! * **A field earns its place by having a reader.** Nothing here is speculative
//!   provenance or reserved extensibility.
//! * **`uuid` is an opaque string, never format-validated.** It is unique per
//!   dataset and nothing more — the reserved Favorites id (`dictation-favorites`)
//!   and the website-era ids (`de_a1_by_system`) are slugs, not RFC 4122 UUIDs.
//! * **No derived or volatile data.** Counts, scores and progress live in
//!   `data.sqlite3`: `info.json` is part of the sync manifest's hashed file set
//!   (see `sync::rest::is_db_file`), so a field that changes on every user action
//!   would re-hash the dataset and look like content churn to every follower.
//! * **Unknown keys round-trip.** They land in [`DatasetInfo::extra`] and are
//!   written back verbatim, so a future field is a pure addition and a rewrite
//!   (`touch_info`) can never silently drop data another version understands.
//! * **`type` mirrors the directory slug** (`datasets/<type>/…`). The directory
//!   stays the routing authority; `type` makes the file self-describing for a
//!   reader that only has the file — and discovery warns on mismatch.

use std::fs;
use std::path::Path;
// Only the desktop cross-type scan produces owned paths.
#[cfg(feature = "desktop")]
use std::path::PathBuf;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::settings::SettingsState;

use super::{find_dataset_dir_typed, DatasetType};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The descriptor file inside every dataset directory. All path construction
/// goes through this so a future rename is a one-line change.
pub(crate) const INFO_FILE: &str = "info.json";

/// Current `info.json` schema version. Bump only on a removal or a change of
/// meaning — adding a field is invisible to old readers thanks to `extra`.
pub(crate) const INFO_SPEC: u32 = 2;

/// Per-type internal layout tags. The rule is `<type-slug>-v<N>` with the
/// directory slug verbatim (`card`, not `cards`; `read_aloud`, not `read-aloud`),
/// so the pair `type` + `format` never contradicts the tree on disk.
pub(crate) const FORMAT_DICTATION: &str = "dictation-v2";
pub(crate) const FORMAT_CARD: &str = "card-v1";
pub(crate) const FORMAT_BOOK: &str = "book-v1";
pub(crate) const FORMAT_READ_ALOUD: &str = "read_aloud-v1";
pub(crate) const FORMAT_WIKI: &str = "wiki-v1";

/// The app-owned "Favorites" dataset. Identified by this reserved id instead of a
/// boolean flag, so finding it is an ordinary uuid lookup rather than a scan, and
/// every device refers to the same logical dataset — favorited clips therefore
/// merge under sync instead of forking per device.
pub(crate) const FAVORITES_DATASET_UUID: &str = "dictation-favorites";

/// Canonical timestamp for `created_at` / `updated_at`: UTC, second precision,
/// `Z` suffix. Previously three shapes coexisted in real data (`…35Z` from
/// Python, `…519+00:00` from Rust with microseconds, `…524600+00:00` with
/// nanoseconds), which made any string comparison of stamps unreliable.
pub(crate) fn now_stamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Publication state, shared by every dataset type.
///
/// Authoritative on the website: `subscribers` is a mirror that local tooling
/// writes only when exporting or pulling, and that app code never appends to —
/// a local append would dirty the dataset for all of its followers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sharing {
    /// `"private"` | `"shared"` | `"public"`.
    #[serde(default = "default_visibility")]
    pub visibility: String,
    /// Owner's user id (email). Gates pushes locally — see `cards::sync`.
    #[serde(default)]
    pub owner_id: String,
    #[serde(default)]
    pub subscribers: Vec<String>,
}

fn default_visibility() -> String {
    "private".to_string()
}

impl Default for Sharing {
    /// Kept in step with the per-field serde defaults so `Sharing::default()` and
    /// a `{}` object parsed from JSON mean the same thing.
    fn default() -> Self {
        Self {
            visibility: default_visibility(),
            owner_id: String::new(),
            subscribers: Vec::new(),
        }
    }
}

/// Contents of a dataset's `info.json`, for every dataset type.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatasetInfo {
    /// Schema version of this file. Defaults so a hand-written file without it
    /// still parses.
    #[serde(default = "default_spec")]
    pub spec: u32,
    /// Globally unique dataset identifier. Opaque string — see the module docs.
    pub uuid: String,
    /// Dataset type; must equal the directory slug it lives under. `type` is a
    /// Rust keyword, hence the rename.
    #[serde(rename = "type")]
    pub dataset_type: DatasetType,
    /// Per-type internal layout tag, e.g. `card-v1`.
    pub format: String,
    /// Display name. Books used to call this `title`.
    pub name: String,
    /// Human-facing blurb only — provenance belongs in the dataset's `README.md`.
    #[serde(default)]
    pub description: String,
    /// BCP 47 primary subtag (`"de"`, `"en"`, `"vi"`), empty when unknown. Typed
    /// rather than a label because code branches on it (`simple_words` maps a
    /// language to the card datasets that count for it).
    #[serde(default)]
    pub language: String,
    /// Set once at creation and never rewritten — a rebuild must not pretend a
    /// dataset is younger than it is.
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub sharing: Sharing,
    /// Anything this build does not model, preserved verbatim on write.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn default_spec() -> u32 {
    INFO_SPEC
}

impl DatasetInfo {
    /// A freshly created dataset: no history, not published, no extra fields.
    pub(crate) fn new(
        uuid: String,
        dataset_type: DatasetType,
        format: &str,
        name: String,
    ) -> Self {
        let now = now_stamp();
        Self {
            spec: INFO_SPEC,
            uuid,
            dataset_type,
            format: format.to_string(),
            name,
            description: String::new(),
            language: String::new(),
            created_at: now.clone(),
            updated_at: now,
            sharing: Sharing::default(),
            extra: Map::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Read / write helpers
// ---------------------------------------------------------------------------

/// Read and validate a directory's `info.json`. Strips a leading BOM, which
/// Windows-side tooling (and PowerShell `Set-Content`) likes to insert.
pub(crate) fn read_info(dir: &Path) -> Result<DatasetInfo, String> {
    let data = fs::read_to_string(dir.join(INFO_FILE)).map_err(|e| e.to_string())?;
    serde_json::from_str(data.trim_start_matches('\u{FEFF}')).map_err(|e| e.to_string())
}

/// Best-effort variant for discovery scans, where one unparseable folder must not
/// abort the walk over a whole root.
pub(crate) fn read_info_opt(dir: &Path) -> Option<DatasetInfo> {
    read_info(dir).ok()
}

pub(crate) fn write_info(dir: &Path, info: &DatasetInfo) -> Result<(), String> {
    let data = serde_json::to_string_pretty(info).map_err(|e| e.to_string())?;
    fs::write(dir.join(INFO_FILE), data).map_err(|e| e.to_string())
}

/// Bump `updated_at` and nothing else, preserving `extra` and `created_at`.
///
/// Writes the *reparsed* file rather than the caller's in-memory copy, so a
/// concurrent change to another field is not clobbered, and unknown keys survive.
pub(crate) fn touch_info(dir: &Path) -> Result<(), String> {
    let mut info = read_info(dir)?;
    info.updated_at = now_stamp();
    write_info(dir, &info)
}

/// Refuse a uuid that another dataset — of any type — already claims.
///
/// uuid is the key of the whole sync protocol (`/api/v1/datasets/{uuid}/…`), so a
/// cross-type collision is not cosmetic: resolution picks whichever type the scan
/// reaches first. Creation and import are the only moments a uuid is minted, so
/// that is where the check belongs. The reserved Favorites id is exempt — it is
/// minted by `ensure_favorites_dataset` precisely so it exists.
pub(crate) fn assert_uuid_free(settings: &SettingsState, uuid: &str) -> Result<(), String> {
    if uuid == FAVORITES_DATASET_UUID {
        return Err(format!(
            "Dataset id '{}' is reserved for the built-in Favorites dataset; pick another name",
            uuid
        ));
    }
    if let Ok((path, ty)) = find_dataset_dir_typed(settings, uuid) {
        return Err(format!(
            "Dataset id {} is already used by the '{}' dataset at '{}'; choose a different id",
            uuid,
            ty.as_str(),
            path.display()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Cross-type discovery
// ---------------------------------------------------------------------------

/// A dataset of any type found on this machine: the shared descriptor plus the
/// two pieces of information `info.json` cannot carry — the directory it lives in
/// and the configured root it was discovered under.
///
/// Only the desktop MCP listings enumerate across types; the mobile build has no
/// caller to enumerate for.
#[cfg(feature = "desktop")]
#[derive(Clone, Debug)]
pub(crate) struct DiscoveredDataset {
    pub info: DatasetInfo,
    pub path: PathBuf,
    pub location: String,
}

/// Enumerate every dataset of one type across its configured roots.
///
/// A folder without a parseable `info.json` is not a dataset: nothing can address
/// it by uuid, so the sync catalog skips it too. The dictation listing
/// (`list_datasets`) additionally surfaces raw-import media folders for the UI and
/// computes a `status` from its artifacts — that stays its own concern.
#[cfg(feature = "desktop")]
pub(crate) fn scan_datasets(
    settings: &SettingsState,
    dataset_type: DatasetType,
) -> Vec<DiscoveredDataset> {
    let mut out = Vec::new();
    for root in super::dataset_roots(settings, dataset_type) {
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
            let Some(info) = read_info_opt(&path) else {
                continue;
            };
            if info.uuid.is_empty() {
                continue;
            }
            out.push(DiscoveredDataset { info, path, location: location.clone() });
        }
    }
    out
}
