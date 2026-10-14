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

use crate::sync::client::sync_state::ensure_sync_tables;


// ---------------------------------------------------------------------------
// Writeback queue (producer side, called from the mobile write commands)
// ---------------------------------------------------------------------------

/// Append a queued change to the writeback queue. Both mobile clients and a
/// desktop running `role = "follower"` funnel here via
/// [`crate::sync::change_log::commit_change`]; the hub role never enqueues. State kinds
/// coalesce (§3.4): a newer edit to the same `(kind, scope, object_id)`
/// replaces the still-pending older one instead of stacking, while append-only
/// and counter kinds keep every distinct row.
///
/// `scope` is the journal scope already resolved by the caller (a dataset uuid for
/// shared content, [`crate::sync::change_log::APP_SCOPE`] for per-user app data),
/// and `user_key` the identity the change belongs to — both are stamped into the
/// row so the hub's conflict lookup and the pull-side guard see the same ids this
/// device computed.
pub fn enqueue_change(
    settings: &SettingsState,
    kind: &str,
    scope: &str,
    payload: &Value,
    user_key: &str,
) -> Result<(), String> {
    let conn = datasets::dictation::open_app_db(settings)?;
    ensure_sync_tables(&conn)?;
    // The row-stamp shape (see `datasets::row_stamp_at`): the hub parses it, but a
    // pulled `edit_time` also lands in live row columns, which compare as text.
    let edit_time = datasets::now_row_stamp();
    let object_id = crate::sync::change_log::object_id_for(kind, payload, user_key);
    // Fold repeat edits to one mutable row so we neither ship stale intermediate
    // payloads nor grow the queue without bound between flushes. An empty
    // `object_id` (bulk inserts) is left uncoalesced by the guard.
    if crate::sync::change_log::ChangeClass::of(kind).coalescible() && !object_id.is_empty() {
        conn.execute(
            "DELETE FROM writeback_queue \
             WHERE kind = ?1 AND dataset_uuid = ?2 AND object_id = ?3",
            params![kind, scope, &object_id],
        )
        .map_err(|e| e.to_string())?;
    }
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO writeback_queue \
         (id, kind, dataset_uuid, payload, queued_at, edit_time, object_id) \
         VALUES (?1, ?2, ?3, ?4, datetime('now'), ?5, ?6)",
        params![id, kind, scope, payload.to_string(), &edit_time, &object_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Sum the `amount` of XP deltas still sitting un-acknowledged in the writeback
/// queue for `user_id` (§3.6 counter display). A follower *bakes* the delta into
/// its local `xp_user` at award time **and** enqueues it, so the local total is
/// already `hub_total + sum(pending)`; this count is therefore the informational
/// "not yet confirmed by the hub" overlay the UI/status surface shows, and it
/// drains on the flush ack (the queue entry is dropped once acknowledged).
/// Reading the queue directly keeps the source of truth single — no separate
/// pending ledger to drift.
pub fn pending_xp_delta(settings: &SettingsState, user_id: &str) -> Result<i64, String> {
    let conn = datasets::dictation::open_app_db(settings)?;
    ensure_sync_tables(&conn)?;
    let mut stmt = conn
        .prepare("SELECT payload FROM writeback_queue WHERE kind = 'xp'")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut total = 0i64;
    for row in rows {
        let raw = row.map_err(|e| e.to_string())?;
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            if v.get("user_id").and_then(|x| x.as_str()) == Some(user_id) {
                total += v.get("amount").and_then(|x| x.as_i64()).unwrap_or(0);
            }
        }
    }
    Ok(total)
}

// ---------------------------------------------------------------------------
// Flush (mobile -> PC), batched, idempotent, last-write-wins
// ---------------------------------------------------------------------------

/// Flush pending changes to the connected hub. Returns the number of changes
/// acknowledged. Safe to call repeatedly; failed items stay queued.
///
/// Two capability-gated passes (sync-v2.md §7, phase 7b): `@app`-scope rows ride
/// `app_journal` (phase 7a), dataset-scope rows ride the `dataset_journal` (7b).
/// A LAN hub always serves both, so `app_ok`/`ds_ok` are true and behavior is
/// byte-identical to the old single pass; against a website each leg goes only if
/// the account advertises the feature, and dataset rows only if their recorded
/// upstream *is* that cloud hub (INV-19 one-upstream-per-scope). A row whose gate
/// is closed stays held and is reported per-scope (INV-20).
pub async fn writeback_flush_inner(
    settings: &SettingsState,
    cluster: &Option<crate::sync::ClusterInfo>,
) -> Result<usize, String> {
    let base = pc_base(settings);
    if base.is_empty() {
        return Ok(0);
    }
    // Capability gates (INV-20). `protocol` is only consulted by the transitional
    // LAN fallback inside `app_journal_advertised` (a hub too old for `/cluster`).
    let protocol = match cluster.as_ref() {
        Some(c) => Some(c.protocol_version),
        None => hub_protocol_version(settings).await,
    };
    let app_ok = app_journal_advertised(cluster, &base, protocol);
    let ds_ok = dataset_journal_advertised(cluster, &base);
    if !app_ok && !ds_ok {
        let held = pending_writeback_count(settings);
        if held > 0 {
            log::warn!(
                "[sync] connected host advertises neither app_journal nor dataset_journal — \
                 {held} change(s) held locally; will retry against a hub that does (INV-20)"
            );
        }
        return Ok(0);
    }

    // Against a website, a dataset row may only leave this device if the deck's
    // upstream is that cloud hub; rows for a desktop-hub (or other) upstream stay
    // held rather than shipping to a host that would refuse them. A LAN hub is
    // the desktop upstream, so no provenance filter applies there.
    let require_cloud_upstream =
        crate::sync::hub_kind(&base) == crate::sync::HubKind::Website;

    let client = reqwest::Client::new();
    let mut flushed = 0usize;
    if app_ok {
        flushed += flush_scope(settings, &client, FlushScope::App).await?;
    }
    if ds_ok {
        flushed += flush_scope(
            settings,
            &client,
            FlushScope::Dataset { require_cloud_upstream },
        )
        .await?;
    }

    // Per-scope INV-20 held report: name what a closed gate left behind.
    if !app_ok || !ds_ok {
        let conn = datasets::dictation::open_app_db(settings)?;
        ensure_sync_tables(&conn)?;
        let (app, dataset) = queue_by_scope(&conn);
        if !app_ok && app.depth > 0 {
            log::warn!(
                "[sync] host does not advertise app_journal — {} @app change(s) held (INV-20)",
                app.depth
            );
        }
        if !ds_ok && dataset.depth > 0 {
            log::warn!(
                "[sync] host does not advertise dataset_journal — {} dataset change(s) held (INV-20)",
                dataset.depth
            );
        }
    }

    Ok(flushed)
}

/// Which half of the writeback queue a flush pass drains. `App` = the per-user
/// `@app` kinds; `Dataset` = everything else, optionally restricted to rows whose
/// recorded upstream is the connected cloud hub.
enum FlushScope {
    App,
    Dataset { require_cloud_upstream: bool },
}

/// The one-scope page SELECT. Kept pure (no HTTP) so the INV-19 provenance gate is
/// directly testable. `app_scope` selects the `@app` kinds via the shared
/// [`crate::sync::change_log::APP_SCOPE_KINDS`] list (one source with
/// `is_app_scope_kind`); the dataset leg otherwise, and — for a website host that
/// is not every scope's upstream — restricts rows to those whose recorded
/// upstream *is* the cloud hub. Unstamped (`''`/NULL) rows adopt the connected
/// host, mirroring `card_sync_target`, so only a *recorded* foreign upstream holds
/// them.
fn writeback_page_sql(app_scope: bool, require_cloud_upstream: bool) -> String {
    let app_kinds = crate::sync::change_log::APP_SCOPE_KINDS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(",");
    if app_scope {
        format!(
            "SELECT w.id, w.kind, w.dataset_uuid, w.payload, w.queued_at, \
                COALESCE(w.edit_time, '') \
             FROM writeback_queue w \
             WHERE w.kind IN ({app_kinds}) \
             ORDER BY w.queued_at LIMIT 50"
        )
    } else {
        let prov = if require_cloud_upstream {
            "AND COALESCE(s.upstream_kind, '') IN ('', 'cloud_hub') "
        } else {
            ""
        };
        format!(
            "SELECT w.id, w.kind, w.dataset_uuid, w.payload, w.queued_at, \
                COALESCE(w.edit_time, '') \
             FROM writeback_queue w \
             LEFT JOIN dataset_sync_state s ON s.dataset_uuid = w.dataset_uuid \
             WHERE w.kind NOT IN ({app_kinds}) {prov} \
             ORDER BY w.queued_at LIMIT 50"
        )
    }
}

/// One scope's flush loop: read a scope-filtered page, POST it to
/// `/api/v1/sync/changes`, drop acknowledged rows, repeat until the scope drains
/// or a batch stops progressing. Auth is origin-selected by `hub_request` (Bearer
/// for a website, device-HMAC for a LAN hub — byte-identical to the old signing).
async fn flush_scope(
    settings: &SettingsState,
    client: &reqwest::Client,
    scope: FlushScope,
) -> Result<usize, String> {
    let base = pc_base(settings);
    let api_path = "/api/v1/sync/changes";
    let (app_scope, require_cloud_upstream) = match scope {
        FlushScope::App => (true, false),
        FlushScope::Dataset {
            require_cloud_upstream,
        } => (false, require_cloud_upstream),
    };
    let sql = writeback_page_sql(app_scope, require_cloud_upstream);
    let mut flushed = 0usize;

    loop {
        // 1. Read a batch (Connection not held across await).
        let batch: Vec<(String, String, String, String, String, String)> = {
            let conn = datasets::dictation::open_app_db(settings)?;
            ensure_sync_tables(&conn)?;
            let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                    ))
                })
                .map_err(|e| e.to_string())?;
            rows.filter_map(|r| r.ok()).collect()
        };
        if batch.is_empty() {
            break;
        }

        // 2. POST the batch.
        let changes: Vec<Value> = batch
            .iter()
            .map(|(id, kind, ds, payload, queued_at, edit_time)| {
                json!({
                    "id": id,
                    "kind": kind,
                    "dataset_uuid": if ds.is_empty() { Value::Null } else { json!(ds) },
                    "payload": serde_json::from_str::<Value>(payload).unwrap_or(Value::Null),
                    // Empty for a pre-Phase-5 queue row; the hub then treats it
                    // as "arrived now" (§3.4 clamp). Otherwise the device stamp.
                    "edit_time": edit_time,
                    "queued_at": queued_at,
                })
            })
            .collect();
        let body = json!({ "user_key": crate::auth::workspace_identity(settings), "changes": changes });

        // Serialize once and sign the exact bytes we put on the wire, so the
        // v2 signature (which covers the body, §3.3) and the request agree.
        let body_bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
        let req = hub_request(
            client.post(format!("{base}{api_path}")),
            settings,
            "POST",
            api_path,
            "",
            &body_bytes,
        )?
        .header("content-type", "application/json")
        .body(body_bytes);
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let results: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            // Server error: keep everything queued and stop for now.
            return Err(format!("sync/changes failed: HTTP {}", status));
        }

        // 3. Drop acknowledged ids.
        let acked: Vec<String> = results
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false))
            .filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(String::from))
            .collect();

        if !acked.is_empty() {
            let conn = datasets::dictation::open_app_db(settings)?;
            ensure_sync_tables(&conn)?;
            for id in &acked {
                let _ = conn.execute("DELETE FROM writeback_queue WHERE id = ?1", [id]);
            }
            flushed += acked.len();
        }

        // No acknowledged items: we cannot make further progress this run.
        if acked.is_empty() {
            break;
        }
    }

    Ok(flushed)
}

