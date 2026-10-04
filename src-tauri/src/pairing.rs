//! Bluetooth-style device pairing between the PC web service and remote
//! clients (the Android thin client over LAN/WLAN).
//!
//! Three trust zones (enforced by the outer layer in `web_service.rs`):
//! * **Trusted** — loopback (with a DNS-rebinding `Host` check) or Tailscale
//!   CGNAT (`100.64.0.0/10`). No auth at all; this is the agent path.
//! * **Untrusted** — everything else requires a per-device Ed25519 signature
//!   on every request, except the handshake routes `/api/v1/status` and
//!   `/api/v1/pair/*`.
//!
//! Pairing bootstraps a device identity (an Ed25519 keypair held by the
//! phone) into the `paired_devices` registry: the phone posts its pubkey, a
//! `pairing-request` event pops a confirm dialog on the PC showing the shared
//! fingerprint, and the phone polls `/pair/status` until the owner answers.
//!
//! Approval always costs a human click at the PC. There is deliberately no
//! out-of-band one-time-secret path: while the transport is cleartext HTTP, a
//! MITM-proof handshake guards a channel whose payload an on-path attacker can
//! read anyway, and it trades away the human approval for possession of a
//! bearer secret. Revisit a QR/one-time-code bootstrap if TLS
//! (`axum-server` + rustls) is ever added.
//!
//! Later connections verify statelessly: signature over
//! `"{ts}\n{METHOD}\n{path}"` with a ±120 s clock window. The legacy shared
//! `api_token` is no longer enforced.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lazy_static::lazy_static;
use ring::signature::UnparsedPublicKey;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::dictation;
use crate::settings::SettingsState;

/// Lifetime of a pending confirm dialog, in seconds.
const PENDING_TTL_SECS: u64 = 180;
/// Clock skew tolerated on request signatures.
const SIGN_WINDOW_SECS: i64 = 120;
/// Throttle for `last_seen_at` DB writes (per device).
const LAST_SEEN_WRITE_INTERVAL_SECS: u64 = 60;

// ---------------------------------------------------------------------------
// Hex helpers (kept local: pairing.rs is desktop-only, sync.rs is all-platform)
// ---------------------------------------------------------------------------

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

pub(crate) fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("hex string has odd length".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| "invalid hex".to_string()))
        .collect()
}

/// First `n` hex chars of SHA-256(pubkey).
fn pubkey_hash_hex(pubkey_hex: &str, n: usize) -> Result<String, String> {
    let bytes = hex_decode(pubkey_hex)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let hash = hasher.finalize();
    let full = hex_encode(&hash);
    Ok(full.chars().take(n).collect())
}

/// Stable device identity: first 16 hex chars of SHA-256(pubkey).
pub fn device_id_of_pubkey(pubkey_hex: &str) -> Result<String, String> {
    pubkey_hash_hex(pubkey_hex, 16)
}

/// Short human-comparable code: first 6 hex chars of SHA-256(pubkey).
pub fn fingerprint_of_pubkey(pubkey_hex: &str) -> Result<String, String> {
    pubkey_hash_hex(pubkey_hex, 6)
}

// ---------------------------------------------------------------------------
// In-memory state: pending requests, registry cache, last-seen throttle
// ---------------------------------------------------------------------------

lazy_static! {
    /// Dialog requests awaiting the PC owner: request_id -> record.
    static ref PENDING: Mutex<HashMap<String, PendingRequest>> = Mutex::new(HashMap::new());
    /// Cached registry so the auth path never hits SQLite per request.
    /// Invalidated on every mutation (approve / deny / revoke / remove).
    static ref REGISTRY_CACHE: Mutex<Option<HashMap<String, (String, String)>>> =
        Mutex::new(None);
    /// device_id -> last `last_seen_at` DB write, to throttle writes.
    static ref LAST_SEEN: Mutex<HashMap<String, Instant>> = Mutex::new(HashMap::new());
}

struct PendingRequest {
    device_id: String,
    name: String,
    pubkey_hex: String,
    created: Instant,
}

#[derive(serde::Serialize)]
pub struct PairedDevice {
    pub device_id: String,
    pub name: String,
    pub status: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Registry persistence (paired_devices table in the app-level DB)
// ---------------------------------------------------------------------------

fn ensure_table(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS paired_devices(
            device_id TEXT PRIMARY KEY,
            pubkey    TEXT NOT NULL,
            name      TEXT NOT NULL,
            status    TEXT NOT NULL,
            created_at TEXT NOT NULL,
            last_seen_at TEXT
        );",
    )
    .map_err(|e| e.to_string())
}

fn invalidate_cache() {
    *REGISTRY_CACHE.lock().unwrap() = None;
}

