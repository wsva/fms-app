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
fn adopt_or_verify_cluster(settings: &SettingsState, hub_cluster: &str) -> Result<(), String> {
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
fn resolve_pc_url(settings: &SettingsState, override_url: Option<&str>) -> String {
    match override_url.map(str::trim).filter(|s| !s.is_empty()) {
        Some(u) => u.trim_end_matches('/').to_string(),
        None => pc_base(settings),
    }
}

// ---------------------------------------------------------------------------
// Device identity + request signing (pairing v1, mirrors `pairing.rs` server-side)
// ---------------------------------------------------------------------------

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("hex string has odd length".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| "invalid hex".to_string()))
        .collect()
}

/// Loaded device identity: keypair + derived `device_id` / public key hex.
struct DeviceIdentity {
    key_pair: Ed25519KeyPair,
    device_id: String,
    pubkey_hex: String,
}

fn identity_from_seed(seed_hex: &str) -> Result<DeviceIdentity, String> {
    let doc = hex_decode(seed_hex)?;
    let key_pair = Ed25519KeyPair::from_pkcs8_maybe_unchecked(&doc)
        .map_err(|_| "device_seed is not a valid Ed25519 keypair".to_string())?;
    let pubkey_hex = hex_encode(key_pair.public_key().as_ref());
    let mut hasher = Sha256::new();
    hasher.update(key_pair.public_key().as_ref());
    let device_id = hex_encode(&hasher.finalize())[..16].to_string();
    Ok(DeviceIdentity {
        key_pair,
        device_id,
        pubkey_hex,
    })
}

/// Lazily generate + persist the device keypair on first use. The identity is
/// the credential; pairing merely registers its public key on the PC.
fn ensure_device_identity(settings: &SettingsState) -> Result<DeviceIdentity, String> {
    let existing = {
        let s = settings.settings.lock().unwrap();
        (s.device_seed.clone(), s.device_id.clone())
    };
    if !existing.0.is_empty() {
        return identity_from_seed(&existing.0);
    }
    let der = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
        .map_err(|e| e.to_string())?
        .as_ref()
        .to_vec();
    let seed_hex = hex_encode(&der);
    let ident = identity_from_seed(&seed_hex)?;
    {
        let mut s = settings.settings.lock().unwrap();
        s.device_seed = seed_hex;
        s.device_id = ident.device_id.clone();
    }
    let ws_dir = settings.workspace_dir.lock().unwrap().clone();
    let snapshot = settings.settings.lock().unwrap().clone();
    settings::SettingsState::save(&snapshot, ws_dir.as_ref())?;
    Ok(ident)
}

/// The device id this machine signs with — the id the hub records as a message's
/// `sender_device`, so the phone's chat mirror can decide which bubbles are its
/// own without a round-trip. Ensures the identity exists (persisting it) on call.
pub(crate) fn local_device_id(settings: &SettingsState) -> Result<String, String> {
    Ok(ensure_device_identity(settings)?.device_id)
}

