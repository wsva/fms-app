#![allow(unused_imports)]
use std::io::{BufReader, Write as _};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use futures_util::StreamExt;
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tar::Archive;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use ring::signature::{Ed25519KeyPair, KeyPair};

use crate::datasets;
use crate::settings::{self, SettingsState};
use super::*;

use crate::sync::client::sync_state::{ensure_sync_tables, upstream_of};



/// Local root a dataset type syncs into: `<datasets>/<type>` (e.g.
/// `<datasets>/dictation`, `<datasets>/card`, `<datasets>/book`). Mirrors the
/// PC-side layout so every existing read path discovers the unpacked dataset.
pub(crate) fn type_root(settings: &SettingsState, dataset_type: &str) -> std::path::PathBuf {
    settings.datasets_dir().join(dataset_type)
}

/// Where a snapshot download installs, and the provenance it carries, decided by
/// the *kind* of the connected host (sync-v2.md §6.1, §8.4):
/// * the website is a **registry** — adopt into the read-only download root under
///   `upstream_kind = "registry"` (a pinned copy, never a mirror of a cluster);
/// * a LAN PC is the **cluster authority** — mirror into the writable type root
///   under `upstream_kind = "desktop_hub"`.
/// Pure path/label selection (no host I/O), so it is unit-testable.
pub(crate) fn adoption_target(
    settings: &SettingsState,
    base: &str,
    dataset_type: &str,
) -> (std::path::PathBuf, &'static str) {
    match crate::sync::hub_kind(base) {
        crate::sync::HubKind::Website => {
            let ty = dataset_type
                .parse::<datasets::DatasetType>()
                .unwrap_or(datasets::DatasetType::Dictation);
            (datasets::download_root(settings, ty), "registry")
        }
        crate::sync::HubKind::LanPc => (type_root(settings, dataset_type), "desktop_hub"),
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------



/// Evict an adopted (download-root) dataset to reclaim device storage (§16.4).
///
/// Deletes the read-only base directory but *keeps* its provenance row
/// (`upstream_kind`/`upstream_origin`, `published_version`, `availability`) with
/// an empty `overall_hash` — the sentinel the Status surface reads as "not on
/// device". Progress and XP live in the app DB, never in the directory (INV-9),
/// so they survive untouched and a re-download ([`dataset_sync_snapshot`]) rejoins
/// them. Refuses anything that is not an adopted base: a writable local dataset,
/// or an edited overlay — the read resolver returns the overlay (not under the
/// download root), so this never pulls a base out from under an overlay.
pub(crate) fn evict_dataset_inner(settings: &SettingsState, uuid: &str) -> Result<Value, String> {
    let (dir, _ty) = datasets::find_dataset_dir_typed(settings, uuid)?;
    if !datasets::is_under_download_root(settings, &dir) {
        return Err(
            "only adopted (downloaded) datasets can be evicted; this copy lives in a writable \
             location — delete it from the Datasets page, or, if it is an edited overlay of a \
             downloaded base, detach the overlay first (dataset_layer_detach)"
                .to_string(),
        );
    }
    // The download size on record is the freed-space figure we report.
    let freed_bytes: i64 = {
        let conn = datasets::dictation::open_app_db(settings)?;
        ensure_sync_tables(&conn)?;
        conn.query_row(
            "SELECT COALESCE(bytes, 0) FROM dataset_sync_state WHERE dataset_uuid = ?1",
            params![uuid],
            |r| r.get(0),
        )
        .unwrap_or(0)
    };
    std::fs::remove_dir_all(&dir).map_err(|e| format!("evict failed: {e}"))?;
    // Clear the on-disk facts, keep provenance. An empty `overall_hash` is how
    // `sync_status_detail` reports "not on device", and a re-download restores
    // the rest via INSERT OR REPLACE.
    {
        let conn = datasets::dictation::open_app_db(settings)?;
        ensure_sync_tables(&conn)?;
        let _ = conn.execute(
            "UPDATE dataset_sync_state \
             SET overall_hash = '', bytes = 0, file_count = 0, \"cursor\" = NULL, \
                 synced_at = datetime('now') \
             WHERE dataset_uuid = ?1",
            params![uuid],
        );
    }
    log::info!("[sync] evicted adopted dataset {uuid} ({freed_bytes} bytes freed)");
    Ok(json!({ "evicted": true, "uuid": uuid, "freed_bytes": freed_bytes }))
}


/// Outcome of a prune-by-absence attempt (sync-v2.md §5 INV-7, §6.1). The
/// caller logs it; the round never fails on a skip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PruneOutcome {
    /// Row's provenance matched the connected host and the local copy is gone.
    Deleted,
    /// Row's provenance names a different upstream — absence from *this* host's
    /// catalog is not evidence of deletion (INV-7).
    SkippedProvenance,
    /// Row is marked withdrawn; INV-22 forbids deleting a practice copy just
    /// because the publisher narrowed visibility.
    SkippedWithdrawn,
    /// No row to prune.
    Missing,
}

/// Remove a local dataset the connected host no longer offers: delete its
/// directory under every type root and drop its `dataset_sync_state` row.
///
/// Prune is provenance-scoped (INV-7): a dataset whose recorded upstream is a
/// different host is *not* evidence of deletion just because the currently
/// connected host does not list it. Without this guard, pointing a machine at
/// the website destroys its LAN-synced copies — the website catalog is
/// visibility-filtered by construction (§3 fact 1).
pub(crate) fn prune_local_dataset(
    settings: &SettingsState,
    conn: &Connection,
    uuid: &str,
    connected: (&str, &str),
) -> PruneOutcome {
    // Read provenance + availability first; the row may not exist (a caller can
    // reach this path from a stale in-memory list).
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT COALESCE(upstream_kind, ''), COALESCE(upstream_origin, ''), \
                    COALESCE(availability, 'listed') \
             FROM dataset_sync_state WHERE dataset_uuid = ?1",
            params![uuid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    let (row_kind, row_origin, availability) = match row {
        Some(v) => v,
        None => return PruneOutcome::Missing,
    };
    if availability == "withdrawn" {
        log::info!(
            "[sync] prune skipped for {uuid}: marked withdrawn (INV-22)"
        );
        return PruneOutcome::SkippedWithdrawn;
    }
    // An empty `row_origin` means the row predates the phase-1 migration and
    // has not yet been re-stamped by a round. Treat as "unknown upstream" and
    // skip rather than delete — the next round either stamps it (and a later
    // prune can act) or the user forgets it explicitly.
    if row_origin.is_empty() || (row_kind.as_str(), row_origin.as_str()) != connected {
        log::warn!(
            "[sync] prune skipped for {uuid}: upstream is ({}, {}), connected host is ({}, {}) — absence from this catalog is not deletion (INV-7)",
            row_kind,
            row_origin,
            connected.0,
            connected.1,
        );
        return PruneOutcome::SkippedProvenance;
    }
    remove_dataset_dirs(settings, uuid);
    let _ = conn.execute("DELETE FROM dataset_sync_state WHERE dataset_uuid = ?1", params![uuid]);
    PruneOutcome::Deleted
}

/// Unconditional local-copy removal: every type root plus the sync-state row.
/// The provenance guard in [`prune_local_dataset`] exists to stop *absence from
/// a foreign catalog* from deleting a copy; an explicit user action
/// ([`sync_forget_dataset`]) is a different question and bypasses the guard.
pub(crate) fn remove_local_dataset_unchecked(settings: &SettingsState, conn: &Connection, uuid: &str) {
    remove_dataset_dirs(settings, uuid);
    let _ = conn.execute("DELETE FROM dataset_sync_state WHERE dataset_uuid = ?1", params![uuid]);
}

/// Remove every on-disk copy of `uuid`: each writable type root *and* the
/// read-only download root, so an adopted base cannot survive a prune or an
/// explicit forget. Best-effort — a missing directory is not an error.
pub(crate) fn remove_dataset_dirs(settings: &SettingsState, uuid: &str) {
    for ty in ["dictation", "card", "book", "read_aloud", "wiki"] {
        let dir = type_root(settings, ty).join(uuid);
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        if let Ok(dt) = ty.parse::<datasets::DatasetType>() {
            let dl = datasets::download_root(settings, dt).join(uuid);
            if dl.exists() {
                let _ = std::fs::remove_dir_all(&dl);
            }
        }
    }
}

/// Where a card dataset's website round trip should go, decided by provenance
/// rather than by the currently connected host (sync-v2.md §3 fact 3, §12).
///
/// The website's `/api/card/sync/{pull,push}` endpoints exist only on the
/// website origin; a LAN hub syncs card content through the dataset-scope
/// journal (kind `card`) and the writeback queue. So the router has three
/// verdicts, not two:
///
/// * [`CardSyncTarget::Website`] — the dataset's upstream is the cloud hub, run
///   today's round trip.
/// * [`CardSyncTarget::Skip`] — the dataset's upstream is a desktop hub, the LAN
///   journal already carries its rows; running the website round trip as well
///   would be the second conflict engine §3 fact 3 names.
/// * `Err(_)` — provenance and the connected host disagree (INV-19). Refuse
///   with an actionable message rather than silently switching targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CardSyncTarget {
    Website { origin: String },
    Skip { reason: String },
}

/// Look up a card dataset's recorded upstream and decide the round-trip target.
/// Falls back to the currently connected host when the row has no provenance
/// yet (pre-migration or freshly-synced copy); the caller's next write stamps
/// it. Reads through the app DB, so it is cheap and safe to call from any
/// card-sync entry point.
pub(crate) fn card_sync_target(
    settings: &SettingsState,
    dataset_uuid: &str,
) -> Result<CardSyncTarget, String> {
    let base = pc_base(settings);
    if base.is_empty() {
        return Err("no hub configured; card sync needs an upstream".to_string());
    }
    let (connected_kind, connected_origin) = upstream_of(&base);
    let conn = datasets::dictation::open_app_db(settings)?;
    ensure_sync_tables(&conn)?;
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT COALESCE(upstream_kind, ''), COALESCE(upstream_origin, '') \
             FROM dataset_sync_state WHERE dataset_uuid = ?1",
            params![dataset_uuid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let (row_kind, row_origin) = match row {
        Some((k, o)) if !o.is_empty() => (k, o),
        // No provenance yet: adopt the connected host as this dataset's upstream
        // and stamp it, so the next call takes the recorded-path branch.
        _ => {
            let _ = conn.execute(
                "INSERT INTO dataset_sync_state \
                 (dataset_uuid, overall_hash, synced_at, upstream_kind, upstream_origin) \
                 VALUES (?1, '', datetime('now'), ?2, ?3) \
                 ON CONFLICT(dataset_uuid) DO UPDATE SET \
                   upstream_kind = excluded.upstream_kind, \
                   upstream_origin = excluded.upstream_origin \
                 WHERE dataset_sync_state.upstream_origin IS NULL \
                    OR dataset_sync_state.upstream_origin = ''",
                params![dataset_uuid, connected_kind, connected_origin],
            );
            (connected_kind.to_string(), connected_origin.clone())
        }
    };
    if row_kind != connected_kind || row_origin != connected_origin {
        return Err(format!(
            "card dataset {dataset_uuid} syncs against {row_kind} '{row_origin}', \
             but the currently connected host is {connected_kind} '{connected_origin}'. \
             One upstream per scope per device (INV-19): reconnect to the recorded \
             upstream, or use 'forget hub' on that copy and re-adopt it from this host."
        ));
    }
    match row_kind.as_str() {
        "cloud_hub" => Ok(CardSyncTarget::Website { origin: row_origin }),
        "desktop_hub" => Ok(CardSyncTarget::Skip {
            reason: format!(
                "dataset is upstreamed by a desktop hub ({row_origin}); its rows travel \
                 through the dataset-scope journal, so the website card round trip \
                 would be a second conflict engine (§3 fact 3)"
            ),
        }),
        // `registry` and `local` are declared in the schema but never written in
        // phase 1 (registry adoption lands with phase 3). Treat as a skip with
        // an explicit reason rather than an error, so a hand-written row does
        // not brick the round.
        other => Ok(CardSyncTarget::Skip {
            reason: format!("upstream_kind '{other}' has no card round trip in phase 1"),
        }),
    }
}


/// `true` when the reachable hub's catalog still contains `uuid`. Any error
/// (offline, unpaired) is treated as "not listed" so the forget action stays
/// usable when the hub is gone.
pub(crate) async fn hub_lists_dataset(settings: &SettingsState, uuid: &str) -> bool {
    match pc_get_json(settings, "/api/v1/datasets", &[("lite", "1".to_string())]).await {
        Ok(v) => v
            .as_array()
            .map(|arr| arr.iter().any(|d| d["uuid"].as_str() == Some(uuid)))
            .unwrap_or(false),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------

