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

// ---------------------------------------------------------------------------
// Website-hub auth seam (T1) — origin-selected auth for every hub request
// ---------------------------------------------------------------------------

/// Resolve the effective hub base for a request: the persisted `pc_url`, which
/// holds either a LAN PC origin or the website origin — [`super::hub_kind`]
/// classifies it per call, so the website path is purely additive and the LAN
/// signing path stays byte-identical (dev guide §10.4).
fn hub_base(settings: &SettingsState) -> Result<String, String> {
    let base = pc_base(settings);
    if base.is_empty() {
        return Err("No hub configured. Set the PC address (or the website origin).".into());
    }
    Ok(base)
}

/// Attach origin-appropriate auth to an outbound hub request. Website:
/// `Authorization: Bearer <access token>` and **no** `x-fms-*` headers — the
/// website's principal is an account, not a device, and a future strict-mode
/// route could reject unknown signing headers (dev guide §2). LAN: the exact
/// device-HMAC signing of [`with_device_auth`]. Missing/empty token on the
/// website path degrades to an anonymous request rather than failing here: a
/// 401 comes back from the gate and [`hub_http_error`] turns it into the
/// sign-in message, while public reads keep working without a login.
pub(crate) fn hub_request(
    req: reqwest::RequestBuilder,
    settings: &SettingsState,
    method: &str,
    path: &str,
    query: &str,
    body: &[u8],
) -> Result<reqwest::RequestBuilder, String> {
    match super::hub_kind(&pc_base(settings)) {
        super::HubKind::Website => match crate::auth::get_access_token(settings) {
            Ok(token) => Ok(req.header("Authorization", format!("Bearer {}", token))),
            Err(_) => Ok(req),
        },
        super::HubKind::LanPc => with_device_auth(req, settings, method, path, query, body),
    }
}

/// Map a non-2xx hub response to an app message that keeps the status code and
/// says what to do next (dev guide §T7). For the website the codes carry fixed
/// meanings (§2): 401 = anonymous/token problem, 403 = identified but not
/// allowed — a token refresh never fixes that, so the message asks for access,
/// not for a re-login. 5xx stays verbatim and marked retryable.
async fn hub_http_error(resp: reqwest::Response, api_path: &str) -> String {
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let server_msg = body["error"].as_str().unwrap_or("");
    match status.as_u16() {
        401 => {
            if server_msg.contains("not paired") {
                format!("{server_msg} — pair this device with the hub first (401)")
            } else {
                "sign in again — the hub rejected or did not receive your access token (401)"
                    .to_string()
            }
        }
        403 => format!(
            "this account cannot access '{}', or the API key is read-only (403){}",
            api_path,
            if server_msg.is_empty() { String::new() } else { format!(" — {server_msg}") }
        ),
        404 => format!("'{api_path}' is not on this hub (404){}",
            if server_msg.is_empty() { String::new() } else { format!(" — {server_msg}") }),
        409 => format!("ownership/type conflict with the hub's catalog (409): {server_msg}"),
        413 => format!("too large for the hub's limits (413): {server_msg}"),
        c => format!("hub request '{api_path}' failed (HTTP {c}): {server_msg}"),
    }
}

/// Signed/bearer GET returning parsed JSON, auth chosen by origin (T1). The
/// shared route for every website-capable hub read: catalog, manifest, file.
async fn hub_get_json(
    settings: &SettingsState,
    api_path: &str,
    qs: &str,
) -> Result<Value, String> {
    let base = hub_base(settings)?;
    let client = crate::auth::hub_client_builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let url = if qs.is_empty() {
        format!("{base}{api_path}")
    } else {
        format!("{base}{api_path}?{qs}")
    };
    let req = hub_request(client.get(&url), settings, "GET", api_path, qs, b"")?;
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Cannot reach hub at {base}: {e}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED
        && super::hub_kind(&base) == super::HubKind::Website
    {
        // The website mints tokens only; an expired/revoked one has no silent
        // refresh route, so clear it and send the user to sign in (T5).
        crate::auth::token_rejected(settings);
    }
    if !status.is_success() {
        return Err(hub_http_error(resp, api_path).await);
    }
    resp.json::<Value>()
        .await
        .map_err(|e| format!("Unexpected response from hub at '{api_path}': {e}"))
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
        );
        /* Single-row cursor for the app-wide journal scope (per-user dictation
           progress + XP). Kept out of `dataset_sync_state` deliberately: that
           table is keyed by real dataset uuids and the round prunes rows whose
           dataset vanished from the hub catalog, which would delete this.
           `backfilled_at` records that the state-based history read (§3.6) has
           run once, so the round does not re-download a whole ledger every round. */
        CREATE TABLE IF NOT EXISTS app_sync_state (
            id            INTEGER PRIMARY KEY CHECK (id = 1),
            cursor_seq    INTEGER,
            backfilled_at TEXT,
            updated_at    TEXT NOT NULL
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
    // Migration: `app_sync_state` shipped without `backfilled_at`; a device that
    // pulled the app scope before the history read existed gets the column here
    // (NULL = never backfilled, which is the truth for those rows).
    let has_backfilled_at: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('app_sync_state') WHERE name = 'backfilled_at'")
        .and_then(|mut s| s.exists([]))
        .unwrap_or(false);
    if !has_backfilled_at {
        conn.execute("ALTER TABLE app_sync_state ADD COLUMN backfilled_at TEXT", [])
            .map_err(|e| e.to_string())?;
    }
    // Migration (sync-v2.md §6.1, phase 1): provenance columns. A cursor is only
    // meaningful against the seq space of the host that minted it, so every row
    // records which host that was. `upstream_kind` defaults to `desktop_hub` for
    // pre-existing rows because a LAN hub was the only origin writing this table
    // before the migration; `upstream_origin` defaults to empty and is treated as
    // "unknown" — the first round after migration NULLs the cursor so the row
    // re-establishes itself against the currently connected host (INV-7, INV-19).
    ensure_column(conn, "dataset_sync_state", "row_hash", "TEXT")?;
    ensure_column(conn, "dataset_sync_state", "upstream_kind", "TEXT NOT NULL DEFAULT 'desktop_hub'")?;
    ensure_column(conn, "dataset_sync_state", "upstream_origin", "TEXT NOT NULL DEFAULT ''")?;
    ensure_column(conn, "dataset_sync_state", "published_version", "INTEGER")?;
    ensure_column(conn, "dataset_sync_state", "availability", "TEXT NOT NULL DEFAULT 'listed'")?;
    ensure_column(conn, "app_sync_state", "upstream_kind", "TEXT NOT NULL DEFAULT 'desktop_hub'")?;
    ensure_column(conn, "app_sync_state", "upstream_origin", "TEXT NOT NULL DEFAULT ''")?;
    Ok(())
}

