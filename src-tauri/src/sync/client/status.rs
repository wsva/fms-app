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

use crate::sync::client::sync_state::{
    app_cursor, app_events_history_backfilled, app_history_backfilled, classify_resync_reason,
    ensure_sync_tables, last_round, local_sync_entries, SyncStateEntry,
};


// ---------------------------------------------------------------------------
// Sync status detail (§3.5) — one object backing the desktop + mobile Status
// surface and its MCP twin. Reads only; never triggers a round.
// ---------------------------------------------------------------------------

/// Where one dataset stands relative to the hub. `state` is a coarse bucket the
/// UI can render directly:
/// * `downloaded`   — present locally with a live row-log cursor;
/// * `needs_resync` — present but the cursor is NULL (predates incremental sync)
///   or sits below the hub's prune watermark, so one snapshot is due;
/// * `not_downloaded` — advertised by the hub, absent locally;
/// * `removed_on_hub` — a local copy the hub no longer lists (deleted upstream).
#[derive(Serialize)]
pub struct DatasetStatus {
    pub dataset_uuid: String,
    pub dataset_type: String,
    pub name: String,
    /// `downloaded` | `needs_resync` | `not_downloaded` | `removed_on_hub`.
    pub state: String,
    /// Incremental cursor; `None` when never pulled or awaiting a resync.
    pub cursor: Option<i64>,
    pub synced_at: String,
    pub bytes: i64,
    pub file_count: i64,
    /// Whether the hub's current `seq` is ahead of our `cursor` (rows to pull).
    pub hub_ahead: bool,
    /// Provenance of the host this copy came from (§6.1 / §19.2): `"desktop_hub"` |
    /// `"registry"` | `"cloud_hub"`, empty when the dataset is catalog-only (never
    /// pulled here).
    pub upstream_kind: String,
    pub upstream_origin: String,
    /// Registry availability: `"listed"` | `"withdrawn"` (INV-22); empty when unknown.
    pub availability: String,
    /// The registry version adopted (`None` for a LAN mirror or a catalog-only row).
    pub published_version: Option<i64>,
    /// Why this copy owes a resync (§19.2), populated only when `state ==
    /// "needs_resync"`: `"retention_gap"` | `"journal_regressed"` |
    /// `"provenance_reset"` | `"no_cursor"`. `None` in every other state.
    pub resync_reason: Option<String>,
    /// The cloud role this hub last recorded for the scope (finish 7b):
    /// `"authority"` = the cloud hosts the journal, `"mirror"` = this hub holds
    /// it. `None` = never handed over (epoch 0; the desktop-only
    /// `journal_handover_state` table has no row, and Android never writes one).
    pub journal_role: Option<String>,
    /// The authority epoch this hub presents/refuses against for the scope
    /// (INV-13) — `max` of the recorded handover echo and `sync_scope_epoch`, so
    /// a local `sync_handover_epoch` bump is never hidden by a stale echo.
    pub journal_epoch: Option<i64>,
    /// The failed handover attempt's actionable message, if the last one errored.
    pub journal_last_error: Option<String>,
}

/// Live view of the connected hub from its unauthenticated `/status`.
#[derive(Serialize)]
pub struct HubStatus {
    pub address: String,
    pub reachable: bool,
    /// Present only when reachable.
    pub role: Option<String>,
    pub cluster_id: Option<String>,
    pub protocol_version: Option<u32>,
    pub dataset_count: Option<i64>,
    /// Set when the probe failed — the reason surfaced on the Status page.
    pub error: Option<String>,
    /// Whether this device is **authorized** against the hub, i.e. whether the
    /// zone guard accepts our signed data requests. `Some(true)` covers both a
    /// paired device and a hub reached from a trusted zone (loopback / Tailscale),
    /// where signatures are not even required — either way nothing is left to pair.
    /// `Some(false)` is the hub's `401 device not paired` answer. `None` means
    /// unknown: no hub configured, unreachable, or a failure unrelated to auth.
    ///
    /// `/status` cannot report this by itself — it is deliberately unsigned so it
    /// works before pairing, which is exactly why the old UI had no way to tell
    /// "connected" from "connected and allowed". The state is read off the first
    /// *signed* call of the round instead of being guessed from an empty catalog.
    pub paired: Option<bool>,
}