/// Percent-encode one query component per RFC 3986 (only unreserved bytes pass
/// through). Kept hand-rolled so the string we sign and the string we splice
/// into the URL are byte-for-byte identical — `reqwest`'s own `.query()` encoder
/// would reorder/re-encode and break the v2 signature.
pub(crate) fn qenc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Build a deterministic `k1=v1&k2=v2` query string (each component RFC 3986
/// encoded) from ordered pairs. Empty when there are no pairs — callers append
/// it to the URL verbatim and sign the exact same string.
pub(crate) fn build_query(pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", qenc(k), qenc(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// sha256 hex of `query ++ b"\n" ++ body`, the payload component of the v2
/// signed message (§3.3 signature hardening). `body` is the exact bytes that
/// will be sent; for streaming multipart uploads (`/chat/message`) both ends
/// pass an empty slice and rely on the message uuid for replay-safety.
pub(crate) fn payload_hash(query: &str, body: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(query.as_bytes());
    h.update([b'\n']);
    h.update(body);
    format!("{:x}", h.finalize())
}

/// The canonical signed preimage. v2 (this build) folds in the payload hash so a
/// captured request cannot be replayed with a mutated body or query.
pub(crate) fn signed_message(ts: u64, method: &str, path: &str, query: &str, body: &[u8]) -> String {
    format!("{}\n{}\n{}\n{}", ts, method.to_uppercase(), path, payload_hash(query, body))
}

/// Add `x-fms-device` / `x-fms-ts` / `x-fms-sig` headers to an outbound PC
/// request. `path` must be the URL path *as the PC sees it* (including the
/// `/api/v1` prefix) and `query` the exact (pre-encoded) query string appended
/// to the URL — both are part of the signed message. `body` must be the exact
/// bytes sent; callers embed it with `.body(body.to_vec())` so the signature
/// and the wire bytes never diverge.
pub(crate) fn with_device_auth(
    req: reqwest::RequestBuilder,
    settings: &SettingsState,
    method: &str,
    path: &str,
    query: &str,
    body: &[u8],
) -> Result<reqwest::RequestBuilder, String> {
    let ident = ensure_device_identity(settings)?;
    let (role, cluster_id) = {
        let s = settings.settings.lock().unwrap();
        (s.role.clone(), s.cluster_id.clone())
    };
    let role = if role.is_empty() { "follower".to_string() } else { role };
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let msg = signed_message(ts, method, path, query, body);
    let sig = ident.key_pair.sign(msg.as_bytes());
    Ok(req
        .header("x-fms-device", ident.device_id.clone())
        .header("x-fms-ts", ts.to_string())
        .header("x-fms-sig", hex_encode(sig.as_ref()))
        .header("x-fms-role", role)
        .header("x-fms-cluster", cluster_id)
        .header("x-fms-protocol", crate::sync::PROTOCOL_VERSION.to_string()))
}

/// Local root a dataset type syncs into: `<datasets>/<type>` (e.g.
/// `<datasets>/dictation`, `<datasets>/card`, `<datasets>/book`). Mirrors the
/// PC-side layout so every existing read path discovers the unpacked dataset.
fn type_root(settings: &SettingsState, dataset_type: &str) -> std::path::PathBuf {
    settings.datasets_dir().join(dataset_type)
}

fn ensure_sync_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS writeback_queue (
            id           TEXT PRIMARY KEY,
            kind         TEXT NOT NULL,
            dataset_uuid TEXT NOT NULL,
            payload      TEXT NOT NULL,
            queued_at    TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS dataset_sync_state (
            dataset_uuid TEXT PRIMARY KEY,
            overall_hash TEXT NOT NULL,
            synced_at    TEXT NOT NULL,
            bytes        INTEGER NOT NULL DEFAULT 0,
            file_count   INTEGER NOT NULL DEFAULT 0
        );",
    )
    .map_err(|e| e.to_string())?;
    // Migration (§3.2): promote `dataset_sync_state` with a nullable incremental
    // row-log cursor. Added with **no DEFAULT** so every pre-existing row stays
    // NULL — a NULL cursor means "this copy predates incremental sync", so the
    // first `/changes` pull is told `resync_required` and the follower takes one
    // fresh snapshot, after which the cursor is a real hub seq.
    let has_cursor: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('dataset_sync_state') WHERE name = 'cursor'")
        .and_then(|mut s| s.exists([]))
        .unwrap_or(false);
    if !has_cursor {
        conn.execute("ALTER TABLE dataset_sync_state ADD COLUMN \"cursor\" INTEGER", [])
            .map_err(|e| e.to_string())?;
    }
    // Migration (Phase 5, §3.4): promote `writeback_queue` with `edit_time` (the
    // device-local action stamp that drives later-wins conflicts on the hub) and
    // `object_id` (the row a *state* change mutates, so coalescing can fold
    // repeat edits to one row). Added nullable so pre-existing rows keep
    // working; `enqueue_change` backfills both on every new insert.
    let has_edit_time: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('writeback_queue') WHERE name = 'edit_time'")
        .and_then(|mut s| s.exists([]))
        .unwrap_or(false);
    if !has_edit_time {
        conn.execute("ALTER TABLE writeback_queue ADD COLUMN \"edit_time\" TEXT", [])
            .map_err(|e| e.to_string())?;
    }
    let has_object_id: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('writeback_queue') WHERE name = 'object_id'")
        .and_then(|mut s| s.exists([]))
        .unwrap_or(false);
    if !has_object_id {
        conn.execute("ALTER TABLE writeback_queue ADD COLUMN object_id TEXT", [])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Writeback queue (producer side, called from the mobile write commands)
// ---------------------------------------------------------------------------

/// Append a queued change to the writeback queue. Both mobile clients and a
/// desktop running `role = "follower"` funnel here via
/// [`crate::sync::change_log::commit_change`]; the hub role never enqueues. State kinds
/// coalesce (§3.4): a newer edit to the same `(kind, dataset, object_id)`
/// replaces the still-pending older one instead of stacking, while append-only
/// and counter kinds keep every distinct row.
pub fn enqueue_change(
    settings: &SettingsState,
    kind: &str,
    dataset_uuid: &str,
    payload: &Value,
) -> Result<(), String> {
    let conn = datasets::dictation::open_app_db(settings)?;
    ensure_sync_tables(&conn)?;
    let edit_time = chrono::Utc::now().to_rfc3339();
    let object_id = crate::sync::change_log::object_id_for(kind, payload);
    // Fold repeat edits to one mutable row so we neither ship stale intermediate
    // payloads nor grow the queue without bound between flushes. An empty
    // `object_id` (bulk inserts) is left uncoalesced by the guard.
    if crate::sync::change_log::ChangeClass::of(kind).coalescible() && !object_id.is_empty() {
        conn.execute(
            "DELETE FROM writeback_queue \
             WHERE kind = ?1 AND dataset_uuid = ?2 AND object_id = ?3",
            params![kind, dataset_uuid, &object_id],
        )
        .map_err(|e| e.to_string())?;
    }
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO writeback_queue \
         (id, kind, dataset_uuid, payload, queued_at, edit_time, object_id) \
         VALUES (?1, ?2, ?3, ?4, datetime('now'), ?5, ?6)",
        params![id, kind, dataset_uuid, payload.to_string(), &edit_time, &object_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Sum the `amount` of XP deltas still sitting un-acknowledged in the writeback
/// queue for `user_id` (§3.6 counter display). A follower *bakes* the delta into
/// its local `xp_user` at award time **and** enqueues it, so the local total is
/// already `hub_total + sum(pending)`; this count is therefore the informational
/// "not yet confirmed by the hub" overlay the UI/status surface shows, and it
/// drains on the flush ack (the queue entry is dropped once acknowledged).
/// Reading the queue directly keeps the source of truth single — no separate
/// pending ledger to drift.
pub fn pending_xp_delta(settings: &SettingsState, user_id: &str) -> Result<i64, String> {
    let conn = datasets::dictation::open_app_db(settings)?;
    ensure_sync_tables(&conn)?;
    let mut stmt = conn
        .prepare("SELECT payload FROM writeback_queue WHERE kind = 'xp'")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut total = 0i64;
    for row in rows {
        let raw = row.map_err(|e| e.to_string())?;
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            if v.get("user_id").and_then(|x| x.as_str()) == Some(user_id) {
                total += v.get("amount").and_then(|x| x.as_i64()).unwrap_or(0);
            }
        }
    }
    Ok(total)
}

// ---------------------------------------------------------------------------
// Flush (mobile -> PC), batched, idempotent, last-write-wins
// ---------------------------------------------------------------------------

/// Flush pending changes to the PC. Returns the number of changes acknowledged.
/// Safe to call repeatedly; failed items stay queued.
pub async fn writeback_flush_inner(settings: &SettingsState) -> Result<usize, String> {
    let base = pc_base(settings);
    if base.is_empty() {
        return Ok(0);
    }
    let client = reqwest::Client::new();
    let mut flushed = 0usize;

    loop {
        // 1. Read a batch (Connection not held across await).
        let batch: Vec<(String, String, String, String, String, String)> = {
            let conn = datasets::dictation::open_app_db(settings)?;
            ensure_sync_tables(&conn)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, kind, dataset_uuid, payload, queued_at, \
                        COALESCE(edit_time, '') \
                     FROM writeback_queue ORDER BY queued_at LIMIT 50",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                    ))
                })
                .map_err(|e| e.to_string())?;
            rows.filter_map(|r| r.ok()).collect()
        };
        if batch.is_empty() {
            break;
        }

        // 2. POST the batch.
        let changes: Vec<Value> = batch
            .iter()
            .map(|(id, kind, ds, payload, queued_at, edit_time)| {
                json!({
                    "id": id,
                    "kind": kind,
                    "dataset_uuid": if ds.is_empty() { Value::Null } else { json!(ds) },
                    "payload": serde_json::from_str::<Value>(payload).unwrap_or(Value::Null),
                    // Empty for a pre-Phase-5 queue row; the hub then treats it
                    // as "arrived now" (§3.4 clamp). Otherwise the device stamp.
                    "edit_time": edit_time,
                    "queued_at": queued_at,
                })
            })
            .collect();
        let body = json!({ "user_key": crate::auth::workspace_identity(settings), "changes": changes });

        // Serialize once and sign the exact bytes we put on the wire, so the
        // v2 signature (which covers the body, §3.3) and the request agree.
        let body_bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
        let req = with_device_auth(
            client.post(format!("{base}/api/v1/sync/changes")),
            settings,
            "POST",
            "/api/v1/sync/changes",
            "",
            &body_bytes,
        )?
        .header("content-type", "application/json")
        .body(body_bytes);
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let results: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            // Server error: keep everything queued and stop for now.
            return Err(format!("sync/changes failed: HTTP {}", status));
        }

        // 3. Drop acknowledged ids.
        let acked: Vec<String> = results
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false))
            .filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(String::from))
            .collect();

        if !acked.is_empty() {
            let conn = datasets::dictation::open_app_db(settings)?;
            ensure_sync_tables(&conn)?;
            for id in &acked {
                let _ = conn.execute("DELETE FROM writeback_queue WHERE id = ?1", [id]);
            }
            flushed += acked.len();
        }

        // No acknowledged items: we cannot make further progress this run.
        if acked.is_empty() {
            break;
        }
    }

    Ok(flushed)
}

