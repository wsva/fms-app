//! Snapshot + batched-writeback client for another FmS machine.
//!
//! Compiled on **all** platforms (it only needs `reqwest`, `tar`, `flate2`,
//! `rusqlite` — all Android-portable) and driven from the Datasets Sync page on
//! both shells: the Android thin client pulls from its PC, a desktop pulls from
//! another desktop. Only the mutation queue stays mobile-specific — the
//! `dictation`/`xp` write commands enqueue under
//! `#[cfg(not(feature = "desktop"))]`, because desktop writes go straight to the
//! DB and would need last-write-wins rules between two full copies.
//!
//! Model:
//! * `dataset_sync_snapshot` pulls the PC's whole dataset directory (media
//!   included) as one tar.gz into app-private storage and atomically swaps it
//!   into `<datasets>/dictation/<uuid>` so every existing read path works.
//! * Local edits (dictation progress, cue save/delete, XP awards) are recorded
//!   in a `writeback_queue` table and flushed to the PC in batches.
//! * Auth uses Bluetooth-style device pairing: an Ed25519 identity keypair
//!   (`device_id` + `device_seed` in global settings) is signed per request
//!   (`pc_pair_start` bootstraps it through the PC's confirm dialog).
//!   The legacy shared `pc_token` is no longer sent.
#![allow(unused_imports)] // shared move-header: some items live in submodules now

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


// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// Effective PC base URL (trimmed of trailing slashes). Empty when unset.
pub(crate) fn pc_base(settings: &SettingsState) -> String {
    settings
        .settings
        .lock()
        .unwrap()
        .pc_url
        .trim_end_matches('/')
        .to_string()
}

/// Trust-on-first-use cluster adoption + guard (§3.1, §7), applied from a hub
/// `/status` payload:
/// * if this device has no `cluster_id` yet, adopt the hub's (TOFU);
/// * if it already belongs to one and the hub reports a different id, refuse
///   rather than silently switch (the user must "forget hub / re-pair").
/// An empty hub id (not-yet-upgraded) is ignored — adoption happens later.
pub(crate) fn adopt_or_verify_cluster(settings: &SettingsState, hub_cluster: &str) -> Result<(), String> {
    if hub_cluster.is_empty() {
        return Ok(());
    }
    let ws_dir = settings.workspace_dir.lock().unwrap().clone();
    let mut s = settings.settings.lock().unwrap().clone();
    if s.cluster_id.is_empty() {
        s.cluster_id = hub_cluster.to_string();
        settings::SettingsState::save(&s, ws_dir.as_ref())?;
        *settings.settings.lock().unwrap() = s;
        log::info!("[sync] adopted hub cluster_id='{hub_cluster}' (trust-on-first-use)");
        Ok(())
    } else if s.cluster_id != hub_cluster {
        Err(format!(
            "this device belongs to cluster '{}' but the hub at {} is '{}'; use 'forget hub / re-pair' to switch",
            s.cluster_id,
            pc_base(settings),
            hub_cluster
        ))
    } else {
        Ok(())
    }
}

/// Effective PC base URL: a caller-supplied override (e.g. a typed-but-unsaved
/// address in Settings) wins if non-empty; otherwise the persisted setting.
pub(crate) fn resolve_pc_url(settings: &SettingsState, override_url: Option<&str>) -> String {
    match override_url.map(str::trim).filter(|s| !s.is_empty()) {
        Some(u) => u.trim_end_matches('/').to_string(),
        None => pc_base(settings),
    }
}

// ---------------------------------------------------------------------------
// Submodules (pure-move split). Each owns one responsibility; the facade below
// re-exports their items so `sync::client::<item>` paths keep resolving here.
// ---------------------------------------------------------------------------

pub(crate) mod auth;
pub(crate) mod availability;
pub(crate) mod cluster;
pub(crate) mod filediff;
pub(crate) mod market;
pub(crate) mod sync_state;
pub(crate) mod writeback;
pub(crate) mod snapshot;
pub(crate) mod status;
pub(crate) mod pairing;
pub(crate) mod round;

pub(crate) use auth::*;
pub(crate) use availability::*;
pub(crate) use cluster::*;
pub(crate) use market::*;
pub(crate) use sync_state::*;
pub(crate) use writeback::*;
pub(crate) use snapshot::*;
pub(crate) use status::*;
pub(crate) use pairing::*;
pub(crate) use round::*;

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Pull a full dataset snapshot from the paired PC into local storage.
#[tauri::command]
pub async fn dataset_sync_snapshot(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<SyncResult, String> {
    let base = hub_base_or_account(&settings)?;
    let client = reqwest::Client::new();

    // Capability negotiation (sync-v2.md §7): a website host that advertises
    // `registry_reads` serves the pinned, per-version registry routes; anything
    // else (a LAN hub, or today's website) falls back to the live dataset routes.
    // The install location and provenance are decided by host kind below either
    // way — only the read route and the recorded `published_version` differ.
    let cluster = fetch_cluster_at(&settings, &base).await;
    let registry_reads =
        super::hub_kind(&base) == super::HubKind::Website && advertises(&cluster, "registry_reads");

    // A website pull *adopts* a read-only registry copy; it must never shadow a
    // writable dataset this workspace already owns under the same uuid (a hub
    // serving that uuid to its followers, or a local original). The mirror
    // install into the type root (LAN hub) keeps overwriting by design.
    if super::hub_kind(&base) == super::HubKind::Website {
        if let Some(dir) = owned_locally_conflict(&settings, &uuid) {
            return Err(format!(
                "owned-locally: dataset {uuid} is served by this workspace (writable copy at {}) \
                 — market adoption refused; export it to the cloud via Publish instead",
                dir.display()
            ));
        }
    }

    // 1. Manifest -> compare hash.
    let api_path = if registry_reads {
        format!("/api/v1/registry/{uuid}/manifest")
    } else {
        format!("/api/v1/datasets/{uuid}/manifest")
    };
    let mreq = hub_request(
        client.get(format!("{base}{api_path}")),
        &settings,
        "GET",
        &api_path,
        "",
        b"",
    )
    .await?;
    let mresp = mreq.send().await.map_err(|e| e.to_string())?;
    if !mresp.status().is_success() {
        return Err(hub_http_error(mresp, &api_path).await);
    }
    let m: Value = mresp.json().await.map_err(|e| e.to_string())?;

    let hash = m["overall_hash"].as_str().unwrap_or("").to_string();
    let total_bytes = m["total_bytes"].as_u64().unwrap_or(0);
    let file_count = m["file_count"].as_u64().unwrap_or(0) as usize;
    // Which local root to unpack into; the PC reports it in the manifest.
    // Fall back to "dictation" for older PCs that predate the field.
    let dataset_type = m["dataset_type"].as_str().unwrap_or("dictation").to_string();
    // The pinned version a registry read resolved to (INV-8); None on the live
    // route, which is not version-addressed.
    let published_version: Option<i64> =
        if registry_reads { m["current_version"].as_i64() } else { None };

    let stored: Option<String> = {
        let conn = datasets::dictation::open_app_db(&settings)?;
        ensure_sync_tables(&conn)?;
        conn.query_row(
            "SELECT overall_hash FROM dataset_sync_state WHERE dataset_uuid = ?1",
            [&uuid],
            |r| r.get(0),
        )
        .ok()
    };
    if stored.as_deref() == Some(hash.as_str()) {
        return Ok(SyncResult {
            updated: false,
            hash,
            bytes: 0,
            file_count,
        });
    }

    // 2. Always flush local changes before replacing the dataset dir.
    let _ = writeback_flush_inner(&settings, &cluster).await;

    // 3. Prepare temp paths under the install root the host kind selects: the
    //    read-only download root for a registry adoption (website), the writable
    //    type root for a cluster mirror (LAN hub).
    let (root, upstream_kind) = adoption_target(&settings, &base, &dataset_type);
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let tmp_dir = root.join(format!("{uuid}.tmp"));
    let final_dir = root.join(&uuid);
    let part = root.join(format!("{uuid}.tar.gz.part"));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let _ = std::fs::remove_file(&part);
    std::fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;

    // 4. Stream the tar.gz to disk, emitting progress. A registry read pins the
    //    snapshot to the version the manifest resolved to, so the bytes and the
    //    `published_version` recorded below can never drift apart (INV-8).
    let snap_path = if registry_reads {
        format!("/api/v1/registry/{uuid}/snapshot")
    } else {
        format!("/api/v1/datasets/{uuid}/snapshot")
    };
    let snap_qs = match published_version {
        Some(v) if registry_reads => format!("version={v}"),
        _ => String::new(),
    };
    let snap_url = if snap_qs.is_empty() {
        format!("{base}{snap_path}")
    } else {
        format!("{base}{snap_path}?{snap_qs}")
    };
    let sreq = hub_request(
        client.get(snap_url),
        &settings,
        "GET",
        &snap_path,
        &snap_qs,
        b"",
    )
    .await?;
    let resp = sreq.send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(hub_http_error(resp, &snap_path).await);
    }

    // The hub's change-log seq this snapshot already reflects. Recorded as the
    // follower's cursor so the next `/changes` pull continues from here (§3.3).
    // Absent from a pre-v2 hub → NULL, which keeps the copy marked "needs resync"
    // (harmless, since a v1 hub is never polled incrementally).
    let snapshot_seq: Option<i64> = resp
        .headers()
        .get("x-snapshot-seq")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok());

    let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut stream = resp.bytes_stream();
    let mut received: u64 = 0;
    let mut last_emit = Instant::now();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        received += bytes.len() as u64;
        if last_emit.elapsed().as_millis() >= 300 {
            let _ = app.emit(
                "dataset-sync-progress",
                json!({ "uuid": &uuid, "received": received, "total": total_bytes, "phase": "download" }),
            );
            last_emit = Instant::now();
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    // 5. Extract.
    let _ = app.emit(
        "dataset-sync-progress",
        json!({ "uuid": &uuid, "received": received, "total": total_bytes, "phase": "extract" }),
    );
    let f = std::fs::File::open(&part).map_err(|e| e.to_string())?;
    let gz = GzDecoder::new(BufReader::new(f));
    let mut ar = Archive::new(gz);
    ar.unpack(&tmp_dir).map_err(|e| format!("extract failed: {}", e))?;

    // 6. Atomic swap (keep the previous copy until the new one is fully in place).
    let old_dir = root.join(format!("{uuid}.old"));
    let _ = std::fs::remove_dir_all(&old_dir);
    if final_dir.exists() {
        std::fs::rename(&final_dir, &old_dir).map_err(|e| e.to_string())?;
    }
    if let Err(e) = std::fs::rename(&tmp_dir, &final_dir) {
        // Roll back the previous copy so the dataset is never left missing.
        if old_dir.exists() {
            let _ = std::fs::rename(&old_dir, &final_dir);
        }
        return Err(format!("swap failed: {}", e));
    }
    let _ = std::fs::remove_dir_all(&old_dir);
    let _ = std::fs::remove_file(&part);

    // 7. Record sync state (with the incremental cursor this snapshot carries)
    //    and stamp provenance (sync-v2.md §6.1). `upstream_kind` came from
    //    `adoption_target` — `registry` for a website adoption, `desktop_hub` for
    //    a LAN mirror; the origin is the connected host either way. A snapshot is
    //    a full re-establishment of the copy, so it is the one place where
    //    overwriting provenance is safe: the bytes on disk now came from *this*
    //    host and no other. `published_version` is the pinned registry version
    //    (NULL on the live route).
    let upstream_origin = upstream_of(&base).1;
    {
        let conn = datasets::dictation::open_app_db(&settings)?;
        ensure_sync_tables(&conn)?;
        conn.execute(
            "INSERT OR REPLACE INTO dataset_sync_state \
             (dataset_uuid, overall_hash, synced_at, bytes, file_count, \"cursor\", \
              upstream_kind, upstream_origin, published_version, availability) \
             VALUES (?1, ?2, datetime('now'), ?3, ?4, ?5, ?6, ?7, ?8, \
                     COALESCE((SELECT availability FROM dataset_sync_state WHERE dataset_uuid = ?1), 'listed'))",
            params![
                &uuid,
                &hash,
                received as i64,
                file_count as i64,
                snapshot_seq,
                upstream_kind,
                upstream_origin,
                published_version,
            ],
        )
        .map_err(|e| e.to_string())?;
    }

    let _ = app.emit(
        "dataset-sync-progress",
        json!({ "uuid": &uuid, "received": received, "total": total_bytes, "phase": "done" }),
    );

    Ok(SyncResult {
        updated: true,
        hash,
        bytes: received,
        file_count,
    })
}