/// One journal scope's writeback backlog: how many changes are waiting, and the
/// oldest pending `edit_time` among them (§19.2). A nonzero depth against a host
/// that does not accept the scope is the "queue keeps growing" signal; the oldest
/// stamp says how long the head has been stuck.
#[derive(Serialize, Clone, Debug)]
pub struct ScopeQueue {
    pub depth: i64,
    pub oldest_edit_time: Option<String>,
}

/// Aggregated local + hub sync state for the Status surface (§3.5).
#[derive(Serialize)]
pub struct SyncStatusDetail {
    /// Which hub implementation `hub.address` points at: `"website"` (OAuth2
    /// Bearer, file + card sync only) or `"lan"` (device-HMAC, full protocol).
    /// The frontend gates pairing/cluster/app-data affordances on this.
    pub hub_kind: String,
    pub role: String,
    pub cluster_id: String,
    pub protocol_version: u32,
    pub device_id: String,
    pub hub: HubStatus,
    pub datasets: Vec<DatasetStatus>,
    /// Rows queued for writeback upload (this device is a follower).
    pub queued_count: i64,
    /// Offline chat messages awaiting flush (follower only; `0` on a hub).
    pub chat_pending: i64,
    /// Paired-device registry entries — populated only when `role == "hub"`.
    /// `Value` (not `sync::pairing::PairedDevice`) because `pairing` is desktop-only;
    /// the hub serializes its `Vec<PairedDevice>` into this JSON array.
    pub devices: Value,
    /// This device's cursor into the hub's app-wide journal scope (per-user
    /// dictation progress + XP). `None` until the first app-data pull of a round
    /// has advanced it — mirrors a dataset's `needs_resync` cursor being unknown.
    pub app_cursor: Option<i64>,
    /// When this device last folded the hub's *whole* app-data history into its own
    /// (`GET /app/state`, §3.6). `None` = never: nothing has owed it, or the hub
    /// predates protocol v4 and cannot serve it — the round result says which.
    pub history_backfilled_at: Option<String>,
    /// When this device last folded the hub's *whole* practice-event history
    /// (reviews + attempts) into its own (§19.2). `None` = never, or the hub
    /// predates protocol v6.
    pub events_backfilled_at: Option<String>,
    /// Authority the cluster is currently serving (§7 / §19.2), from the
    /// capability page: `"lan_hub"` | `"registry"` | `"cloud_hub"`. `None` when
    /// the host serves no `/cluster` (unreachable, or a website hub).
    pub authority_kind: Option<String>,
    /// The authority's election epoch (`None` alongside `authority_kind`).
    pub epoch: Option<u32>,
    /// Scopes the authority advertises it can serve (§7); empty when no
    /// capability page is reachable.
    pub features: Vec<String>,
    /// Per-scope writeback backlog (§19.2): the `queued_count` total split into
    /// the app-wide journal scope and the per-dataset scope, each with the oldest
    /// pending `edit_time`. A nonzero depth against a host that does not accept
    /// that scope is the "queue keeps growing" signal.
    pub queue_app: ScopeQueue,
    pub queue_dataset: ScopeQueue,
    /// The five-cause "queue keeps growing" diagnosis (§19.4), derived from the
    /// signals above plus the last round: names *why* the queue is not draining
    /// (or that it is idle/draining) so the surface shows a cause, not a spinner.
    pub queue_diagnosis: QueueDiagnosis,
    /// Whether the hub advertises the app plane this device has *not* folded
    /// history from (§19.2): a visible gap rather than a silent zero.
    pub history_gap: bool,
    /// Same for the practice-event half (protocol v6): the hub can serve events
    /// this device has never backfilled — a hub upgrade may be owed, or a round.
    pub events_gap: bool,
    /// The last completed round's result JSON (§19.2) — status, pushed/held/
    /// applied/resynced/pruned/errors — so the surface reports the previous round
    /// without running one. `None` until the first round on this build.
    pub last_round: Option<Value>,
    /// When that round was persisted.
    pub last_round_at: Option<String>,
}

