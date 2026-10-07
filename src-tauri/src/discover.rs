//! PC auto-discovery for the dataset sync page.
//!
//! Compiled on all platforms; used by the Android thin client and by a desktop
//! that wants to pull datasets from another desktop. It
//! finds the PC `web_service` in three ways and returns ranked candidates:
//!
//! 1. **LAN / WLAN** — UDP probe `fms-probe` to the broadcast + multicast group
//!    on port 35712 and collect the JSON replies (`web_service::spawn_discovery`).
//! 2. **Tailscale** — if a tailnet address (100.64.0.0/10) is active, TCP-probe
//!    the phone's own tailnet /24 on the HTTP port and validate via
//!    `GET /api/v1/status`.
//! 3. **Saved** — revalidate the remembered `pc_url` from settings.
//!
//! Note: receiving WLAN broadcast/multicast on Android additionally requires the
//! `CHANGE_WIFI_MULTICAST_STATE` permission and a held `MulticastLock`; where
//! that is unavailable the TCP probe + saved URL paths still work.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use tauri::{State};

use crate::settings::SettingsState;

#[allow(dead_code)]
const DISCOVERY_PORT: u16 = 35712;
const DEFAULT_HTTP_PORT: u16 = 35711;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Candidate {
    pub url: String,
    /// "lan" | "tailscale" | "saved"
    pub source: String,
    pub name: String,
}

/// Discover nearby FmS PCs. Returns candidates ranked lan -> tailscale -> saved.
#[tauri::command]
pub async fn pc_discover(
    settings: State<'_, SettingsState>,
    timeout_ms: u64,
) -> Result<Vec<Candidate>, String> {
    let timeout = Duration::from_millis(if timeout_ms == 0 { 2000 } else { timeout_ms });
    let saved_url = settings.settings.lock().unwrap().pc_url.clone();
    let saved_token = settings.settings.lock().unwrap().pc_token.clone();
    drop(settings);

    let mut candidates: Vec<Candidate> = Vec::new();

    // 1. LAN UDP probe (blocking socket on a helper thread).
    let lan = tauri::async_runtime::spawn_blocking(move || discover_lan(timeout)).await
        .unwrap_or_default();
    candidates.extend(lan);

    // 2. Tailscale /24 TCP probe.
    let ts = discover_tailscale(timeout).await;
    candidates.extend(ts);

    // 3. Saved URL revalidation.
    if let Some(c) = discover_saved(&saved_url, &saved_token, timeout).await {
        candidates.push(c);
    }

    // Deduplicate by URL, preserving the earlier (higher-ranked) source, and
    // drop candidates that point back at this very machine.
    let my_ip = local_ip();
    let mut out: Vec<Candidate> = Vec::new();
    for c in candidates {
        if is_self(&c.url, my_ip) {
            log::debug!("[discover] ignoring self candidate {}", c.url);
            continue;
        }
        if !out.iter().any(|e| e.url == c.url) {
            out.push(c);
        }
    }
    Ok(out)
}

/// Best-effort primary local address (no packet leaves the socket).
fn local_ip() -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    Some(sock.local_addr().ok()?.ip())
}

/// Does `url` address this machine? A desktop running the sync page also runs
/// the discovery beacon, so its own web service answers its own broadcast and
/// multicast probe; without this check the PC would be offered as a sync source
/// for the datasets it already holds.
fn is_self(url: &str, my_ip: Option<IpAddr>) -> bool {
    let Some(rest) = url.split("://").nth(1) else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or(rest).split(':').next().unwrap_or("");
    if host == "localhost" || host.starts_with("127.") {
        return true;
    }
    match (host.parse::<IpAddr>().ok(), my_ip) {
        (Some(h), Some(m)) => h == m,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// LAN (WLAN) UDP probe
// ---------------------------------------------------------------------------

fn discover_lan(timeout: Duration) -> Vec<Candidate> {
    let mut out = Vec::new();
    let sock = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(_) => return out,
    };
    let _ = sock.set_broadcast(true);
    let _ = sock.set_read_timeout(Some(Duration::from_millis(300)));

    // Multicast join so the periodic beacon can also reach us (best-effort).
    let _ = sock.join_multicast_v4(&[224, 0, 1, 87].into(), &std::net::Ipv4Addr::UNSPECIFIED);

    let msg = b"fms-probe";
    let _ = sock.send_to(msg, "255.255.255.255:35712");
    let _ = sock.send_to(msg, "224.0.1.87:35712");

    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 2048];
    while Instant::now() < deadline {
        if let Ok((n, peer)) = sock.recv_from(&mut buf) {
            if let Some(c) = parse_beacon(&buf[..n], peer) {
                if !out.iter().any(|e: &Candidate| e.url == c.url) {
                    out.push(c);
                }
            }
        }
    }
    out
}