#[derive(Serialize)]
pub struct SyncResult {
    pub updated: bool,
    pub hash: String,
    pub bytes: u64,
    pub file_count: usize,
}

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
    let base = pc_base(&settings);
    if base.is_empty() {
        return Err("No PC configured. Use Discover PC or set the PC address.".into());
    }
    let client = reqwest::Client::new();
    
    // 1. Manifest -> compare hash.
    let mreq = with_device_auth(
        client.get(format!("{base}/api/v1/datasets/{uuid}/manifest")),
        &settings,
        "GET",
        &format!("/api/v1/datasets/{uuid}/manifest"),
        "",
        b"",
    )?;
    let m: Value = mreq
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let hash = m["overall_hash"].as_str().unwrap_or("").to_string();
    let total_bytes = m["total_bytes"].as_u64().unwrap_or(0);
    let file_count = m["file_count"].as_u64().unwrap_or(0) as usize;
    // Which local root to unpack into; the PC reports it in the manifest.
    // Fall back to "dictation" for older PCs that predate the field.
    let dataset_type = m["dataset_type"].as_str().unwrap_or("dictation").to_string();

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
    let _ = writeback_flush_inner(&settings).await;

    // 3. Prepare temp paths under <datasets>/<type>.
    let root = type_root(&settings, &dataset_type);
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let tmp_dir = root.join(format!("{uuid}.tmp"));
    let final_dir = root.join(&uuid);
    let part = root.join(format!("{uuid}.tar.gz.part"));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let _ = std::fs::remove_file(&part);
    std::fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;

    // 4. Stream the tar.gz to disk, emitting progress.
    let sreq = with_device_auth(
        client.get(format!("{base}/api/v1/datasets/{uuid}/snapshot")),
        &settings,
        "GET",
        &format!("/api/v1/datasets/{uuid}/snapshot"),
        "",
        b"",
    )?;
    let resp = sreq
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;

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

    // 7. Record sync state (with the incremental cursor this snapshot carries).
    {
        let conn = datasets::dictation::open_app_db(&settings)?;
        ensure_sync_tables(&conn)?;
        conn.execute(
            "INSERT OR REPLACE INTO dataset_sync_state \
             (dataset_uuid, overall_hash, synced_at, bytes, file_count, \"cursor\") \
             VALUES (?1, ?2, datetime('now'), ?3, ?4, ?5)",
            params![&uuid, &hash, received as i64, file_count as i64, snapshot_seq],
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

/// Upload all pending writeback changes to the PC.
#[tauri::command]
pub async fn writeback_flush(settings: State<'_, SettingsState>) -> Result<usize, String> {
    writeback_flush_inner(&settings).await
}

/// Per-dataset local sync state, for the phone's dataset list.
#[derive(Serialize)]
pub struct SyncStateEntry {
    pub dataset_uuid: String,
    pub overall_hash: String,
    pub synced_at: String,
    pub bytes: i64,
    pub file_count: i64,
    /// Incremental row-log cursor (§3.2). `None` = a NULL cursor, i.e. this copy
    /// predates incremental sync and is due one resync before deltas apply.
    pub cursor: Option<i64>,
}

/// Read the local `dataset_sync_state` table (what has been pulled and when).
#[tauri::command]
pub async fn dataset_sync_state(
    settings: State<'_, SettingsState>,
) -> Result<Vec<SyncStateEntry>, String> {
    let conn = datasets::dictation::open_app_db(&settings)?;
    ensure_sync_tables(&conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT dataset_uuid, overall_hash, synced_at, bytes, file_count, \"cursor\" \
             FROM dataset_sync_state",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(SyncStateEntry {
                dataset_uuid: r.get(0)?,
                overall_hash: r.get(1)?,
                synced_at: r.get(2)?,
                bytes: r.get(3)?,
                file_count: r.get(4)?,
                cursor: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(|r| r.ok()).collect())
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

/// Aggregated local + hub sync state for the Status surface (§3.5).
#[derive(Serialize)]
pub struct SyncStatusDetail {
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
    } else {
        let probe = async {
            let client = reqwest::Client::builder()
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

    // 3. Everything below is synchronous SQLite / in-process reads.
    let role = settings.role();
    let cluster_id = settings.cluster_id();
    let device_id = local_device_id(settings).unwrap_or_default();

    let conn = datasets::dictation::open_app_db(settings)?;
    ensure_sync_tables(&conn)?;

    let mut local: Vec<SyncStateEntry> = {
        let mut stmt = conn
            .prepare(
                "SELECT dataset_uuid, overall_hash, synced_at, bytes, file_count, \"cursor\" \
                 FROM dataset_sync_state",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok(SyncStateEntry {
                    dataset_uuid: r.get(0)?,
                    overall_hash: r.get(1)?,
                    synced_at: r.get(2)?,
                    bytes: r.get(3)?,
                    file_count: r.get(4)?,
                    cursor: r.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok()).collect()
    };
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
    drop(conn);

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
            Some(_) if cursor.is_none() => "needs_resync",
            Some(_) => "downloaded",
        };
        let hub_seq = c["hub_seq"].as_i64();
        let hub_ahead = match (hub_seq, cursor) {
            (Some(seq), Some(c)) => seq > c,
            (Some(_), None) => true,
            _ => false,
        };
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
        });
    }
    for e in &local {
        if seen.contains(&e.dataset_uuid) {
            continue;
        }
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
        });
    }

    Ok(SyncStatusDetail {
        role,
        cluster_id,
        protocol_version: crate::sync::PROTOCOL_VERSION,
        device_id,
        hub,
        datasets,
        queued_count,
        chat_pending,
        devices,
    })
}

/// `#[tauri::command]` wrapper for the Status page on both desktop + mobile.
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
    let result = async {
        let client = reqwest::Client::builder()
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
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| e.to_string())?;
        let req = with_device_auth(
            client.get(format!("{base}/api/v1/datasets")),
            &settings,
            "GET",
            "/api/v1/datasets",
            "",
            b"",
        )?;
        let resp = req
            .send()
            .await
            .map_err(|e| format!("Cannot reach PC at {base}: {e}"))?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            let body: Value = resp.json().await.unwrap_or(Value::Null);
            let msg = body["error"].as_str().unwrap_or("device not paired");
            return Err(format!("{msg} (http://{base})"));
        }
        resp.error_for_status()
            .map_err(|e| e.to_string())?
            .json::<Value>()
            .await
            .map_err(|e| format!("Unexpected response from PC: {e}"))
    }
    .await;
    match &result {
        Ok(v) => log::info!("[sync] pc_list_datasets OK ({} bytes of JSON)", v.to_string().len()),
        Err(e) => log::error!("[sync] pc_list_datasets FAILED: {e}"),
    }
    result
}