/// Idempotent `ALTER TABLE ... ADD COLUMN` guarded by a `pragma_table_info`
/// check — the same pattern the surrounding migrations use. Kept as a helper so
/// the phase-1 provenance additions read as one block.
fn ensure_column(conn: &Connection, table: &str, name: &str, decl: &str) -> Result<(), String> {
    let exists: bool = conn
        .prepare(&format!(
            "SELECT 1 FROM pragma_table_info('{table}') WHERE name = '{name}'"
        ))
        .and_then(|mut s| s.exists([]))
        .unwrap_or(false);
    if exists {
        return Ok(());
    }
    conn.execute(&format!("ALTER TABLE {table} ADD COLUMN \"{name}\" {decl}"), [])
        .map_err(|e| e.to_string())
        .map(|_| ())
}

/// One-time cursor reset for pre-migration rows: a `dataset_sync_state` row whose
/// `upstream_origin` is empty was written before provenance existed, so its
/// `cursor` belongs to an unknown host's seq space. NULLing it forces the
/// existing "NULL cursor → one snapshot resync" path (documented at
/// [client.rs#L398-L410]) to re-establish the cursor against whichever host the
/// device is now talking to. Runs at most once per row (the migration stamps
/// `upstream_origin` from then on). Returns the number of rows reset so the
/// caller can log it.
fn reset_cursors_without_provenance(conn: &Connection) -> Result<usize, String> {
    let n = conn
        .execute(
            "UPDATE dataset_sync_state SET \"cursor\" = NULL \
             WHERE (upstream_origin IS NULL OR upstream_origin = '') AND \"cursor\" IS NOT NULL",
            [],
        )
        .map_err(|e| e.to_string())?;
    Ok(n)
}

/// Provenance stamp for the currently connected host, derived from its base URL.
/// `upstream_kind` is the wire-visible value from sync-v2.md §6.1
/// (`desktop_hub | cloud_hub | registry | local`); phase 1 only ever writes the
/// first two — `registry` lands with the adoption flow in phase 3.
fn upstream_of(base: &str) -> (&'static str, String) {
    match super::hub_kind(base) {
        super::HubKind::Website => ("cloud_hub", base.trim_end_matches('/').to_string()),
        super::HubKind::LanPc => ("desktop_hub", base.trim_end_matches('/').to_string()),
    }
}

/// Read this device's cursor into the app-wide journal scope (`None` = never
/// pulled). The single-row table is inserted on first advance, so a fresh device
/// simply has no row yet.
fn app_cursor(conn: &Connection) -> Option<i64> {
    conn.query_row("SELECT cursor_seq FROM app_sync_state WHERE id = 1", [], |r| {
        r.get::<_, Option<i64>>(0)
    })
    .unwrap_or(None)
}