/// Assemble the full [`SyncStatusDetail`]. Awaits (hub `/status` + catalog) run
/// to completion *before* any `Connection` is opened, so no rusqlite handle
/// (which is `!Send`) is ever held across an `.await`.
pub(crate) async fn sync_status_detail(settings: &SettingsState) -> Result<SyncStatusDetail, String> {
    let base = pc_base(settings);

    // 1. Probe the hub's `/status` (left open by the zone guard, so it works
    //    before pairing). A failure is recorded, not propagated — a partially
    //    unreachable hub should still show local state.
    let mut hub = HubStatus {
        address: base.clone(),
        reachable: false,
        role: None,
        cluster_id: None,
        protocol_version: None,
        dataset_count: None,
        error: None,
        paired: None,
    };
    if base.is_empty() {
        hub.error = Some("no hub configured — use Settings › Discover PC".to_string());
    } else if crate::sync::hub_kind(&base) == crate::sync::HubKind::Website {
        // The website serves no `/status` (dev guide §6) and its catalog is
        // visibility-gated, so an *anonymous* lite list is the honest
        // reachability probe: any HTTP answer (incl. 401/403) proves the
        // origin is up. Role/cluster/protocol are LAN concepts — left null.
        hub.role = Some("website".to_string());
        let probe = async {
            let client = crate::auth::hub_client_builder()
                .connect_timeout(Duration::from_secs(6))
                .timeout(Duration::from_secs(12))
                .build()
                .map_err(|e| e.to_string())?;
            client
                .get(format!("{base}/api/v1/datasets?lite=1"))
                .send()
                .await
                .map_err(|e| format!("cannot reach website hub at {base}: {e}"))
        }
        .await;
        match probe {
            Ok(_) => hub.reachable = true,
            Err(e) => hub.error = Some(e),
        }
    } else {
        let probe = async {
            let client = crate::auth::hub_client_builder()
                .connect_timeout(Duration::from_secs(6))
                .timeout(Duration::from_secs(12))
                .build()
                .map_err(|e| e.to_string())?;
            let resp = client
                .get(format!("{base}/api/v1/status"))
                .send()
                .await
                .map_err(|e| format!("cannot reach hub at {base}: {e}"))?;
            resp.error_for_status()
                .map_err(|e| format!("{e}"))?
                .json::<Value>()
                .await
                .map_err(|e| format!("unexpected /status response: {e}"))
        }
        .await;
        match probe {
            Ok(v) => {
                hub.reachable = true;
                hub.role = v.get("role").and_then(|r| r.as_str()).map(|s| s.to_string());
                hub.cluster_id =
                    v.get("cluster_id").and_then(|c| c.as_str()).map(|s| s.to_string());
                hub.protocol_version =
                    v.get("protocol_version").and_then(|p| p.as_u64()).map(|p| p as u32);
                hub.dataset_count = v.get("dataset_count").and_then(|d| d.as_i64());
            }
            Err(e) => hub.error = Some(e),
        }
    }

    // 2. Pull the hub catalog (cheap `lite` list). Empty when unreachable/unpaired
    //    — the merge below still reports local-only datasets. This is also the
    //    round's first *signed* call, so its outcome doubles as the pairing probe:
    //    accepted ⇒ authorized, `not paired` ⇒ Pair is still owed. Any other
    //    failure leaves `paired` unknown rather than guessing at it.
    let mut catalog: Vec<Value> = Vec::new();
    if hub.reachable {
        match pc_get_json(settings, "/api/v1/datasets", &[("lite", "1".to_string())]).await {
            Ok(v) => {
                hub.paired = Some(true);
                catalog = v.as_array().cloned().unwrap_or_default();
            }
            Err(e) => {
                if e.contains("not paired") {
                    hub.paired = Some(false);
                }
                log::info!("[sync] catalog fetch skipped: {e}");
            }
        }
    }

    // Capability page (§7 / §19.2): one extra GET so the surface can name the
    // authority and its advertised scopes. Guarded — None on any failure (an
    // unreachable hub, or a website that serves no /cluster), exactly like the
    // round degrades. Fetched above the synchronous reads so no rusqlite handle
    // (which is !Send) is ever held across the await.
    let cluster = fetch_cluster(settings).await;

    // 3. Everything below is synchronous SQLite / in-process reads.
    let role = settings.role();
    let cluster_id = settings.cluster_id();
    let device_id = local_device_id(settings).unwrap_or_default();

    let conn = datasets::dictation::open_app_db(settings)?;
    ensure_sync_tables(&conn)?;
    // The hub tables (`sync_scope_epoch`) may never have been created on this
    // device — ensure defensively so the journal-authority read below never
    // trips over a missing table (same defensive read as `scope_epoch`).
    let _ = crate::sync::change_log::ensure_hub_tables(&conn);

    // Journal authority (finish 7b): the recorded handover states, desktop-only
    // (the write side lives in `sync/mirror.rs`, gated with its table), plus
    // every scope's current authority epoch from `sync_scope_epoch` — read as one
    // map so per-dataset rows cost no query each. A failure just leaves the
    // surface without authority info, exactly like a never-handed-over hub.
    let handover_map: std::collections::HashMap<String, (Option<String>, i64, Option<String>)> = {
        #[cfg(feature = "desktop")]
        {
            crate::sync::mirror::handover_states(&conn)
                .unwrap_or_default()
                .into_iter()
                .map(|(scope, role, epoch, err)| (scope, (role, epoch, err)))
                .collect()
        }
        #[cfg(not(feature = "desktop"))]
        {
            Default::default()
        }
    };
    let scope_epochs: std::collections::HashMap<String, i64> = conn
        .prepare("SELECT scope, epoch FROM sync_scope_epoch")
        .and_then(|mut s| {
            s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
        })
        .unwrap_or_default();
    // Attach the recorded handover state to one status row: the role/error from
    // the state table (None = never handed over), the epoch as the max of the
    // cloud echo and what this hub now presents (INV-13).
    let journal_of = |uuid: &str| -> (Option<String>, Option<i64>, Option<String>) {
        let state = handover_map.get(uuid);
        let echo = state.map(|s| s.1).unwrap_or(0);
        let present = scope_epochs.get(uuid).copied().unwrap_or(0);
        // `sync_scope_epoch` is always at least the recorded echo (the handover
        // writes both), so the max is the live presented epoch (INV-13) — and a
        // scope at 0 has never been handed over: no epoch to show.
        let epoch = echo.max(present);
        (
            state.and_then(|s| s.0.clone()),
            if epoch > 0 { Some(epoch) } else { None },
            state.and_then(|s| s.2.clone()),
        )
    };

    let mut local: Vec<SyncStateEntry> = local_sync_entries(&conn)?;
    // Mark datasets the hub pruned past our cursor (a resync is forced on the
    // next round — surface it here so the Status page can warn before one runs).
    for e in &mut local {
        if let Some(c) = e.cursor {
            let pruned: Option<i64> = conn
                .query_row(
                    "SELECT pruned_up_to FROM sync_prune_marks WHERE dataset_uuid = ?1",
                    params![e.dataset_uuid],
                    |r| r.get(0),
                )
                .ok();
            if pruned.map(|p| p > c).unwrap_or(false) {
                e.cursor = None;
            }
        }
    }

    let queued_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM writeback_queue", [], |r| r.get(0))
        .unwrap_or(0);
    let (queue_app, queue_dataset) = queue_by_scope(&conn);
    let app_cursor = app_cursor(&conn);
    let history_backfilled_at = app_history_backfilled(&conn);
    let events_backfilled_at = app_events_history_backfilled(&conn);
    let (last_round_val, last_round_at) = last_round(&conn);
    drop(conn);

    // Authority + gaps (§19.2). `hub_proto` prefers the capability page and falls
    // back to the /status probe; a host that can serve the app plane but whose
    // history this device has not folded is a *visible* gap, not a silent zero.
    let hub_proto = cluster
        .as_ref()
        .map(|c| c.protocol_version)
        .or(hub.protocol_version);
    let app_adv = app_journal_advertised(&cluster, &base, hub_proto);
    let history_gap = app_adv && history_backfilled_at.is_none();
    let events_gap =
        matches!(hub_proto, Some(p) if p >= EVENTS_PROTOCOL) && events_backfilled_at.is_none();
    let authority_kind = cluster.as_ref().map(|c| c.authority_kind.clone());
    let epoch = cluster.as_ref().map(|c| c.epoch);
    let features = cluster
        .as_ref()
        .map(|c| c.features.clone())
        .unwrap_or_default();

    let chat_pending = if role == "hub" {
        0
    } else {
        crate::ai::chat::pending_outbox_count(settings)
    };

    let devices: Value = if role == "hub" {
        #[cfg(feature = "desktop")]
        {
            serde_json::to_value(crate::sync::pairing::list_devices(settings).unwrap_or_default())
                .unwrap_or_else(|_| Value::Array(Vec::new()))
        }
        #[cfg(not(feature = "desktop"))]
        {
            Value::Array(Vec::new())
        }
    } else {
        Value::Array(Vec::new())
    };

    // 4. Merge local rows + hub catalog into a per-dataset classification. The
    //    hub owns no `deleted` flag, so absence from the catalog means the
    //    dataset was removed upstream (`removed_on_hub`).
    let mut datasets: Vec<DatasetStatus> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    // On the website path the `/status` probe carries no counts (T3), so the
    // catalog doubles as the dataset count once it is fetched below.
    if hub.role.as_deref() == Some("website") {
        hub.dataset_count = Some(catalog.len() as i64);
    }
    for c in &catalog {
        let uuid = c["uuid"].as_str().unwrap_or("").to_string();
        if uuid.is_empty() {
            continue;
        }
        seen.insert(uuid.clone());
        let entry = local.iter().find(|e| e.dataset_uuid == uuid);
        let cursor = entry.and_then(|e| e.cursor);
        let state = match entry {
            None => "not_downloaded",
            // An evicted adoption keeps its provenance row but clears the hash:
            // no bytes on device, so it reports exactly like a never-downloaded
            // dataset and re-downloads the same way. See `dataset_evict`.
            Some(e) if e.overall_hash.is_empty() => "not_downloaded",
            Some(_) if cursor.is_none() => "needs_resync",
            Some(_) => "downloaded",
        };
        let hub_seq = c["hub_seq"].as_i64();
        let hub_ahead = match (hub_seq, cursor) {
            (Some(seq), Some(c)) => seq > c,
            (Some(_), None) => true,
            _ => false,
        };
        let journal = journal_of(&uuid);
        datasets.push(DatasetStatus {
            dataset_uuid: uuid,
            dataset_type: c["dataset_type"].as_str().unwrap_or("").to_string(),
            name: c["name"].as_str().unwrap_or("").to_string(),
            state: state.to_string(),
            cursor,
            synced_at: entry.map(|e| e.synced_at.clone()).unwrap_or_default(),
            bytes: entry.map(|e| e.bytes).unwrap_or(0),
            file_count: entry.map(|e| e.file_count).unwrap_or(0),
            hub_ahead,
            upstream_kind: entry.map(|e| e.upstream_kind.clone()).unwrap_or_default(),
            upstream_origin: entry.map(|e| e.upstream_origin.clone()).unwrap_or_default(),
            availability: entry.map(|e| e.availability.clone()).unwrap_or_default(),
            published_version: entry.and_then(|e| e.published_version),
            resync_reason: classify_resync_reason(state, entry.and_then(|e| e.resync_reason.clone())),
            journal_role: journal.0,
            journal_epoch: journal.1,
            journal_last_error: journal.2,
        });
    }
    for e in &local {
        if seen.contains(&e.dataset_uuid) {
            continue;
        }
        let journal = journal_of(&e.dataset_uuid);
        datasets.push(DatasetStatus {
            dataset_uuid: e.dataset_uuid.clone(),
            dataset_type: String::new(),
            name: String::new(),
            state: if hub.reachable { "removed_on_hub" } else { "needs_resync" }.to_string(),
            cursor: e.cursor,
            synced_at: e.synced_at.clone(),
            bytes: e.bytes,
            file_count: e.file_count,
            hub_ahead: false,
            upstream_kind: e.upstream_kind.clone(),
            upstream_origin: e.upstream_origin.clone(),
            availability: e.availability.clone(),
            published_version: e.published_version,
            resync_reason: classify_resync_reason(
                if hub.reachable { "removed_on_hub" } else { "needs_resync" },
                e.resync_reason.clone(),
            ),
            journal_role: journal.0,
            journal_epoch: journal.1,
            journal_last_error: journal.2,
        });
    }

    // §19.4: name why the queue is not draining, from signals already in hand.
    // Pure and synchronous; computed before the struct literal moves `hub`,
    // `datasets`, `queue_*` and `last_round_val`.
    let any_needs_resync = datasets.iter().any(|d| d.state == "needs_resync");
    let queue_diagnosis = diagnose_queue(&QueueSignals {
        hub_configured: !base.is_empty(),
        hub_reachable: hub.reachable,
        hub_paired: hub.paired,
        app_advertised: app_adv,
        queue_app_depth: queue_app.depth,
        queue_dataset_depth: queue_dataset.depth,
        any_needs_resync,
        last_round: last_round_val.clone(),
    });

    Ok(SyncStatusDetail {
        hub_kind: if crate::sync::hub_kind(&base) == crate::sync::HubKind::Website {
            "website".to_string()
        } else {
            "lan".to_string()
        },
        role,
        cluster_id,
        protocol_version: crate::sync::PROTOCOL_VERSION,
        device_id,
        hub,
        datasets,
        queued_count,
        chat_pending,
        devices,
        app_cursor,
        history_backfilled_at,
        events_backfilled_at,
        authority_kind,
        epoch,
        features,
        queue_app,
        queue_dataset,
        queue_diagnosis,
        history_gap,
        events_gap,
        last_round: last_round_val,
        last_round_at,
    })
}

