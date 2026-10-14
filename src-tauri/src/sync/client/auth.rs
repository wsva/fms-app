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


// ---------------------------------------------------------------------------
// Device identity + request signing (pairing v1, mirrors `pairing.rs` server-side)
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

/// Loaded device identity: keypair + derived `device_id` / public key hex.
pub(crate) struct DeviceIdentity {
    pub(crate) key_pair: Ed25519KeyPair,
    pub(crate) device_id: String,
    pub(crate) pubkey_hex: String,
}

pub(crate) fn identity_from_seed(seed_hex: &str) -> Result<DeviceIdentity, String> {
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
pub(crate) fn ensure_device_identity(settings: &SettingsState) -> Result<DeviceIdentity, String> {
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
/// holds either a LAN PC origin or the website origin — [`crate::sync::hub_kind`]
/// classifies it per call, so the website path is purely additive and the LAN
/// signing path stays byte-identical (dev guide §10.4).
pub(crate) fn hub_base(settings: &SettingsState) -> Result<String, String> {
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
    match crate::sync::hub_kind(&pc_base(settings)) {
        crate::sync::HubKind::Website => match crate::auth::get_access_token(settings) {
            Ok(token) => Ok(req.header("Authorization", format!("Bearer {}", token))),
            Err(_) => Ok(req),
        },
        crate::sync::HubKind::LanPc => with_device_auth(req, settings, method, path, query, body),
    }
}

/// Map a non-2xx hub response to an app message that keeps the status code and
/// says what to do next (dev guide §T7). For the website the codes carry fixed
/// meanings (§2): 401 = anonymous/token problem, 403 = identified but not
/// allowed — a token refresh never fixes that, so the message asks for access,
/// not for a re-login. 5xx stays verbatim and marked retryable.
pub(crate) async fn hub_http_error(resp: reqwest::Response, api_path: &str) -> String {
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
        410 => format!(
            "the publisher took this version down (410) — your installed copy stays and \
             remains practicable (INV-22){}",
            if server_msg.is_empty() { String::new() } else { format!(" — {server_msg}") }
        ),
        413 => format!("too large for the hub's limits (413): {server_msg}"),
        c => format!("hub request '{api_path}' failed (HTTP {c}): {server_msg}"),
    }
}

/// Signed/bearer GET returning parsed JSON, auth chosen by origin (T1). The
/// shared route for every website-capable hub read: catalog, manifest, file.
pub(crate) async fn hub_get_json(
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
        && crate::sync::hub_kind(&base) == crate::sync::HubKind::Website
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


