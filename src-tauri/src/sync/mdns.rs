//! mDNS/DNS-SD as the second discovery beacon.
//!
//! The UDP probe in [`super::server::spawn_discovery`] dies quietly on real
//! networks: a limited broadcast routed into a virtual switch, a firewall that
//! drops unicast replies to broadcast traffic, or a Wi-Fi network that filters
//! broadcasts between clients. mDNS answers those cases differently — the
//! service record is published per *name*, so a client resolves
//! `machine._fms-sync._tcp.local.` to whatever address the hub has *right now*,
//! which is exactly the property auto-connect needs when a DHCP lease moves.
//!
//! Split, and it matters which half is which:
//!
//! * [`resolve_mdns`] runs on **every** platform and feeds the same
//!   [`super::discover::Candidate`] list as the probe, the sweep and the Tailscale
//!   walk, so the Phase 3 reconnect ladder treats an mDNS hit as one more address
//!   to prove with `POST /hub/challenge` — discovery finds, identity decides. It
//!   never trusts a TXT record to *be* the hub.
//! * [`publish_hub`] is desktop-only and hub-only: a phone is a client and
//!   advertises nothing (the same rule as the beacon's `role`).
//!
//! A peer that predates this module simply has no `_fms-sync` record: the
//! candidate list is the union of all sources, so its absence costs nothing.
//!
//! Note for Android: like the UDP probe, mDNS needs the
//! `CHANGE_WIFI_MULTICAST_STATE` permission plus a held `MulticastLock` (see
//! `super::discover`'s module doc). Where that is unavailable this path returns
//! empty and the probe/sweep/tailscale/saved ladder still stands.

use std::collections::HashSet;
use std::time::Duration;

use super::discover::Candidate;

/// The service type we publish and browse. `_tcp` because the thing found is the
/// HTTP service itself, so a resolver gets the port from the SRV record rather
/// than assuming the default.
const SERVICE_TYPE: &str = "_fms-sync._tcp.local.";
/// TXT keys, mirroring the beacon's JSON so the two beacons say the same thing.
/// Short and lowercase for maximum interoperability across mDNS stacks.
const TXT_CLUSTER: &str = "cluster";
const TXT_FINGERPRINT: &str = "fp";
const TXT_ROLE: &str = "role";
const TXT_MACHINE: &str = "machine";
/// A resolved record is only a candidate if it answers on one of these; a
/// zero port would hand the ladder a URL that cannot be dialed.
const DEFAULT_HTTP_PORT: u16 = 35711;

/// Publishing is a single per-process loop no matter how many times a hub
/// starts/stops or switches workspaces (see [`publish_hub`]).
#[cfg(feature = "desktop")]
static PUBLISHER_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Build the candidate list from mDNS service records.
///
/// Kept short and hard-bounded: it runs inside the discovery ladder, so a
/// network with no mDNS at all must cost `timeout` and nothing else. A fresh
/// daemon per scan is deliberate — the alternative is a long-lived browse
/// subscription whose receiver would have to be threaded through every scan,
/// and browse results are cached by the OS stack anyway.
pub async fn resolve_mdns(timeout: Duration) -> Vec<Candidate> {
    let out = tauri::async_runtime::spawn_blocking(move || browse_blocking(timeout))
        .await
        .unwrap_or_default();
    if !out.is_empty() {
        log::debug!("[mdns] resolved {} service record(s)", out.len());
    }
    out
}

/// The synchronous browse. `mdns-sd` drives a background daemon thread and hands
/// back a channel receiver whose `recv` blocks, so it goes on a blocking thread.
fn browse_blocking(timeout: Duration) -> Vec<Candidate> {
    let mdns = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            log::debug!("[mdns] cannot start a browse daemon: {e}");
            return Vec::new();
        }
    };

    let receiver = match mdns.browse(SERVICE_TYPE) {
        Ok(r) => r,
        Err(e) => {
            log::debug!("[mdns] browse failed: {e}");
            let _ = mdns.shutdown();
            return Vec::new();
        }
    };

    let mut out: Vec<Candidate> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        match receiver.recv_timeout(left) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                let Some(c) = to_candidate(&info) else {
                    continue;
                };
                if seen.insert(c.url.clone()) {
                    log::debug!("[mdns] {} -> {}", info.fullname, c.url);
                    out.push(c);
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }

    // Shutting down drops the daemon's thread; leaving it would leak one per scan.
    let _ = mdns.shutdown();
    out
}

/// Turn one resolved record into a candidate. The TXT keys carry the *claims*
/// (`cluster`, `fp`) that the reconnect ladder goes on to prove over HTTP; a
/// record we cannot address is dropped rather than guessed at.
fn to_candidate(info: &mdns_sd::ResolvedService) -> Option<Candidate> {
    let port = match info.port {
        0 => DEFAULT_HTTP_PORT,
        p => p,
    };
    // A link-local or IPv6-only answer is not dialable from another interface, so
    // prefer a routable v4 — the same address family the probe and sweep use.
    let host = info
        .addresses
        .iter()
        .find(|a| a.is_ipv4() && !a.is_loopback())
        .or_else(|| info.addresses.iter().find(|a| a.is_ipv6() && !a.is_loopback()))
        .map(|a| a.to_string())?;

    let txt = |key: &str| {
        info.txt_properties
            .get_property_val_str(key)
            .unwrap_or("")
            .to_string()
    };
    let machine = txt(TXT_MACHINE);
    // `fullname` is `instance._fms-sync._tcp.local.`; the instance name is the
    // hub's machine name, so it is the fallback when TXT has no `machine`.
    let instance = info
        .fullname
        .split('.')
        .next()
        .unwrap_or("")
        .to_string();
    let name = if machine.is_empty() {
        if instance.is_empty() {
            "FmS PC".to_string()
        } else {
            instance.clone()
        }
    } else {
        machine.clone()
    };

    Some(Candidate {
        url: format!("http://{host}:{port}"),
        source: "mdns".to_string(),
        name,
        machine: if machine.is_empty() { instance } else { machine },
        role: txt(TXT_ROLE),
        cluster_id: txt(TXT_CLUSTER),
        hub_fingerprint: txt(TXT_FINGERPRINT),
    })
}