/// Evict an adopted dataset to reclaim device storage (command twin of
/// [`evict_dataset_inner`]). Progress and XP are kept; re-download with
/// [`dataset_sync_snapshot`].
#[tauri::command]
pub async fn dataset_evict(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<Value, String> {
    evict_dataset_inner(&settings, &uuid)
}

/// Upload all pending writeback changes to the PC.
#[tauri::command]
pub async fn writeback_flush(settings: State<'_, SettingsState>) -> Result<usize, String> {
    let cluster = fetch_cluster(&settings).await;
    writeback_flush_inner(&settings, &cluster).await
}

#[tauri::command]
pub async fn dataset_sync_state(
    settings: State<'_, SettingsState>,
) -> Result<Vec<SyncStateEntry>, String> {
    let conn = datasets::dictation::open_app_db(&settings)?;
    ensure_sync_tables(&conn)?;
    local_sync_entries(&conn)
}

/// Number of changes waiting to be uploaded (for the UI badge).
#[tauri::command]
pub async fn writeback_pending_count(settings: State<'_, SettingsState>) -> Result<i64, String> {
    let conn = datasets::dictation::open_app_db(&settings)?;
    ensure_sync_tables(&conn)?;
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM writeback_queue", [], |r| r.get(0))
        .unwrap_or(0);
    Ok(n)
}

#[tauri::command]
pub async fn sync_status(settings: State<'_, SettingsState>) -> Result<SyncStatusDetail, String> {
    sync_status_detail(&settings).await
}

/// Ping the PC's `GET /api/v1/status` endpoint from native Rust.
///
/// Used by the frontend instead of a `fetch()` so the WebView never issues a
/// cross-origin request (which would be blocked by CORS). Reads `pc_url`
/// straight from settings and returns the PC's raw status JSON:
/// `{ ok, app_name, dataset_count, version }`. `/status` is one of the two
/// routes the zone guard leaves open, so this works before pairing too.
#[tauri::command]
pub async fn pc_check_status(
    settings: State<'_, SettingsState>,
    pc_url: Option<String>,
) -> Result<Value, String> {
    let base = resolve_pc_url(&settings, pc_url.as_deref());
    log::info!(
        "[sync] pc_check_status -> {}/api/v1/status",
        if base.is_empty() { "<unset>" } else { &base }
    );
    if base.is_empty() {
        return Err("pc_url is not set — use Settings › Discover PC".to_string());
    }
    // Website hub: no `/api/v1/status` route (dev guide §6). Probe reachability
    // through the anonymous-safe lite catalog instead, and report a synthetic
    // status so the connect flow succeeds without cluster adoption.
    if super::hub_kind(&base) == super::HubKind::Website {
        let count = match hub_get_json(&settings, "/api/v1/datasets", "lite=1").await {
            Ok(v) => v.as_array().map(|a| a.len() as i64).unwrap_or(0),
            Err(e) => {
                log::error!("[sync] pc_check_status (website) FAILED: {e}");
                return Err(e);
            }
        };
        log::info!("[sync] pc_check_status (website) OK: {count} readable dataset(s)");
        return Ok(json!({
            "ok": true,
            "app_name": "fms-website-hub",
            "website_contract": super::WEBSITE_CONTRACT_VERSION,
            "dataset_count": count,
        }));
    }
    let result = async {
        let client = crate::auth::hub_client_builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client
            .get(format!("{base}/api/v1/status"))
            .send()
            .await
            .map_err(|e| format!("Cannot reach PC at {base}: {e}"))?;
        let v: Value = resp
            .error_for_status()
            .map_err(|e| format!("{e} — is the PC's web service running?"))?
            .json()
            .await
            .map_err(|e| format!("Unexpected response from PC: {e}"))?;
        Ok(v)
    }
    .await;
    match &result {
        Ok(v) => log::info!("[sync] pc_check_status OK: {v}"),
        Err(e) => log::error!("[sync] pc_check_status FAILED: {e}"),
    }
    // Trust-on-first-use cluster adoption + mis-pairing guard from the hub's
    // advertised cluster (§3.1, §7). Runs on the connect path so a follower
    // binds to its hub and refuses a different cluster thereafter.
    if let Ok(v) = &result {
        let hub_cluster = v.get("cluster_id").and_then(|c| c.as_str()).unwrap_or("");
        adopt_or_verify_cluster(&settings, hub_cluster)?;
    }
    result
}

/// Fetch the PC's dataset list (`GET /api/v1/datasets`) from native Rust.
///
/// Same CORS-avoidance rationale as [`pc_check_status`]; returns a JSON array
/// of `{ uuid, name, updated, media_count, status }`. Signed with the device
/// identity — a 401 here means "pair first" and the message is passed through.
#[tauri::command]
pub async fn pc_list_datasets(
    settings: State<'_, SettingsState>,
    pc_url: Option<String>,
) -> Result<Value, String> {
    let base = resolve_pc_url(&settings, pc_url.as_deref());
    log::info!(
        "[sync] pc_list_datasets -> {}/api/v1/datasets",
        if base.is_empty() { "<unset>" } else { &base }
    );
    if base.is_empty() {
        return Err("pc_url is not set — use Settings › Discover PC".to_string());
    }
    let result = async {
        let client = crate::auth::hub_client_builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| e.to_string())?;
        let req = hub_request(
            client.get(format!("{base}/api/v1/datasets")),
            &settings,
            "GET",
            "/api/v1/datasets",
            "",
            b"",
        )
        .await?;
        let resp = req
            .send()
            .await
            .map_err(|e| format!("Cannot reach hub at {base}: {e}"))?;
        if !resp.status().is_success() {
            return Err(hub_http_error(resp, "/api/v1/datasets").await);
        }
        resp.json::<Value>()
            .await
            .map_err(|e| format!("Unexpected response from hub: {e}"))
    }
    .await;
    match &result {
        Ok(v) => log::info!("[sync] pc_list_datasets OK ({} bytes of JSON)", v.to_string().len()),
        Err(e) => log::error!("[sync] pc_list_datasets FAILED: {e}"),
    }
    result
}

