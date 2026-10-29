//! PC auto-discovery for the dataset sync page.
//!
//! Compiled on all platforms; used by the Android thin client and by a desktop
//! that wants to pull datasets from another desktop. It
//! finds the PC `web_service` in four ways and returns ranked candidates:
//!
//! 1. **LAN / WLAN** — UDP probe `fms-probe` to the broadcast + multicast group
//!    on port 35712 and collect the JSON replies (`sync::server::spawn_discovery`).
//!    Sent once from an unbound socket *and* once from every local address,
//!    because binding the source address is the only way to pin the egress
//!    interface — see [`discover_lan`].
//! 2. **LAN sweep** — `/api/v1/status` over plain TCP on the local /24, used only
//!    when the probe heard nothing. It sends no broadcast, so it survives every
//!    network that blocks, filters or misroutes one ([`discover_sweep`]).
//! 3. **Tailscale** — if a tailnet address (100.64.0.0/10) is active, TCP-probe
//!    the phone's own tailnet /24 on the HTTP port and validate via
//!    `GET /api/v1/status`.
//! 4. **Saved** — revalidate the remembered `pc_url` from settings.
//!
//! Note: receiving WLAN broadcast/multicast on Android additionally requires the
//! `CHANGE_WIFI_MULTICAST_STATE` permission and a held `MulticastLock`; where
//! that is unavailable the TCP probe + saved URL paths still work.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use tauri::{State};

use crate::settings::SettingsState;

const DISCOVERY_PORT: u16 = 35712;
const DEFAULT_HTTP_PORT: u16 = 35711;
/// The private probe datagram the desktop responder answers
/// (`sync::server::spawn_discovery`).
const PROBE: &[u8] = b"fms-probe";

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Candidate {
    pub url: String,
    /// "lan" | "sweep" | "tailscale" | "saved". `sweep` is the TCP fallback that
    /// only runs when the broadcast probe came back empty — seeing it names the
    /// reason: this machine's broadcast never left, or was filtered.
    pub source: String,
    pub name: String,
    /// Hostname the machine advertises. `name` is the *app* name — identical on
    /// every candidate — so this is what tells two rows apart in a scanned list.
    /// Empty for peers that predate the field. Additive, so it costs no protocol
    /// version bump.
    #[serde(default)]
    pub machine: String,
    /// Sync role advertised by that machine ("hub" | "follower"). Both desktops
    /// run the web service, so the phone uses this to offer only hubs as sync
    /// targets (§3.1). Empty for beacons/status predating the field.
    #[serde(default)]
    pub role: String,
    /// Cluster id the machine belongs to (empty when not yet designated).
    #[serde(default)]
    pub cluster_id: String,
}

/// Discover nearby FmS PCs. Returns candidates ranked lan -> sweep -> tailscale -> saved.
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

    // Every local address this host can source from. It seeds the sweep below and
    // it is what the self filter compares against, because a multi-homed machine
    // hears its own beacon from whichever address that interface happens to have.
    let my_ips = local_addrs();

    // 1. LAN UDP probe (blocking sockets on a helper thread).
    let lan = tauri::async_runtime::spawn_blocking(move || discover_lan(timeout)).await
        .unwrap_or_default();
    candidates.extend(lan);
    let heard_lan = !candidates.is_empty();

    // 2. The TCP sweep is a fallback, so it only runs when the probe heard
    //    nothing; it and the Tailscale /24 probe are independent of each other.
    let (sweep, ts) = tokio::join!(
        async {
            if heard_lan {
                Vec::new()
            } else {
                discover_sweep(&my_ips, timeout).await
            }
        },
        discover_tailscale(timeout),
    );
    candidates.extend(sweep);
    candidates.extend(ts);

    // 3. Saved URL revalidation.
    if let Some(c) = discover_saved(&saved_url, &saved_token, timeout).await {
        candidates.push(c);
    }

    // Deduplicate by URL, preserving the earlier (higher-ranked) source, and
    // drop candidates that point back at this very machine.
    let mut out: Vec<Candidate> = Vec::new();
    for c in candidates {
        if is_self(&c.url, &my_ips) {
            log::debug!("[discover] ignoring self candidate {}", c.url);
            continue;
        }
        if !out.iter().any(|e| e.url == c.url) {
            out.push(c);
        }
    }
    Ok(out)
}

