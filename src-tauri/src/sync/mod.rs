//! Cross-device sync: the PC-side server stack and the client that pulls from it.
//!
//! Both halves speak one wire protocol — see [`PROTOCOL_VERSION`] and
//! `docs/design/sync.md`. The split:
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

use serde::{Deserialize, Serialize};

#[cfg(feature = "desktop")]
pub(crate) mod pairing;
#[cfg(feature = "desktop")]
pub(crate) mod mirror;
#[cfg(feature = "desktop")]
pub(crate) mod rest;
#[cfg(feature = "desktop")]
pub(crate) mod server;

pub(crate) mod change_log;
pub(crate) mod client;
pub(crate) mod discover;
pub(crate) mod publish;
pub(crate) mod upload;
pub(crate) mod blobpush;

// ---------------------------------------------------------------------------
// Hub kind — which of the two hub implementations a base URL points at
// ---------------------------------------------------------------------------

/// The two hubs a follower can talk to. The LAN PC hub (`sync::server` +
/// `sync::rest`) speaks the full device-HMAC protocol with a row journal;
/// the website hub (`auth::BASE_URL`, Next.js + Postgres) speaks only the
/// read contract + card sync documented in
/// `docs/app_dev_guide_20261013_063113.md` — no journal, no pairing, no
/// cluster/role headers. Every outbound round asks [`hub_kind`] once and
/// degrades instead of failing on the endpoints the website does not serve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HubKind {
    /// The website at `crate::auth::BASE_URL` — OAuth2 Bearer auth.
    Website,
    /// Another FmS machine on the LAN/tailnet — device-HMAC signing.
    LanPc,
}

/// Compare two base URLs by origin only (scheme and trailing slashes ignored),
/// the same tolerance `pc_base`/`resolve_pc_url` apply when storing them.
fn same_origin(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.trim()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string()
    };
    !a.is_empty() && norm(a) == norm(b)
}

/// Classify a resolved hub base URL. Only the configured website origin
/// (`crate::auth::base_url()`, overridable via `FMS_BASE_URL` for dev) counts as
/// [`HubKind::Website`] — a `pc_url` of anything else keeps the byte-identical
/// LAN path (dev guide §10.4).
pub(crate) fn hub_kind(base: &str) -> HubKind {
    if same_origin(base, &crate::auth::base_url()) {
        HubKind::Website
    } else {
        HubKind::LanPc
    }
}

/// The journal capabilities a desktop hub serves today (sync-v2.md §7
/// `features[]`). Single source of truth: the `GET /api/v1/cluster` handler
/// serializes it and the conformance seam asserts it, so the advertised list can
/// never drift from the routes actually mounted in [`rest`].
///
/// * `dataset_journal` — `GET /datasets/{uuid}/changes` row deltas.
/// * `app_journal` — `GET /app/changes` + `GET /app/state` for the `@app` scope.
///
/// Phase 7a grows this with `registry_reads`, `chat`, ... as the website host
/// learns to serve them. A capability the host does not list is refused, never
/// silently attempted (INV-20).
pub(crate) const HUB_FEATURES: &[&str] = &["dataset_journal", "app_journal"];

/// The `GET /api/v1/cluster` payload — "who am I talking to, and what can it do
/// for me" (sync-v2.md §7). Serialized by the desktop hub, deserialized by the
/// client, and sample-built by the conformance seam, so one struct pins the
/// contract on all three sides.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ClusterInfo {
    /// Provenance vocabulary shared with `client::upstream_of`: `"desktop_hub"`
    /// for a LAN/tailnet PC, `"cloud_hub"` for the website authority. Lets the
    /// client cross-check the host it reached against the upstream it stamps
    /// (INV-19).
    pub authority_kind: String,
    /// Cluster (hub identity group) this host belongs to.
    pub cluster_id: String,
    /// Authority epoch — a placeholder `0` until handover (phase 7b) gives it a
    /// bump rule. Reserved now so the field is on the wire from the start.
    pub epoch: u32,
    /// Wire protocol version, mirrored from `/status` so one read suffices.
    pub protocol_version: u32,
    /// Advertised capabilities; see [`HUB_FEATURES`].
    pub features: Vec<String>,
}

/// Wire-contract pin (dev guide §T8): the version of the website's
/// list/manifest/snapshot/file/card-sync field names this client implements
/// (guide §3). The names are the contract — a renamed field on either side is
/// breaking, so bump this *with* the guide, and never silently accept a new
/// field shape. Logged once per website round so a mismatch is on the record.
pub(crate) const WEBSITE_CONTRACT_VERSION: u32 = 1;

/// Wire protocol version for the hub<->follower sync REST API and the request
/// signature format. Carried by `/status`, the discovery beacon, and every
/// signed request header, so a follower can refuse `/changes`, `/file`, and the
/// chat `after_id` path against a hub that predates them (see docs/design/sync.md
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
/// * `5`: the unified `info.json` schema (see docs/design/dataset-info.md). One
///   typed descriptor for every dataset type replaces the five divergent shapes,
///   so `updated` becomes `updated_at`, a book's `title` becomes `name`, `type` /
///   `format` split out of `structure`, and per-dataset `sync_url` is gone. There
///   is **no** compatibility reader: an old follower pointed at a migrated dataset
///   loses `name`/`type`, and a v5 follower reading a pre-migration file loses the
///   same fields under the typed parser — both mis-parse silently, so the bump
///   exists to make a mixed-version cluster refuse each other instead.
/// * `6`: `GET /api/v1/app/state` grows per-user **review** (`card_review_event`)
///   and **attempt** (`read_attempt_event`) pages, so a reinstalled or late-paired
///   device recovers its practice history past journal retention (sync-v2.md §17.6,
///   INV-21). The response is wire-compatible with v4/v5 — an older client ignores
///   the extra fields — so the bump exists purely so a follower can tell the event
///   streams are present and refuse to stamp the practice backfill complete against
///   a host that omits them (rather than silently reporting zero events).
///
/// Versions 1-4 degrade safely in both directions: a v3 follower only pulls
/// `/app/changes` when the hub's `/status` reports `protocol_version >= 3`, and a
/// v4 follower only asks for `/app/state` at `>= 4`, so an older hub is never
/// probed with a route it lacks. A v2 follower's app-data pushes still land in the
/// v3+ hub's journal — under `@app` rather than the dataset scope, so they are no
/// longer echoed through that device's per-dataset pull. v5 is the exception: the
/// on-disk descriptor format, not a route, is what changed, so no `>=` guard can
/// express it — migrate every dataset before running a v5 binary. Everyone
/// converges once the whole cluster is on the same version.
pub const PROTOCOL_VERSION: u32 = 6;

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