/// Advance the app-wide journal cursor to the hub's high-water `seq`.
fn set_app_cursor(conn: &Connection, seq: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO app_sync_state (id, cursor_seq, updated_at) \
         VALUES (1, ?1, datetime('now')) \
         ON CONFLICT(id) DO UPDATE SET cursor_seq = excluded.cursor_seq, updated_at = excluded.updated_at",
        params![seq],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Stamp provenance on the single-row `app_sync_state` (sync-v2.md §6.1). Only
/// fills an empty origin — a row already stamped for a different host is left
/// alone, so a partial round against a foreign host cannot silently rewrite the
/// `@app` scope's upstream (INV-19). Returns `true` when the row was stamped,
/// `false` when the existing stamp was kept.
fn stamp_app_provenance(
    conn: &Connection,
    upstream_kind: &str,
    upstream_origin: &str,
) -> Result<bool, String> {
    // Make sure the row exists so the UPDATE has something to touch. The
    // ON CONFLICT branch preserves any cursor/backfill stamp already there.
    conn.execute(
        "INSERT INTO app_sync_state (id, updated_at) VALUES (1, datetime('now')) \
         ON CONFLICT(id) DO NOTHING",
        [],
    )
    .map_err(|e| e.to_string())?;
    let n = conn
        .execute(
            "UPDATE app_sync_state \
             SET upstream_kind = ?1, upstream_origin = ?2 \
             WHERE id = 1 AND (upstream_origin IS NULL OR upstream_origin = '')",
            params![upstream_kind, upstream_origin],
        )
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

/// Read the recorded `@app` upstream, if any; a fresh install returns `None`.
/// This is the read side of the [`stamp_app_provenance`] stamp. Phase 1 only
/// *writes* `@app` provenance (so a later round can see which host minted the
/// cursor); the INV-19 check that refuses an `@app` round against a foreign host
/// reads it in a later phase, so today it is exercised by the unit tests only.
#[allow(dead_code)]
fn app_provenance(conn: &Connection) -> Option<(String, String)> {
    conn.query_row(
        "SELECT upstream_kind, upstream_origin FROM app_sync_state WHERE id = 1",
        [],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .ok()
    .filter(|(_, origin)| !origin.is_empty())
}

/// When this device last folded the hub's *whole* app-data history into its own
/// (`None` = never). See [`backfill_app_history`].
fn app_history_backfilled(conn: &Connection) -> Option<String> {
    conn.query_row(
        "SELECT backfilled_at FROM app_sync_state WHERE id = 1",
        [],
        |r| r.get::<_, Option<String>>(0),
    )
    .unwrap_or(None)
}

/// Stamp the history backfill as done. Kept separate from `set_app_cursor` rather
/// than folded into it: a journal pull advances the cursor on every round, while
/// the backfill is a once-per-device (or once-per-gap) event, and conflating the two
/// would silently stop the retry after a *truncated* fetch.
fn set_app_history_backfilled(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "INSERT INTO app_sync_state (id, backfilled_at, updated_at) \
         VALUES (1, datetime('now'), datetime('now')) \
         ON CONFLICT(id) DO UPDATE SET backfilled_at = excluded.backfilled_at, updated_at = excluded.updated_at",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Writeback queue (producer side, called from the mobile write commands)
// ---------------------------------------------------------------------------

/// Append a queued change to the writeback queue. Both mobile clients and a
/// desktop running `role = "follower"` funnel here via
/// [`crate::sync::change_log::commit_change`]; the hub role never enqueues. State kinds
/// coalesce (§3.4): a newer edit to the same `(kind, scope, object_id)`
/// replaces the still-pending older one instead of stacking, while append-only
/// and counter kinds keep every distinct row.
///
/// `scope` is the journal scope already resolved by the caller (a dataset uuid for
/// shared content, [`crate::sync::change_log::APP_SCOPE`] for per-user app data),
/// and `user_key` the identity the change belongs to — both are stamped into the
/// row so the hub's conflict lookup and the pull-side guard see the same ids this
/// device computed.
pub fn enqueue_change(
    settings: &SettingsState,
    kind: &str,
    scope: &str,
    payload: &Value,
    user_key: &str,
) -> Result<(), String> {
    let conn = datasets::dictation::open_app_db(settings)?;
    ensure_sync_tables(&conn)?;
    // The row-stamp shape (see `datasets::row_stamp_at`): the hub parses it, but a
    // pulled `edit_time` also lands in live row columns, which compare as text.
    let edit_time = datasets::now_row_stamp();
    let object_id = crate::sync::change_log::object_id_for(kind, payload, user_key);
    // Fold repeat edits to one mutable row so we neither ship stale intermediate
    // payloads nor grow the queue without bound between flushes. An empty
    // `object_id` (bulk inserts) is left uncoalesced by the guard.
    if crate::sync::change_log::ChangeClass::of(kind).coalescible() && !object_id.is_empty() {
        conn.execute(
            "DELETE FROM writeback_queue \
             WHERE kind = ?1 AND dataset_uuid = ?2 AND object_id = ?3",
            params![kind, scope, &object_id],
        )
        .map_err(|e| e.to_string())?;
    }
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO writeback_queue \
         (id, kind, dataset_uuid, payload, queued_at, edit_time, object_id) \
         VALUES (?1, ?2, ?3, ?4, datetime('now'), ?5, ?6)",
        params![id, kind, scope, payload.to_string(), &edit_time, &object_id],
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
pub async fn writeback_flush_inner(
    settings: &SettingsState,
    cluster: &Option<super::ClusterInfo>,
) -> Result<usize, String> {
    let base = pc_base(settings);
    if base.is_empty() {
        return Ok(0);
    }
    // Capability gate (INV-20): only push to a host that advertises the app
    // journal. A host that served no `/cluster` page default-denies (today's
    // website), and a LAN hub too old for `/cluster` falls back to the protocol
    // probe — so entries stay queued for a hub that can take them instead of
    // failing every round. The held count is surfaced by the round, not here.
    let protocol = match cluster.as_ref() {
        Some(c) => Some(c.protocol_version),
        None => hub_protocol_version(settings).await,
    };
    if !app_journal_advertised(cluster, &base, protocol) {
        let held = pending_writeback_count(settings);
        if held > 0 {
            log::warn!(
                "[sync] connected host does not advertise app_journal — {held} change(s) held \
                 locally; will retry against a hub that does (INV-20)"
            );
        }
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
    let base = hub_base(&settings)?;
    let client = reqwest::Client::new();

    // 1. Manifest -> compare hash.
    let api_path = format!("/api/v1/datasets/{uuid}/manifest");
    let mreq = hub_request(
        client.get(format!("{base}{api_path}")),
        &settings,
        "GET",
        &api_path,
        "",
        b"",
    )?;
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
    let cluster = fetch_cluster(&settings).await;
    let _ = writeback_flush_inner(&settings, &cluster).await;

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
    let snap_path = format!("/api/v1/datasets/{uuid}/snapshot");
    let sreq = hub_request(
        client.get(format!("{base}{snap_path}")),
        &settings,
        "GET",
        &snap_path,
        "",
        b"",
    )?;
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
    //    and stamp provenance from the connected host (sync-v2.md §6.1). A
    //    snapshot is a full re-establishment of the copy, so it is the one
    //    place where overwriting `upstream_kind` / `upstream_origin` is safe:
    //    the bytes on disk now came from *this* host and no other.
    let (upstream_kind, upstream_origin) = upstream_of(&base);
    {
        let conn = datasets::dictation::open_app_db(&settings)?;
        ensure_sync_tables(&conn)?;
        conn.execute(
            "INSERT OR REPLACE INTO dataset_sync_state \
             (dataset_uuid, overall_hash, synced_at, bytes, file_count, \"cursor\", \
              upstream_kind, upstream_origin, availability) \
             VALUES (?1, ?2, datetime('now'), ?3, ?4, ?5, ?6, ?7, \
                     COALESCE((SELECT availability FROM dataset_sync_state WHERE dataset_uuid = ?1), 'listed'))",
            params![
                &uuid,
                &hash,
                received as i64,
                file_count as i64,
                snapshot_seq,
                upstream_kind,
                upstream_origin,
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

/// Upload all pending writeback changes to the PC.
#[tauri::command]
pub async fn writeback_flush(settings: State<'_, SettingsState>) -> Result<usize, String> {
    let cluster = fetch_cluster(&settings).await;
    writeback_flush_inner(&settings, &cluster).await
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
    } else if super::hub_kind(&base) == super::HubKind::Website {
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
    let app_cursor = app_cursor(&conn);
    let history_backfilled_at = app_history_backfilled(&conn);
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
        hub_kind: if super::hub_kind(&base) == super::HubKind::Website {
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
        )?;
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
/// page from the hub (§3.2/§3.3). The query string is signed verbatim (v2
/// hardening). Returns the hub's `{ entries, pruned_up_to, hub_seq,
/// resync_required }` JSON. A follower refuses this against a hub reporting an
/// older `protocol_version` — enforced by the caller (Phase 4 round).
///
/// Passing [`crate::sync::change_log::APP_SCOPE`] as the uuid pulls the app-wide
/// per-user journal from `GET /api/v1/app/changes` instead — same page shape, no
/// dataset to resolve.
pub async fn pc_changes_since(
    settings: State<'_, SettingsState>,
    uuid: String,
    after: i64,
) -> Result<Value, String> {
    let base = pc_base(&settings);
    // No row journal on the website hub — the caller's round must not even ask
    // (dev guide §6); file level converges by manifest hash instead.
    if super::hub_kind(&base) == super::HubKind::Website {
        return Err("the website hub serves no row journal (/changes)".to_string());
    }
    if base.is_empty() {
        return Err("pc_url is not set — use Settings › Discover PC".to_string());
    }
    let api_path = if uuid == crate::sync::change_log::APP_SCOPE {
        "/api/v1/app/changes".to_string()
    } else {
        format!("/api/v1/datasets/{uuid}/changes")
    };
    let qs = build_query(&[("after", after.to_string())]);
    let client = crate::auth::hub_client_builder()
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
    )?;
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

// ---------------------------------------------------------------------------
// Incremental sync round (§3.3): push → per-dataset /changes apply → file-drift
// snapshot → catalog prune. Reuses the snapshot path for any non-DB file change
// (media/vtt), and folds the DB-only row deltas in place with the row log.
// ---------------------------------------------------------------------------

/// Signed GET returning parsed JSON. `path` is the URL path (no query), `qs`
/// the exact pre-encoded query string appended to the URL and folded into the
/// v2 signature. Shared by the round's catalog + manifest reads. Website
/// origins take the Bearer path of [`hub_get_json`] instead — same signature.
async fn signed_get_json(settings: &SettingsState, path: &str, qs: &str) -> Result<Value, String> {
    hub_get_json(settings, path, qs).await
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
            // Awards journal under the app-wide scope, so the dataset a pull was
            // made for says nothing about attribution — take it from the payload,
            // mirroring the hub's `rest.rs::replay`.
            let ds_payload = payload.get("dataset_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let dsref = if !ds.is_empty() {
                Some(ds.as_str())
            } else if !ds_payload.is_empty() {
                Some(ds_payload)
            } else {
                None
            };
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
        // A review is a stored *result*, so it is applied by keeping the event the
        // author sent — never by re-running the rating against this node's history,
        // which is how one review used to fold twice per round (INV-23, §17.1).
        "card_review" => crate::datasets::cards::review::apply_remote(
            settings.inner(),
            &ds,
            payload,
            write_identity,
        ),
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
        // Wiki datasets: the .md files *are* the rows. Both arms resolve the
        // local copy under <datasets>/wiki and journal through commit_change,
        // which the caller's ApplyGuard turns into a no-op here.
        "wiki_file_save" => {
            let rel = str_field("rel_path")?;
            let content = payload.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
            crate::datasets::wiki::write_file(settings.inner(), &ds, &rel, &content).map(|_| true)
        }
        "wiki_file_delete" => {
            let rel = str_field("rel_path")?;
            crate::datasets::wiki::delete_file(settings.inner(), &ds, &rel).map(|_| true)
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
/// A kind with no single-row identity (`book_sentences_save` bulk inserts) can
/// never be matched meaningfully, so an empty `object_id` on either side must not
/// participate in the guard — comparing "" to "" would suppress every pulled row
/// of that kind for as long as one un-pushed row sat in the queue. Matching is on
/// the `object_id` column `enqueue_change` stamped at queue time, so both sides
/// are compared in the same per-user form (see
/// [`crate::sync::change_log::object_id_for`]); pre-Phase-6 rows carry a NULL id
/// and simply never block a pull.
fn has_pending_local_edit(conn: &Connection, dataset_uuid: &str, kind: &str, object_id: &str) -> bool {
    if object_id.is_empty() {
        return false;
    }
    conn
        .prepare_cached(
            "SELECT 1 FROM writeback_queue \
             WHERE kind = ?1 AND dataset_uuid = ?2 AND object_id = ?3 LIMIT 1",
        )
        .and_then(|mut s| s.exists(params![kind, dataset_uuid, object_id]))
        .unwrap_or(false)
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
fn prune_local_dataset(
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
    for ty in ["dictation", "card", "book", "read_aloud", "wiki"] {
        let dir = type_root(settings, ty).join(uuid);
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    let _ = conn.execute("DELETE FROM dataset_sync_state WHERE dataset_uuid = ?1", params![uuid]);
    PruneOutcome::Deleted
}

/// Unconditional local-copy removal: every type root plus the sync-state row.
/// The provenance guard in [`prune_local_dataset`] exists to stop *absence from
/// a foreign catalog* from deleting a copy; an explicit user action
/// ([`sync_forget_dataset`]) is a different question and bypasses the guard.
fn remove_local_dataset_unchecked(settings: &SettingsState, conn: &Connection, uuid: &str) {
    for ty in ["dictation", "card", "book", "read_aloud", "wiki"] {
        let dir = type_root(settings, ty).join(uuid);
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    let _ = conn.execute("DELETE FROM dataset_sync_state WHERE dataset_uuid = ?1", params![uuid]);
}

/// Where a card dataset's website round trip should go, decided by provenance
/// rather than by the currently connected host (sync-v2.md §3 fact 3, §12).
///
/// The website's `/api/card/sync/{pull,push}` endpoints exist only on the
/// website origin; a LAN hub syncs card content through the dataset-scope
/// journal (kind `card`) and the writeback queue. So the router has three
/// verdicts, not two:
///
/// * [`CardSyncTarget::Website`] — the deck's upstream is the cloud hub, run
///   today's round trip.
/// * [`CardSyncTarget::Skip`] — the deck's upstream is a desktop hub, the LAN
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
        // No provenance yet: adopt the connected host as this deck's upstream
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
                "deck is upstreamed by a desktop hub ({row_origin}); its rows travel \
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

// ---------------------------------------------------------------------------
// App-wide journal pull (§3.2): per-user dictation progress + XP
// ---------------------------------------------------------------------------

/// Wire protocol version that introduced the hub's `GET /api/v1/app/changes`
/// endpoint. A follower must not pull that path from an older hub: the route does
/// not exist there, so every round would log a 404 while the data it guards stays
/// on the hub.
const APP_SCOPE_PROTOCOL: u32 = 3;

/// Wire protocol version that introduced `GET /api/v1/app/state`, the state-based
/// history read. Same reasoning as [`APP_SCOPE_PROTOCOL`]: a pre-v4 hub has no such
/// route, and asking for it every round would log a 404 where the honest answer is
/// "this hub cannot backfill your history".
const HISTORY_PROTOCOL: u32 = 4;

/// Upper bound on `/app/state` pages one backfill fetches (the hub caps each page at
/// 20k rows per table, so 50 pages is far past any real history). A cursor that
/// never settles means a bug on one side; stopping here marks the import explicitly
/// incomplete and lets the next round retry, instead of spinning inside one round.
const HISTORY_PAGE_BUDGET: i64 = 50;

/// What one app-scope journal pull achieved.
struct AppPull {
    /// Journal rows folded into the local app DB.
    applied: i64,
    /// The journal cannot deliver this device's whole history — it has never
    /// completed a state read, and/or this round met a gap (never pulled, a range
    /// pruned away, a regressed hub seq) — so the state-based backfill
    /// ([`backfill_app_history`]) is owed.
    needs_history: bool,
    /// The hub's protocol as probed this round, handed to the backfill so it does
    /// not need a second `/status` GET.
    hub_protocol: Option<u32>,
}

/// The hub's advertised `protocol_version`, read off its unsigned `/status` (so
/// it works before pairing). `None` when the hub is unreachable or predates the
/// field — both mean "do not assume new endpoints exist".
async fn hub_protocol_version(settings: &SettingsState) -> Option<u32> {
    let base = pc_base(settings);
    if base.is_empty() {
        return None;
    }
    // The website advertises no protocol version (no `/status` route, dev
    // guide §6) — `None` keeps every protocol-gated pull skipped.
    if super::hub_kind(&base) == super::HubKind::Website {
        return None;
    }
    let client = crate::auth::hub_client_builder()
        .connect_timeout(Duration::from_secs(6))
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;
    let resp = client.get(format!("{base}/api/v1/status")).send().await.ok()?;
    let v: Value = resp.error_for_status().ok()?.json().await.ok()?;
    v.get("protocol_version").and_then(|p| p.as_u64()).map(|p| p as u32)
}

/// Read the connected host's `GET /api/v1/cluster` capability page (sync-v2.md
/// §7). `None` when the host is unreachable, predates the route, or serves a
/// malformed payload — all of which mean "advertises nothing", so every
/// capability gate default-denies and the round degrades to today's behavior
/// instead of failing (INV-20). The website has no `/cluster` route yet, so it
/// resolves to `None` and the app-data plane stays skipped exactly as before.
async fn fetch_cluster(settings: &SettingsState) -> Option<super::ClusterInfo> {
    let base = pc_base(settings);
    if base.is_empty() {
        return None;
    }
    let client = crate::auth::hub_client_builder()
        .connect_timeout(Duration::from_secs(6))
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;
    let api_path = "/api/v1/cluster";
    let req = hub_request(
        client.get(format!("{base}{api_path}")),
        settings,
        "GET",
        api_path,
        "",
        b"",
    )
    .ok()?;
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<super::ClusterInfo>().await.ok()
}

/// Whether `cluster` advertises `feature`. A host that served no `/cluster` page
/// (`None`) advertises nothing — default-deny (INV-20).
fn advertises(cluster: &Option<super::ClusterInfo>, feature: &str) -> bool {
    cluster
        .as_ref()
        .map(|c| c.features.iter().any(|f| f == feature))
        .unwrap_or(false)
}

/// Whether the app-data plane (`@app` push / pull / history backfill) may run
/// against the connected host. Capability-first: true when `/cluster` advertises
/// `app_journal`. Falls back to the pre-`/cluster` protocol gate for a LAN hub
/// too old to serve the route, so a mixed-version cluster keeps working during
/// rollout. A website — which serves neither `/cluster` nor `/status` — is
/// false, i.e. today's skip, unchanged (INV-20).
fn app_journal_advertised(
    cluster: &Option<super::ClusterInfo>,
    base: &str,
    protocol: Option<u32>,
) -> bool {
    if advertises(cluster, "app_journal") {
        return true;
    }
    // No `/cluster` page: only a LAN hub that predates the route keeps the old
    // protocol-version behavior. The website never runs the app plane here.
    cluster.is_none()
        && super::hub_kind(base) == super::HubKind::LanPc
        && matches!(protocol, Some(p) if p >= APP_SCOPE_PROTOCOL)
}

/// Number of changes waiting in the writeback queue, for the INV-20 "held"
/// report when the connected host does not advertise the app journal.
fn pending_writeback_count(settings: &SettingsState) -> i64 {
    let Ok(conn) = datasets::dictation::open_app_db(settings) else {
        return 0;
    };
    if ensure_sync_tables(&conn).is_err() {
        return 0;
    }
    conn.query_row("SELECT COUNT(*) FROM writeback_queue", [], |r| r.get::<_, i64>(0))
        .unwrap_or(0)
}

/// Pull the app-wide journal scope (`sync_log` rows logged under
/// [`crate::sync::change_log::APP_SCOPE`]): per-user `dictation` progress and `xp`
/// awards. Those tables live in the app DB, so they are never part of a dataset
/// snapshot — the row log is their only route. Runs separately from the dataset
/// loop, so a device that subscribes to no dataset still converges, and awards
/// with no dataset at all (book reading) are finally delivered. Reports how many
/// changes were applied and whether a history backfill is owed.
///
/// Every replay here is idempotent (`xp` dedups on (user, source, reference),
/// `dictation` is an upsert), which is what makes the two gap recoveries below
/// (restart from 0, jump to the prune mark) safe without a snapshot fallback.
async fn pull_app_scope(
    settings: &State<'_, SettingsState>,
    cluster: &Option<super::ClusterInfo>,
    hub_protocol: Option<u32>,
    errors: &mut Vec<Value>,
) -> Result<AppPull, String> {
    let scope = crate::sync::change_log::APP_SCOPE;
    // Capability negotiation (INV-20): run the app plane only against a host
    // that advertises `app_journal` (or a pre-`/cluster` LAN hub new enough by
    // protocol). Replaces the old website-only origin test. The website serves
    // neither `/cluster` nor `/status`, so `hub_protocol` is `None` for it and
    // this skips exactly as before — no 404 storm, no probe (dev guide §6).
    if !app_journal_advertised(cluster, &pc_base(settings.inner()), hub_protocol) {
        log::info!("[sync] host advertises no app_journal — skipping app-data pull");
        return Ok(AppPull { applied: 0, needs_history: false, hub_protocol });
    }

    let (cursor, stamped) = {
        let conn = datasets::dictation::open_app_db(settings.inner())?;
        ensure_sync_tables(&conn)?;
        (app_cursor(&conn), app_history_backfilled(&conn).is_some())
    };
    // A device that has never completed a state read is missing everything older
    // than the retained journal, whatever its cursor says; the gap branches below
    // raise the same flag for devices that were stamped but then lost rows.
    let mut needs_history = !stamped || cursor.is_none();
    let mut after = cursor.unwrap_or(0);
    // Bounded retry: a regressed hub seq restarts at 0, a prune gap jumps to the
    // prune mark and re-reads what is still retained. Each branch moves `after`
    // strictly, so this cannot spin.
    let mut page: Option<Value> = None;
    for attempt in 0..3 {
        let p = match pc_changes_since(settings.clone(), scope.to_string(), after).await {
            Ok(p) => p,
            Err(e) => {
                errors.push(json!({ "scope": scope, "stage": "changes", "error": e }));
                // The hub just refused or failed this device — a state read would
                // fail the same way and only double-report one fault. `needs_history`
                // stays a per-round decision: the flag is recomputed next round from
                // the cursor and the stamp, so a transient failure is retried anyway.
                return Ok(AppPull { applied: 0, needs_history: false, hub_protocol });
            }
        };
        let hub_seq = p["hub_seq"].as_i64().unwrap_or(after);
        // Safety alarm (§3.5): a hub seq *below* our cursor means its journal
        // regressed (reset, re-install, or a different machine answering the same
        // address). A dataset would take a snapshot here; app data has no
        // snapshot, so restart the cursor at 0 and re-apply the retained journal.
        if attempt == 0 && hub_seq < after {
            log::error!(
                "[sync] {scope}: hub_seq {hub_seq} < local cursor {after} — journal regressed, restarting from 0"
            );
            errors.push(json!({ "scope": scope, "stage": "alarm", "error": format!("hub seq {hub_seq} regressed below cursor {after}; re-pulling from 0") }));
            needs_history = true;
            after = 0;
            continue;
        }
        let pruned_up_to = p["pruned_up_to"].as_i64().unwrap_or(0);
        if p["resync_required"].as_bool().unwrap_or(false) && pruned_up_to > after {
            // Gap: journal rows below the mark are gone, so the delta path can no
            // longer reach this device's older history — report it, then re-read
            // from the mark so the retained tail still lands. The state read in
            // [`backfill_app_history`] (flagged by `needs_history`) is what actually
            // recovers the past; that also gives a never-pulled device the whole
            // retained journal instead of nothing.
            errors.push(json!({
                "scope": scope,
                "stage": "prune",
                "error": format!("hub pruned the app-data journal up to {pruned_up_to}; cursor {after} is behind — older rows are unrecoverable, continuing from the mark"),
            }));
            needs_history = true;
            after = pruned_up_to;
            continue;
        }
        page = Some(p);
        break;
    }
    let page = match page {
        Some(p) => p,
        // Retries exhausted: keep the position we reached so the next round
        // continues from it instead of re-walking the same gaps.
        None => {
            let conn = datasets::dictation::open_app_db(settings.inner())?;
            ensure_sync_tables(&conn)?;
            let _ = set_app_cursor(&conn, after);
            // The round reached the host even if the retries did not — stamp
            // provenance so a future round against a different host is visible
            // to the INV-19 check rather than silently switching.
            let (k, o) = upstream_of(&pc_base(settings.inner()));
            let _ = stamp_app_provenance(&conn, k, &o);
            return Ok(AppPull { applied: 0, needs_history, hub_protocol });
        }
    };
    let hub_seq = page["hub_seq"].as_i64().unwrap_or(after);

    let entries = page["entries"].as_array().cloned().unwrap_or_default();
    let mut applied = 0i64;
    if !entries.is_empty() {
        let _guard = crate::sync::change_log::ApplyGuard::new();
        let conn = datasets::dictation::open_app_db(settings.inner())?;
        ensure_sync_tables(&conn)?;
        for ent in &entries {
            let kind = ent["kind"].as_str().unwrap_or("");
            let object_id = ent["object_id"].as_str().unwrap_or("");
            let user_key = ent["user_key"].as_str().unwrap_or("");
            let payload = ent["payload"].clone();
            // A local edit to the same row that has not been pushed yet wins; it
            // lands on the hub with the next push and echoes back afterwards.
            if has_pending_local_edit(&conn, scope, kind, object_id) {
                continue;
            }
            // `dictation` needs no dataset and `xp` carries its own, so the row is
            // applied with an empty dataset argument.
            match apply_change(settings, "", kind, &payload, user_key).await {
                Ok(true) => applied += 1,
                Ok(false) => {}
                Err(e) => errors.push(json!({ "scope": scope, "kind": kind, "error": e })),
            }
        }
    }
    // Advance even when the page was empty: `after` may have moved past a prune
    // mark this round, and a stale cursor would re-walk (and re-report) the same
    // gap on every following round.
    let conn = datasets::dictation::open_app_db(settings.inner())?;
    ensure_sync_tables(&conn)?;
    let _ = set_app_cursor(&conn, hub_seq.max(after));
    // Stamp the `@app` upstream from the connected host (sync-v2.md §6.1). Only
    // fills an empty origin — see [`stamp_app_provenance`].
    let (k, o) = upstream_of(&pc_base(settings.inner()));
    let _ = stamp_app_provenance(&conn, k, &o);
    Ok(AppPull { applied, needs_history, hub_protocol })
}

/// Fold the hub's *whole* history for this identity into the local app DB (§3.6).
///
/// The journal pull above is forward-only and retention-pruned, so it can only ever
/// deliver changes newer than the cursor. A device paired after a year of practice —
/// or one that fell through a prune gap — gets the rest from `/app/state`, which
/// ships state rather than deltas: the current progress row per (media, subtitle)
/// pair, plus the XP ledger. Both halves are paged to the end (the hub caps one
/// request, not the history) and land through their ordinary idempotent write paths
/// under an [`crate::sync::change_log::ApplyGuard`], so importing years of history is
/// not immediately re-pushed to the hub as a storm of fresh edits, and the XP total
/// is derived from the merged ledger instead of accumulated.
///
/// `Ok(None)` means "nothing to do": already stamped (unless `force`) or the hub
/// predates the endpoint. Neither is an error worth warning every round about.
///
/// `hub_lifetime_xp` may come out **ahead** of the local `lifetime_xp` and that is
/// not a bug: the hub's ledger can hold two rows for one award (written before
/// dedup, or pushed from two devices), while importing the second is correctly
/// skipped. The gap is reported instead of hidden, and only the deduped local total
/// is what the user sees.
async fn backfill_app_history(
    settings: &SettingsState,
    cluster: &Option<super::ClusterInfo>,
    hub_protocol: Option<u32>,
    force: bool,
) -> Result<Option<Value>, String> {
    let scope = crate::sync::change_log::APP_SCOPE;
    let proto = match hub_protocol.or_else(|| cluster.as_ref().map(|c| c.protocol_version)) {
        Some(p) => Some(p),
        None => hub_protocol_version(settings).await,
    };
    // Capability gate (INV-20): only a host that advertises the app journal can
    // serve `/app/state`. Replaces the old website-only origin test; the website
    // (no `/cluster`, no `/status`) resolves to `None` and skips as before.
    if !app_journal_advertised(cluster, &pc_base(settings), proto) {
        log::info!("[sync] host advertises no app_journal — history backfill not available");
        return Ok(None);
    }
    let conn = datasets::dictation::open_app_db(settings)?;
    ensure_sync_tables(&conn)?;
    if !force && app_history_backfilled(&conn).is_some() {
        return Ok(None);
    }
    if !matches!(proto, Some(p) if p >= HISTORY_PROTOCOL) {
        log::info!("[sync] hub has no /app/state (protocol {proto:?}) — history backfill skipped");
        return Ok(None);
    }

    let user_key = crate::auth::workspace_identity(settings);

    // Page both tables to the end: the hub caps one *request*, not the history, so a
    // large ledger arrives over several GETs. The two cursors advance independently
    // because the lists differ in length and end on different pages.
    let mut after_dictation = 0i64;
    let mut after_xp = 0i64;
    let mut pages = 0i64;
    let mut progress_rows = 0i64;
    let mut xp_row_count = 0i64;
    let mut progress_applied = 0i64;
    let mut xp_applied = 0i64;
    let mut skipped = 0i64;
    let mut hub_total: Option<i64> = None;
    let mut unfinished = false;
    let mut failures: Vec<String> = Vec::new();
    let _guard = crate::sync::change_log::ApplyGuard::new();
    loop {
        pages += 1;
        let page = match pc_get_json(
            settings,
            "/api/v1/app/state",
            &[
                ("user_key", user_key.clone()),
                ("after_dictation", after_dictation.to_string()),
                ("after_xp", after_xp.to_string()),
            ],
        )
        .await
        {
            Ok(p) => p,
            Err(e) => {
                failures.push(e);
                unfinished = true;
                break;
            }
        };
        hub_total = page["lifetime_xp"].as_i64().or(hub_total);
        let dictation = page["dictation"].as_array().cloned().unwrap_or_default();
        let xp_rows = page["xp"].as_array().cloned().unwrap_or_default();
        progress_rows += dictation.len() as i64;
        xp_row_count += xp_rows.len() as i64;
        for row in &dictation {
            // An un-pushed local edit to this pair beats the hub's stored state:
            // an import must never undo practice done here while offline.
            let object_id = crate::sync::change_log::object_id_for("dictation", row, &user_key);
            if has_pending_local_edit(&conn, scope, "dictation", &object_id) {
                skipped += 1;
                continue;
            }
            let Ok(d) = serde_json::from_value::<datasets::dictation::ListenDictation>(row.clone())
            else {
                skipped += 1;
                continue;
            };
            match datasets::dictation::listen_save_dictation_as(settings, "", &d, &user_key).await {
                Ok(_) => progress_applied += 1,
                Err(e) => failures.push(format!("dictation {}: {e}", d.media_uuid)),
            }
        }
        for row in &xp_rows {
            let source = row["source"].as_str().unwrap_or("");
            let reference = row["reference_id"].as_str().unwrap_or("");
            let amount = row["amount"].as_i64().unwrap_or(0);
            if source.is_empty() || amount == 0 {
                continue;
            }
            let ds = row["dataset_uuid"].as_str().filter(|s| !s.is_empty());
            // No pending-edit guard on this half: the award path dedups on exactly
            // (user, source, reference), so re-importing an award made here and not
            // yet pushed is a no-op rather than a double count.
            match crate::xp::xp_award_internal(settings, &user_key, amount, source, reference, ds) {
                Ok(r) if r.is_new => xp_applied += 1,
                Ok(_) => skipped += 1,
                Err(e) => failures.push(format!("xp {source}/{reference}: {e}")),
            }
        }
        let next_dictation = page["next_dictation_after"].as_i64();
        let next_xp = page["next_xp_after"].as_i64();
        if next_dictation.is_none() && next_xp.is_none() {
            break;
        }
        if pages >= HISTORY_PAGE_BUDGET {
            log::warn!("[sync] app history hit the {pages}-page budget with rows still pending");
            unfinished = true;
            break;
        }
        after_dictation = next_dictation.unwrap_or(after_dictation);
        after_xp = next_xp.unwrap_or(after_xp);
    }
    // The ledger is now the union of local and imported awards, so read the total
    // back off it. Falls back to the hub's figure if the local recompute failed —
    // this number is reporting only, and a report of 0 would look like data loss.
    let lifetime_xp = crate::xp::recompute_totals(settings, &user_key).unwrap_or_else(|e| {
        log::warn!("[sync] could not recompute XP total after backfill: {e}");
        hub_total.unwrap_or(0)
    });
    // Only stamp a *complete* import: a failed row or an interrupted page walk means
    // the next round should retry rather than treat the history as settled.
    let complete = failures.is_empty() && !unfinished;
    if complete {
        set_app_history_backfilled(&conn)?;
    }

    let result = json!({
        "user_key": user_key,
        "progress_rows": progress_rows,
        "progress_applied": progress_applied,
        "xp_rows": xp_row_count,
        "xp_applied": xp_applied,
        "skipped": skipped,
        "pages": pages,
        "lifetime_xp": lifetime_xp,
        "hub_lifetime_xp": hub_total,
        "complete": complete,
        "failures": failures.iter().take(5).cloned().collect::<Vec<_>>(),
    });
    log::info!("[sync] app history backfill: {result}");
    Ok(Some(result))
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

/// Run one full incremental sync round against the connected hub. Triggers:
/// app start, after a local write, a timer, or a network change (§3.3).
pub async fn sync_round_inner(
    app: AppHandle,
    settings: State<'_, SettingsState>,
) -> Result<Value, String> {
    if pc_base(&settings).is_empty() {
        return Err("No PC configured. Use Discover PC or set the PC address.".into());
    }
    // Website hub (T4): it serves no row journal (`/changes`, `/app/changes`,
    // `/app/state` — dev guide §6), so one round is file-level only: catalog ->
    // per-dataset manifest hash -> snapshot re-pull on drift, plus prune-by-
    // absence. Card datasets additionally run their own Bearer pull/push.
    let base = pc_base(&settings);
    let is_website = super::hub_kind(&base) == super::HubKind::Website;
    // Provenance of the host this round is talking to (sync-v2.md §6.1). Every
    // write to `dataset_sync_state` / `app_sync_state` in this round stamps
    // these two values; every prune-by-absence checks against them (INV-7).
    let (connected_kind, connected_origin) = upstream_of(&base);
    let connected = (connected_kind, connected_origin.as_str());
    if is_website {
        log::info!(
            "[sync] website hub contract v{} round",
            super::WEBSITE_CONTRACT_VERSION
        );
    }
    // Phase-1 migration: pre-provenance rows carry a cursor from an unknown
    // host's seq space. NULL them once so the normal "NULL cursor → snapshot
    // resync" path re-establishes each row against whichever host is connected
    // now. Idempotent — after the first round every row has a real origin.
    {
        let conn = datasets::dictation::open_app_db(&settings)?;
        ensure_sync_tables(&conn)?;
        match reset_cursors_without_provenance(&conn) {
            Ok(0) => {}
            Ok(n) => log::info!(
                "[sync] phase-1 migration: reset {n} cursor(s) with unknown provenance"
            ),
            Err(e) => log::warn!("[sync] phase-1 cursor reset failed: {e}"),
        }
    }
    let mut errors: Vec<Value> = Vec::new();
    let mut resynced: Vec<String> = Vec::new();
    let mut pruned: Vec<String> = Vec::new();
    let mut applied = 0i64;
    let mut history: Option<Value> = None;
    let mut cards: Vec<Value> = Vec::new();

    // Capability negotiation (sync-v2.md §7, INV-12): read the host's `/cluster`
    // page once per round. `None` (today's website, or an old/unreachable hub)
    // advertises nothing, so every app-plane gate below default-denies and the
    // round degrades to file-level + card sync — unchanged behavior. `app_ok`
    // drives the INV-20 "held" report; each plane call re-checks for itself.
    let cluster = fetch_cluster(&settings).await;
    let cluster_protocol = match cluster.as_ref() {
        Some(c) => Some(c.protocol_version),
        None => hub_protocol_version(&settings).await,
    };
    let app_ok = app_journal_advertised(&cluster, &base, cluster_protocol);

    // 1. Push local edits first so the hub holds our changes before we pull.
    let pushed = writeback_flush_inner(&settings, &cluster).await.unwrap_or_else(|e| {
        errors.push(json!({ "stage": "push", "error": e }));
        0
    });
    // INV-20: a non-empty queue against a host that does not advertise the app
    // journal is no longer silent — report it as held so the status surface can
    // show "N changes waiting for a hub that accepts them" instead of nothing.
    let held = if app_ok {
        0i64
    } else {
        let n = pending_writeback_count(&settings);
        if n > 0 {
            errors.push(json!({
                "stage": "push",
                "scope": "@app",
                "error": format!("host does not advertise app_journal; {n} change(s) held locally"),
            }));
        }
        n
    };

    // 2. Refresh the hub catalog (uuid -> type) with the cheap `lite` list.
    let mut catalog_ok = true;
    let catalog = fetch_catalog(&settings).await.unwrap_or_else(|e| {
        catalog_ok = false;
        errors.push(json!({ "stage": "catalog", "error": e }));
        Vec::new()
    });
    let mut catalog_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut catalog_type: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for d in &catalog {
        let u = d["uuid"].as_str().unwrap_or("").to_string();
        let t = d["dataset_type"].as_str().unwrap_or("").to_string();
        if !u.is_empty() {
            catalog_map.insert(u.clone(), t.clone());
            catalog_type.insert(u, t);
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
                match prune_local_dataset(&settings, &conn, &uuid, connected) {
                    PruneOutcome::Deleted => pruned.push(uuid),
                    PruneOutcome::SkippedProvenance | PruneOutcome::SkippedWithdrawn => {
                        // Not an error and not a prune: the copy stays, the row
                        // stays, and the next round against the recorded upstream
                        // is the one that can act on it (INV-7, INV-22).
                    }
                    PruneOutcome::Missing => {}
                }
            }
            continue;
        }

        let cursor_is_hash = cursor_opt.is_some() && !stored_hash.is_empty();
        if is_website {
            // File drift only: the website's manifest hash covers the same
            // frozen non-DB recipe (§4.1); DB rows travel through card sync.
            if !cursor_is_hash {
                match dataset_sync_snapshot(app.clone(), settings.clone(), uuid.clone()).await {
                    Ok(_) => resynced.push(uuid.clone()),
                    Err(e) => errors.push(json!({ "uuid": uuid, "stage": "snapshot", "error": e })),
                }
            } else {
                match fetch_manifest_hash(&settings, &uuid).await {
                    Ok(h) if !h.is_empty() && h != stored_hash => {
                        match dataset_sync_snapshot(app.clone(), settings.clone(), uuid.clone()).await
                        {
                            Ok(_) => resynced.push(uuid.clone()),
                            Err(e) => {
                                errors.push(json!({ "uuid": uuid, "stage": "snapshot", "error": e }))
                            }
                        }
                    }
                    Err(e) => {
                        errors.push(json!({ "uuid": uuid, "stage": "manifest", "error": e }))
                    }
                    _ => {}
                }
            }
            // Catalog rows absent locally are left to the explicit per-dataset
            // Download — a round never auto-pulls media the user hasn't adopted.
            if catalog_type.get(&uuid).map(String::as_str) == Some("card") {
                let local_copy = type_root(&settings, "card").join(&uuid).exists();
                if local_copy {
                    match crate::datasets::cards::sync::card_sync_full(settings.clone(), uuid.clone()).await
                    {
                        Ok(r) => cards.push(json!({
                            "uuid": uuid,
                            "pulled": r.pulled,
                            "pushed": r.pushed,
                            "conflicts": r.conflicts,
                        })),
                        Err(e) => {
                            errors.push(json!({ "uuid": uuid, "stage": "card_sync", "error": e }))
                        }
                    }
                }
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
            // Advance the row cursor to the hub's high-water mark, and stamp
            // provenance if the row does not yet have it (a pre-migration copy
            // reaching its first incremental round against a real host). We do
            // *not* overwrite provenance on a partial round — only a snapshot
            // re-establishes a copy from scratch (INV-7).
            let conn = datasets::dictation::open_app_db(&settings)?;
            ensure_sync_tables(&conn)?;
            let _ = conn.execute(
                "UPDATE dataset_sync_state SET \"cursor\" = ?2 WHERE dataset_uuid = ?1",
                params![uuid, hub_seq],
            );
            let _ = conn.execute(
                "UPDATE dataset_sync_state \
                 SET upstream_kind = ?2, upstream_origin = ?3 \
                 WHERE dataset_uuid = ?1 AND (upstream_origin IS NULL OR upstream_origin = '')",
                params![uuid, connected_kind, connected_origin],
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

    // 5. Per-user app data (`dictation` progress + `xp` awards) journals under its
    //    own scope, so it is pulled once per round regardless of which datasets
    //    this device subscribes to (see [`pull_app_scope`]). The plane runs only
    //    against a host that advertises `app_journal` (INV-20); `pull_app_scope`
    //    re-checks and skips cleanly when the host does not.
    let app = pull_app_scope(&settings, &cluster, cluster_protocol, &mut errors)
        .await
        .unwrap_or_else(|e| {
            errors.push(json!({ "scope": "@app", "stage": "app_changes", "error": e }));
            AppPull { applied: 0, needs_history: false, hub_protocol: None }
        });
    applied += app.applied;

    // 6. History backfill (§3.6): the journal is forward-only and pruned, so a
    //    device that has never completed a state read — new, or behind a gap — is
    //    still missing everything older than its cursor. One extra GET until it
    //    succeeds, then the stamp in `app_sync_state` keeps it out of every
    //    following round.
    if app.needs_history {
        match backfill_app_history(settings.inner(), &cluster, app.hub_protocol, false).await {
            Ok(Some(v)) => history = Some(v),
            // `Ok(None)` = hub predates the endpoint, already stamped. Expected,
            // not a fault — `pull_app_scope` logged the reason at info level.
            Ok(None) => {}
            Err(e) => errors.push(json!({ "scope": "@app", "stage": "history", "error": e })),
        }
    }

    Ok(json!({
        "status": if errors.is_empty() { "ok" } else { "partial" },
        "hub_kind": if is_website { "website" } else { "lan" },
        "pushed": pushed,
        "held": held,
        "applied": applied,
        "resynced": resynced,
        "pruned": pruned,
        "cards": cards,
        "history": history,
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

// ---------------------------------------------------------------------------
// Remote PC browsing helper
// ---------------------------------------------------------------------------
// The wiki and chat pages on the phone are pure read-through proxies: listing,
// reading, and searching hit the paired PC's `/api/v1/*` REST endpoints over
// native reqwest (WebView `fetch()` would be CORS-blocked — see module docs),
// and nothing is ever stored locally. Compiled only on mobile; the desktop
// build serves the same data straight from its own subsystems.

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
    let qs = build_query(query);
    hub_get_json(settings, api_path, &qs).await
}

// ---------------------------------------------------------------------------
// Hub wiki browse (read-only) — `wiki_hub_*` commands
//
// The "Hub" button in the wiki page: browse and search the hub's syncable wiki
// datasets without downloading anything. Deliberately not `cfg`-gated: a
// desktop running `role = "follower"` uses the same proxies to reach the hub.
// Reads only — there is no mutating wiki-dataset route on the hub.
// ---------------------------------------------------------------------------

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
        let uuid = "deck-web";
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
        let uuid = "deck-lan";
        {
            let conn = datasets::dictation::open_app_db(&settings).unwrap();
            ensure_sync_tables(&conn).unwrap();
            insert_state(&conn, uuid, "desktop_hub", "http://192.168.1.50:8756", "listed");
        }
        // A LAN-upstreamed deck travels through the dataset journal, not the
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
        let uuid = "deck-fresh";
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
        // The fallback stamped the connected host as this deck's upstream.
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
        // ...but the deck is recorded against the website. INV-19: refuse.
        let website = crate::auth::base_url();
        let (ck, co) = upstream_of(&website);
        let uuid = "deck-split";
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
}