/// `GET /api/v1/datasets/{uuid}/changes?after=S` — one incremental change-log
/// page from the hub (§3.2/§3.3). The query string is signed verbatim (v2
/// hardening). Returns the hub's `{ entries, pruned_up_to, hub_seq,
/// resync_required }` JSON. A follower refuses this against a hub reporting an
/// older `protocol_version` — enforced by the caller (Phase 4 round).
pub async fn pc_changes_since(
    settings: State<'_, SettingsState>,
    uuid: String,
    after: i64,
) -> Result<Value, String> {
    let base = pc_base(&settings);
    if base.is_empty() {
        return Err("pc_url is not set — use Settings › Discover PC".to_string());
    }
    let api_path = format!("/api/v1/datasets/{uuid}/changes");
    let qs = build_query(&[("after", after.to_string())]);
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let req = with_device_auth(
        client.get(format!("{base}{api_path}?{qs}")),
        &settings,
        "GET",
        &api_path,
        &qs,
        b"",
    )?;
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Cannot reach PC at {base}: {e}"))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        let msg = body["error"].as_str().unwrap_or("device not paired");
        return Err(format!("{msg} (http://{base})"));
    }
    resp.error_for_status()
        .map_err(|e| format!("/changes request failed: {e}"))?
        .json::<Value>()
        .await
        .map_err(|e| format!("Unexpected /changes response from PC: {e}"))
}