/// Load the whole registry (pubkey included) into the cache and return a clone.
fn cached_registry(
    settings: &SettingsState,
) -> Result<HashMap<String, (String, String)>, String> {
    {
        let cache = REGISTRY_CACHE.lock().unwrap();
        if let Some(map) = cache.as_ref() {
            return Ok(map.clone());
        }
    }
    let conn = dictation::open_app_db(settings)?;
    ensure_table(&conn)?;
    let mut stmt = conn
        .prepare("SELECT device_id, pubkey, status FROM paired_devices")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut map = HashMap::new();
    for row in rows {
        let (id, pubkey, status) = row.map_err(|e| e.to_string())?;
        map.insert(id, (pubkey, status));
    }
    *REGISTRY_CACHE.lock().unwrap() = Some(map.clone());
    Ok(map)
}

fn upsert_device(
    settings: &SettingsState,
    device_id: &str,
    pubkey_hex: &str,
    name: &str,
    status: &str,
) -> Result<(), String> {
    let conn = dictation::open_app_db(settings)?;
    ensure_table(&conn)?;
    conn.execute(
        "INSERT INTO paired_devices(device_id, pubkey, name, status, created_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'))
         ON CONFLICT(device_id) DO UPDATE SET
            pubkey=excluded.pubkey, name=excluded.name, status=excluded.status",
        rusqlite::params![device_id, pubkey_hex, name, status],
    )
    .map_err(|e| e.to_string())?;
    invalidate_cache();
    Ok(())
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Pending requests (confirm dialog path)
// ---------------------------------------------------------------------------

/// Outcome of a pairing request.
pub enum PairRequestOutcome {
    /// Already approved in the registry — re-pairing is frictionless.
    Approved,
    /// Blocked by a prior denial — no dialog.
    Denied,
    /// New pending request registered; `pairing-request` event emitted.
    Pending {
        request_id: String,
        fingerprint: String,
    },
}

pub fn request_pair(
    app: &AppHandle,
    settings: &SettingsState,
    pubkey_hex: &str,
    name: &str,
) -> Result<PairRequestOutcome, String> {
    let device_id = device_id_of_pubkey(pubkey_hex)?;
    let status = cached_registry(settings)?
        .get(&device_id)
        .map(|(_, s)| s.clone())
        .unwrap_or_else(|| "none".to_string());
    match status.as_str() {
        "approved" => Ok(PairRequestOutcome::Approved),
        "denied" => Ok(PairRequestOutcome::Denied),
        _ => {
            // Register (or reuse) a pending request for the PC owner's dialog.
            let fingerprint = fingerprint_of_pubkey(pubkey_hex)?;
            let mut map = PENDING.lock().unwrap();
            let now = Instant::now();
            map.retain(|_, p| p.created + Duration::from_secs(PENDING_TTL_SECS) > now);
            let request_id = map
                .iter()
                .find(|(_, p)| p.device_id == device_id)
                .map(|(id, _)| id.clone())
                .unwrap_or_else(|| {
                    let id = Uuid::new_v4().to_string();
                    map.insert(
                        id.clone(),
                        PendingRequest {
                            device_id: device_id.clone(),
                            name: name.to_string(),
                            pubkey_hex: pubkey_hex.to_string(),
                            created: Instant::now(),
                        },
                    );
                    id
                });
            let payload = serde_json::json!({
                "request_id": request_id,
                "device_id": device_id,
                "name": name,
                "fingerprint": fingerprint,
            });
            let _ = app.emit("pairing-request", &payload);
            Ok(PairRequestOutcome::Pending { request_id, fingerprint })
        }
    }
}

pub fn pending_status(settings: &SettingsState, device_id: &str) -> Result<String, String> {
    // A resolved request is reflected through the registry, which is truth.
    if let Some((_, status)) = cached_registry(settings)?.get(device_id) {
        return Ok(status.clone());
    }
    let map = PENDING.lock().unwrap();
    let hit = map.values().any(|p| p.device_id == device_id);
    Ok(if hit { "pending" } else { "none" }.to_string())
}

/// PC owner answered the dialog: write the registry row and drop the request.
pub fn resolve_pending(
    settings: &SettingsState,
    request_id: &str,
    approve: bool,
) -> Result<(), String> {
    let pending = PENDING.lock().unwrap().remove(request_id);
    match pending {
        Some(p) => upsert_device(
            settings,
            &p.device_id,
            &p.pubkey_hex,
            &p.name,
            if approve { "approved" } else { "denied" },
        ),
        None => Err("pairing request no longer pending (expired?)".to_string()),
    }
}

// ---------------------------------------------------------------------------
// Stateless request verification (hot path called by the zone guard)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum AuthError {
    Unpaired,
    Denied,
    StaleTimestamp,
    BadSignature,
    /// The registry could not be read — fail closed with a 500, not a 403.
    Registry,
}