/// Parse a discovery beacon reply. `peer` is the source address we heard from,
/// which is authoritative for the reachable IP on this network segment.
fn parse_beacon(bytes: &[u8], peer: SocketAddr) -> Option<Candidate> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    if v.get("app").and_then(|a| a.as_str()) != Some("fms-app") {
        return None;
    }
    let port = v.get("http_port").and_then(|p| p.as_u64()).unwrap_or(DEFAULT_HTTP_PORT as u64) as u16;
    let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("FmS PC").to_string();
    Some(Candidate {
        url: format!("http://{}:{}", peer.ip(), port),
        source: "lan".to_string(),
        name,
    })
}

// ---------------------------------------------------------------------------
// Tailscale /24 TCP probe
// ---------------------------------------------------------------------------

/// Return the phone's active tailnet address (100.64.0.0/10), if any. Uses the
/// classic "connect a UDP socket and read back the local address" trick against
/// the CGNAT range: if a tailnet route exists the OS sources through it.
fn tailscale_local_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("100.64.0.1:41641").ok()?;
    let ip = s.local_addr().ok()?.ip();
    match ip {
        IpAddr::V4(v4) if v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0 == 64) => Some(ip),
        _ => None,
    }
}

async fn discover_tailscale(timeout: Duration) -> Vec<Candidate> {
    let ip = match tailscale_local_ip() {
        Some(i) => i,
        None => return Vec::new(),
    };
    let oct = match ip {
        IpAddr::V4(v4) => v4.octets(),
        _ => return Vec::new(),
    };
    // Probe the phone's own /24 (a==b==c fixed, host 1..=254).
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .unwrap_or_default();

    let futs: Vec<_> = (1..=254)
        .map(|host| {
            let client = client.clone();
            let url = format!("http://{}.{}.{}.{}:{}/api/v1/status", oct[0], oct[1], oct[2], host, DEFAULT_HTTP_PORT);
            async move {
                let r = client.get(&url).send().await.ok()?;
                if !r.status().is_success() {
                    return None;
                }
                let v: serde_json::Value = r.json().await.ok()?;
                if v.get("ok").and_then(|o| o.as_bool()) != Some(true) {
                    return None;
                }
                Some(Candidate {
                    url: url.trim_end_matches("/api/v1/status").to_string(),
                    source: "tailscale".to_string(),
                    name: "FmS PC".to_string(),
                })
            }
        })
        .collect();

    join_all(futs).await.into_iter().flatten().collect()
}

// ---------------------------------------------------------------------------
// Saved URL revalidation
// ---------------------------------------------------------------------------

async fn discover_saved(url: &str, token: &str, timeout: Duration) -> Option<Candidate> {
    let base = url.trim_end_matches('/');
    if base.is_empty() {
        return None;
    }
    let client = reqwest::Client::builder().timeout(timeout).build().ok()?;
    let mut req = client.get(format!("{base}/api/v1/status"));
    if !token.is_empty() {
        req = req.header("x-fms-token", token);
    }
    let v: serde_json::Value = req.send().await.ok()?.json().await.ok()?;
    if v.get("ok").and_then(|o| o.as_bool()) != Some(true) {
        return None;
    }
    Some(Candidate {
        url: base.to_string(),
        source: "saved".to_string(),
        name: v
            .get("app_name")
            .and_then(|a| a.as_str())
            .unwrap_or("Saved PC")
            .to_string(),
    })
}
