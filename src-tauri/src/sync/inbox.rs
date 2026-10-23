//! Desktop-hub → website **inbox drainer** (sync-v2.md §8.3 step 2, phase 5).
//!
//! The website is the *host* of a per-cluster inbox (§8.3): a browser-side
//! editor on a `mirror` dataset does not write the shared journal directly
//! (INV-2: only the desktop hub can append to a mirror-owned scope). It
//! *proposes* the change into `app_inbox` (`POST /api/v1/inbox`, `9339b79`
//! on the website repo — the `card_proposals.ts` producer). This module is the
//! app-side counterpart: it reads that page, applies every proposal through
//! the SAME [`crate::sync::rest::apply_one_change`] kernel the LAN writeback
//! uses (INV-18: no second dispatch table, so LWW, tombstones, dedup, the
//! epoch guard and the journal append choke point behave identically), acks
//! the verdict back to the host (`POST /api/v1/inbox/ack`), and advances a
//! per-scope drain cursor.
//!
//! Contract facts this file relies on (verified against the website routes):
//! * **Target** is the account website (`crate::auth::base_url()`), OAuth
//!   Bearer — same credential `sync/upload.rs` and `sync/mirror.rs` use.
//! * **Page** is `{ dataset_uuid, entries: [InboxEntry], pruned_up_to,
//!   inbox_seq, resync_required }` with `entries.proposal_seq` dense and
//!   strictly `> after`. There is no `live/` write on the website (§8.3
//!   step 1 is deliberately proposal-only until this drainer fires), so the
//!   hub's apply IS the write of record.
//! * **Ack** is `{ accepted: [{change_id, seq?}], rejected: [{change_id,
//!   winner_seq, winner_payload, winner_kind?}] }` → `{ accepted, rejected,
//!   unknown[] }`. `winner_kind` is additive on the wire (a producer ignoring
//!   unknown fields still works); it disambiguates `card_save` vs
//!   `card_set_tags` (both keyed to `card_uuid` by `object_id_for`) so the
//!   producer's rollback consumer does not have to shape-detect the winner
//!   payload. Absent for a tombstone-only drop, where there is no live row to
//!   name. A proposal we do NOT ack (a `replay` failure) stays `pending` on
//!   the host; the next drain re-reads it. So the cursor must never advance
//!   past a failed `proposal_seq` — see [`next_cursor`].
//! * Dedup is on `(cluster, change_id)`: `already_applied` short-circuits
//!   an already-drained proposal (INV-3 mirrors this on the host side).
//! * INV-18 is enforced structurally: `apply_one_change` is the sole apply
//!   entry point (extracted from the pre-existing `sync_changes` loop). The
//!   drainer never bypasses the journal.
//!
//! Desktop-only (`mod` declaration in `sync/mod.rs` carries the `desktop`
//! gate) — a follower has no authority to apply proposals locally; it must
//! forward them through the writeback handler, which is a different code path
//! that already exists.

use std::time::Duration;

use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::State;

use crate::datasets;
use crate::settings::SettingsState;
use crate::sync::change_log;
use crate::sync::rest::{apply_one_change, Change, ChangeResult};

/// Website route that serves one scope's pending proposals
/// (`learning/fms/src/app/api/v1/inbox/route.ts`, sync-v2.md §8.3).
const INBOX_GET_PATH: &str = "/api/v1/inbox";

/// Website route that records this hub's verdict on a drained page
/// (`learning/fms/src/app/api/v1/inbox/ack/route.ts`).
const INBOX_ACK_PATH: &str = "/api/v1/inbox/ack";

// ---------------------------------------------------------------------------
// Wire shapes (mirror `learning/fms/src/lib/sync/inbox.ts` verbatim)
// ---------------------------------------------------------------------------

/// One proposal row as the host serves it. `change_id` is client-supplied by
/// the producer as an idempotency token (INV-3 dedups on `(cluster, change_id)`).
#[derive(Deserialize, Clone, Debug)]
pub(crate) struct InboxEntry {
    pub(crate) change_id: String,
    pub(crate) proposal_seq: i64,
    #[allow(dead_code)]
    pub(crate) object_id: String,
    pub(crate) kind: String,
    #[allow(dead_code)]
    pub(crate) op: String,
    pub(crate) edit_time: String,
    pub(crate) author: String,
    pub(crate) payload: Value,
}

