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


// App-wide journal pull (§3.2): per-user dictation progress + XP
// ---------------------------------------------------------------------------

/// Wire protocol version that introduced the hub's `GET /api/v1/app/changes`
/// endpoint. A follower must not pull that path from an older hub: the route does
/// not exist there, so every round would log a 404 while the data it guards stays
/// on the hub.
pub(crate) const APP_SCOPE_PROTOCOL: u32 = 3;

/// Wire protocol version that introduced `GET /api/v1/app/state`, the state-based
/// history read. Same reasoning as [`APP_SCOPE_PROTOCOL`]: a pre-v4 hub has no such
/// route, and asking for it every round would log a 404 where the honest answer is
/// "this hub cannot backfill your history".
pub(crate) const HISTORY_PROTOCOL: u32 = 4;

/// Wire protocol version at which `GET /api/v1/app/state` grew the per-user
/// practice-event streams (`card_review_event` + `read_attempt_event`). A v6
/// follower that talks to an older hub must not stamp the practice backfill
/// complete while silently missing reviews/attempts (sync-v2.md §17.6 / INV-21),
/// so the event half is requested and applied only against a hub that advertises
/// at least this version.
pub(crate) const EVENTS_PROTOCOL: u32 = 6;


pub(crate) async fn hub_protocol_version(settings: &SettingsState) -> Option<u32> {
    let base = pc_base(settings);
    if base.is_empty() {
        return None;
    }
    // The website advertises no protocol version (no `/status` route, dev
    // guide §6) — `None` keeps every protocol-gated pull skipped.
    if crate::sync::hub_kind(&base) == crate::sync::HubKind::Website {
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
pub(crate) async fn fetch_cluster(settings: &SettingsState) -> Option<crate::sync::ClusterInfo> {
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
    resp.json::<crate::sync::ClusterInfo>().await.ok()
}

/// Whether `cluster` advertises `feature`. A host that served no `/cluster` page
/// (`None`) advertises nothing — default-deny (INV-20).
pub(crate) fn advertises(cluster: &Option<crate::sync::ClusterInfo>, feature: &str) -> bool {
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
pub(crate) fn app_journal_advertised(
    cluster: &Option<crate::sync::ClusterInfo>,
    base: &str,
    protocol: Option<u32>,
) -> bool {
    if advertises(cluster, "app_journal") {
        return true;
    }
    // No `/cluster` page: only a LAN hub that predates the route keeps the old
    // protocol-version behavior. The website never runs the app plane here.
    cluster.is_none()
        && crate::sync::hub_kind(base) == crate::sync::HubKind::LanPc
        && matches!(protocol, Some(p) if p >= APP_SCOPE_PROTOCOL)
}