/// Classify a hub address the UI is about to use: `"website"` (OAuth Bearer,
/// file + card only — no pairing/cluster) or `"lan"` (device-HMAC, full
/// protocol). Lets the Devices & Hub page hide the pairing flow for the website
/// origin *before* the address is saved. Thin wrapper over [`super::hub_kind`],
/// so the compile-time website origin stays the single source of truth; falls
/// back to the saved `pc_url` when no candidate is passed.
#[tauri::command]
pub async fn sync_hub_kind(
    settings: State<'_, SettingsState>,
    pc_url: Option<String>,
) -> Result<String, String> {
    let base = resolve_pc_url(&settings, pc_url.as_deref());
    Ok(if super::hub_kind(&base) == super::HubKind::Website {
        "website".to_string()
    } else {
        "lan".to_string()
    })
}

/// `GET /api/v1/datasets/{uuid}/changes?after=S` — one incremental change-log
/// page from the hub (§3.2/§3.3). Returns the hub's `{ entries, pruned_up_to,
/// hub_seq, resync_required }` JSON.
///
/// Passing [`crate::sync::change_log::APP_SCOPE`] as the uuid pulls the app-wide
/// per-user journal from `GET /api/v1/app/changes` instead — same page shape, no
/// dataset to resolve.
///
/// Auth is origin-selected by [`hub_get_json`] (`hub_request`): device-HMAC for a
/// LAN hub (byte-identical to the old in-line signing) and Bearer for a website
/// origin. Against a website this path exists only when the host advertises
/// `dataset_journal` for the caller's account (phase 7b) — the round gates the
/// call on [`dataset_journal_advertised`] and never asks for a mirror-only scope;
/// an explicit MCP pull against an unhosted scope simply surfaces the host's
/// 404/401 as an actionable error. A dataset-scope resync alarm (`hub_seq` below
/// our cursor) is enforced by the caller (Phase 4 round).
pub async fn pc_changes_since(
    settings: State<'_, SettingsState>,
    uuid: String,
    after: i64,
) -> Result<Value, String> {
    let base = pc_base(&settings);
    if base.is_empty() {
        return Err("pc_url is not set — use Settings › Discover PC".to_string());
    }
    let api_path = if uuid == crate::sync::change_log::APP_SCOPE {
        "/api/v1/app/changes".to_string()
    } else {
        format!("/api/v1/datasets/{uuid}/changes")
    };
    let qs = build_query(&[("after", after.to_string())]);
    hub_get_json(settings.inner(), &api_path, &qs).await
}

