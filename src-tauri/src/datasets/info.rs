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

use std::cmp::Ordering;
use std::fs;
use std::path::Path;
// Only the desktop cross-type scan produces owned paths.
#[cfg(feature = "desktop")]
use std::path::PathBuf;

use chrono::{DateTime, SecondsFormat, Utc};
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
// Row timestamps — the ones code has to compare
// ---------------------------------------------------------------------------

/// Row-level counterpart of [`now_stamp`]: UTC, millisecond precision, `Z`.
///
/// Descriptor stamps are only displayed and hashed, so second precision is
/// enough. Row stamps are different: `card.updated_at` decides which side of a
/// conflict wins, and it is compared two ways — as text inside SQLite
/// (`WHERE updated_at > ?1`, which no Rust-side parsing can help) and in Rust.
/// Text order is chronological only if every stored value has the same width
/// and the same suffix, so the shape is a storage contract, not a style choice.
/// Milliseconds plus `Z` is the one shape the three writers here agree on byte
/// for byte: `chrono` with `SecondsFormat::Millis`, JavaScript's
/// `Date.toISOString()` (what the frontend binds into `updated_at`), and
/// SQLite's `strftime('%Y-%m-%dT%H:%M:%fZ','now')`.
pub(crate) fn row_stamp_at(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// [`row_stamp_at`] for the current instant.
pub(crate) fn now_row_stamp() -> String {
    row_stamp_at(Utc::now())
}

/// Read any stamp a sync peer, the frontend or SQLite may have produced as a UTC
/// instant.
///
/// An explicit offset is honoured rather than assumed away, which is what makes
/// a phone on Asia/Shanghai time comparable with a PC on Europe/Berlin: a peer
/// that stamps `2026-10-09T15:12:34.567+08:00` means the same instant as one
/// that stamps `2026-10-09T07:12:34.567Z`. A value already in UTC needs no
/// timezone knowledge at all — no writer here emits local time — but a peer or a
/// future server might, so the offset is parsed instead of trusted.
///
/// The two naive forms are SQLite's `datetime('now')` and an RFC-3339-shaped
/// value with no zone; SQLite documents both of its forms as UTC, so a naive
/// stamp is read as UTC rather than as local wall time.
pub(crate) fn parse_stamp(raw: &str) -> Option<DateTime<Utc>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let with_offset =
        DateTime::parse_from_rfc3339(trimmed).map(|dt| dt.with_timezone(&Utc));
    match with_offset {
        Ok(dt) => Some(dt),
        Err(_) => ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"]
            .iter()
            .find_map(|fmt| chrono::NaiveDateTime::parse_from_str(trimmed, fmt).ok())
            .map(|naive| naive.and_utc()),
    }
}

/// `a` versus `b` as instants, or `None` when either side is unreadable.
pub(crate) fn stamps_cmp(a: &str, b: &str) -> Option<Ordering> {
    Some(parse_stamp(a)?.cmp(&parse_stamp(b)?))
}