/// `#[tauri::command]` wrapper for the Status page on both desktop + mobile.

/// Per-scope writeback queue depth + oldest pending `edit_time` (§19.2). Splits
/// the single `writeback_queue` into the `@app` scope (per-user practice/progress)
/// and the dataset scope (shared content) using the one classifier both the hub
/// append and the follower enqueue use, so the surface reports the same scopes the
/// journal does. Stamps are fixed-width UTC (INV-6), so the lexicographic min is
/// the oldest. Returns `(app, dataset)`; an empty/unreadable queue yields zeros.
pub(crate) fn queue_by_scope(conn: &Connection) -> (ScopeQueue, ScopeQueue) {
    let mut app = ScopeQueue { depth: 0, oldest_edit_time: None };
    let mut dataset = ScopeQueue { depth: 0, oldest_edit_time: None };
    let rows: Vec<(String, Option<String>)> = (|| {
        let mut stmt = conn
            .prepare("SELECT kind, COALESCE(edit_time, queued_at) FROM writeback_queue")
            .ok()?;
        let mapped = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })
            .ok()?;
        Some(mapped.filter_map(|x| x.ok()).collect())
    })()
    .unwrap_or_default();
    for (kind, stamp) in rows {
        let target = if crate::sync::change_log::is_app_scope_kind(&kind) {
            &mut app
        } else {
            &mut dataset
        };
        target.depth += 1;
        if let Some(s) = stamp {
            match &target.oldest_edit_time {
                None => target.oldest_edit_time = Some(s),
                Some(cur) if s < *cur => target.oldest_edit_time = Some(s),
                Some(_) => {}
            }
        }
    }
    (app, dataset)
}