#[derive(Serialize)]
pub struct SyncResult {
    pub updated: bool,
    pub hash: String,
    pub bytes: u64,
    pub file_count: usize,
}


/// The un-pushed local edit for one row, if any: the outer `Option` is `None`
/// when nothing is queued (or `object_id` is empty, which never guards), and
/// `Some(inner)` when a row exists, where `inner` is its `edit_time` (`None` for
/// a legacy pre-Phase-5 row queued before the column existed).
///
/// A kind with no single-row identity (`book_sentences_save` bulk inserts) can
/// never be matched meaningfully, so an empty `object_id` on either side must not
/// participate in the guard — comparing "" to "" would suppress every pulled row
/// of that kind for as long as one un-pushed row sat in the queue. Matching is on
/// the `object_id` column `enqueue_change` stamped at queue time, so both sides
/// are compared in the same per-user form (see
/// [`crate::sync::change_log::object_id_for`]). Coalescing in `enqueue_change`
/// folds repeat edits to one row per key, so `LIMIT 1` is the whole pending set.
pub(crate) fn pending_local_edit(
    conn: &Connection,
    dataset_uuid: &str,
    kind: &str,
    object_id: &str,
) -> Option<Option<String>> {
    if object_id.is_empty() {
        return None;
    }
    conn
        .prepare_cached(
            "SELECT edit_time FROM writeback_queue \
             WHERE kind = ?1 AND dataset_uuid = ?2 AND object_id = ?3 LIMIT 1",
        )
        .and_then(|mut s| {
            s.query_row(params![kind, dataset_uuid, object_id], |r| {
                r.get::<_, Option<String>>(0)
            })
        })
        // `Ok(None)` (a row with a NULL `edit_time`) stays `Some(None)`; a
        // no-row `Err` becomes the outer `None`.
        .ok()
}

