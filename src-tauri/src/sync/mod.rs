//! Cross-device sync: the PC-side server stack and the client that pulls from it.
//!
//! Both halves speak one wire protocol — see [`PROTOCOL_VERSION`] and
//! `docs/my_sync_design.md`. The split:
//!
//! * [`server`] — the axum HTTP service (desktop only) that hosts the REST API,
//!   the MCP endpoint, and the discovery beacon; [`rest`] implements the
//!   `/api/v1` dataset-snapshot + batched-writeback endpoints and [`pairing`]
//!   the Ed25519 device-identity handshake whose trust zones `server` enforces.
//! * [`client`] — snapshot pull + writeback queue. Compiles on **every**
//!   platform: the Android thin client pulls from its PC, and a desktop can pull
//!   from another desktop. [`discover`] finds the PC's `server` on LAN/Tailscale.
//! * [`change_log`] — hub-side append-only change journal (the `sync_log` table
//!   in the app DB) so followers can pull row-level deltas instead of
//!   whole-directory snapshots. Named `change_log` rather than `log` to avoid
//!   colliding with the `log` crate under uniform paths.

#[cfg(feature = "desktop")]
pub(crate) mod pairing;
#[cfg(feature = "desktop")]
pub(crate) mod rest;
#[cfg(feature = "desktop")]
pub(crate) mod server;

pub(crate) mod change_log;
pub(crate) mod client;
pub(crate) mod discover;

/// Wire protocol version for the hub<->follower sync REST API and the request
/// signature format. Carried by `/status`, the discovery beacon, and every
/// signed request header, so a follower can refuse `/changes`, `/file`, and the
/// chat `after_id` path against a hub that predates them (see docs/my_sync_design.md
/// §3.1, §7). Bump only on backwards-incompatible protocol changes.
///
/// * `1` (Phase 1): role/cluster/protocol headers + `/status` fields; request
///   signature still covers only `ts\nMETHOD\npath`.
/// * `2` (Phase 2): adds `/datasets/{uuid}/changes`, `/file`, per-file manifest
///   hashes, and the **hardened signature** covering `sha256(query\nbody)`.
/// * `3`: adds `GET /api/v1/app/changes`, the journal scope serving per-user app
///   data (`dictation` progress + `xp`) under `@app` instead of under one dataset,
///   and makes those two kinds' conflict keys per-user (`user_key:…`) so one user
///   can never overwrite another's row. `xp` payloads now carry `dataset_uuid`,
///   since the scope no longer encodes it.
/// * `4`: adds `GET /api/v1/app/state`, the **state-based** read that hands one
///   identity its whole per-user history (current `dictation` rows + the `xp`
///   ledger) regardless of journal retention — `/app/changes` is forward-only and
///   pruned, so it can never backfill the past. Wire-compatible with v3; the bump
///   exists so a follower can tell whether the route is there. (The matching local
///   change — `lifetime_xp` is now recomputed from the ledger instead of accumulated
///   — touches no wire format at all.)
///
/// Mixed versions degrade safely in both directions: a v3 follower only pulls
/// `/app/changes` when the hub's `/status` reports `protocol_version >= 3`, and a
/// v4 follower only asks for `/app/state` at `>= 4`, so an older hub is never
/// probed with a route it lacks. A v2 follower's app-data pushes still land in the
/// v3+ hub's journal — under `@app` rather than the dataset scope, so they are no
/// longer echoed through that device's per-dataset pull. Everyone converges once
/// the whole cluster is on the same version.
pub const PROTOCOL_VERSION: u32 = 4;

/// Hostname of this machine, as advertised in the discovery beacon and
/// `/api/v1/status` so a scanned list can say *which* PC it found rather than
/// only "fms-app". Empty when it cannot be determined — callers must then fall
/// back to the address, which is always unique.
///
/// Same source the pairing device name uses (`client::device_name`), kept as an
/// env lookup plus a unix fallback. Desktop-only because only the beacon and the
/// `/api/v1/status` handler report it — a phone never advertises itself.
#[cfg(feature = "desktop")]
pub(crate) fn machine_name() -> String {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        for key in ["COMPUTERNAME", "HOSTNAME"] {
            if let Ok(v) = std::env::var(key) {
                let v = v.trim().to_string();
                if !v.is_empty() {
                    return v;
                }
            }
        }
        #[cfg(unix)]
        {
            if let Ok(out) = std::process::Command::new("hostname").output() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return s;
                }
            }
        }
        String::new()
    })
    .clone()
}