/// `GET /api/v1/file?dataset=D&path=P` — fetch one non-DB file from the hub
/// (§3.2). Streams the raw bytes back; callers splice them into the dataset
/// directory. The signed query matches the URL verbatim so a captured request
/// cannot be re-pointed at another path.
pub async fn pc_fetch_file(
    settings: State<'_, SettingsState>,
    uuid: String,
    path: String,
) -> Result<Vec<u8>, String> {
    let base = pc_base(&settings);
    if base.is_empty() {
        return Err("pc_url is not set — use Settings › Discover PC".to_string());
    }
    let api_path = "/api/v1/file";
    let qs = build_query(&[("dataset", uuid), ("path", path)]);
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    let req = with_device_auth(
        client.get(format!("{base}{api_path}?{qs}")),
        &settings,
        "GET",
        api_path,
        &qs,
        b"",
    )?;
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Cannot reach PC at {base}: {e}"))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        let msg = body["error"].as_str().unwrap_or("device not paired");
        return Err(format!("{msg} (http://{base})"));
    }
    let bytes = resp
        .error_for_status()
        .map_err(|e| format!("/file request failed: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("Failed to read /file body: {e}"))?;
    Ok(bytes.to_vec())
}

// ---------------------------------------------------------------------------
// Incremental sync round (§3.3): push → per-dataset /changes apply → file-drift
// snapshot → catalog prune. Reuses the snapshot path for any non-DB file change
// (media/vtt), and folds the DB-only row deltas in place with the row log.
// ---------------------------------------------------------------------------

/// Signed GET returning parsed JSON. `path` is the URL path (no query), `qs`
/// the exact pre-encoded query string appended to the URL and folded into the
/// v2 signature. Shared by the round's catalog + manifest reads.
async fn signed_get_json(settings: &SettingsState, path: &str, qs: &str) -> Result<Value, String> {
    let base = pc_base(settings);
    if base.is_empty() {
        return Err("pc_url is not set — use Settings › Discover PC".to_string());
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let url = if qs.is_empty() {
        format!("{base}{path}")
    } else {
        format!("{base}{path}?{qs}")
    };
    let req = with_device_auth(client.get(&url), settings, "GET", path, qs, b"")?;
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Cannot reach PC at {base}: {e}"))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        let b: Value = resp.json().await.unwrap_or(Value::Null);
        return Err(format!("{} (http://{base})", b["error"].as_str().unwrap_or("device not paired")));
    }
    resp.error_for_status()
        .map_err(|e| e.to_string())?
        .json::<Value>()
        .await
        .map_err(|e| e.to_string())
}

/// `GET /datasets?lite=1` — the hub's current dataset catalog (uuid → type).
async fn fetch_catalog(settings: &SettingsState) -> Result<Vec<Value>, String> {
    let v = signed_get_json(settings, "/api/v1/datasets", "lite=1").await?;
    Ok(v.as_array().cloned().unwrap_or_default())
}

/// `GET /datasets/{uuid}/manifest` — the hub's current non-DB `overall_hash`.
async fn fetch_manifest_hash(settings: &SettingsState, uuid: &str) -> Result<String, String> {
    let v = signed_get_json(
        settings,
        &format!("/api/v1/datasets/{uuid}/manifest"),
        "",
    )
    .await?;
    Ok(v["overall_hash"].as_str().unwrap_or("").to_string())
}

/// Apply one pulled `sync_log` entry to the local dataset DB by dispatching to
/// the same write commands the UI uses. MUST be wrapped in a
/// [`crate::sync::change_log::ApplyGuard`] by the caller so the write does not
/// re-enqueue/re-log (the change is arriving *from* the hub). Mirrors the hub's
/// `rest.rs::replay` dispatch, but cross-platform (this module compiles on
/// mobile too). Returns `Ok(false)` for an unknown kind (skipped, not fatal).
async fn apply_change(
    settings: &State<'_, SettingsState>,
    dataset_uuid: &str,
    kind: &str,
    payload: &Value,
    write_identity: &str,
) -> Result<bool, String> {
    use crate::datasets::book::{BookChapter, BookSentence, BookSentenceWord};
    use crate::datasets::cards::{Card, Tag};
    use crate::datasets::dictation::{ListenCue, ListenDictation};
    use crate::datasets::read_aloud::{ReadAttempt, ReadText};
    let ds = dataset_uuid.to_string();
    let str_field = |k: &str| -> Result<String, String> {
        payload
            .get(k)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("payload missing '{k}'"))
    };
    match kind {
        "cue_save" => {
            let cue: ListenCue = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            crate::datasets::dictation::listen_save_cue(settings.clone(), ds, cue).await.map(|_| true)
        }
        "cue_delete" => {
            let cue_uuid = payload
                .get("cue_uuid")
                .and_then(|v| v.as_str())
                .or_else(|| payload.as_str())
                .or_else(|| payload.get("uuid").and_then(|v| v.as_str()))
                .map(|s| s.to_string())
                .ok_or_else(|| "cue_delete missing uuid".to_string())?;
            crate::datasets::dictation::listen_delete_cue(settings.clone(), ds, cue_uuid).await.map(|_| true)
        }
        "dictation" => {
            let d: ListenDictation = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            crate::datasets::dictation::listen_save_dictation_as(settings.inner(), &ds, &d, write_identity)
                .await
                .map(|_| true)
        }
        "xp" => {
            let amount = payload.get("amount").and_then(|v| v.as_i64()).unwrap_or(0);
            let source = payload.get("source").and_then(|v| v.as_str()).unwrap_or("sync").to_string();
            let reference_id = payload
                .get("reference_id")
                .and_then(|v| v.as_str())
                .unwrap_or("sync")
                .to_string();
            let dsref = if ds.is_empty() { None } else { Some(ds.as_str()) };
            crate::xp::xp_award_internal(settings.inner(), write_identity, amount, &source, &reference_id, dsref)
                .map(|_| true)
                .map_err(|e| e.to_string())
        }
        "card_save" => {
            let card: Card = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            crate::datasets::cards::card_save(settings.clone(), ds, card).await.map(|_| true)
        }
        "card_delete" => {
            let card_uuid = str_field("card_uuid")?;
            crate::datasets::cards::card_delete(settings.clone(), ds, card_uuid).await.map(|_| true)
        }
        "card_review" => {
            let card_uuid = str_field("card_uuid")?;
            let quality = payload.get("quality").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            crate::datasets::cards::card_test_submit(settings.clone(), ds, card_uuid, quality).await.map(|_| true)
        }
        "card_tag_save" => {
            let tag: Tag = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            crate::datasets::cards::card_tag_save(settings.clone(), ds, tag).await.map(|_| true)
        }
        "card_tag_delete" => {
            let tag_uuid = str_field("tag_uuid")?;
            crate::datasets::cards::card_tag_delete(settings.clone(), ds, tag_uuid).await.map(|_| true)
        }
        "card_set_tags" => {
            let card_uuid = str_field("card_uuid")?;
            let tag_uuids: Vec<String> = payload
                .get("tag_uuids")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            crate::datasets::cards::card_set_tags(settings.clone(), ds, card_uuid, tag_uuids).await.map(|_| true)
        }
        "book_chapter_save" => {
            let chapter: BookChapter = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            crate::datasets::book::book_save_chapter(settings.clone(), ds, chapter).await.map(|_| true)
        }
        "book_chapter_delete" => {
            let uuid = str_field("uuid")?;
            crate::datasets::book::book_delete_chapter(settings.clone(), ds, uuid).await.map(|_| true)
        }
        "book_sentence_save" => {
            let sentence: BookSentence = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            crate::datasets::book::book_save_sentence(settings.clone(), ds, sentence).await.map(|_| true)
        }
        "book_sentences_save" => {
            let sentences: Vec<BookSentence> = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            crate::datasets::book::book_save_sentences(settings.clone(), ds, sentences).await.map(|_| true)
        }
        "book_sentence_delete" => {
            let uuid = str_field("uuid")?;
            crate::datasets::book::book_delete_sentence(settings.clone(), ds, uuid).await.map(|_| true)
        }
        "book_word_save" => {
            let word: BookSentenceWord = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            crate::datasets::book::book_save_word(settings.clone(), ds, word).await.map(|_| true)
        }
        "book_word_delete" => {
            let uuid = str_field("uuid")?;
            crate::datasets::book::book_delete_word(settings.clone(), ds, uuid).await.map(|_| true)
        }
        "read_text_save" => {
            let text: ReadText = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            crate::datasets::read_aloud::save_text_row(settings.inner(), &ds, &text).map(|_| true)
        }
        "read_text_delete" => {
            let uuid = str_field("uuid")?;
            crate::datasets::read_aloud::read_aloud_delete_text(settings.clone(), ds, uuid).await.map(|_| true)
        }
        "read_attempt_save" => {
            let mut attempt: ReadAttempt = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            // Per-user history: rebind to this device's identity, mirroring the
            // hub's `dictation` rule rather than trusting the payload.
            attempt.user_id = write_identity.to_string();
            crate::datasets::read_aloud::save_attempt_row(settings.inner(), &ds, &attempt).map(|_| true)
        }
        "read_attempt_delete" => {
            let uuid = str_field("uuid")?;
            crate::datasets::read_aloud::read_aloud_delete_attempt(settings.clone(), ds, uuid).await.map(|_| true)
        }
        other => {
            log::warn!("[sync_round] skipping unknown change kind '{other}'");
            Ok(false)
        }
    }
}