/// Every local address this host can source traffic from, default route first.
///
/// The broadcast answer is the one that matters: it is the address the OS would
/// put in a limited-broadcast datagram, and Windows picks that egress from the
/// adapter metrics alone. A machine whose best `255.255.255.255/32` route is a
/// virtual switch (WSL / Hyper-V / a VPN TAP) or the loopback interface therefore
/// loses every probe silently — its own responder is the only thing that answers
/// and [`is_self`] drops that. Binding a socket to an address found here is what
/// pins a probe to a real interface.
fn local_addrs() -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = Vec::new();
    for target in [
        "8.8.8.8:80".to_string(),
        format!("255.255.255.255:{}", DISCOVERY_PORT),
        format!("224.0.1.87:{}", DISCOVERY_PORT),
    ] {
        if let Some(ip) = source_ip_for(&target) {
            if !out.contains(&ip) {
                out.push(ip);
            }
        }
    }
    out
}

/// The local address the OS would source a datagram to `target` from. No packet
/// leaves the socket: a UDP `connect` only runs the route lookup.
fn source_ip_for(target: &str) -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect(target).ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_unspecified() && !v4.is_link_local() => {
            Some(IpAddr::V4(v4))
        }
        _ => None,
    }
}

/// RFC 1918 space — the only subnets the sweep walks. Public and CGNAT addresses
/// (a VPN tunnel, a tailnet) are deliberately excluded: probing 254 strangers is
/// not discovery.
fn is_private_v4(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    o[0] == 10 || (o[0] == 172 && (16..=31).contains(&o[1])) || (o[0] == 192 && o[1] == 168)
}

/// Does `url` address this machine? A desktop running the sync page also runs
/// the discovery beacon, so its own web service answers its own broadcast and
/// multicast probe; without this check the PC would be offered as a sync source
/// for the datasets it already holds. Every local address counts, not just the
/// default-route one.
fn is_self(url: &str, my_ips: &[IpAddr]) -> bool {
    let Some(rest) = url.split("://").nth(1) else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or(rest).split(':').next().unwrap_or("");
    if host == "localhost" || host.starts_with("127.") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(h) => my_ips.contains(&h),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// LAN (WLAN) UDP probe
// ---------------------------------------------------------------------------

/// Say it out loud when the OS would route a limited broadcast through the
/// loopback interface. The probe then never leaves the machine, the only reply is
/// the machine's own, [`is_self`] filters it, and the scan shows an empty list
/// with nothing anywhere to explain it — this warning is the whole diagnosis.
fn warn_if_broadcast_is_looped() {
    let sock = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(_) => return,
    };
    if sock.connect(format!("255.255.255.255:{}", DISCOVERY_PORT)).is_err() {
        return;
    }
    if sock.local_addr().map(|a| a.ip().is_loopback()).unwrap_or(false) {
        log::warn!(
            "[discover] the OS routes limited broadcasts via the loopback interface, so the LAN \
             probe cannot leave this machine (a virtual/VPN adapter or the loopback route \
             outranking WLAN is the usual cause) — the /24 sweep is the working path"
        );
    }
}

fn discover_lan(timeout: Duration) -> Vec<Candidate> {
    let mut out = Vec::new();
    warn_if_broadcast_is_looped();

    // One sender per local address, plus the unbound one this app has always
    // used. Binding pins both the egress interface and the source IP the
    // responder replies to, which is the entire fix for a multi-homed machine:
    // an unbound socket lets the OS choose from the route table, and the route
    // table chooses from adapter metrics.
    let addrs = local_addrs();
    let mut senders: Vec<UdpSocket> = Vec::new();
    for ip in &addrs {
        match UdpSocket::bind((*ip, 0)) {
            Ok(s) => senders.push(s),
            Err(e) => log::debug!("[discover] cannot probe from {}: {}", ip, e),
        }
    }
    let unbound = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => Some(s),
        Err(e) => {
            log::warn!("[discover] cannot bind an unbound probe socket: {}", e);
            None
        }
    };
    if let Some(s) = unbound {
        senders.push(s);
    }
    if senders.is_empty() {
        log::warn!("[discover] no socket to probe from — LAN discovery skipped");
        return out;
    }

    for (i, sock) in senders.iter().enumerate() {
        let _ = sock.set_broadcast(true);
        let _ = sock.set_read_timeout(Some(Duration::from_millis(200)));
        if i + 1 == senders.len() {
            // The multicast join stays on the unbound sender only: it exists so
            // the periodic beacon can reach us, and a bound socket would narrow
            // the membership to one interface.
            let _ = sock.join_multicast_v4(&Ipv4Addr::new(224, 0, 1, 87), &Ipv4Addr::UNSPECIFIED);
        }
        let _ = sock.send_to(PROBE, ("255.255.255.255", DISCOVERY_PORT));
        let _ = sock.send_to(PROBE, ("224.0.1.87", DISCOVERY_PORT));
    }
    log::debug!(
        "[discover] LAN probe sent from {} socket(s), sources {:?}",
        senders.len(),
        addrs
    );

    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 2048];
    while Instant::now() < deadline {
        for sock in &senders {
            if let Ok((n, peer)) = sock.recv_from(&mut buf) {
                if let Some(c) = parse_beacon(&buf[..n], peer) {
                    if !out.iter().any(|e: &Candidate| e.url == c.url) {
                        log::debug!("[discover] LAN reply from {} ({})", c.url, c.machine);
                        out.push(c);
                    }
                }
            }
        }
    }
    log::debug!("[discover] LAN probe heard {} candidate(s)", out.len());
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
    let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("").to_string();
    let cluster_id = v.get("cluster_id").and_then(|c| c.as_str()).unwrap_or("").to_string();
    let machine = v.get("machine").and_then(|m| m.as_str()).unwrap_or("").to_string();
    Some(Candidate {
        url: format!("http://{}:{}", peer.ip(), port),
        source: "lan".to_string(),
        name,
        machine,
        role,
        cluster_id,
    })
}