/// Re-shape a stamp into [`row_stamp_at`] form before storing it, so a column
/// keeps one width and one suffix however many producers feed it.
///
/// Truncates to milliseconds: the remainder is below what any of the three
/// producers can express, and keeping it would only re-mix the shapes this
/// exists to prevent. An unparseable value is returned as-is rather than replaced
/// by a plausible instant — a garbage stamp should stay visibly garbage.
pub(crate) fn canonical_stamp(raw: &str) -> String {
    match parse_stamp(raw) {
        Some(dt) => row_stamp_at(dt),
        None => raw.trim().to_string(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this codec exists for: text order is not instant order once two
    /// shapes meet. `'Z'` (0x5A) out-ranks `'+'` (0x2B) and every digit (0x30-39),
    /// so a finer-grained local edit loses to a coarser remote stamp no matter
    /// which one actually happened later.
    #[test]
    fn text_order_and_instant_order_disagree_across_shapes() {
        let local = "2026-10-09T07:12:34.100500000+00:00"; // chrono, nanoseconds
        let remote = "2026-10-09T07:12:34.100Z"; // JavaScript, milliseconds
        assert_eq!(local.cmp(remote), Ordering::Less, "text crowns the remote");
        assert_eq!(
            stamps_cmp(local, remote),
            Some(Ordering::Greater),
            "the instant keeps the newer local edit"
        );
    }

    /// A phone on Asia/Shanghai time and a PC on Europe/Berlin time recording the
    /// same instant must not be able to disagree about which edit is newer.
    #[test]
    fn explicit_offsets_resolve_to_one_instant() {
        let utc = "2026-10-09T07:12:34.567Z";
        for peer in [
            "2026-10-09T15:12:34.567+08:00", // Shanghai
            "2026-10-09T09:12:34.567+02:00", // Berlin, before the DST switch
        ] {
            assert_eq!(stamps_cmp(utc, peer), Some(Ordering::Equal), "{peer}");
        }
        assert_eq!(
            stamps_cmp(utc, "2026-10-09T07:12:35.000Z"),
            Some(Ordering::Less),
            "a real difference still shows up after the offset maths"
        );
    }

    #[test]
    fn every_shape_a_producer_emits_canonicalizes() {
        for (raw, want) in [
            ("2026-10-09T07:12:34.567Z", "2026-10-09T07:12:34.567Z"), // JS, our rows
            ("2026-10-09T07:12:34.567890+00:00", "2026-10-09T07:12:34.567Z"), // chrono micros
            ("2026-10-09T07:12:34.567890123+00:00", "2026-10-09T07:12:34.567Z"), // chrono nanos
            ("2026-10-09T07:12:34+00:00", "2026-10-09T07:12:34.000Z"), // chrono, whole second
            ("2026-10-09T07:12:34Z", "2026-10-09T07:12:34.000Z"), // now_stamp(), Python
            ("2026-10-09 07:12:34", "2026-10-09T07:12:34.000Z"), // SQLite datetime('now')
            ("2026-10-09T15:12:34.567+08:00", "2026-10-09T07:12:34.567Z"), // non-UTC offset
        ] {
            assert_eq!(canonical_stamp(raw), want, "canonicalizing {raw}");
        }
        // An unreadable stamp stays visibly unreadable rather than acquiring a
        // plausible instant.
        assert_eq!(canonical_stamp("not a stamp"), "not a stamp");
        assert_eq!(canonical_stamp("  "), "");
        assert!(parse_stamp("2026-10-09").is_none(), "a bare date is not a stamp");
    }

    /// Fixed width plus one suffix is the whole reason text order may be trusted
    /// inside SQLite, where `updated_at > ?1` has no parser to lean on.
    #[test]
    fn row_stamps_are_fixed_width() {
        let stamp = now_row_stamp();
        assert_eq!(stamp.len(), 24, "{stamp}");
        assert_eq!(&stamp[10..11], "T");
        assert!(stamp.ends_with('Z'));
        assert_eq!("2020-01-01T00:00:00.000Z".cmp(stamp.as_str()), Ordering::Less);
    }

    /// The descriptor keeps second precision on purpose: nothing compares it, and
    /// `docs/design/dataset-info.md` plus the migration scripts are written to it.
    #[test]
    fn descriptor_stamps_stay_second_precision() {
        let stamp = now_stamp();
        assert_eq!(stamp.len(), 20, "{stamp}");
        assert!(stamp.ends_with('Z'));
    }

    /// The card tables fall back to `strftime('%Y-%m-%dT%H:%M:%fZ','now')` for a
    /// stamp they were not given, so that expression must agree with what Rust
    /// writes into the same column — otherwise a defaulted row sorts against its
    /// siblings for the wrong reason. `%f` is SQLite's `SS.SSS`, seconds included.
    #[test]
    fn sqlite_default_matches_the_rust_writer() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let stored: String = conn
            .query_row(
                "SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored.len(), 24, "{stored}");
        assert_eq!(canonical_stamp(&stored), stored, "already canonical");
        assert_eq!(
            parse_stamp(&stored).unwrap().to_rfc3339_opts(SecondsFormat::Millis, true),
            stored,
            "round-trips through the codec"
        );
        // The naive form the old default produced still reads as the instant SQLite
        // means by it (UTC), and lands on this shape.
        assert_eq!(
            canonical_stamp("2026-10-09 07:12:34"),
            "2026-10-09T07:12:34.000Z"
        );
    }
}