impl AuthError {
    pub fn http_status(&self) -> u16 {
        match self {
            AuthError::Unpaired => 401,
            AuthError::Registry => 500,
            AuthError::Denied | AuthError::StaleTimestamp | AuthError::BadSignature => 403,
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            AuthError::Unpaired => {
                "device not paired — POST /api/v1/pair/request first"
            }
            AuthError::Denied => "pairing denied by PC owner",
            AuthError::StaleTimestamp => "stale x-fms-ts",
            AuthError::BadSignature => "invalid device signature",
            AuthError::Registry => "pairing registry unavailable",
        }
    }
}

/// Verify a signed request: `sig` = hex Ed25519 signature over
/// `"{ts}\n{METHOD}\n{path}"`. Checks the registry cache, the ±120 s window,
/// and (on success) throttled `last_seen_at` bookkeeping.
pub fn verify_request(
    settings: &SettingsState,
    device_id: &str,
    method: &str,
    path: &str,
    ts: &str,
    sig_hex: &str,
) -> Result<(), AuthError> {
    let (pubkey_hex, status) = cached_registry(settings)
        .map_err(|_| AuthError::Registry)?
        .get(device_id)
        .cloned()
        .ok_or(AuthError::Unpaired)?;
    match status.as_str() {
        "approved" => {}
        _ => return Err(AuthError::Denied),
    }

    let ts_num: i64 = ts.parse().map_err(|_| AuthError::StaleTimestamp)?;
    let skew = (unix_now() - ts_num).abs();
    if skew > SIGN_WINDOW_SECS {
        return Err(AuthError::StaleTimestamp);
    }

    let pubkey = hex_decode(&pubkey_hex).map_err(|_| AuthError::BadSignature)?;
    let sig = hex_decode(sig_hex).map_err(|_| AuthError::BadSignature)?;
    let msg = format!("{}\n{}\n{}", ts, method.to_uppercase(), path);
    UnparsedPublicKey::new(&ring::signature::ED25519, &pubkey)
        .verify(msg.as_bytes(), &sig)
        .map_err(|_| AuthError::BadSignature)?;

    touch_last_seen(settings, device_id);
    Ok(())
}

fn touch_last_seen(settings: &SettingsState, device_id: &str) {
    let due = {
        let mut map = LAST_SEEN.lock().unwrap();
        let now = Instant::now();
        match map.get(device_id) {
            Some(t) if now.duration_since(*t) < Duration::from_secs(LAST_SEEN_WRITE_INTERVAL_SECS) => {
                false
            }
            _ => {
                map.insert(device_id.to_string(), now);
                true
            }
        }
    };
    if due {
        if let Ok(conn) = dictation::open_app_db(settings) {
            let _ = conn.execute(
                "UPDATE paired_devices SET last_seen_at = datetime('now') WHERE device_id = ?1",
                rusqlite::params![device_id],
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Registry management (settings UI + MCP tools)
// ---------------------------------------------------------------------------

pub fn list_devices(settings: &SettingsState) -> Result<Vec<PairedDevice>, String> {
    let conn = dictation::open_app_db(settings)?;
    ensure_table(&conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT device_id, name, status, created_at, last_seen_at
             FROM paired_devices ORDER BY created_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(PairedDevice {
                device_id: r.get(0)?,
                name: r.get(1)?,
                status: r.get(2)?,
                created_at: r.get(3)?,
                last_seen_at: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn revoke(settings: &SettingsState, device_id: &str) -> Result<(), String> {
    let conn = dictation::open_app_db(settings)?;
    ensure_table(&conn)?;
    conn.execute("DELETE FROM paired_devices WHERE device_id = ?1", rusqlite::params![device_id])
        .map_err(|e| e.to_string())?;
    invalidate_cache();
    Ok(())
}

pub fn remove_denied(settings: &SettingsState, device_id: &str) -> Result<(), String> {
    let conn = dictation::open_app_db(settings)?;
    ensure_table(&conn)?;
    conn.execute(
        "DELETE FROM paired_devices WHERE device_id = ?1 AND status = 'denied'",
        rusqlite::params![device_id],
    )
    .map_err(|e| e.to_string())?;
    invalidate_cache();
    Ok(())
}

// ---------------------------------------------------------------------------
// Tauri commands (desktop; registered in lib.rs)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn pairing_list(settings: State<'_, SettingsState>) -> Result<Vec<PairedDevice>, String> {
    list_devices(&settings)
}

#[tauri::command]
pub fn pairing_respond(
    settings: State<'_, SettingsState>,
    request_id: String,
    approve: bool,
) -> Result<(), String> {
    resolve_pending(&settings, &request_id, approve)
}

#[tauri::command]
pub fn pairing_revoke(settings: State<'_, SettingsState>, device_id: String) -> Result<(), String> {
    revoke(&settings, &device_id)
}

#[tauri::command]
pub fn pairing_remove_denied(
    settings: State<'_, SettingsState>,
    device_id: String,
) -> Result<(), String> {
    remove_denied(&settings, &device_id)
}