/// Conservative later-wins guard: `true` when a local writeback entry for the
/// same (kind, dataset, object) is still un-pushed. Used by the history-backfill
/// fold, where an import must never undo offline practice (local always wins,
/// no stamp comparison). The journal-pull paths use [`pending_local_edit`] +
/// [`lww_outcome`] instead, which let a *newer* pulled entry drop the pending
/// edit (INV-14).
pub(crate) fn has_pending_local_edit(conn: &Connection, dataset_uuid: &str, kind: &str, object_id: &str) -> bool {
    pending_local_edit(conn, dataset_uuid, kind, object_id).is_some()
}

/// Delete the un-pushed local edit for one row (INV-14: a newer pulled entry
/// drops the pending one). Returns the number of queue rows removed. A no-op
/// when `object_id` is empty or nothing is queued.
pub(crate) fn drop_pending_local_edit(
    conn: &Connection,
    dataset_uuid: &str,
    kind: &str,
    object_id: &str,
) -> Result<usize, String> {
    if object_id.is_empty() {
        return Ok(0);
    }
    conn.execute(
        "DELETE FROM writeback_queue \
         WHERE kind = ?1 AND dataset_uuid = ?2 AND object_id = ?3",
        params![kind, dataset_uuid, object_id],
    )
    .map_err(|e| e.to_string())
}