/// One page of `GET /inbox?scope&after`. `resync_required` fires only on the
/// unknown-cursor sentinel (`after < 0`); we never send -1 (the stored cursor
/// defaults to `0` = "from the beginning"), so this is a defensive arm for a
/// future retention-prune implementation.
#[derive(Deserialize, Default)]
pub(crate) struct InboxPage {
    #[allow(dead_code)]
    pub(crate) dataset_uuid: String,
    #[serde(default)]
    pub(crate) entries: Vec<InboxEntry>,
    #[serde(default)]
    pub(crate) pruned_up_to: i64,
    #[serde(default)]
    pub(crate) inbox_seq: i64,
    #[serde(default)]
    pub(crate) resync_required: bool,
}

// ---------------------------------------------------------------------------
// Drain cursor (desktop-only app-DB table, same posture as `mirror_push_state`)
// ---------------------------------------------------------------------------

/// Idempotent create for the drain-cursor table. Kept out of cross-platform
/// `ensure_sync_tables` — a follower never drains, so the table would be dead
/// weight on mobile.
pub(crate) fn ensure_inbox_table(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS inbox_drain_state (
            scope       TEXT PRIMARY KEY,
            cursor      INTEGER NOT NULL DEFAULT 0,
            drained_at  TEXT,
            last_error  TEXT
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// The last drained `proposal_seq` for `scope` (0 = never drained).
pub(crate) fn get_drain_cursor(conn: &Connection, scope: &str) -> i64 {
    conn.query_row(
        "SELECT cursor FROM inbox_drain_state WHERE scope = ?1",
        params![scope],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// Persist the drain high-water for `scope`, plus an optional last-error note
/// (surfaced via the status surface, never blocking). Idempotent upsert.
pub(crate) fn set_drain_cursor(conn: &Connection, scope: &str, cursor: i64, err: Option<&str>) {
    let _ = conn.execute(
        "INSERT INTO inbox_drain_state (scope, cursor, drained_at, last_error) \
         VALUES (?1, ?2, datetime('now'), ?3) \
         ON CONFLICT(scope) DO UPDATE SET \
             cursor = ?2, drained_at = datetime('now'), last_error = ?3",
        params![scope, cursor, err],
    );
}

// ---------------------------------------------------------------------------
// Scope enumeration (identical to `mirror::mirror_scopes`, duplicated so the
// drainer does not depend on mirror.rs's private helpers across the gate)
// ---------------------------------------------------------------------------

/// Every dataset scope the hub has journaled for (excludes `@app`). The
/// producer never posts proposals against a `journal_role='authority'` scope
/// (website 9339b79 hard-refuses), so draining authority-or-not is the host's
/// decision — we simply ask about every scope we've ever touched.
pub(crate) fn drainable_scopes(conn: &Connection) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT dataset_uuid FROM sync_log \
             WHERE dataset_uuid <> ?1 ORDER BY dataset_uuid",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![change_log::APP_SCOPE], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested)
// ---------------------------------------------------------------------------

/// Build the writeback-shaped `Change` from one drained proposal. `epoch` is
/// the hub's own current scope epoch, so the INV-13 guard inside
/// `apply_one_change` always passes locally — a follower, not this hub, is
/// the only node whose epoch can lag.
pub(crate) fn change_from_proposal(scope: &str, e: &InboxEntry, epoch: i64) -> Change {
    Change {
        id: e.change_id.clone(),
        kind: e.kind.clone(),
        dataset_uuid: Some(scope.to_string()),
        payload: e.payload.clone(),
        edit_time: e.edit_time.clone(),
        queued_at: String::new(),
        epoch,
    }
}

/// What the host's `POST /api/v1/inbox/ack` will receive and what the cursor
/// advance will respect. `failed_seqs` is the sorted list of proposal_seqs
/// whose replay errored; those proposals stay `pending` on the host and the
/// cursor must not advance past them.
pub(crate) struct DrainPlan {
    pub(crate) accepted: Vec<Value>,
    pub(crate) rejected: Vec<Value>,
    pub(crate) failed_seqs: Vec<i64>,
}

/// Split the per-entry verdicts into the two ack arrays. Applied entries carry
/// the newly-minted journal `seq` (traceability only, INV-4); losing entries
/// carry the winner the hub already had (`winner_seq`, `winner_payload`, and
/// `winner_kind` when the winner was a live journal row rather than a
/// tombstone-only drop); failed entries are omitted from both so the proposal
/// stays pending on the host and the next drain retries it.
///
/// `winner_kind` is the disambiguator the producer's rollback path needs:
/// `object_id_for` maps both `card_save` and `card_set_tags` to `card_uuid`,
/// so without a kind the producer would have to shape-detect the winner
/// payload. Additive on the wire — a host ignoring unknown ack fields still
/// works; a host reading `winner_kind` gets an unambiguous rollback. Absent
/// (not serialized) when the losing ChangeResult came from the tombstone-only
/// branch, since the winning delete's kind was not looked up.
pub(crate) fn partition_verdicts(
    results: Vec<(InboxEntry, ChangeResult, Option<i64>)>,
) -> DrainPlan {
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();
    let mut failed_seqs = Vec::new();
    for (entry, r, seq) in results {
        // failed: `!ok` and `error` set. Losing: `ok && rejected == Some(true)`.
        // Anything else (already_applied short-circuit + successful replay) is
        // counted as accepted so the host stops resending the proposal.
        if !r.ok {
            failed_seqs.push(entry.proposal_seq);
            continue;
        }
        if r.rejected == Some(true) {
            let mut v = json!({
                "change_id": entry.change_id,
                "winner_seq": r.winner_seq,
                "winner_payload": r.winner_payload,
            });
            if let Some(k) = r.winner_kind {
                v["winner_kind"] = json!(k);
            }
            rejected.push(v);
        } else {
            let mut v = json!({ "change_id": entry.change_id });
            if let Some(s) = seq {
                v["seq"] = json!(s);
            }
            accepted.push(v);
        }
    }
    DrainPlan { accepted, rejected, failed_seqs }
}

/// Cursor to persist after a successful ack. Never advance past a failed
/// proposal: if entry seq 50 failed but 51..100 succeeded, the host removes
/// 51..100 from `pending` (they're acked) and 50 stays pending, so the next
/// drain (`after = 49`) reads only 50 again — no resurrection, no silent skip.
pub(crate) fn next_cursor(page: &InboxPage, failed_seqs: &[i64]) -> i64 {
    match failed_seqs.iter().min() {
        Some(&first_failed) => (page.inbox_seq).min(first_failed - 1).max(0),
        None => page.inbox_seq,
    }
}

// ---------------------------------------------------------------------------
// One scope's drain
// ---------------------------------------------------------------------------

/// Pull the pending page for `scope`, apply each entry through the shared
/// writeback kernel, ack the verdicts, and advance the cursor. Holds the
/// owned ledger `Connection` across the HTTP awaits — the same pattern
/// `sync/rest.rs::sync_changes` uses today (`Connection` is `Send` here), so
/// `&mut ledger` handed to `apply_one_change` keeps the enclosing future
/// `Send` (only `&Connection` would break it, since `Connection: !Sync`).
async fn drain_scope(
    ledger: &mut Connection,
    settings: &State<'_, SettingsState>,
    client: &reqwest::Client,
    base: &str,
    token: &str,
    scope: &str,
) -> Result<Value, String> {
    ensure_inbox_table(ledger)?;
    change_log::ensure_hub_tables(ledger)?;
    let cursor_before = get_drain_cursor(ledger, scope);
    let hub_epoch = change_log::scope_epoch(ledger, scope);

    let url = format!(
        "{base}{INBOX_GET_PATH}?scope={}&after={}",
        crate::sync::client::qenc(scope),
        cursor_before,
    );
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("cannot reach the inbox at {url}: {e}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        // Renew for the next round; the session itself is not dropped (only a
        // refused refresh token clears the file).
        crate::auth::token_rejected(settings.inner()).await;
        set_drain_cursor(ledger, scope, cursor_before, Some("token_rejected"));
        return Err("token_rejected: the website refused the OAuth Bearer".to_string());
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        // The host does not track this scope for this account (never seen a
        // producer write against it, or it's an authority scope). Best-effort
        // skip — do NOT record as an error.
        return Ok(json!({
            "scope": scope,
            "drained": 0, "accepted": 0, "rejected": 0, "pending": 0,
            "cursor_before": cursor_before, "cursor_after": cursor_before,
            "note": "scope_not_hosted",
        }));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let err = format!("inbox_http_error: status={} body={}", status, body);
        set_drain_cursor(ledger, scope, cursor_before, Some(&err));
        return Err(err);
    }
    let page: InboxPage = resp
        .json()
        .await
        .map_err(|e| format!("inbox page parse failed: {e}"))?;

    if page.resync_required {
        // Only fires on the -1 unknown-cursor sentinel (host has no retention
        // prune for the inbox this phase). Reset to `pruned_up_to` and continue
        // — the next drain starts clean. No ack (an empty entries page).
        set_drain_cursor(ledger, scope, page.pruned_up_to, Some("resync_required"));
        return Ok(json!({
            "scope": scope,
            "drained": 0, "accepted": 0, "rejected": 0, "pending": 0,
            "cursor_before": cursor_before, "cursor_after": page.pruned_up_to,
            "note": "resync_required",
        }));
    }

    // Apply each entry through the shared kernel. Per-entry `write_identity`
    // is the producer's `author` (the account email that made the edit) so
    // per-user `object_id_for` folding keeps future @app-scoped proposals
    // attributed to the right user (INV-3 / INV-16). For the current card
    // producer, author is unused for object keying but still travels through
    // `replay` for the journal row's `author` column.
    let hub_now = chrono::Utc::now();
    let mut verdicts: Vec<(InboxEntry, ChangeResult, Option<i64>)> =
        Vec::with_capacity(page.entries.len());
    for entry in page.entries.iter() {
        let ch = change_from_proposal(scope, entry, hub_epoch);
        let write_identity = entry.author.clone();
        let result = apply_one_change(ledger, settings, &write_identity, hub_now, &ch).await;
        // Traceability-only `seq` for an applied verdict: re-read the just-
        // minted journal row (`apply_one_change` succeeded => `replay`'s
        // command already went through `write_and_log`, so `sync_log` has the
        // row). Cheap; a second lookup beats threading the seq through the
        // shared kernel and bloating the ChangeResult API.
        let seq = if result.ok && result.rejected != Some(true) {
            let sc = change_log::scope_for(&entry.kind, scope);
            let oid = change_log::object_id_for(&entry.kind, &entry.payload, &write_identity);
            change_log::latest_for_object(ledger, sc, &oid)
                .ok()
                .flatten()
                .map(|(s, _, _, _, _)| s)
        } else {
            None
        };
        verdicts.push((entry.clone(), result, seq));
    }

    let plan = partition_verdicts(verdicts);

    // Only ack when there is something to say. An empty page costs nothing but
    // a POST round-trip, and skipping it lets us leave the cursor exactly where
    // it was on a scope that has no pending proposals.
    if !plan.accepted.is_empty() || !plan.rejected.is_empty() {
        let ack_body = json!({ "accepted": plan.accepted, "rejected": plan.rejected });
        let ack_url = format!("{base}{INBOX_ACK_PATH}");
        let ack_resp = client
            .post(&ack_url)
            .bearer_auth(token)
            .json(&ack_body)
            .send()
            .await
            .map_err(|e| format!("cannot reach the inbox ack at {ack_url}: {e}"))?;
        let ack_status = ack_resp.status();
        if ack_status == reqwest::StatusCode::UNAUTHORIZED {
            crate::auth::token_rejected(settings.inner()).await;
            let err = "token_rejected (ack)".to_string();
            set_drain_cursor(ledger, scope, cursor_before, Some(&err));
            return Err(err);
        }
        if !ack_status.is_success() {
            let body = ack_resp.text().await.unwrap_or_default();
            let err = format!("ack_http_error: status={} body={}", ack_status, body);
            set_drain_cursor(ledger, scope, cursor_before, Some(&err));
            return Err(err);
        }
        // Parse the ack echo — `unknown[]` means the host doesn't recognize
        // those change_ids (already acked elsewhere or wrong cluster); log
        // and carry on, the cursor still advances.
        if let Ok(v) = ack_resp.json::<Value>().await {
            if let Some(unknown) = v["unknown"].as_array().filter(|a| !a.is_empty()) {
                log::warn!("[inbox] ack unknown for scope {scope}: {:?}", unknown);
            }
        }
    }

    let next = next_cursor(&page, &plan.failed_seqs);
    let err_note = if plan.failed_seqs.is_empty() {
        None
    } else {
        Some("one_or_more_replay_failed")
    };
    set_drain_cursor(ledger, scope, next, err_note);

    Ok(json!({
        "scope": scope,
        "drained": page.entries.len() as i64,
        "accepted": plan.accepted.len() as i64,
        "rejected": plan.rejected.len() as i64,
        "pending": plan.failed_seqs.len() as i64,
        "cursor_before": cursor_before,
        "cursor_after": next,
    }))
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

/// Drain the website inbox for one or every dataset scope this hub owns
/// (sync-v2.md §8.3 step 2 / §14 phase 5). For each scope: `GET /api/v1/inbox
/// ?scope&after=<cursor>` → apply each proposal through the SAME
/// `apply_one_change` kernel the LAN writeback uses (INV-18) → `POST
/// /api/v1/inbox/ack` with `{accepted:[{change_id,seq?}], rejected:[{change_id,
/// winner_seq, winner_payload, winner_kind?}]}` → advance the per-scope
/// cursor in `inbox_drain_state`. A proposal whose `replay` errored is NOT
/// acked and the cursor refuses to advance past its `proposal_seq`, so the
/// next drain retries it. `already_applied` short-circuits a re-drain (INV-3
/// idempotency). Hub-only (a follower has no authority to apply locally);
/// needs a logged-in account. `auto: true` marks the round's cadence drain:
/// with no logged-in account it skips silently, because a sync round must not
/// fail over an optional leg.
#[tauri::command]
pub async fn sync_inbox_drain(
    settings: State<'_, SettingsState>,
    scope: Option<String>,
    auto: Option<bool>,
) -> Result<Value, String> {
    if !settings.is_hub() {
        return Err(
            "this device is not role=hub; only a desktop hub drains the website inbox (a follower must forward edits via writeback instead)".to_string(),
        );
    }
    if auto == Some(true) && crate::auth::get_access_token(&settings).is_err() {
        return Ok(json!({ "skipped": "not_logged_in" }));
    }
    drain_inner(&settings, scope.as_deref()).await
}

/// Command body, reachable from the sync round's step-7 (auto-cadence) with
/// the same `State<'_, SettingsState>` the round already holds. Returns a
/// summary `{ target, role, scopes:[...], totals:{drained,accepted,rejected,
/// pending}, errors:[...] }`; a per-scope failure never aborts the loop.
pub(crate) async fn drain_inner(
    settings: &State<'_, SettingsState>,
    scope: Option<&str>,
) -> Result<Value, String> {
    let base = crate::auth::base_url();
    let token = crate::auth::access_token(settings.inner()).await?;
    let client = crate::auth::hub_client_builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;

    // Open the ledger once, keep it owned across the awaits (the pattern
    // `sync/rest.rs::sync_changes` already relies on). Enumerate scopes from
    // the same set the mirror pusher uses (every dataset scope with journal
    // rows, `@app` excluded) unless the caller pinned one.
    let mut ledger = datasets::dictation::open_app_db(settings.inner())?;
    ensure_inbox_table(&ledger)?;
    change_log::ensure_hub_tables(&ledger)?;
    let scopes: Vec<String> = match scope.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => vec![s.to_string()],
        None => drainable_scopes(&ledger)?,
    };

    let mut scope_results: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    let (mut t_drained, mut t_accepted, mut t_rejected, mut t_pending) = (0i64, 0i64, 0i64, 0i64);

    for sc in &scopes {
        match drain_scope(&mut ledger, settings, &client, &base, &token, sc).await {
            Ok(v) => {
                t_drained += v["drained"].as_i64().unwrap_or(0);
                t_accepted += v["accepted"].as_i64().unwrap_or(0);
                t_rejected += v["rejected"].as_i64().unwrap_or(0);
                t_pending += v["pending"].as_i64().unwrap_or(0);
                scope_results.push(v);
            }
            Err(e) => errors.push(json!({ "scope": sc, "error": e })),
        }
    }

    log::info!(
        "[sync] inbox drain from {base}: {} scope(s), drained={t_drained} accepted={t_accepted} rejected={t_rejected} pending={t_pending}",
        scope_results.len()
    );

    Ok(json!({
        "target": base,
        "role": "hub",
        "scopes": scope_results,
        "totals": {
            "drained": t_drained,
            "accepted": t_accepted,
            "rejected": t_rejected,
            "pending": t_pending,
        },
        "errors": errors,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(change_id: &str, seq: i64) -> InboxEntry {
        InboxEntry {
            change_id: change_id.to_string(),
            proposal_seq: seq,
            object_id: "card-1".to_string(),
            kind: "card_save".to_string(),
            op: "upsert".to_string(),
            edit_time: "2026-01-01T00:00:00Z".to_string(),
            author: "me@example.com".to_string(),
            payload: json!({ "uuid": "card-1" }),
        }
    }

    #[test]
    fn change_from_proposal_stamps_epoch_and_scope() {
        // INV-18 seam: every proposal maps onto a writeback-shaped Change;
        // the hub's own epoch rides along so the INV-13 guard passes locally.
        let e = entry("c-1", 7);
        let c = change_from_proposal("ds-abc", &e, 42);
        assert_eq!(c.id, "c-1");
        assert_eq!(c.kind, "card_save");
        assert_eq!(c.dataset_uuid.as_deref(), Some("ds-abc"));
        assert_eq!(c.edit_time, "2026-01-01T00:00:00Z");
        assert_eq!(c.epoch, 42);
        assert_eq!(c.payload, json!({ "uuid": "card-1" }));
        assert_eq!(c.queued_at, "");
    }

    #[test]
    fn partition_verdicts_splits_accepted_losing_failed() {
        // Applied -> accepted (with `seq`); losing -> rejected (with winner +
        // kind when known); failed -> NOT acked at all (stays pending on the
        // host).
        let results = vec![
            (entry("c-1", 1), ChangeResult::applied("c-1".into()), Some(11)),
            (
                entry("c-2", 2),
                ChangeResult::losing(
                    "c-2".into(),
                    9,
                    json!({ "v": 1 }),
                    Some("card_save".to_string()),
                ),
                None,
            ),
            (
                entry("c-3", 3),
                ChangeResult::failed("c-3".into(), "boom".into()),
                None,
            ),
        ];
        let plan = partition_verdicts(results);
        assert_eq!(plan.accepted.len(), 1);
        assert_eq!(plan.accepted[0]["change_id"], "c-1");
        assert_eq!(plan.accepted[0]["seq"], 11);
        assert_eq!(plan.rejected.len(), 1);
        assert_eq!(plan.rejected[0]["change_id"], "c-2");
        assert_eq!(plan.rejected[0]["winner_seq"], 9);
        assert_eq!(plan.rejected[0]["winner_kind"], "card_save");
        assert_eq!(plan.failed_seqs, vec![3]);
    }

    #[test]
    fn partition_verdicts_omits_winner_kind_for_tombstone_rejection() {
        // The tombstone-only drop path in `apply_one_change` reports
        // `winner_kind = None` (there is no live row to disambiguate against).
        // The ack body must NOT serialize `winner_kind` at all in that case —
        // a `null` field would trip a strict producer reading `.kind` off the
        // wire. `winner_seq = 0` + `winner_payload = null` are the tombstone
        // signature and stay.
        let results = vec![(
            entry("c-1", 1),
            ChangeResult::losing("c-1".into(), 0, serde_json::Value::Null, None),
            None,
        )];
        let plan = partition_verdicts(results);
        assert_eq!(plan.rejected.len(), 1);
        assert_eq!(plan.rejected[0]["winner_seq"], 0);
        assert!(plan.rejected[0].get("winner_kind").is_none());
    }

    #[test]
    fn partition_verdicts_omits_seq_when_lookup_failed() {
        // Applied with `seq = None` (latest_for_object miss) — still accepted,
        // just without the traceability field. The host's `seq` column is
        // traceability-only (INV-4), never gating.
        let results = vec![(entry("c-1", 1), ChangeResult::applied("c-1".into()), None)];
        let plan = partition_verdicts(results);
        assert_eq!(plan.accepted[0], json!({ "change_id": "c-1" }));
    }

    #[test]
    fn next_cursor_never_advances_past_a_failed_proposal() {
        // No failures -> jump to the reported high-water.
        let page = InboxPage {
            dataset_uuid: "ds".into(),
            entries: vec![],
            pruned_up_to: 0,
            inbox_seq: 100,
            resync_required: false,
        };
        assert_eq!(next_cursor(&page, &[]), 100);

        // Failure at seq 50 -> stall just below it so the host re-serves it.
        // Entries above 50 that succeeded were acked and are gone from
        // `pending`, so the next GET from cursor 49 returns only the failed
        // one (and any later still-pending rows).
        assert_eq!(next_cursor(&page, &[50]), 49);
        assert_eq!(next_cursor(&page, &[3, 50, 78]), 2);
    }

    #[test]
    fn next_cursor_handles_the_first_seq_failure() {
        // Failure at seq 1 -> cursor cannot go negative; floor at 0 (which
        // re-reads everything from the start; `already_applied` short-circuits
        // anything we already drained).
        let page = InboxPage {
            dataset_uuid: "ds".into(),
            entries: vec![],
            pruned_up_to: 0,
            inbox_seq: 10,
            resync_required: false,
        };
        assert_eq!(next_cursor(&page, &[1]), 0);
    }

    #[test]
    fn cursor_roundtrip_and_error_note() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_inbox_table(&conn).unwrap();
        assert_eq!(get_drain_cursor(&conn, "ds-a"), 0);
        set_drain_cursor(&conn, "ds-a", 42, None);
        assert_eq!(get_drain_cursor(&conn, "ds-a"), 42);
        set_drain_cursor(&conn, "ds-a", 43, Some("boom"));
        assert_eq!(get_drain_cursor(&conn, "ds-a"), 43);
        let note: String = conn
            .query_row(
                "SELECT last_error FROM inbox_drain_state WHERE scope = ?1",
                params!["ds-a"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(note, "boom");
        // Idempotent create.
        ensure_inbox_table(&conn).unwrap();
    }

    #[test]
    fn drainable_scopes_excludes_the_app_scope() {
        let conn = Connection::open_in_memory().unwrap();
        change_log::ensure_hub_tables(&conn).unwrap();
        change_log::append(&conn, "ds-a", "o1", "cue_save", "2026-01-01T00:00:00Z", "me", "{}").unwrap();
        change_log::append(&conn, "ds-b", "o2", "cue_save", "2026-01-01T00:00:00Z", "me", "{}").unwrap();
        change_log::append(&conn, change_log::APP_SCOPE, "me:o3", "xp", "2026-01-01T00:00:01Z", "me", "{}").unwrap();

        let scopes = drainable_scopes(&conn).unwrap();
        assert_eq!(scopes, vec!["ds-a".to_string(), "ds-b".to_string()]);
        assert!(!scopes.iter().any(|s| s == change_log::APP_SCOPE));
    }
}
