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

use std::io::{BufReader, Write as _};
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use futures_util::StreamExt;
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::{json, Value};
use tar::Archive;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::dictation;
use crate::settings::SettingsState;

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

fn pc_token(settings: &SettingsState) -> String {
    settings.settings.lock().unwrap().pc_token.clone()
}

fn with_token(req: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    if token.is_empty() {
        req
    } else {
        req.header("x-fms-token", token)
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

/// Effective PC token: caller override if non-empty, else the persisted setting.
fn resolve_pc_token(settings: &SettingsState, override_token: Option<&str>) -> String {
    match override_token.map(str::trim).filter(|s| !s.is_empty()) {
        Some(t) => t.to_string(),
        None => pc_token(settings),
    }
}

/// Directory that dictation datasets live in: `<datasets>/dictation`.
fn dictation_root(settings: &SettingsState) -> std::path::PathBuf {
    settings.datasets_dir().join("dictation")
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
    let token = pc_token(settings);
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
        let body = json!({ "user_key": "", "changes": changes });

        let req = with_token(client.post(format!("{base}/api/v1/sync/changes")), &token).json(&body);
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
    let token = pc_token(&settings);
    let client = reqwest::Client::new();

    // 1. Manifest -> compare hash.
    let mreq = with_token(
        client.get(format!("{base}/api/v1/datasets/{uuid}/manifest")),
        &token,
    );
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

    // 3. Prepare temp paths under <datasets>/dictation.
    let root = dictation_root(&settings);
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let tmp_dir = root.join(format!("{uuid}.tmp"));
    let final_dir = root.join(&uuid);
    let part = root.join(format!("{uuid}.tar.gz.part"));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let _ = std::fs::remove_file(&part);
    std::fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;

    // 4. Stream the tar.gz to disk, emitting progress.
    let sreq = with_token(
        client.get(format!("{base}/api/v1/datasets/{uuid}/snapshot")),
        &token,
    );
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
/// cross-origin request (which would be blocked by CORS). Reads `pc_url` /
/// `pc_token` straight from settings and returns the PC's raw status JSON:
/// `{ ok, app_name, dataset_count, version }`.
#[tauri::command]
pub async fn pc_check_status(
    settings: State<'_, SettingsState>,
    pc_url: Option<String>,
    pc_token: Option<String>,
) -> Result<Value, String> {
    let base = resolve_pc_url(&settings, pc_url.as_deref());
    log::info!(
        "[sync] pc_check_status -> {}/api/v1/status",
        if base.is_empty() { "<unset>" } else { &base }
    );
    if base.is_empty() {
        return Err("pc_url is not set — use Settings › Discover PC".to_string());
    }
    let token = resolve_pc_token(&settings, pc_token.as_deref());
    let result = async {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?;
        with_token(client.get(format!("{base}/api/v1/status")), &token)
            .send()
            .await
            .map_err(|e| format!("Cannot reach PC at {base}: {e}"))?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .json::<Value>()
            .await
            .map_err(|e| format!("Unexpected response from PC: {e}"))
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
/// of `{ uuid, name, updated, media_count, status }`.
#[tauri::command]
pub async fn pc_list_datasets(
    settings: State<'_, SettingsState>,
    pc_url: Option<String>,
    pc_token: Option<String>,
) -> Result<Value, String> {
    let base = resolve_pc_url(&settings, pc_url.as_deref());
    log::info!(
        "[sync] pc_list_datasets -> {}/api/v1/datasets",
        if base.is_empty() { "<unset>" } else { &base }
    );
    if base.is_empty() {
        return Err("pc_url is not set — use Settings › Discover PC".to_string());
    }
    let token = resolve_pc_token(&settings, pc_token.as_deref());
    let result = async {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| e.to_string())?;
        with_token(client.get(format!("{base}/api/v1/datasets")), &token)
            .send()
            .await
            .map_err(|e| format!("Cannot reach PC at {base}: {e}"))?
            .error_for_status()
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
