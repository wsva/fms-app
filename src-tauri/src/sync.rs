//! Phone-side snapshot + batched-writeback client for the Android thin build.
//!
//! Compiled on **all** platforms (it only needs `reqwest`, `tar`, `flate2`,
//! `rusqlite` — all Android-portable). On desktop it registers cleanly but is
//! normally unused; the mutation-queue helpers below are only fed on mobile
//! (the `dictation`/`xp` write commands enqueue under `#[cfg(not(feature = "desktop"))]`).
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

use crate::dictation;
use crate::settings::{self, SettingsState};

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// Effective PC base URL (trimmed of trailing slashes). Empty when unset.
fn pc_base(settings: &SettingsState) -> String {
    settings
        .settings
        .lock()
        .unwrap()
        .pc_url
        .trim_end_matches('/')
        .to_string()
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

/// Add `x-fms-device` / `x-fms-ts` / `x-fms-sig` headers to an outbound PC
/// request. `path` must be the URL path *as the PC sees it* (including the
/// `/api/v1` prefix) — it is part of the signed message.
fn with_device_auth(
    req: reqwest::RequestBuilder,
    settings: &SettingsState,
    method: &str,
    path: &str,
) -> Result<reqwest::RequestBuilder, String> {
    let ident = ensure_device_identity(settings)?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let msg = format!("{}\n{}\n{}", ts, method.to_uppercase(), path);
    let sig = ident.key_pair.sign(msg.as_bytes());
    Ok(req
        .header("x-fms-device", ident.device_id.clone())
        .header("x-fms-ts", ts.to_string())
        .header("x-fms-sig", hex_encode(sig.as_ref())))
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
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Writeback queue (producer side, called from the mobile write commands)
// ---------------------------------------------------------------------------

/// Append a queued change to the writeback queue. Only fed on mobile: the
/// `dictation`/`xp` write commands call this under `#[cfg(not(feature = "desktop"))]`
/// (desktop writes straight to the DB), so it is legitimately unused on desktop.
#[allow(dead_code)]
pub fn enqueue_change(
    settings: &SettingsState,
    kind: &str,
    dataset_uuid: &str,
    payload: &Value,
) -> Result<(), String> {
    let conn = dictation::open_app_db(settings)?;
    ensure_sync_tables(&conn)?;
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT OR REPLACE INTO writeback_queue (id, kind, dataset_uuid, payload, queued_at) \
         VALUES (?1, ?2, ?3, ?4, datetime('now'))",
        params![id, kind, dataset_uuid, payload.to_string()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
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
        let batch: Vec<(String, String, String, String, String)> = {
            let conn = dictation::open_app_db(settings)?;
            ensure_sync_tables(&conn)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, kind, dataset_uuid, payload, queued_at \
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
            .map(|(id, kind, ds, payload, queued_at)| {
                json!({
                    "id": id,
                    "kind": kind,
                    "dataset_uuid": if ds.is_empty() { Value::Null } else { json!(ds) },
                    "payload": serde_json::from_str::<Value>(payload).unwrap_or(Value::Null),
                    "queued_at": queued_at,
                })
            })
            .collect();
        let body = json!({ "user_key": crate::auth::workspace_identity(settings), "changes": changes });

        let req = with_device_auth(
            client.post(format!("{base}/api/v1/sync/changes")),
            settings,
            "POST",
            "/api/v1/sync/changes",
        )?
        .json(&body);
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
            let conn = dictation::open_app_db(settings)?;
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
        let conn = dictation::open_app_db(&settings)?;
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
    )?;
    let resp = sreq
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;

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

    // 7. Record sync state.
    {
        let conn = dictation::open_app_db(&settings)?;
        ensure_sync_tables(&conn)?;
        conn.execute(
            "INSERT OR REPLACE INTO dataset_sync_state \
             (dataset_uuid, overall_hash, synced_at, bytes, file_count) \
             VALUES (?1, ?2, datetime('now'), ?3, ?4)",
            params![&uuid, &hash, received as i64, file_count as i64],
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
}

/// Read the local `dataset_sync_state` table (what has been pulled and when).
#[tauri::command]
pub async fn dataset_sync_state(
    settings: State<'_, SettingsState>,
) -> Result<Vec<SyncStateEntry>, String> {
    let conn = dictation::open_app_db(&settings)?;
    ensure_sync_tables(&conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT dataset_uuid, overall_hash, synced_at, bytes, file_count \
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
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Number of changes waiting to be uploaded (for the UI badge).
#[tauri::command]
pub async fn writeback_pending_count(settings: State<'_, SettingsState>) -> Result<i64, String> {
    let conn = dictation::open_app_db(&settings)?;
    ensure_sync_tables(&conn)?;
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM writeback_queue", [], |r| r.get(0))
        .unwrap_or(0);
    Ok(n)
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

// ---------------------------------------------------------------------------
// Pairing (phone side)
// ---------------------------------------------------------------------------

/// Best-effort human-readable device name for the PC's confirm dialog.
fn device_name() -> String {
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
            None => format!("{} device", std::env::consts::OS),
        }
    }).clone()
}

/// Pair this device with the PC. The PC owner gets a confirm dialog; we poll
/// `/pair/status` every 2 s (up to 120 s) until they answer.
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
        // Re-pair (already approved) and denied short-circuit — no dialog.
        return Ok(json!({ "state": state, "fingerprint": v["fingerprint"] }));
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
            return Ok(json!({ "state": state, "fingerprint": fingerprint, "request_id": request_id }));
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

/// Issue a signed GET against a PC `/api/v1/wiki/*` endpoint and return the
/// raw JSON value. The request path (excluding the query string) is what gets
/// signed, matching `zone_guard`'s verification on the PC.
#[cfg(not(feature = "desktop"))]
async fn wiki_get_json(
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
    let req = with_device_auth(client.get(format!("{base}{api_path}")), settings, "GET", api_path)?
        .query(query);
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
        .map_err(|e| format!("Wiki request to PC failed: {e}"))?
        .json::<Value>()
        .await
        .map_err(|e| format!("Unexpected response from PC: {e}"))
}

/// `GET /api/v1/wiki/dirs` — top-level wiki roots on the PC.
#[cfg(not(feature = "desktop"))]
pub(crate) async fn wiki_remote_list_dirs(
    settings: &SettingsState,
) -> Result<Vec<crate::wiki::WikiEntry>, String> {
    let v = wiki_get_json(settings, "/api/v1/wiki/dirs", &[]).await?;
    serde_json::from_value(v).map_err(|e| format!("Unexpected wiki/dirs response: {e}"))
}

/// `GET /api/v1/wiki/dir?path=` — contents of one wiki directory on the PC.
#[cfg(not(feature = "desktop"))]
pub(crate) async fn wiki_remote_list_dir(
    settings: &SettingsState,
    path: &str,
) -> Result<Vec<crate::wiki::WikiEntry>, String> {
    let v =
        wiki_get_json(settings, "/api/v1/wiki/dir", &[("path", path.to_string())]).await?;
    serde_json::from_value(v).map_err(|e| format!("Unexpected wiki/dir response: {e}"))
}

/// `GET /api/v1/wiki/file?path=` — markdown content of one file on the PC.
/// The PC wraps the body as `{ "content": "..." }`.
#[cfg(not(feature = "desktop"))]
pub(crate) async fn wiki_remote_read_file(settings: &SettingsState, path: &str) -> Result<String, String> {
    let v = wiki_get_json(settings, "/api/v1/wiki/file", &[("path", path.to_string())]).await?;
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
        wiki_get_json(settings, "/api/v1/wiki/search", &[("keyword", keyword.to_string())]).await?;
    serde_json::from_value(v).map_err(|e| format!("Unexpected wiki/search response: {e}"))
}