/// `GET /api/v1/file?dataset=D&path=P` — fetch one non-DB file from the hub
/// (§3.2). Streams the raw bytes back; callers splice them into the dataset
/// directory. The signed query matches the URL verbatim so a captured request
/// cannot be re-pointed at another path. `range` (T6, e.g. `"bytes=0-1023"`) is
/// passed through — both hubs honour it (206 + Content-Range), which is what
/// lets a follower resume a partial media fetch instead of re-pulling whole
/// files. Never point it at `data.sqlite3` or its sidecars: the hub refuses
/// those by design, rows go through card sync.
pub async fn pc_fetch_file(
    settings: State<'_, SettingsState>,
    uuid: String,
    path: String,
    range: Option<String>,
) -> Result<Vec<u8>, String> {
    let base = hub_base(&settings)?;
    let api_path = "/api/v1/file";
    let qs = build_query(&[("dataset", uuid), ("path", path)]);
    let client = crate::auth::hub_client_builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    let req = hub_request(
        client.get(format!("{base}{api_path}?{qs}")),
        &settings,
        "GET",
        api_path,
        &qs,
        b"",
    )
    .await?;
    let req = match range {
        Some(r) => req.header("Range", r),
        None => req,
    };
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Cannot reach hub at {base}: {e}"))?;
    if !resp.status().is_success() {
        return Err(hub_http_error(resp, api_path).await);
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read /file body: {e}"))?;
    Ok(bytes.to_vec())
}

/// Explicit "unsubscribe" for a dataset the hub no longer offers (or a copy the
/// user wants dropped): remove the local directory + `dataset_sync_state` row.
/// The round's prune-by-absence only cleans up datasets it can confirm are gone
/// from a reachable hub, so this is the manual path for the `removed_on_hub`
/// Status row. Refuses to act while a hub is reachable and still lists the
/// dataset, so it can never delete something that is still being synced.
#[tauri::command]
pub async fn sync_forget_dataset(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<(), String> {
    if hub_lists_dataset(&settings, &uuid).await {
        return Err("the hub still offers this dataset — it stays in sync".to_string());
    }
    let conn = datasets::dictation::open_app_db(&settings)?;
    ensure_sync_tables(&conn)?;
    remove_local_dataset_unchecked(&settings, &conn, &uuid);
    log::info!("[sync] forgot local dataset {uuid}");
    Ok(())
}

/// Manual "fetch my history now" entry point (Sync page + MCP): pulls the hub's
/// whole app-data history for this identity, ignoring the once-per-device stamp.
/// Re-running it is harmless — every row lands through an idempotent write path.
#[tauri::command]
pub async fn sync_backfill_history(
    settings: State<'_, SettingsState>,
) -> Result<Value, String> {
    if pc_base(&settings).is_empty() {
        return Err("No PC configured. Use Discover PC or set the PC address.".into());
    }
    let unavailable = json!({
        "status": "unavailable",
        "reason": "the hub does not serve /app/state (predates sync protocol v4)",
        "next": "update the PC app, run a sync round, then retry",
    });
    let cluster = fetch_cluster(&settings).await;
    Ok(backfill_app_history(&settings, &cluster, None, true)
        .await?
        .unwrap_or(unavailable))
}

/// Tauri entry point for the round (Datasets Sync page + MCP).
#[tauri::command]
pub async fn sync_run_round(
    app: AppHandle,
    settings: State<'_, SettingsState>,
) -> Result<Value, String> {
    sync_round_inner(app, settings).await
}

/// Pair this device with the PC. The PC owner gets a confirm dialog; we poll
/// `/pair/status` every 2 s (up to 120 s) until they answer. An approval also
/// binds this device to the hub's cluster (see [`bind_cluster_after_pairing`]).
#[tauri::command]
pub async fn pc_pair_start(
    settings: State<'_, SettingsState>,
    pc_url: Option<String>,
) -> Result<Value, String> {
    let base = resolve_pc_url(&settings, pc_url.as_deref());
    if super::hub_kind(&base) == super::HubKind::Website {
        return Err("Pairing is a local-network concept — the website hub authenticates \
                    your account, so sign in instead."
            .into());
    }
    if base.is_empty() {
        return Err("No PC address — enter or discover the PC address first.".into());
    }
    let ident = ensure_device_identity(&settings)?;
    let client = crate::auth::hub_client_builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    let body = json!({
        "device_id": ident.device_id,
        "name": device_name(),
        "pubkey_hex": ident.pubkey_hex,
        "user_key": crate::auth::workspace_identity(&settings),
    });
    let resp = client
        .post(format!("{base}/api/v1/pair/request"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Cannot reach PC at {base}: {e}"))?;
    let status = resp.status();
    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("Unexpected pairing response from PC (HTTP {status}): {e}"))?;
    if !status.is_success() {
        // Surface the PC's actionable text (malformed key, registry error, ...).
        let msg = v["error"].as_str().unwrap_or("pairing request rejected");
        return Err(msg.to_string());
    }

    let state = v["state"].as_str().unwrap_or("none").to_string();
    if state != "pending" {
        // Re-pair (already approved) and denied short-circuit — no dialog. An
        // already-approved device still binds: the trust event is what adoption
        // belongs to, and devices that paired before this hook existed have no
        // cluster id of their own yet.
        let cluster_warning = if state == "approved" {
            bind_cluster_after_pairing(&settings, &client, &base).await
        } else {
            None
        };
        return Ok(json!({
            "state": state,
            "fingerprint": v["fingerprint"],
            "cluster_warning": cluster_warning,
        }));
    }

    // Dialog path: poll until the PC owner answers (or we give up).
    let fingerprint = v["fingerprint"].as_str().unwrap_or("").to_string();
    let request_id = v["request_id"].as_str().unwrap_or("").to_string();
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let s: Value = client
            .get(format!(
                "{base}/api/v1/pair/status?device_id={}",
                ident.device_id
            ))
            .send()
            .await
            .map_err(|e| format!("Lost contact with the PC while pairing: {e}"))?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        let state = s["state"].as_str().unwrap_or("pending").to_string();
        if state != "pending" {
            let cluster_warning = if state == "approved" {
                bind_cluster_after_pairing(&settings, &client, &base).await
            } else {
                None
            };
            return Ok(json!({
                "state": state,
                "fingerprint": fingerprint,
                "request_id": request_id,
                "cluster_warning": cluster_warning,
            }));
        }
    }
    Err("Timed out waiting for the PC owner to confirm pairing.".into())
}

/// Regenerate this device's identity keypair. Recovery path when the PC owner
/// denied the pairing: a fresh identity pops a new dialog instead of being
/// silently blocked by the stored denial.
#[tauri::command]
pub async fn pc_pair_reset_identity(settings: State<'_, SettingsState>) -> Result<String, String> {
    {
        let mut s = settings.settings.lock().unwrap();
        s.device_seed.clear();
        s.device_id.clear();
    }
    let ident = ensure_device_identity(&settings)?;
    log::info!("[sync] device identity reset -> {}", ident.device_id);
    Ok(ident.device_id)
}

/// `GET /api/v1/wiki/datasets` — the hub's wiki datasets.
#[tauri::command]
pub async fn wiki_hub_list(settings: State<'_, SettingsState>) -> Result<Value, String> {
    pc_get_json(settings.inner(), "/api/v1/wiki/datasets", &[]).await
}

/// `GET /api/v1/wiki/dataset/dir?uuid=&rel=` — one directory of a hub wiki dataset.
#[tauri::command]
pub async fn wiki_hub_list_dir(
    settings: State<'_, SettingsState>,
    uuid: String,
    rel: String,
) -> Result<Vec<crate::datasets::wiki::WikiFileEntry>, String> {
    let v = pc_get_json(
        settings.inner(),
        "/api/v1/wiki/dataset/dir",
        &[("uuid", uuid), ("rel", rel)],
    )
    .await?;
    serde_json::from_value(v).map_err(|e| format!("Unexpected wiki/dataset/dir response: {e}"))
}

/// `GET /api/v1/wiki/dataset/file?uuid=&rel=` — classified file preview from the hub
/// (`{ kind, content, size, truncated }`). A pre-classification hub replies with
/// only `{ content }`, which is treated as markdown; the hub-side absolute path
/// is never sent, so `path` stays None and images/media degrade to the
/// unsupported preview message on followers.
#[tauri::command]
pub async fn wiki_hub_read_file(
    settings: State<'_, SettingsState>,
    uuid: String,
    rel: String,
) -> Result<crate::datasets::wiki::WikiFileContent, String> {
    let v = pc_get_json(
        settings.inner(),
        "/api/v1/wiki/dataset/file",
        &[("uuid", uuid), ("rel", rel)],
    )
    .await?;
    Ok(crate::datasets::wiki::WikiFileContent {
        kind: v
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("markdown")
            .to_string(),
        content: v.get("content").and_then(|c| c.as_str()).map(String::from),
        size: v.get("size").and_then(|s| s.as_u64()).unwrap_or_default(),
        truncated: v.get("truncated").and_then(|t| t.as_bool()).unwrap_or_default(),
        path: None,
    })
}

/// `GET /api/v1/wiki/dataset/search?uuid=&keyword=` — FTS inside one hub dataset.
#[tauri::command]
pub async fn wiki_hub_search(
    settings: State<'_, SettingsState>,
    uuid: String,
    keyword: String,
) -> Result<Vec<crate::datasets::wiki::WikiSearchResult>, String> {
    let v = pc_get_json(
        settings.inner(),
        "/api/v1/wiki/dataset/search",
        &[("uuid", uuid), ("keyword", keyword)],
    )
    .await?;
    serde_json::from_value(v).map_err(|e| format!("Unexpected wiki/dataset/search response: {e}"))
}

// ---------------------------------------------------------------------------
// Tests — phase-1 provenance floor (sync-v2.md §6.1, INV-7 / INV-19 / INV-22)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn mem_conn() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    /// A `SettingsState` backed by a throwaway temp workspace, so every
    /// filesystem/DB touch stays hermetic. The caller removes `dir` afterwards.
    fn temp_settings(tag: &str) -> (SettingsState, PathBuf) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("fms-sync-test-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let settings = SettingsState::new();
        settings.set_workspace_dir(Some(dir.clone()));
        (settings, dir)
    }

    fn insert_state(conn: &Connection, uuid: &str, kind: &str, origin: &str, availability: &str) {
        conn.execute(
            "INSERT INTO dataset_sync_state \
             (dataset_uuid, overall_hash, synced_at, upstream_kind, upstream_origin, availability) \
             VALUES (?1, '', datetime('now'), ?2, ?3, ?4)",
            params![uuid, kind, origin, availability],
        )
        .unwrap();
    }

    /// Create a minimal dataset directory (just a valid `info.json`) at `dir`, so
    /// `find_dataset_dir_typed` resolves it. Mirrors what a snapshot install
    /// leaves on disk for the resolver/eviction/fork tests below.
    fn write_dataset(dir: &std::path::Path, uuid: &str, ty: datasets::DatasetType, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let info = datasets::info::DatasetInfo::new(uuid.to_string(), ty, "test-v1", name.to_string());
        datasets::info::write_info(dir, &info).unwrap();
    }

    // -- migration ----------------------------------------------------------

    #[test]
    fn ensure_sync_tables_is_idempotent() {
        let conn = mem_conn();
        ensure_sync_tables(&conn).unwrap();
        ensure_sync_tables(&conn).unwrap();
        for (table, col) in [
            ("dataset_sync_state", "upstream_kind"),
            ("dataset_sync_state", "upstream_origin"),
            ("dataset_sync_state", "availability"),
            ("dataset_sync_state", "published_version"),
            ("dataset_sync_state", "row_hash"),
            ("dataset_sync_state", "resync_reason"),
            ("app_sync_state", "upstream_kind"),
            ("app_sync_state", "upstream_origin"),
        ] {
            let n: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = '{col}'"),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{table}.{col} should exist exactly once");
        }
    }

    // -- §19.2 status-slice helpers ----------------------------------------

    #[test]
    fn queue_by_scope_splits_app_and_dataset() {
        let conn = mem_conn();
        ensure_sync_tables(&conn).unwrap();
        // One app-scope row (`xp`) and one dataset-scope row (`card`), distinct
        // `edit_time`. `queue_by_scope` splits on `is_app_scope_kind`.
        conn.execute(
            "INSERT INTO writeback_queue (id, kind, dataset_uuid, payload, queued_at, edit_time) \
             VALUES ('q1', 'xp', 'app', '{}', '2026-01-01 00:00:00', '2026-01-05 00:00:00')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO writeback_queue (id, kind, dataset_uuid, payload, queued_at, edit_time) \
             VALUES ('q2', 'card', 'ds-1', '{}', '2026-01-01 00:00:00', '2026-01-03 00:00:00')",
            [],
        )
        .unwrap();
        let (app, dataset) = queue_by_scope(&conn);
        assert_eq!(app.depth, 1);
        assert_eq!(dataset.depth, 1);
        assert_eq!(app.oldest_edit_time.as_deref(), Some("2026-01-05 00:00:00"));
        assert_eq!(dataset.oldest_edit_time.as_deref(), Some("2026-01-03 00:00:00"));

        // A second, older app-scope row moves the app oldest back (min stamp);
        // the dataset scope is untouched.
        conn.execute(
            "INSERT INTO writeback_queue (id, kind, dataset_uuid, payload, queued_at, edit_time) \
             VALUES ('q3', 'dictation', 'app', '{}', '2026-01-01 00:00:00', '2026-01-02 00:00:00')",
            [],
        )
        .unwrap();
        let (app, dataset) = queue_by_scope(&conn);
        assert_eq!(app.depth, 2);
        assert_eq!(app.oldest_edit_time.as_deref(), Some("2026-01-02 00:00:00"));
        assert_eq!(dataset.depth, 1);

        // An empty queue reports zero depth and no stamp.
        let empty = mem_conn();
        ensure_sync_tables(&empty).unwrap();
        let (a, d) = queue_by_scope(&empty);
        assert_eq!(a.depth, 0);
        assert!(a.oldest_edit_time.is_none());
        assert_eq!(d.depth, 0);
        assert!(d.oldest_edit_time.is_none());
    }

    #[test]
    fn last_round_round_trips() {
        let conn = mem_conn();
        ensure_sync_tables(&conn).unwrap();
        // A fresh DB has no persisted round.
        let (v, at) = last_round(&conn);
        assert!(v.is_none());
        assert!(at.is_none());

        let result = json!({"status": "ok", "applied": 3});
        set_last_round(&conn, &result).unwrap();
        let (v, at) = last_round(&conn);
        assert_eq!(v, Some(result));
        assert!(at.is_some(), "a stamp is written alongside the JSON");

        // Upsert: a second round replaces the first and stays a single row.
        set_last_round(&conn, &json!({"status": "ok", "applied": 5})).unwrap();
        let (v, _) = last_round(&conn);
        assert_eq!(v.unwrap()["applied"].as_i64(), Some(5));
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM app_sync_state", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "single-row table stays single-row");
    }

    #[test]
    fn local_sync_entries_reads_provenance() {
        let conn = mem_conn();
        ensure_sync_tables(&conn).unwrap();
        conn.execute(
            "INSERT INTO dataset_sync_state \
             (dataset_uuid, overall_hash, synced_at, bytes, file_count, \"cursor\", \
              upstream_kind, upstream_origin, availability, published_version) \
             VALUES ('ds-1', 'h', '2026-01-01', 10, 2, 9, 'registry', 'https://x', 'listed', 7)",
            [],
        )
        .unwrap();
        let entries = local_sync_entries(&conn).unwrap();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.dataset_uuid, "ds-1");
        assert_eq!(e.cursor, Some(9));
        assert_eq!(e.upstream_kind, "registry");
        assert_eq!(e.upstream_origin, "https://x");
        assert_eq!(e.availability, "listed");
        assert_eq!(e.published_version, Some(7));
    }

    // -- §19.2 resync reason history ---------------------------------------

    #[test]
    fn classify_resync_reason_only_names_needs_resync() {
        // A recorded reason passes through verbatim when a resync is owed...
        assert_eq!(
            classify_resync_reason("needs_resync", Some("retention_gap".into())),
            Some("retention_gap".into())
        );
        // ...a NULL cursor with no recorded reason falls back to `no_cursor`...
        assert_eq!(
            classify_resync_reason("needs_resync", None),
            Some("no_cursor".into())
        );
        // ...and every other state carries no reason even if one is stored.
        assert_eq!(classify_resync_reason("downloaded", Some("retention_gap".into())), None);
        assert_eq!(classify_resync_reason("removed_on_hub", None), None);
        assert_eq!(classify_resync_reason("not_downloaded", None), None);
    }

    #[test]
    fn set_resync_reason_round_trips() {
        let conn = mem_conn();
        ensure_sync_tables(&conn).unwrap();
        insert_state(&conn, "ds-1", "desktop_hub", "https://pc", "listed");
        // A no-op on a row that does not exist; the write lands on one that does.
        set_resync_reason(&conn, "missing", "retention_gap").unwrap();
        set_resync_reason(&conn, "ds-1", "retention_gap").unwrap();
        let entries = local_sync_entries(&conn).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].resync_reason.as_deref(), Some("retention_gap"));
    }

    #[test]
    fn reset_cursors_without_provenance_records_reason() {
        let conn = mem_conn();
        ensure_sync_tables(&conn).unwrap();
        // A pre-provenance row: empty origin, live cursor. The migration NULLs the
        // cursor and names why, so the status surface can say "re-establishing".
        insert_state(&conn, "ds-old", "desktop_hub", "", "listed");
        conn.execute(
            "UPDATE dataset_sync_state SET \"cursor\" = 5 WHERE dataset_uuid = 'ds-old'",
            [],
        )
        .unwrap();
        // A row with real provenance is left alone.
        insert_state(&conn, "ds-new", "registry", "https://x", "listed");
        conn.execute(
            "UPDATE dataset_sync_state SET \"cursor\" = 7 WHERE dataset_uuid = 'ds-new'",
            [],
        )
        .unwrap();

        let n = reset_cursors_without_provenance(&conn).unwrap();
        assert_eq!(n, 1, "only the pre-provenance row is reset");
        let entries = local_sync_entries(&conn).unwrap();
        let old = entries.iter().find(|e| e.dataset_uuid == "ds-old").unwrap();
        let new = entries.iter().find(|e| e.dataset_uuid == "ds-new").unwrap();
        assert_eq!(old.cursor, None);
        assert_eq!(old.resync_reason.as_deref(), Some("provenance_reset"));
        assert_eq!(new.cursor, Some(7));
        assert_eq!(new.resync_reason, None);
    }

    // -- INV-14 LWW drop (§19.2 last open item) ----------------------------

    /// Insert one writeback-queue row with an explicit `object_id` + `edit_time`,
    /// the two columns `pending_local_edit` matches/reads on.
    fn insert_queue_row(
        conn: &Connection,
        id: &str,
        kind: &str,
        scope: &str,
        object_id: &str,
        edit_time: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO writeback_queue \
             (id, kind, dataset_uuid, payload, queued_at, edit_time, object_id) \
             VALUES (?1, ?2, ?3, '{}', datetime('now'), ?4, ?5)",
            params![id, kind, scope, edit_time, object_id],
        )
        .unwrap();
    }

    #[test]
    fn lww_outcome_drops_only_when_pulled_strictly_newer() {
        // Pulled strictly newer than the pending local edit -> drop + apply.
        assert_eq!(
            lww_outcome(Some("2026-01-05T00:00:00.000Z"), Some("2026-01-03T00:00:00.000Z")),
            LwwOutcome::DropAndApply
        );
        // Pulled older -> protect the local edit.
        assert_eq!(
            lww_outcome(Some("2026-01-01T00:00:00.000Z"), Some("2026-01-03T00:00:00.000Z")),
            LwwOutcome::Skip
        );
        // Equal -> not strictly newer -> protect.
        assert_eq!(
            lww_outcome(Some("2026-01-03T00:00:00.000Z"), Some("2026-01-03T00:00:00.000Z")),
            LwwOutcome::Skip
        );
        // Either stamp missing -> protect (no silent loss on a legacy row).
        assert_eq!(lww_outcome(None, Some("2026-01-03T00:00:00.000Z")), LwwOutcome::Skip);
        assert_eq!(lww_outcome(Some("2026-01-05T00:00:00.000Z"), None), LwwOutcome::Skip);
        // Offsets are honoured as instants, not compared as text: 07:12:34Z is
        // the same instant as 15:12:34+08:00, so neither is strictly newer.
        assert_eq!(
            lww_outcome(
                Some("2026-01-03T07:12:34.000Z"),
                Some("2026-01-03T15:12:34.000+08:00")
            ),
            LwwOutcome::Skip
        );
    }

    #[test]
    fn pending_local_edit_reads_the_queued_stamp() {
        let conn = mem_conn();
        ensure_sync_tables(&conn).unwrap();
        // An empty object_id never guards (bulk-insert kinds have no row identity).
        insert_queue_row(&conn, "q0", "book_sentences_save", "ds-1", "", Some("2026-01-03T00:00:00.000Z"));
        assert_eq!(pending_local_edit(&conn, "ds-1", "book_sentences_save", ""), None);
        // No row for this key -> outer None.
        assert_eq!(pending_local_edit(&conn, "ds-1", "card_save", "missing"), None);
        // A queued row -> Some(Some(edit_time)).
        insert_queue_row(&conn, "q1", "card_save", "ds-1", "card-9", Some("2026-01-03T00:00:00.000Z"));
        assert_eq!(
            pending_local_edit(&conn, "ds-1", "card_save", "card-9"),
            Some(Some("2026-01-03T00:00:00.000Z".to_string()))
        );
        // A legacy row with a NULL edit_time -> Some(None) (row exists, unstamped).
        insert_queue_row(&conn, "q2", "card_save", "ds-1", "card-old", None);
        assert_eq!(pending_local_edit(&conn, "ds-1", "card_save", "card-old"), Some(None));
        // has_pending_local_edit delegates: true for a queued row, false for the
        // empty-object_id kind that never guards.
        assert!(has_pending_local_edit(&conn, "ds-1", "card_save", "card-9"));
        assert!(!has_pending_local_edit(&conn, "ds-1", "book_sentences_save", ""));
    }

    #[test]
    fn drop_pending_local_edit_removes_the_row_once() {
        let conn = mem_conn();
        ensure_sync_tables(&conn).unwrap();
        insert_queue_row(&conn, "q1", "card_save", "ds-1", "card-9", Some("2026-01-03T00:00:00.000Z"));
        // First drop removes the one queued row.
        assert_eq!(drop_pending_local_edit(&conn, "ds-1", "card_save", "card-9").unwrap(), 1);
        assert_eq!(pending_local_edit(&conn, "ds-1", "card_save", "card-9"), None);
        // A second drop is a no-op.
        assert_eq!(drop_pending_local_edit(&conn, "ds-1", "card_save", "card-9").unwrap(), 0);
        // An empty object_id never deletes (it would sweep unrelated bulk rows).
        insert_queue_row(&conn, "q2", "book_sentences_save", "ds-1", "", Some("2026-01-03T00:00:00.000Z"));
        assert_eq!(drop_pending_local_edit(&conn, "ds-1", "book_sentences_save", "").unwrap(), 0);
    }

    // -- §19.4 queue diagnosis ---------------------------------------------

    /// A healthy, fully-connected follower with an empty queue — each test
    /// perturbs one signal so the asserted cause is unambiguous.
    fn signals() -> QueueSignals {
        QueueSignals {
            hub_configured: true,
            hub_reachable: true,
            hub_paired: Some(true),
            app_advertised: true,
            queue_app_depth: 0,
            queue_dataset_depth: 0,
            any_needs_resync: false,
            last_round: None,
        }
    }

    #[test]
    fn round_error_strings_flattens_stages() {
        let lr = json!({"errors": [
            {"stage": "push", "scope": "@app", "error": "Host Does Not Advertise app_journal"},
            {"kind": "xp", "error": "boom"}
        ]});
        let s = round_error_strings(&lr);
        assert_eq!(s.len(), 2);
        assert!(s[0].contains("push") && s[0].contains("@app"));
        assert!(s[0].contains("does not advertise app_journal"), "lowercased: {}", s[0]);
        assert!(s[1].contains("xp") && s[1].contains("boom"));
        // No errors[] -> empty.
        assert!(round_error_strings(&json!({"status": "ok"})).is_empty());
    }

    #[test]
    fn diagnose_idle_when_nothing_queued() {
        let d = diagnose_queue(&signals());
        assert_eq!(d.cause, "idle");
        assert_eq!(d.cause_index, None);
        assert_eq!(d.severity, "ok");
    }

    #[test]
    fn diagnose_no_hub() {
        let mut s = signals();
        s.hub_configured = false;
        s.queue_app_depth = 2;
        let d = diagnose_queue(&s);
        assert_eq!(d.cause, "no_hub");
        assert_eq!(d.cause_index, None);
    }

    #[test]
    fn diagnose_unreachable_wins_over_scope() {
        let mut s = signals();
        s.hub_reachable = false;
        s.app_advertised = false; // would be cause 1 if the host were up
        s.queue_app_depth = 2;
        let d = diagnose_queue(&s);
        assert_eq!(d.cause, "unreachable");
        assert_eq!(d.cause_index, Some(4));
        assert_eq!(d.severity, "error");
    }

    #[test]
    fn diagnose_auth_from_paired_false() {
        let mut s = signals();
        s.hub_paired = Some(false);
        s.queue_dataset_depth = 2;
        let d = diagnose_queue(&s);
        assert_eq!(d.cause, "auth");
        assert_eq!(d.cause_index, Some(3));
    }

    #[test]
    fn diagnose_auth_from_401_error() {
        let mut s = signals();
        s.queue_dataset_depth = 2;
        s.last_round = Some(json!({"status": "error", "errors": [
            {"stage": "push", "error": "device not paired — pair this device with the hub first (401)"}
        ]}));
        let d = diagnose_queue(&s);
        assert_eq!(d.cause, "auth");
        assert_eq!(d.cause_index, Some(3));
    }

    #[test]
    fn diagnose_suspended_from_403_error() {
        let mut s = signals();
        s.queue_app_depth = 1;
        s.last_round = Some(json!({"status": "error", "errors": [
            {"stage": "push", "error": "this account cannot access '/api/v1/app/changes' (403)"}
        ]}));
        let d = diagnose_queue(&s);
        assert_eq!(d.cause, "suspended_or_quota");
        assert_eq!(d.cause_index, Some(2));
    }

    #[test]
    fn diagnose_scope_not_advertised() {
        let mut s = signals();
        s.app_advertised = false; // the website-today case (§16.2)
        s.queue_app_depth = 3;
        let d = diagnose_queue(&s);
        assert_eq!(d.cause, "scope_not_advertised");
        assert_eq!(d.cause_index, Some(1));
        assert_eq!(d.severity, "warn");
    }

    #[test]
    fn diagnose_resync_from_alarm_error() {
        let mut s = signals();
        s.queue_dataset_depth = 1;
        s.last_round = Some(json!({"status": "error", "errors": [
            {"scope": "@app", "stage": "alarm", "error": "hub seq 5 regressed below cursor 9; re-pulling from 0"}
        ]}));
        let d = diagnose_queue(&s);
        assert_eq!(d.cause, "resync_required");
        assert_eq!(d.cause_index, Some(5));
    }

    #[test]
    fn diagnose_resync_from_needs_resync() {
        let mut s = signals();
        s.any_needs_resync = true;
        s.queue_app_depth = 1;
        let d = diagnose_queue(&s);
        assert_eq!(d.cause, "resync_required");
        assert_eq!(d.cause_index, Some(5));
    }

    #[test]
    fn diagnose_draining() {
        let mut s = signals();
        s.queue_app_depth = 1;
        s.last_round = Some(json!({"status": "ok", "pushed": 3, "errors": []}));
        let d = diagnose_queue(&s);
        assert_eq!(d.cause, "draining");
        assert_eq!(d.cause_index, None);
        assert_eq!(d.severity, "ok");
    }

    #[test]
    fn diagnose_unknown() {
        let mut s = signals();
        s.queue_app_depth = 1; // healthy host, no last round, no signal
        let d = diagnose_queue(&s);
        assert_eq!(d.cause, "unknown");
        assert_eq!(d.cause_index, None);
        assert_eq!(d.severity, "warn");
    }

    #[test]
    fn reset_cursors_nulls_pre_migration_row_once() {
        let conn = mem_conn();
        ensure_sync_tables(&conn).unwrap();
        // Pre-migration copy: a real cursor but no recorded upstream.
        conn.execute(
            "INSERT INTO dataset_sync_state (dataset_uuid, overall_hash, synced_at, \"cursor\") \
             VALUES ('ds-old', 'h', '2026-01-01', 42)",
            [],
        )
        .unwrap();
        // A row that already knows its upstream must be left alone.
        insert_state(&conn, "ds-stamped", "desktop_hub", "http://lan-a", "listed");
        conn.execute(
            "UPDATE dataset_sync_state SET \"cursor\" = 7 WHERE dataset_uuid = 'ds-stamped'",
            [],
        )
        .unwrap();

        let n = reset_cursors_without_provenance(&conn).unwrap();
        assert_eq!(n, 1, "only the unstamped row is reset");
        let cur: Option<i64> = conn
            .query_row("SELECT \"cursor\" FROM dataset_sync_state WHERE dataset_uuid = 'ds-old'", [], |r| r.get(0))
            .unwrap();
        assert!(cur.is_none(), "pre-migration cursor is NULLed");
        let kept: Option<i64> = conn
            .query_row("SELECT \"cursor\" FROM dataset_sync_state WHERE dataset_uuid = 'ds-stamped'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, Some(7), "a stamped row keeps its cursor");
        // Idempotent: a second pass finds nothing to reset.
        assert_eq!(reset_cursors_without_provenance(&conn).unwrap(), 0);
    }

    #[test]
    fn upstream_of_classifies_hub_kind() {
        let (k, o) = upstream_of("http://192.168.1.50:8756/");
        assert_eq!(k, "desktop_hub");
        assert_eq!(o, "http://192.168.1.50:8756", "trailing slash is trimmed");
        let website = crate::auth::base_url();
        let (k2, o2) = upstream_of(&website);
        assert_eq!(k2, "cloud_hub");
        assert_eq!(o2, website.trim_end_matches('/'));
    }

    #[test]
    fn stamp_app_provenance_only_fills_empty() {
        let conn = mem_conn();
        ensure_sync_tables(&conn).unwrap();
        assert!(app_provenance(&conn).is_none());
        assert!(stamp_app_provenance(&conn, "desktop_hub", "http://a").unwrap());
        assert_eq!(
            app_provenance(&conn),
            Some(("desktop_hub".to_string(), "http://a".to_string()))
        );
        // INV-19: a round against a different host must not rewrite the stamp.
        assert!(!stamp_app_provenance(&conn, "cloud_hub", "http://b").unwrap());
        assert_eq!(app_provenance(&conn).unwrap().1, "http://a");
    }

    // -- prune scoping (INV-7 / INV-22) -------------------------------------

    #[test]
    fn prune_refuses_foreign_upstream() {
        let (settings, dir) = temp_settings("prune-foreign");
        let uuid = "ds-foreign";
        let target = type_root(&settings, "dictation").join(uuid);
        std::fs::create_dir_all(&target).unwrap();
        let conn = mem_conn();
        ensure_sync_tables(&conn).unwrap();
        insert_state(&conn, uuid, "desktop_hub", "http://lan-a", "listed");

        let out = prune_local_dataset(&settings, &conn, uuid, ("cloud_hub", "http://website"));
        assert_eq!(out, PruneOutcome::SkippedProvenance);
        assert!(target.exists(), "a foreign-upstreamed copy survives absence");
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM dataset_sync_state WHERE dataset_uuid = ?1", params![uuid], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_deletes_matching_upstream() {
        let (settings, dir) = temp_settings("prune-match");
        let uuid = "ds-match";
        let target = type_root(&settings, "dictation").join(uuid);
        std::fs::create_dir_all(&target).unwrap();
        let conn = mem_conn();
        ensure_sync_tables(&conn).unwrap();
        insert_state(&conn, uuid, "desktop_hub", "http://lan-a", "listed");

        let out = prune_local_dataset(&settings, &conn, uuid, ("desktop_hub", "http://lan-a"));
        assert_eq!(out, PruneOutcome::Deleted);
        assert!(!target.exists(), "the matching copy is removed");
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM dataset_sync_state WHERE dataset_uuid = ?1", params![uuid], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_never_deletes_withdrawn() {
        let (settings, dir) = temp_settings("prune-withdrawn");
        let uuid = "ds-withdrawn";
        let target = type_root(&settings, "dictation").join(uuid);
        std::fs::create_dir_all(&target).unwrap();
        let conn = mem_conn();
        ensure_sync_tables(&conn).unwrap();
        insert_state(&conn, uuid, "desktop_hub", "http://lan-a", "withdrawn");

        // Provenance matches, but INV-22 forbids deleting a withdrawn copy.
        let out = prune_local_dataset(&settings, &conn, uuid, ("desktop_hub", "http://lan-a"));
        assert_eq!(out, PruneOutcome::SkippedWithdrawn);
        assert!(target.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_missing_row_is_missing() {
        let (settings, dir) = temp_settings("prune-missing");
        let conn = mem_conn();
        ensure_sync_tables(&conn).unwrap();
        let out = prune_local_dataset(&settings, &conn, "nope", ("desktop_hub", "http://lan-a"));
        assert_eq!(out, PruneOutcome::Missing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- card-sync routing (INV-19) -----------------------------------------

    #[test]
    fn card_sync_target_routes_cloud_hub_to_website() {
        let (settings, dir) = temp_settings("cst-web");
        let website = crate::auth::base_url();
        settings.settings.lock().unwrap().pc_url = website.clone();
        let (ck, co) = upstream_of(&website);
        let uuid = "dataset-web";
        {
            let conn = datasets::dictation::open_app_db(&settings).unwrap();
            ensure_sync_tables(&conn).unwrap();
            insert_state(&conn, uuid, ck, &co, "listed");
        }
        match card_sync_target(&settings, uuid).unwrap() {
            CardSyncTarget::Website { origin } => assert_eq!(origin, co),
            other => panic!("expected Website, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn card_sync_target_skips_desktop_hub() {
        let (settings, dir) = temp_settings("cst-lan");
        settings.settings.lock().unwrap().pc_url = "http://192.168.1.50:8756".to_string();
        let uuid = "dataset-lan";
        {
            let conn = datasets::dictation::open_app_db(&settings).unwrap();
            ensure_sync_tables(&conn).unwrap();
            insert_state(&conn, uuid, "desktop_hub", "http://192.168.1.50:8756", "listed");
        }
        // A LAN-upstreamed dataset travels through the dataset journal, not the
        // website card round trip (§3 fact 3).
        assert!(matches!(
            card_sync_target(&settings, uuid).unwrap(),
            CardSyncTarget::Skip { .. }
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn card_sync_target_adopts_connected_when_unstamped() {
        let (settings, dir) = temp_settings("cst-adopt");
        settings.settings.lock().unwrap().pc_url = "http://192.168.1.50:8756".to_string();
        let uuid = "dataset-fresh";
        {
            let conn = datasets::dictation::open_app_db(&settings).unwrap();
            ensure_sync_tables(&conn).unwrap();
            // Pre-migration row: no provenance recorded.
            conn.execute(
                "INSERT INTO dataset_sync_state (dataset_uuid, overall_hash, synced_at) \
                 VALUES (?1, '', datetime('now'))",
                params![uuid],
            )
            .unwrap();
        }
        assert!(matches!(
            card_sync_target(&settings, uuid).unwrap(),
            CardSyncTarget::Skip { .. }
        ));
        // The fallback stamped the connected host as this dataset's upstream.
        let conn = datasets::dictation::open_app_db(&settings).unwrap();
        let (k, o): (String, String) = conn
            .query_row(
                "SELECT upstream_kind, upstream_origin FROM dataset_sync_state WHERE dataset_uuid = ?1",
                params![uuid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(k, "desktop_hub");
        assert_eq!(o, "http://192.168.1.50:8756");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn card_sync_target_refuses_disagreement() {
        let (settings, dir) = temp_settings("cst-refuse");
        // Connected host is a LAN desktop hub...
        settings.settings.lock().unwrap().pc_url = "http://192.168.1.50:8756".to_string();
        // ...but the dataset is recorded against the website. INV-19: refuse.
        let website = crate::auth::base_url();
        let (ck, co) = upstream_of(&website);
        let uuid = "dataset-split";
        {
            let conn = datasets::dictation::open_app_db(&settings).unwrap();
            ensure_sync_tables(&conn).unwrap();
            insert_state(&conn, uuid, ck, &co, "listed");
        }
        let err = card_sync_target(&settings, uuid).unwrap_err();
        assert!(err.contains("INV-19"), "error should name INV-19: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- phase-2 capability negotiation (sync-v2.md §7, INV-12 / INV-20) -----

    fn cluster_with(features: &[&str], protocol: u32) -> crate::sync::ClusterInfo {
        crate::sync::ClusterInfo {
            authority_kind: "desktop_hub".to_string(),
            cluster_id: "c1".to_string(),
            epoch: 0,
            protocol_version: protocol,
            features: features.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn advertises_default_denies_a_host_with_no_cluster_page() {
        // INV-20: no `/cluster` page means the host advertises nothing.
        assert!(!advertises(&None, "app_journal"));
        assert!(!advertises(&None, "dataset_journal"));
    }

    #[test]
    fn advertises_reads_the_feature_list() {
        let c = Some(cluster_with(&["dataset_journal", "app_journal"], 5));
        assert!(advertises(&c, "app_journal"));
        assert!(advertises(&c, "dataset_journal"));
        assert!(!advertises(&c, "registry_reads"), "an unlisted feature is not advertised");
    }

    #[test]
    fn app_journal_advertised_is_capability_first() {
        let lan = "http://192.168.1.50:8756";
        // A `/cluster` page that lists app_journal → run, whatever the protocol.
        let yes = Some(cluster_with(&["app_journal"], 5));
        assert!(app_journal_advertised(&yes, lan, Some(5)));
        // A `/cluster` page that omits it → refuse. The protocol fallback only
        // applies when there is no page at all, so a host that served one and
        // did not list the scope is taken at its word.
        let no = Some(cluster_with(&["dataset_journal"], 5));
        assert!(!app_journal_advertised(&no, lan, Some(5)));
    }

    #[test]
    fn app_journal_advertised_lan_falls_back_to_protocol() {
        let lan = "http://192.168.1.50:8756";
        // No `/cluster` (a hub that predates the route) + LAN + new-enough
        // protocol → run, preserving today's behavior during rollout.
        assert!(app_journal_advertised(&None, lan, Some(APP_SCOPE_PROTOCOL)));
        // Same host, protocol too old → skip.
        assert!(!app_journal_advertised(&None, lan, Some(APP_SCOPE_PROTOCOL - 1)));
        // Protocol unknown → skip.
        assert!(!app_journal_advertised(&None, lan, None));
    }

    #[test]
    fn app_journal_advertised_denies_the_website_until_it_advertises() {
        let website = crate::auth::base_url();
        // Today's website: no `/cluster`, no `/status` → skip (unchanged).
        assert!(!app_journal_advertised(&None, website.as_str(), None));
        // The protocol fallback is LAN-only, so a high version never turns the
        // website on by itself.
        assert!(!app_journal_advertised(&None, website.as_str(), Some(APP_SCOPE_PROTOCOL)));
        // Phase 7a: once the website advertises app_journal, the same gate runs.
        let cloud = Some(cluster_with(&["app_journal"], 5));
        assert!(app_journal_advertised(&cloud, website.as_str(), Some(5)));
    }

    #[test]
    fn dataset_journal_advertised_always_true_for_a_lan_hub() {
        // A LAN hub has always served `/api/v1/datasets/{uuid}/changes`, so the
        // dataset journal is on regardless of the `/cluster` page or features.
        let lan = "http://192.168.1.50:8756";
        assert!(dataset_journal_advertised(&None, lan));
        assert!(dataset_journal_advertised(
            &Some(cluster_with(&["app_journal"], 5)),
            lan
        ));
    }

    #[test]
    fn dataset_journal_advertised_gated_on_website_feature() {
        let website = crate::auth::base_url();
        // Today's website (no `/cluster`) advertises nothing → deny (INV-20).
        assert!(!dataset_journal_advertised(&None, website.as_str()));
        // A mirror-only account: app_journal but no dataset_journal → dataset denied.
        let mirror = Some(cluster_with(&["app_journal"], 6));
        assert!(!dataset_journal_advertised(&mirror, website.as_str()));
        // Phase 7b: once the account hosts an authority scope, it turns on.
        let auth = Some(cluster_with(&["app_journal", "dataset_journal"], 6));
        assert!(dataset_journal_advertised(&auth, website.as_str()));
    }

    #[test]
    fn cluster_info_json_round_trips() {
        let c = cluster_with(&["dataset_journal", "app_journal"], 5);
        let json = serde_json::to_value(&c).unwrap();
        for k in ["authority_kind", "cluster_id", "epoch", "protocol_version", "features"] {
            assert!(json.get(k).is_some(), "payload is missing {k}");
        }
        let back: crate::sync::ClusterInfo = serde_json::from_value(json).unwrap();
        assert_eq!(back.authority_kind, "desktop_hub");
        assert_eq!(back.epoch, 0, "epoch is the phase-2 placeholder");
        assert_eq!(back.protocol_version, 5);
        assert_eq!(
            back.features,
            vec!["dataset_journal".to_string(), "app_journal".to_string()]
        );
    }

    // -- phase-3 registry adoption (sync-v2.md §6.1 / §8.4 / §16.4) ----------

    #[test]
    fn adoption_target_website_adopts_into_download_root() {
        let (settings, dir) = temp_settings("adopt-web");
        let website = crate::auth::base_url();
        let (root, kind) = adoption_target(&settings, &website, "dictation");
        assert_eq!(kind, "registry", "a website download is a registry adoption");
        assert_eq!(
            root,
            datasets::download_root(&settings, datasets::DatasetType::Dictation),
            "adoption installs under the read-only download root"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adoption_target_lan_mirrors_into_type_root() {
        let (settings, dir) = temp_settings("adopt-lan");
        let (root, kind) = adoption_target(&settings, "http://192.168.1.50:8756", "dictation");
        assert_eq!(kind, "desktop_hub", "a LAN pull is a cluster mirror");
        assert_eq!(
            root,
            type_root(&settings, "dictation"),
            "a mirror installs into the writable type root"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_skips_registry_adoption_against_website_catalog() {
        let (settings, dir) = temp_settings("prune-registry");
        let uuid = "ds-adopted";
        let website = crate::auth::base_url();
        let (_, origin) = upstream_of(&website);
        let base = datasets::download_root(&settings, datasets::DatasetType::Dictation).join(uuid);
        write_dataset(&base, uuid, datasets::DatasetType::Dictation, "Adopted");
        let conn = mem_conn();
        ensure_sync_tables(&conn).unwrap();
        insert_state(&conn, uuid, "registry", &origin, "listed");
        // The website round reports itself as `cloud_hub`; a `registry` adoption
        // never matches, so absence from the visibility-filtered catalog is not
        // deletion (INV-7). This is the phase-1 SkippedProvenance floor, extended
        // to the phase-3 registry kind.
        let out = prune_local_dataset(&settings, &conn, uuid, ("cloud_hub", &origin));
        assert_eq!(out, PruneOutcome::SkippedProvenance);
        assert!(base.exists(), "an adopted registry copy survives catalog absence");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn removal_sweeps_download_root_copy() {
        let (settings, dir) = temp_settings("sweep-download");
        let uuid = "ds-sweep";
        // An adopted base lives under the download root, which the old sweep
        // (type roots only) would have missed — §3.
        let base = datasets::download_root(&settings, datasets::DatasetType::Dictation).join(uuid);
        write_dataset(&base, uuid, datasets::DatasetType::Dictation, "Sweep");
        let conn = mem_conn();
        ensure_sync_tables(&conn).unwrap();
        insert_state(&conn, uuid, "registry", "http://website", "listed");
        remove_local_dataset_unchecked(&settings, &conn, uuid);
        assert!(!base.exists(), "an adopted download-root copy is swept on removal");
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM dataset_sync_state WHERE dataset_uuid = ?1", params![uuid], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn evict_refuses_a_writable_copy() {
        let (settings, dir) = temp_settings("evict-refuse");
        let uuid = "ds-writable";
        // A mirror in the writable type root is not an adoption — eviction must
        // refuse it and point at the Datasets page (§16.4).
        let target = type_root(&settings, "dictation").join(uuid);
        write_dataset(&target, uuid, datasets::DatasetType::Dictation, "Writable");
        let err = evict_dataset_inner(&settings, uuid).unwrap_err();
        assert!(err.contains("adopted"), "error should explain only adopted copies evict: {err}");
        assert!(target.exists(), "a refused copy is untouched");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn evict_deletes_base_and_clears_hash_keeping_provenance() {
        let (settings, dir) = temp_settings("evict-ok");
        let uuid = "ds-evict";
        let base = datasets::download_root(&settings, datasets::DatasetType::Dictation).join(uuid);
        write_dataset(&base, uuid, datasets::DatasetType::Dictation, "Evict");
        {
            let conn = datasets::dictation::open_app_db(&settings).unwrap();
            ensure_sync_tables(&conn).unwrap();
            conn.execute(
                "INSERT INTO dataset_sync_state \
                 (dataset_uuid, overall_hash, synced_at, bytes, file_count, \
                  upstream_kind, upstream_origin, published_version, availability) \
                 VALUES (?1, 'abc', datetime('now'), 4096, 3, 'registry', 'http://website', 7, 'listed')",
                params![uuid],
            )
            .unwrap();
        }
        let out = evict_dataset_inner(&settings, uuid).unwrap();
        assert_eq!(out["evicted"], json!(true));
        assert_eq!(out["freed_bytes"], json!(4096), "the recorded size is the freed figure");
        assert!(!base.exists(), "the adopted base directory is removed");
        // The row survives with provenance but an empty hash — the sentinel the
        // Status surface reads as "not on device"; progress/XP live in the app DB
        // and are untouched (INV-9).
        let conn = datasets::dictation::open_app_db(&settings).unwrap();
        let (hash, kind, pv): (String, String, Option<i64>) = conn
            .query_row(
                "SELECT overall_hash, upstream_kind, published_version \
                 FROM dataset_sync_state WHERE dataset_uuid = ?1",
                params![uuid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(hash, "", "eviction clears the hash so Status reports not-downloaded");
        assert_eq!(kind, "registry", "provenance is kept");
        assert_eq!(pv, Some(7), "published_version is kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_dataset_dir_write_forks_download_base_to_overlay() {
        let (settings, dir) = temp_settings("fork-overlay");
        let uuid = "ds-fork";
        let base = datasets::download_root(&settings, datasets::DatasetType::Dictation).join(uuid);
        write_dataset(&base, uuid, datasets::DatasetType::Dictation, "Forkable");
        // The approved edit policy: a write resolves to a writable overlay, never
        // the read-only base...
        let overlay = datasets::find_dataset_dir_write(&settings, uuid).unwrap();
        assert!(
            !datasets::is_under_download_root(&settings, &overlay),
            "the write target is outside the read-only download root"
        );
        assert!(
            overlay.starts_with(type_root(&settings, "dictation")),
            "the overlay lands in the writable type root: {}",
            overlay.display()
        );
        // ...and the base stays pristine and re-downloadable.
        assert!(base.join("info.json").exists(), "the base is left intact");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
