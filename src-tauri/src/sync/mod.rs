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
pub const PROTOCOL_VERSION: u32 = 2;