/// The five-cause "queue keeps growing" diagnosis (§19.4). `cause` is a stable
/// machine slug; `cause_index` ties a fault back to its §19.4 number (1-5) and is
/// `None` for the non-fault states (`idle`/`draining`/`no_hub`/`unknown`);
/// `severity` (`ok`/`warn`/`error`) lets the UI and an agent color or triage
/// without re-deriving it from `cause`.
#[derive(Serialize, Clone, Debug)]
pub struct QueueDiagnosis {
    pub cause: String,
    pub cause_index: Option<u8>,
    pub severity: String,
    pub detail: String,
    pub action: String,
}

/// Plain inputs to [`diagnose_queue`], so the cause mapping is unit-testable with
/// no DB and no network. Every field is already computed by `sync_status_detail`.
#[derive(Clone, Debug)]
pub struct QueueSignals {
    pub hub_configured: bool,
    pub hub_reachable: bool,
    pub hub_paired: Option<bool>,
    pub app_advertised: bool,
    pub queue_app_depth: i64,
    pub queue_dataset_depth: i64,
    pub any_needs_resync: bool,
    /// The persisted last-round result; scanned for `status`/`pushed`/`held` and
    /// `errors[]`. `None` until the first round on this build.
    pub last_round: Option<Value>,
}

/// Flatten a round result's `errors[]` into lowercase strings, joining each
/// entry's `stage`/`scope`/`kind` tags with its `error` text so one `contains`
/// check can match either the stage tag or the message.
pub(crate) fn round_error_strings(lr: &Value) -> Vec<String> {
    lr["errors"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|e| {
                    let mut parts: Vec<String> = Vec::new();
                    for k in ["stage", "scope", "kind", "error"] {
                        if let Some(s) = e[k].as_str() {
                            parts.push(s.to_string());
                        }
                    }
                    parts.join(" ").to_lowercase()
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn mk_diagnosis(
    cause: &str,
    cause_index: Option<u8>,
    severity: &str,
    detail: String,
    action: &str,
) -> QueueDiagnosis {
    QueueDiagnosis {
        cause: cause.to_string(),
        cause_index,
        severity: severity.to_string(),
        detail,
        action: action.to_string(),
    }
}

/// Name why the writeback queue is not draining (§19.4). First match wins; the
/// evaluation order is by signal reliability (an unreachable host is named before
/// a scope check, because `features[]` is unknown when the host is down), while
/// `cause_index` preserves the §19.4 numbering. Cause matching is substring-based
/// on the fixed `hub_http_error` vocabulary (`(401)`/`(403)`/`(413)`/"not paired"/
/// "suspended"/"Cannot reach hub") and the round's own error strings — if that
/// vocabulary changes, these matchers move with it.
pub(crate) fn diagnose_queue(sig: &QueueSignals) -> QueueDiagnosis {
    let total = sig.queue_app_depth + sig.queue_dataset_depth;
    let errors = sig.last_round.as_ref().map(round_error_strings).unwrap_or_default();
    let has = |needle: &str| errors.iter().any(|e| e.contains(needle));
    let lr_num = |k: &str| sig.last_round.as_ref().and_then(|lr| lr[k].as_i64()).unwrap_or(0);

    // Nothing queued — no diagnosis owed.
    if total == 0 {
        return mk_diagnosis("idle", None, "ok", "no changes queued".to_string(), "");
    }
    // No hub to drain to.
    if !sig.hub_configured {
        return mk_diagnosis(
            "no_hub",
            None,
            "warn",
            format!("{total} change(s) queued but no hub is configured"),
            "set a hub in Settings > Discover PC",
        );
    }
    // Cause 4: unreachable. Named before scope/auth — with the host down the
    // capability page and pairing state are unknown, so nothing more specific is
    // trustworthy.
    if !sig.hub_reachable {
        return mk_diagnosis(
            "unreachable",
            Some(4),
            "error",
            format!("{total} change(s) queued; the hub is unreachable"),
            "check the network and that the hub is running; every round retries",
        );
    }
    // Cause 3: auth (401 / not paired).
    if sig.hub_paired == Some(false)
        || has("(401)")
        || has("not paired")
        || has("sign in again")
        || has("pair this device")
    {
        return mk_diagnosis(
            "auth",
            Some(3),
            "error",
            format!("{total} change(s) queued; the hub is not accepting this device's auth"),
            "LAN: run pc_pair_start; website: sign in again",
        );
    }
    // Cause 2: suspended / over quota / payload too large.
    if has("(403)")
        || has("suspended")
        || has("(413)")
        || has("quota")
        || has("too large for the hub")
    {
        return mk_diagnosis(
            "suspended_or_quota",
            Some(2),
            "error",
            format!("{total} change(s) queued; the hub refused the account (suspended, over quota, or too large)"),
            "resolve the account limit or access with the hub owner",
        );
    }
    // Cause 1: the connected host does not advertise the app journal, so the
    // app-scope queue is held, not lost (INV-20).
    if sig.queue_app_depth > 0
        && (!sig.app_advertised || lr_num("held") > 0 || has("does not advertise app_journal"))
    {
        return mk_diagnosis(
            "scope_not_advertised",
            Some(1),
            "warn",
            format!(
                "{} app-scope change(s) held: the connected host does not advertise app_journal",
                sig.queue_app_depth
            ),
            "connect a hub that advertises app_journal; the queue is held, not lost (INV-20)",
        );
    }
    // Cause 5: the journal regressed or was pruned past the cursor — a resync is owed.
    if has("alarm") || has("prune") || has("regressed") || has("resync") || sig.any_needs_resync {
        return mk_diagnosis(
            "resync_required",
            Some(5),
            "warn",
            format!("{total} change(s) queued; a resync is owed (journal reset or pruned past the cursor)"),
            "run sync_run_round to take a fresh snapshot; sync_history_backfill for pruned history",
        );
    }
    // A healthy last round against a non-empty queue: it is working down.
    let status_ok = sig.last_round.as_ref().and_then(|lr| lr["status"].as_str()) == Some("ok");
    let pushed = lr_num("pushed");
    if status_ok && pushed > 0 && errors.is_empty() {
        return mk_diagnosis(
            "draining",
            None,
            "ok",
            format!("{total} change(s) queued; the queue is working down (last round pushed {pushed})"),
            "",
        );
    }
    // No signal explains a non-empty queue.
    mk_diagnosis(
        "unknown",
        None,
        "warn",
        format!("{total} change(s) queued and not draining; the last round gave no reason"),
        "run sync_run_round, then re-check",
    )
}