/// Conservative later-wins guard for Phase 4: when a local writeback entry for
/// the same (kind, dataset, object) is still un-pushed, the local edit wins and
/// the pulled row is skipped (it will be reconciled once our push lands and
/// echoes back). Phase 5 sharpens this with a real `edit_time` column.
fn has_pending_local_edit(conn: &Connection, dataset_uuid: &str, kind: &str, object_id: &str) -> bool {
    let mut stmt = match conn.prepare(
        "SELECT payload FROM writeback_queue WHERE kind = ?1 AND dataset_uuid = ?2",
    ) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let rows = match stmt.query_map(params![kind, dataset_uuid], |r| r.get::<_, String>(0)) {
        Ok(r) => r,
        Err(_) => return false,
    };
    for payload_str in rows.flatten() {
        let payload: Value = serde_json::from_str(&payload_str).unwrap_or(Value::Null);
        let local_obj = crate::sync::change_log::object_id_for(kind, &payload);
        if local_obj == object_id {
            return true;
        }
    }
    false
}

/// Remove a local dataset the hub no longer offers: delete its directory under
/// every type root and drop its `dataset_sync_state` row (prune-by-absence).
fn prune_local_dataset(settings: &SettingsState, conn: &Connection, uuid: &str) {
    for ty in ["dictation", "card", "book", "read_aloud"] {
        let dir = type_root(settings, ty).join(uuid);
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    let _ = conn.execute("DELETE FROM dataset_sync_state WHERE dataset_uuid = ?1", params![uuid]);
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
    prune_local_dataset(&settings, &conn, &uuid);
    log::info!("[sync] forgot local dataset {uuid}");
    Ok(())
}

/// `true` when the reachable hub's catalog still contains `uuid`. Any error
/// (offline, unpaired) is treated as "not listed" so the forget action stays
/// usable when the hub is gone.
async fn hub_lists_dataset(settings: &SettingsState, uuid: &str) -> bool {
    match pc_get_json(settings, "/api/v1/datasets", &[("lite", "1".to_string())]).await {
        Ok(v) => v
            .as_array()
            .map(|arr| arr.iter().any(|d| d["uuid"].as_str() == Some(uuid)))
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// Run one full incremental sync round against the connected hub. Triggers:
/// app start, after a local write, a timer, or a network change (§3.3).
pub async fn sync_round_inner(
    app: AppHandle,
    settings: State<'_, SettingsState>,
) -> Result<Value, String> {
    if pc_base(&settings).is_empty() {
        return Err("No PC configured. Use Discover PC or set the PC address.".into());
    }
    let mut errors: Vec<Value> = Vec::new();
    let mut resynced: Vec<String> = Vec::new();
    let mut pruned: Vec<String> = Vec::new();
    let mut applied = 0i64;

    // 1. Push local edits first so the hub holds our changes before we pull.
    let pushed = writeback_flush_inner(&settings).await.unwrap_or_else(|e| {
        errors.push(json!({ "stage": "push", "error": e }));
        0
    });

    // 2. Refresh the hub catalog (uuid -> type) with the cheap `lite` list.
    let mut catalog_ok = true;
    let catalog = fetch_catalog(&settings).await.unwrap_or_else(|e| {
        catalog_ok = false;
        errors.push(json!({ "stage": "catalog", "error": e }));
        Vec::new()
    });
    let mut catalog_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for d in &catalog {
        let u = d["uuid"].as_str().unwrap_or("").to_string();
        let t = d["dataset_type"].as_str().unwrap_or("").to_string();
        if !u.is_empty() {
            catalog_map.insert(u, t);
        }
    }

    // 3. Per already-synced dataset: incremental apply, or a full resync when
    //    the cursor is unknown / a prune gap is reported / files drifted.
    let local: Vec<(String, Option<i64>, String)> = {
        let conn = datasets::dictation::open_app_db(&settings)?;
        ensure_sync_tables(&conn)?;
        let mut stmt = conn
            .prepare("SELECT dataset_uuid, \"cursor\", overall_hash FROM dataset_sync_state")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.filter_map(|x| x.ok()).collect()
    };

    for (uuid, cursor_opt, stored_hash) in local {
        if !catalog_map.contains_key(&uuid) {
            if catalog_ok {
                let conn = datasets::dictation::open_app_db(&settings)?;
                prune_local_dataset(&settings, &conn, &uuid);
                pruned.push(uuid);
            }
            continue;
        }

        let after = cursor_opt.unwrap_or(-1);
        let page = match pc_changes_since(settings.clone(), uuid.clone(), after).await {
            Ok(p) => p,
            Err(e) => {
                errors.push(json!({ "uuid": uuid, "stage": "changes", "error": e }));
                continue;
            }
        };
        let resync_required = page["resync_required"].as_bool().unwrap_or(true);
        let hub_seq = page["hub_seq"].as_i64().unwrap_or(after);
        // Safety alarm (§3.5): a hub seq *below* our cursor means its journal
        // regressed (a reset / re-install / different cluster answering the same
        // address). Its deltas can no longer be trusted to be the continuation
        // of what we already applied, so resync rather than advance backwards.
        if let Some(c) = cursor_opt {
            if hub_seq < c {
                log::error!(
                    "[sync] {uuid}: hub_seq {hub_seq} < local cursor {c} — journal regressed, forcing resync"
                );
                errors.push(json!({ "uuid": &uuid, "stage": "alarm", "error": format!("hub seq {hub_seq} regressed below cursor {c}") }));
                match dataset_sync_snapshot(app.clone(), settings.clone(), uuid.clone()).await {
                    Ok(r) => {
                        if r.updated {
                            resynced.push(uuid);
                        }
                    }
                    Err(e) => errors.push(json!({ "uuid": uuid, "stage": "resync", "error": e })),
                }
                continue;
            }
        }
        if resync_required {
            // NULL/gap cursor: take one fresh snapshot (sets cursor + hash).
            match dataset_sync_snapshot(app.clone(), settings.clone(), uuid.clone()).await {
                Ok(r) => {
                    if r.updated {
                        resynced.push(uuid);
                    }
                }
                Err(e) => errors.push(json!({ "uuid": uuid, "stage": "resync", "error": e })),
            }
            continue;
        }

        let entries = page["entries"].as_array().cloned().unwrap_or_default();
        if !entries.is_empty() {
            let _guard = crate::sync::change_log::ApplyGuard::new();
            let conn = datasets::dictation::open_app_db(&settings)?;
            ensure_sync_tables(&conn)?;
            for ent in &entries {
                let kind = ent["kind"].as_str().unwrap_or("");
                let object_id = ent["object_id"].as_str().unwrap_or("");
                let user_key = ent["user_key"].as_str().unwrap_or("");
                let payload = ent["payload"].clone();
                if has_pending_local_edit(&conn, &uuid, kind, object_id) {
                    continue; // local un-pushed edit wins; reconciled on next echo
                }
                match apply_change(&settings, &uuid, kind, &payload, user_key).await {
                    Ok(true) => applied += 1,
                    Ok(false) => {}
                    Err(e) => errors.push(json!({ "uuid": uuid, "kind": kind, "error": e })),
                }
            }
            drop(_guard);
            // Advance the row cursor to the hub's high-water mark.
            let conn = datasets::dictation::open_app_db(&settings)?;
            ensure_sync_tables(&conn)?;
            let _ = conn.execute(
                "UPDATE dataset_sync_state SET \"cursor\" = ?2 WHERE dataset_uuid = ?1",
                params![uuid, hub_seq],
            );
        }

        // 4. Non-DB file drift (media/vtt/etc. change the manifest hash, which
        //    the row log does not cover): fall back to one snapshot re-pull.
        match fetch_manifest_hash(&settings, &uuid).await {
            Ok(h) if !h.is_empty() && h != stored_hash => {
                match dataset_sync_snapshot(app.clone(), settings.clone(), uuid.clone()).await {
                    Ok(_) => resynced.push(uuid),
                    Err(e) => errors.push(json!({ "uuid": uuid, "stage": "files", "error": e })),
                }
            }
            Err(e) => errors.push(json!({ "uuid": uuid, "stage": "manifest", "error": e })),
            _ => {}
        }
    }

    Ok(json!({
        "status": if errors.is_empty() { "ok" } else { "partial" },
        "pushed": pushed,
        "applied": applied,
        "resynced": resynced,
        "pruned": pruned,
        "errors": errors,
    }))
}

/// Tauri entry point for the round (Datasets Sync page + MCP).
#[tauri::command]
pub async fn sync_run_round(
    app: AppHandle,
    settings: State<'_, SettingsState>,
) -> Result<Value, String> {
    sync_round_inner(app, settings).await
}

// ---------------------------------------------------------------------------
// Pairing (phone side)
// ---------------------------------------------------------------------------

/// Best-effort human-readable device name for the PC's confirm dialog.
pub(crate) fn device_name() -> String {
    // Android: the product model, e.g. "Pixel 8". Static after first call.
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        let probe = |key: &str| {
            Command::new("getprop")
                .arg(key)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        };
        match (|| Some(format!("{} ({})", probe("ro.product.model")?, probe("ro.product.brand")?)))()
            .or_else(|| probe("ro.product.model"))
        {
            Some(m) => m,
            // Desktop (PC-to-PC pairing): name the machine, so the dialog says
            // *which* of the owner's computers is asking.
            None => std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .ok()
                .map(|h| h.trim().to_string())
                .filter(|h| !h.is_empty())
                .map(|h| format!("{} ({})", h, std::env::consts::OS))
                .unwrap_or_else(|| format!("{} device", std::env::consts::OS)),
        }
    }).clone()
}

/// The hub's advertised `cluster_id`, read from its unsigned `/status`. Empty on
/// any failure: a hub that predates the field, or one that is momentarily
/// unreachable, must not turn a completed pairing into an error.
async fn hub_cluster_id(client: &reqwest::Client, base: &str) -> String {
    match client.get(format!("{base}/api/v1/status")).send().await {
        Ok(resp) => resp
            .json::<Value>()
            .await
            .unwrap_or(Value::Null)
            .get("cluster_id")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string(),
        Err(_) => String::new(),
    }
}

/// Trust-on-first-use cluster binding on the **pairing** path (§3.1, §7): a
/// follower adopts the id of the hub that has just approved it, and refuses a hub
/// belonging to a different cluster instead of silently switching.
///
/// This hook is what makes the binding real. Adoption previously lived only
/// inside [`pc_check_status`], which no screen has called since the connect UI
/// moved out of Settings, so followers ran with an empty `x-fms-cluster`: the hub's
/// mis-pairing check never fired and nothing recorded which hub they belonged to.
/// Pairing has already succeeded by the time this runs, so a mismatch is returned
/// as a warning next to the approval rather than an error that would misreport the
/// pairing itself as failed.
async fn bind_cluster_after_pairing(
    settings: &SettingsState,
    client: &reqwest::Client,
    base: &str,
) -> Option<String> {
    let hub_cluster = hub_cluster_id(client, base).await;
    if hub_cluster.is_empty() {
        return None;
    }
    match adopt_or_verify_cluster(settings, &hub_cluster) {
        Ok(()) => None,
        Err(e) => {
            log::warn!("[sync] pair approved but cluster binding refused: {e}");
            Some(e)
        }
    }
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
    if base.is_empty() {
        return Err("No PC address — enter or discover the PC address first.".into());
    }
    let ident = ensure_device_identity(&settings)?;
    let client = reqwest::Client::builder()
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

// ---------------------------------------------------------------------------
// Remote wiki browsing (Android thin client)
// ---------------------------------------------------------------------------
// The wiki page on the phone is a pure read-through proxy: listing, reading,
// and searching all hit the paired PC's `/api/v1/wiki/*` REST endpoints over
// native reqwest (WebView `fetch()` would be CORS-blocked — see module docs),
// and nothing is ever stored locally. Compiled only on mobile; the desktop
// build serves the same data straight from `wiki::local_impl`.

/// Issue a signed GET against a PC `/api/v1/*` endpoint and return the raw
/// JSON value. The request path (excluding the query string) is what gets
/// signed, matching `zone_guard`'s verification on the PC. Shared by the wiki
/// and chat remote clients — on mobile always, and on the desktop when this
/// workspace runs `role = "follower"` (a demoted PC relays chat over REST).
#[cfg_attr(feature = "desktop", allow(dead_code))]
pub(crate) async fn pc_get_json(
    settings: &SettingsState,
    api_path: &str,
    query: &[(&str, String)],
) -> Result<Value, String> {
    let base = pc_base(settings);
    if base.is_empty() {
        return Err("pc_url is not set — use Settings › Discover PC".to_string());
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let qs = build_query(query);
    let url = if qs.is_empty() {
        format!("{base}{api_path}")
    } else {
        format!("{base}{api_path}?{qs}")
    };
    let req = with_device_auth(client.get(&url), settings, "GET", api_path, &qs, b"")?;
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Cannot reach PC at {base}: {e}"))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        let msg = body["error"].as_str().unwrap_or("device not paired");
        return Err(format!("{msg} (http://{base})"));
    }
    resp.error_for_status()
        .map_err(|e| format!("Request to PC failed: {e}"))?
        .json::<Value>()
        .await
        .map_err(|e| format!("Unexpected response from PC: {e}"))
}

/// `GET /api/v1/wiki/dirs` — top-level wiki roots on the PC.
#[cfg(not(feature = "desktop"))]
pub(crate) async fn wiki_remote_list_dirs(
    settings: &SettingsState,
) -> Result<Vec<crate::wiki::WikiEntry>, String> {
    let v = pc_get_json(settings, "/api/v1/wiki/dirs", &[]).await?;
    serde_json::from_value(v).map_err(|e| format!("Unexpected wiki/dirs response: {e}"))
}

/// `GET /api/v1/wiki/dir?path=` — contents of one wiki directory on the PC.
#[cfg(not(feature = "desktop"))]
pub(crate) async fn wiki_remote_list_dir(
    settings: &SettingsState,
    path: &str,
) -> Result<Vec<crate::wiki::WikiEntry>, String> {
    let v =
        pc_get_json(settings, "/api/v1/wiki/dir", &[("path", path.to_string())]).await?;
    serde_json::from_value(v).map_err(|e| format!("Unexpected wiki/dir response: {e}"))
}

/// `GET /api/v1/wiki/file?path=` — markdown content of one file on the PC.
/// The PC wraps the body as `{ "content": "..." }`.
#[cfg(not(feature = "desktop"))]
pub(crate) async fn wiki_remote_read_file(settings: &SettingsState, path: &str) -> Result<String, String> {
    let v = pc_get_json(settings, "/api/v1/wiki/file", &[("path", path.to_string())]).await?;
    v.get("content")
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "PC wiki/file response missing 'content'".to_string())
}

/// `GET /api/v1/wiki/search?keyword=` — full-text results from the PC.
#[cfg(not(feature = "desktop"))]
pub(crate) async fn wiki_remote_search(
    settings: &SettingsState,
    keyword: &str,
) -> Result<Vec<crate::wiki::WikiSearchResult>, String> {
    let v =
        pc_get_json(settings, "/api/v1/wiki/search", &[("keyword", keyword.to_string())]).await?;
    serde_json::from_value(v).map_err(|e| format!("Unexpected wiki/search response: {e}"))
}