/// What the journal-pull apply path does with a pulled entry when a local edit
/// to the same row is still un-pushed (INV-14).
#[derive(PartialEq, Eq, Debug)]
pub(crate) enum LwwOutcome {
    /// The local edit is newer, equal, or unstamped — protect it, skip the pull.
    Skip,
    /// The pulled entry is provably newer — drop the pending local edit, count
    /// the loss, and apply the pulled entry.
    DropAndApply,
}

/// Pure INV-14 last-writer-wins decision, called ONLY when a pending local edit
/// exists (`pending_local_edit` returned `Some`). `pulled` is the incoming
/// entry's `edit_time`; `pending_stamp` is the queued local edit's. The local
/// edit is dropped only when the pulled entry is *strictly* newer.
///
/// The comparison parses both stamps via [`datasets::info::stamps_cmp`] rather
/// than comparing text: text order is not instant order across stamp shapes
/// (`'Z'` out-ranks `'+'`, and SQLite's `datetime('now')` space form sorts before
/// every RFC-3339 stamp), which is exactly how `apply_pulled_cards` once lost a
/// newer local edit to an older remote one. `None` (either side unreadable) or
/// any non-`Greater` result protects the local edit.
pub(crate) fn lww_outcome(pulled: Option<&str>, pending_stamp: Option<&str>) -> LwwOutcome {
    match (pulled, pending_stamp) {
        (Some(q), Some(p)) => match datasets::info::stamps_cmp(q, p) {
            Some(std::cmp::Ordering::Greater) => LwwOutcome::DropAndApply,
            _ => LwwOutcome::Skip,
        },
        _ => LwwOutcome::Skip,
    }
}