// ---------------------------------------------------------------------------
// LAN sweep (TCP fallback)
// ---------------------------------------------------------------------------

/// Last-resort LAN discovery over plain TCP: ask `/api/v1/status` on every host of
/// the local /24.
///
/// It runs only when the broadcast probe heard nothing, and it sends no broadcast
/// at all — which is the point, because every way that probe dies on a real
/// network is a broadcast problem: a limited broadcast routed into a virtual
/// switch or the loopback interface, a firewall dropping unicast replies to
/// broadcast traffic, or a Wi-Fi network that filters broadcasts between clients.
/// Costs 254 short requests, and only on the scan that needs it.
///
/// Only the default HTTP port is tried: a peer whose service moved to another
/// port is exactly the peer whose saved address is needed anyway.
async fn discover_sweep(seeds: &[IpAddr], timeout: Duration) -> Vec<Candidate> {
    let mut hosts: Vec<Ipv4Addr> = Vec::new();
    for ip in seeds {
        let IpAddr::V4(v4) = ip else { continue };
        if !is_private_v4(*v4) {
            continue;
        }
        let o = v4.octets();
        for host in 1..=254u8 {
            let candidate = Ipv4Addr::new(o[0], o[1], o[2], host);
            if candidate != *v4 && !hosts.contains(&candidate) {
                hosts.push(candidate);
            }
        }
    }
    if hosts.is_empty() {
        return Vec::new();
    }

    // No point spending the whole scan budget per host: an unused address either
    // refuses at once or never answers at all.
    let per = timeout.min(Duration::from_millis(900));
    let client = match reqwest::Client::builder().timeout(per).build() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    log::debug!("[discover] LAN probe silent — sweeping {} host(s) over HTTP", hosts.len());

    let futs: Vec<_> = hosts
        .into_iter()
        .map(|host| {
            let client = client.clone();
            async move {
                let url = format!("http://{}:{}/api/v1/status", host, DEFAULT_HTTP_PORT);
                let r = client.get(&url).send().await.ok()?;
                if !r.status().is_success() {
                    return None;
                }
                let v: serde_json::Value = r.json().await.ok()?;
                if v.get("ok").and_then(|o| o.as_bool()) != Some(true) {
                    return None;
                }
                log::debug!("[discover] LAN sweep found {}", url);
                Some(Candidate {
                    url: format!("http://{}:{}", host, DEFAULT_HTTP_PORT),
                    source: "sweep".to_string(),
                    name: v
                        .get("app_name")
                        .and_then(|a| a.as_str())
                        .unwrap_or("FmS PC")
                        .to_string(),
                    machine: v.get("machine").and_then(|m| m.as_str()).unwrap_or("").to_string(),
                    role: v.get("role").and_then(|r| r.as_str()).unwrap_or("").to_string(),
                    cluster_id: v
                        .get("cluster_id")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
            }
        })
        .collect();

    join_all(futs).await.into_iter().flatten().collect()
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
                    machine: v.get("machine").and_then(|m| m.as_str()).unwrap_or("").to_string(),
                    role: v.get("role").and_then(|r| r.as_str()).unwrap_or("").to_string(),
                    cluster_id: v.get("cluster_id").and_then(|c| c.as_str()).unwrap_or("").to_string(),
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
        machine: v.get("machine").and_then(|m| m.as_str()).unwrap_or("").to_string(),
        role: v.get("role").and_then(|r| r.as_str()).unwrap_or("").to_string(),
        cluster_id: v.get("cluster_id").and_then(|c| c.as_str()).unwrap_or("").to_string(),
    })
}
