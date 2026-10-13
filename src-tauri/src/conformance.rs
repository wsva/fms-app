//! Test-only conformance seam (sync-v2.md §13).
//!
//! The conformance suite under `tests/sync_conformance/` drives the hub journal
//! engine directly instead of booting a full Tauri app: [`crate::sync::change_log`]
//! is the shared authority that produces the exact `ChangesPage` both
//! `GET /api/v1/datasets/{uuid}/changes` and `GET /api/v1/app/changes` serialize,
//! so pinning its output pins the wire shape those routes emit. Phase 2 points the
//! same vectors at the website routes unchanged — that is the whole point of the
//! parity claim in §13.
//!
//! This module is `#[doc(hidden)]` scaffolding, not app API. It is always
//! compiled (no feature gate) because `change_log` is available on every platform.

#![doc(hidden)]

use rusqlite::Connection;
use serde_json::Value;

use crate::sync::change_log;

/// The app-scope journal key (`@app`), re-exported so vectors can name it
/// without reaching into the private `sync` tree.
pub const APP_SCOPE: &str = change_log::APP_SCOPE;

/// Open an in-memory hub DB with the journal tables created.
pub fn new_hub_db() -> Connection {
    let conn = Connection::open_in_memory().expect("conformance: open in-memory db");
    change_log::ensure_hub_tables(&conn).expect("conformance: ensure hub tables");
    conn
}

/// Append one journal row under `scope`, returning its `seq`.
pub fn append(
    conn: &Connection,
    scope: &str,
    object_id: &str,
    kind: &str,
    edit_time: &str,
    user_key: &str,
    payload: &str,
) -> i64 {
    change_log::append(conn, scope, object_id, kind, edit_time, user_key, payload)
        .expect("conformance: append")
}

/// Read a journal page and serialize it to the exact JSON shape the `/changes`
/// routes emit. This is the payload a conformance vector asserts against.
pub fn read_page_json(conn: &Connection, scope: &str, after: i64) -> Value {
    let page = change_log::read_changes(conn, scope, after).expect("conformance: read_changes");
    serde_json::to_value(page).expect("conformance: serialize page")
}

/// Run a retention pass pruning rows older than `keep_days`, returning the
/// per-scope high-water marks that were raised.
pub fn prune(conn: &Connection, keep_days: i64) -> Vec<(String, i64)> {
    change_log::prune_log(conn, keep_days).expect("conformance: prune_log")
}

/// Directly raise a scope's prune mark, simulating a retention pass that already
/// ran. Lets a vector exercise the resync gap deterministically instead of
/// depending on wall-clock ageing.
pub fn set_prune_mark(conn: &Connection, scope: &str, pruned_up_to: i64) {
    conn.execute(
        "INSERT INTO sync_prune_marks(dataset_uuid, pruned_up_to) VALUES(?1, ?2) \
         ON CONFLICT(dataset_uuid) DO UPDATE SET \
             pruned_up_to = MAX(pruned_up_to, excluded.pruned_up_to)",
        rusqlite::params![scope, pruned_up_to],
    )
    .expect("conformance: set prune mark");
}

/// The journal capabilities this host serves today (sync-v2.md §7 `features[]`).
///
/// Delegates to [`crate::sync::HUB_FEATURES`] — the same single source the
/// `GET /api/v1/cluster` handler serializes — so the `features_advertised`
/// vector asserts the real advertised list and can never drift from the routes
/// the hub mounts. Phase 7a grows that list (`registry_reads`, `chat`, ...) as
/// the website host learns to serve them; this vector is the cross-host parity
/// assertion INV-20 names.
pub fn advertised_features() -> Vec<&'static str> {
    crate::sync::HUB_FEATURES.to_vec()
}

/// A sample `GET /api/v1/cluster` payload, built from the same
/// [`crate::sync::ClusterInfo`] the hub serializes and the client deserializes.
/// The `cluster_contract` vector asserts its shape without booting axum or
/// opening a socket — pinning the negotiated contract on all three sides.
pub fn sample_cluster_json() -> Value {
    let info = crate::sync::ClusterInfo {
        authority_kind: "desktop_hub".to_string(),
        cluster_id: "cluster-conformance".to_string(),
        // Placeholder until handover (phase 7b) defines the bump rule.
        epoch: 0,
        protocol_version: crate::sync::PROTOCOL_VERSION,
        features: crate::sync::HUB_FEATURES.iter().map(|s| s.to_string()).collect(),
    };
    serde_json::to_value(info).expect("conformance: serialize cluster")
}