/// Number of changes waiting in the writeback queue, for the INV-20 "held"
/// report when the connected host does not advertise the app journal.
pub(crate) fn pending_writeback_count(settings: &SettingsState) -> i64 {
    let Ok(conn) = datasets::dictation::open_app_db(settings) else {
        return 0;
    };
    if ensure_sync_tables(&conn).is_err() {
        return 0;
    }
    conn.query_row("SELECT COUNT(*) FROM writeback_queue", [], |r| r.get::<_, i64>(0))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh app-DB-equivalent with both pipeline tables and the same rows:
    /// one `@app` row, a cloud-hub deck row, a desktop-hub deck row, and a deck
    /// row with *no* provenance record (an unstamped deck).
    fn seeded_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        ensure_sync_tables(&conn).unwrap();
        let queue_rows = [
            ("a1", "xp", "@app"),
            ("d1", "card_save", "deck-cloud"),
            ("d2", "cue_save", "deck-desktop"),
            ("d3", "card_save", "deck-unstamped"),
        ];
        for (id, kind, ds) in queue_rows {
            conn.execute(
                "INSERT INTO writeback_queue (id, kind, dataset_uuid, payload, queued_at, edit_time) \n                 VALUES (?1, ?2, ?3, '{}', '2026-01-01 00:00:00', '2026-01-02 00:00:00')",
                params![id, kind, ds],
            )
            .unwrap();
        }
        for (uuid, kind) in [("deck-cloud", "cloud_hub"), ("deck-desktop", "desktop_hub")] {
            conn.execute(
                "INSERT INTO dataset_sync_state \n                 (dataset_uuid, overall_hash, synced_at, upstream_kind, upstream_origin) \n                 VALUES (?1, '', datetime('now'), ?2, 'origin')",
                params![uuid, kind],
            )
            .unwrap();
        }
        conn
    }

    fn ids_for(conn: &Connection, sql: &str) -> Vec<String> {
        let mut stmt = conn.prepare(sql).unwrap();
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        rows
    }

    #[test]
    fn app_pass_selects_only_app_scope_kinds() {
        let conn = seeded_conn();
        let ids = ids_for(&conn, &writeback_page_sql(true, false));
        assert_eq!(ids, vec!["a1".to_string()]);
    }

    #[test]
    fn website_dataset_pass_holds_foreign_and_flushes_cloud_plus_unstamped() {
        let conn = seeded_conn();
        // require_cloud_upstream = true (connected host is the website): a deck
        // recorded against a desktop hub stays held; cloud-hub and unstamped rows
        // go (INV-19 one upstream, unstamped adopts the connected host).
        let mut ids = ids_for(&conn, &writeback_page_sql(false, true));
        ids.sort();
        assert_eq!(ids, vec!["d1".to_string(), "d3".to_string()]);
    }

    #[test]
    fn lan_dataset_pass_flushes_every_dataset_row() {
        let conn = seeded_conn();
        // require_cloud_upstream = false (a LAN hub): every dataset-scope row is
        // this host's to carry, regardless of recorded provenance.
        let mut ids = ids_for(&conn, &writeback_page_sql(false, false));
        ids.sort();
        assert_eq!(
            ids,
            vec!["d1".to_string(), "d2".to_string(), "d3".to_string()]
        );
    }
}