/// Publish this machine's hub service for as long as it serves.
///
/// Desktop-only and started from the web service's own launch points, guarded by
/// a process-wide flag so repeated role changes do not stack daemons. The loop
/// mirrors the UDP beacon's discipline: it reads live settings every tick,
/// re-registers when the advertised identity changed, and unregisters when the
/// web service stops — a record pointing at a port nothing answers is worse than
/// no record, because it outranks the sweep in a scan.
#[cfg(feature = "desktop")]
pub fn publish_hub(app: tauri::AppHandle, http_port: u16) {
    if PUBLISHER_UP.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("fms-mdns-publisher".into())
        .spawn(move || {
            use tauri::Manager;
            let mdns = match mdns_sd::ServiceDaemon::new() {
                Ok(d) => d,
                Err(e) => {
                    PUBLISHER_UP.store(false, std::sync::atomic::Ordering::SeqCst);
                    log::warn!("[mdns] cannot start the publish daemon: {e} (UDP beacon only)");
                    return;
                }
            };
            // What is registered right now: (fullname, TXT signature). Re-
            // registering the same fullname would fail, so it is torn down first.
            let mut current: Option<(String, String)> = None;
            loop {
                let settings = app.state::<crate::settings::SettingsState>();
                let serving = app
                    .try_state::<super::server::WebServiceState>()
                    .map(|s| s.is_running())
                    .unwrap_or(false);
                let wanted = serving && settings.role() == "hub" && !settings.cluster_id().is_empty();
                let props = Props {
                    instance: instance_name(),
                    cluster_id: settings.cluster_id(),
                    hub_fingerprint: settings.hub_fingerprint(),
                    role: settings.role(),
                    signature: String::new(),
                }
                .signed();
                drop(settings);

                if wanted && current.as_ref().map(|(_, sig)| *sig != props.signature).unwrap_or(true) {
                    if let Some((name, _)) = current.take() {
                        let _ = mdns.unregister(&name);
                    }
                    match register(&mdns, &props, http_port) {
                        Ok(fullname) => {
                            log::info!(
                                "[mdns] publishing {} (cluster {}, fp {})",
                                props.instance,
                                &props.cluster_id,
                                &props.hub_fingerprint
                            );
                            current = Some((fullname, props.signature.clone()));
                        }
                        Err(e) => log::warn!("[mdns] register failed: {e}"),
                    }
                } else if !wanted {
                    if let Some((name, _)) = current.take() {
                        let _ = mdns.unregister(&name);
                        log::info!("[mdns] unpublished {name} (no longer a serving hub)");
                    }
                }

                std::thread::sleep(Duration::from_secs(15));
            }
        })
        .ok();
}

/// The TXT-bearing values we publish, read live from settings.
#[cfg(feature = "desktop")]
struct Props {
    instance: String,
    cluster_id: String,
    hub_fingerprint: String,
    role: String,
    /// A stable rendering of the fields above, filled in by [`Props::signed`]:
    /// a change re-registers the record so a follower resolving the name sees
    /// the new fingerprint immediately instead of a cached one.
    signature: String,
}

#[cfg(feature = "desktop")]
impl Props {
    fn signed(mut self) -> Self {
        self.signature = format!(
            "{}|{}|{}|{}",
            self.instance, self.cluster_id, self.hub_fingerprint, self.role
        );
        self
    }
}

/// The instance name: this machine's hostname, so two hubs on one LAN stay
/// distinguishable, falling back to a fixed label when the hostname is unknown.
#[cfg(feature = "desktop")]
fn instance_name() -> String {
    let m = crate::sync::machine_name();
    if m.is_empty() {
        "fms-hub".to_string()
    } else {
        // mDNS labels cannot contain a dot, and a hostname may be a FQDN.
        m.replace('.', "-")
    }
}

#[cfg(feature = "desktop")]
fn register(
    mdns: &mdns_sd::ServiceDaemon,
    props: &Props,
    http_port: u16,
) -> Result<String, mdns_sd::Error> {
    let hostname = format!("{}.local.", instance_name());
    let info = mdns_sd::ServiceInfo::new(
        SERVICE_TYPE,
        &props.instance,
        &hostname,
        // Empty addrs + `enable_addr_auto` lets the library fill in every local
        // interface, which is what makes the record follow a DHCP change.
        "",
        http_port,
        &[
            (TXT_CLUSTER, props.cluster_id.as_str()),
            (TXT_FINGERPRINT, props.hub_fingerprint.as_str()),
            (TXT_ROLE, props.role.as_str()),
            (TXT_MACHINE, props.instance.as_str()),
        ][..],
    )?
    .enable_addr_auto();
    let fullname = info.get_fullname().to_string();
    mdns.register(info)?;
    Ok(fullname)
}
