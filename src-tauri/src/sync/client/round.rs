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

use crate::sync::client::sync_state::{
    app_cursor, app_events_history_backfilled, app_history_backfilled, ensure_sync_tables,
    reset_cursors_without_provenance, set_app_cursor, set_app_events_history_backfilled,
    set_app_history_backfilled, set_last_round, set_resync_reason, stamp_app_provenance, upstream_of,
};
use crate::sync::client::cluster::{
    app_journal_advertised, advertises, dataset_journal_advertised, fetch_cluster, HISTORY_PROTOCOL,
    APP_SCOPE_PROTOCOL,
};
use crate::sync::client::writeback::{
    drop_pending_local_edit, has_pending_local_edit, lww_outcome, pending_local_edit, LwwOutcome,
};


// ---------------------------------------------------------------------------
// Incremental sync round (§3.3): push → per-dataset /changes apply → file-drift
// snapshot → catalog prune. Reuses the snapshot path for any non-DB file change
// (media/vtt), and folds the DB-only row deltas in place with the row log.
// ---------------------------------------------------------------------------

/// Signed GET returning parsed JSON. `path` is the URL path (no query), `qs`
/// the exact pre-encoded query string appended to the URL and folded into the
/// v2 signature. Shared by the round's catalog + manifest reads. Website
/// origins take the Bearer path of [`hub_get_json`] instead — same signature.
pub(crate) async fn signed_get_json(settings: &SettingsState, path: &str, qs: &str) -> Result<Value, String> {
    hub_get_json(settings, path, qs).await
}

/// `GET /datasets?lite=1` — the hub's current dataset catalog (uuid → type).
pub(crate) async fn fetch_catalog(settings: &SettingsState) -> Result<Vec<Value>, String> {
    let v = signed_get_json(settings, "/api/v1/datasets", "lite=1").await?;
    Ok(v.as_array().cloned().unwrap_or_default())
}

/// `GET /datasets/{uuid}/manifest` — the hub's current non-DB `overall_hash`.
pub(crate) async fn fetch_manifest_hash(settings: &SettingsState, uuid: &str) -> Result<String, String> {
    let v = signed_get_json(
        settings,
        &format!("/api/v1/datasets/{uuid}/manifest"),
        "",
    )
    .await?;
    Ok(v["overall_hash"].as_str().unwrap_or("").to_string())
}

/// Apply one pulled `sync_log` entry to the local dataset DB by dispatching to
/// the same write commands the UI uses. MUST be wrapped in a
/// [`crate::sync::change_log::ApplyGuard`] by the caller so the write does not
/// re-enqueue/re-log (the change is arriving *from* the hub). Mirrors the hub's
/// `rest.rs::replay` dispatch, but cross-platform (this module compiles on
/// mobile too). Returns `Ok(false)` for an unknown kind (skipped, not fatal).
pub(crate) async fn apply_change(
    settings: &State<'_, SettingsState>,
    dataset_uuid: &str,
    kind: &str,
    payload: &Value,
    write_identity: &str,
) -> Result<bool, String> {
    use crate::datasets::book::{BookChapter, BookSentence, BookSentenceWord};
    use crate::datasets::cards::{Card, Tag};
    use crate::datasets::dictation::{ListenCue, ListenDictation};
    use crate::datasets::read_aloud::ReadText;
    let ds = dataset_uuid.to_string();
    let str_field = |k: &str| -> Result<String, String> {
        payload
            .get(k)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("payload missing '{k}'"))
    };
    match kind {
        "cue_save" => {
            let cue: ListenCue = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            crate::datasets::dictation::listen_save_cue(settings.clone(), ds, cue).await.map(|_| true)
        }
        "cue_delete" => {
            let cue_uuid = payload
                .get("cue_uuid")
                .and_then(|v| v.as_str())
                .or_else(|| payload.as_str())
                .or_else(|| payload.get("uuid").and_then(|v| v.as_str()))
                .map(|s| s.to_string())
                .ok_or_else(|| "cue_delete missing uuid".to_string())?;
            crate::datasets::dictation::listen_delete_cue(settings.clone(), ds, cue_uuid).await.map(|_| true)
        }
        "dictation" => {
            let d: ListenDictation = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            crate::datasets::dictation::listen_save_dictation_as(settings.inner(), &ds, &d, write_identity)
                .await
                .map(|_| true)
        }
        "xp" => {
            let amount = payload.get("amount").and_then(|v| v.as_i64()).unwrap_or(0);
            let source = payload.get("source").and_then(|v| v.as_str()).unwrap_or("sync").to_string();
            let reference_id = payload
                .get("reference_id")
                .and_then(|v| v.as_str())
                .unwrap_or("sync")
                .to_string();
            // Awards journal under the app-wide scope, so the dataset a pull was
            // made for says nothing about attribution — take it from the payload,
            // mirroring the hub's `rest.rs::replay`.
            let ds_payload = payload.get("dataset_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let dsref = if !ds.is_empty() {
                Some(ds.as_str())
            } else if !ds_payload.is_empty() {
                Some(ds_payload)
            } else {
                None
            };
            crate::xp::xp_award_internal(settings.inner(), write_identity, amount, &source, &reference_id, dsref)
                .map(|_| true)
                .map_err(|e| e.to_string())
        }
        "card_save" => {
            let card: Card = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            crate::datasets::cards::card_save(settings.clone(), ds, card).await.map(|_| true)
        }
        "card_delete" => {
            let card_uuid = str_field("card_uuid")?;
            crate::datasets::cards::card_delete(settings.clone(), ds, card_uuid).await.map(|_| true)
        }
        // A review is a stored *result*, so it is applied by keeping the event the
        // author sent — never by re-running the rating against this node's history,
        // which is how one review used to fold twice per round (INV-23, §17.1).
        "card_review" => crate::datasets::cards::review::apply_remote(
            settings.inner(),
            &ds,
            payload,
            write_identity,
        ),
        "card_tag_save" => {
            let tag: Tag = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            crate::datasets::cards::card_tag_save(settings.clone(), ds, tag).await.map(|_| true)
        }
        "card_tag_delete" => {
            let tag_uuid = str_field("tag_uuid")?;
            crate::datasets::cards::card_tag_delete(settings.clone(), ds, tag_uuid).await.map(|_| true)
        }
        "card_set_tags" => {
            let card_uuid = str_field("card_uuid")?;
            let tag_uuids: Vec<String> = payload
                .get("tag_uuids")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            crate::datasets::cards::card_set_tags(settings.clone(), ds, card_uuid, tag_uuids).await.map(|_| true)
        }
        "book_chapter_save" => {
            let chapter: BookChapter = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            crate::datasets::book::book_save_chapter(settings.clone(), ds, chapter).await.map(|_| true)
        }
        "book_chapter_delete" => {
            let uuid = str_field("uuid")?;
            crate::datasets::book::book_delete_chapter(settings.clone(), ds, uuid).await.map(|_| true)
        }
        "book_sentence_save" => {
            let sentence: BookSentence = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            crate::datasets::book::book_save_sentence(settings.clone(), ds, sentence).await.map(|_| true)
        }
        "book_sentences_save" => {
            let sentences: Vec<BookSentence> = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            crate::datasets::book::book_save_sentences(settings.clone(), ds, sentences).await.map(|_| true)
        }
        "book_sentence_delete" => {
            let uuid = str_field("uuid")?;
            crate::datasets::book::book_delete_sentence(settings.clone(), ds, uuid).await.map(|_| true)
        }
        "book_word_save" => {
            let word: BookSentenceWord = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            crate::datasets::book::book_save_word(settings.clone(), ds, word).await.map(|_| true)
        }
        "book_word_delete" => {
            let uuid = str_field("uuid")?;
            crate::datasets::book::book_delete_word(settings.clone(), ds, uuid).await.map(|_| true)
        }
        "read_text_save" => {
            let text: ReadText = serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
            crate::datasets::read_aloud::save_text_row(settings.inner(), &ds, &text).map(|_| true)
        }
        "read_text_delete" => {
            let uuid = str_field("uuid")?;
            crate::datasets::read_aloud::read_aloud_delete_text(settings.clone(), ds, uuid).await.map(|_| true)
        }
        // Attempts are per-user events in the app DB, applied by keeping the
        // event the author sent (INSERT OR IGNORE) — never by rewriting the
        // dataset. `@app` passes no dataset, so it is read from the payload.
        "read_attempt_save" => {
            crate::datasets::read_aloud::apply_remote_save(settings.inner(), &ds, payload, write_identity)
        }
        "read_attempt_delete" => {
            crate::datasets::read_aloud::apply_remote_delete(settings.inner(), &ds, payload, write_identity)
        }
        // Wiki datasets: the .md files *are* the rows. Both arms resolve the
        // local copy under <datasets>/wiki and journal through commit_change,
        // which the caller's ApplyGuard turns into a no-op here.
        "wiki_file_save" => {
            let rel = str_field("rel_path")?;
            let content = payload.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
            crate::datasets::wiki::write_file(settings.inner(), &ds, &rel, &content).map(|_| true)
        }
        "wiki_file_delete" => {
            let rel = str_field("rel_path")?;
            crate::datasets::wiki::delete_file(settings.inner(), &ds, &rel).map(|_| true)
        }
        other => {
            log::warn!("[sync_round] skipping unknown change kind '{other}'");
            Ok(false)
        }
    }
}


/// Upper bound on `/app/state` pages one backfill fetches (the hub caps each page at
/// 20k rows per table, so 50 pages is far past any real history). A cursor that
/// never settles means a bug on one side; stopping here marks the import explicitly
/// incomplete and lets the next round retry, instead of spinning inside one round.
pub(crate) const HISTORY_PAGE_BUDGET: i64 = 50;

/// What one app-scope journal pull achieved.
pub(crate) struct AppPull {
    /// Journal rows folded into the local app DB.
    pub(crate) applied: i64,
    /// The journal cannot deliver this device's whole history — it has never
    /// completed a state read, and/or this round met a gap (never pulled, a range
    /// pruned away, a regressed hub seq) — so the state-based backfill
    /// ([`backfill_app_history`]) is owed.
    pub(crate) needs_history: bool,
    /// The hub's protocol as probed this round, handed to the backfill so it does
    /// not need a second `/status` GET.
    pub(crate) hub_protocol: Option<u32>,
    /// Un-pushed local edits this pull dropped to a provably newer hub entry
    /// (INV-14 LWW). A nonzero value is a red flag surfaced on the status page.
    pub(crate) dropped_pending: i64,
}

/// The hub's advertised `protocol_version`, read off its unsigned `/status` (so
/// it works before pairing). `None` when the hub is unreachable or predates the
/// field — both mean "do not assume new endpoints exist".

/// Pull the app-wide journal scope (`sync_log` rows logged under
/// [`crate::sync::change_log::APP_SCOPE`]): per-user `dictation` progress and `xp`
/// awards. Those tables live in the app DB, so they are never part of a dataset
/// snapshot — the row log is their only route. Runs separately from the dataset
/// loop, so a device that subscribes to no dataset still converges, and awards
/// with no dataset at all (book reading) are finally delivered. Reports how many
/// changes were applied and whether a history backfill is owed.
///
/// Every replay here is idempotent (`xp` dedups on (user, source, reference),
/// `dictation` is an upsert), which is what makes the two gap recoveries below
/// (restart from 0, jump to the prune mark) safe without a snapshot fallback.
pub(crate) async fn pull_app_scope(
    settings: &State<'_, SettingsState>,
    cluster: &Option<crate::sync::ClusterInfo>,
    hub_protocol: Option<u32>,
    errors: &mut Vec<Value>,
) -> Result<AppPull, String> {
    let scope = crate::sync::change_log::APP_SCOPE;
    // Capability negotiation (INV-20): run the app plane only against a host
    // that advertises `app_journal` (or a pre-`/cluster` LAN hub new enough by
    // protocol). Replaces the old website-only origin test. The website serves
    // neither `/cluster` nor `/status`, so `hub_protocol` is `None` for it and
    // this skips exactly as before — no 404 storm, no probe (dev guide §6).
    if !app_journal_advertised(cluster, &pc_base(settings.inner()), hub_protocol) {
        log::info!("[sync] host advertises no app_journal — skipping app-data pull");
        return Ok(AppPull { applied: 0, needs_history: false, hub_protocol, dropped_pending: 0 });
    }

    let (cursor, stamped) = {
        let conn = datasets::dictation::open_app_db(settings.inner())?;
        ensure_sync_tables(&conn)?;
        (app_cursor(&conn), app_history_backfilled(&conn).is_some())
    };
    // A device that has never completed a state read is missing everything older
    // than the retained journal, whatever its cursor says; the gap branches below
    // raise the same flag for devices that were stamped but then lost rows.
    let mut needs_history = !stamped || cursor.is_none();
    let mut after = cursor.unwrap_or(0);
    // Bounded retry: a regressed hub seq restarts at 0, a prune gap jumps to the
    // prune mark and re-reads what is still retained. Each branch moves `after`
    // strictly, so this cannot spin.
    let mut page: Option<Value> = None;
    for attempt in 0..3 {
        let p = match pc_changes_since(settings.clone(), scope.to_string(), after).await {
            Ok(p) => p,
            Err(e) => {
                errors.push(json!({ "scope": scope, "stage": "changes", "error": e }));
                // The hub just refused or failed this device — a state read would
                // fail the same way and only double-report one fault. `needs_history`
                // stays a per-round decision: the flag is recomputed next round from
                // the cursor and the stamp, so a transient failure is retried anyway.
                return Ok(AppPull { applied: 0, needs_history: false, hub_protocol, dropped_pending: 0 });
            }
        };
        let hub_seq = p["hub_seq"].as_i64().unwrap_or(after);
        // Safety alarm (§3.5): a hub seq *below* our cursor means its journal
        // regressed (reset, re-install, or a different machine answering the same
        // address). A dataset would take a snapshot here; app data has no
        // snapshot, so restart the cursor at 0 and re-apply the retained journal.
        if attempt == 0 && hub_seq < after {
            log::error!(
                "[sync] {scope}: hub_seq {hub_seq} < local cursor {after} — journal regressed, restarting from 0"
            );
            errors.push(json!({ "scope": scope, "stage": "alarm", "error": format!("hub seq {hub_seq} regressed below cursor {after}; re-pulling from 0") }));
            needs_history = true;
            after = 0;
            continue;
        }
        let pruned_up_to = p["pruned_up_to"].as_i64().unwrap_or(0);
        if p["resync_required"].as_bool().unwrap_or(false) && pruned_up_to > after {
            // Gap: journal rows below the mark are gone, so the delta path can no
            // longer reach this device's older history — report it, then re-read
            // from the mark so the retained tail still lands. The state read in
            // [`backfill_app_history`] (flagged by `needs_history`) is what actually
            // recovers the past; that also gives a never-pulled device the whole
            // retained journal instead of nothing.
            errors.push(json!({
                "scope": scope,
                "stage": "prune",
                "error": format!("hub pruned the app-data journal up to {pruned_up_to}; cursor {after} is behind — older rows are unrecoverable, continuing from the mark"),
            }));
            needs_history = true;
            after = pruned_up_to;
            continue;
        }
        page = Some(p);
        break;
    }
    let page = match page {
        Some(p) => p,
        // Retries exhausted: keep the position we reached so the next round
        // continues from it instead of re-walking the same gaps.
        None => {
            let conn = datasets::dictation::open_app_db(settings.inner())?;
            ensure_sync_tables(&conn)?;
            let _ = set_app_cursor(&conn, after);
            // The round reached the host even if the retries did not — stamp
            // provenance so a future round against a different host is visible
            // to the INV-19 check rather than silently switching.
            let (k, o) = upstream_of(&pc_base(settings.inner()));
            let _ = stamp_app_provenance(&conn, k, &o);
            return Ok(AppPull { applied: 0, needs_history, hub_protocol, dropped_pending: 0 });
        }
    };
    let hub_seq = page["hub_seq"].as_i64().unwrap_or(after);

    let entries = page["entries"].as_array().cloned().unwrap_or_default();
    let mut applied = 0i64;
    let mut dropped = 0i64;
    if !entries.is_empty() {
        let _guard = crate::sync::change_log::ApplyGuard::new();
        let conn = datasets::dictation::open_app_db(settings.inner())?;
        ensure_sync_tables(&conn)?;
        for ent in &entries {
            let kind = ent["kind"].as_str().unwrap_or("");
            let object_id = ent["object_id"].as_str().unwrap_or("");
            let user_key = ent["user_key"].as_str().unwrap_or("");
            let payload = ent["payload"].clone();
            // INV-14: an un-pushed local edit to the same row wins unless the
            // pulled entry is provably newer, in which case the pending edit is
            // dropped (LWW) and the loss counted + surfaced (§19.2).
            if let Some(pe) = pending_local_edit(&conn, scope, kind, object_id) {
                if lww_outcome(ent["edit_time"].as_str(), pe.as_deref()) == LwwOutcome::Skip {
                    continue; // local edit is newer/equal (or unstamped) — protect it
                }
                let _ = drop_pending_local_edit(&conn, scope, kind, object_id);
                dropped += 1;
            }
            // `dictation` needs no dataset and `xp` carries its own, so the row is
            // applied with an empty dataset argument.
            match apply_change(settings, "", kind, &payload, user_key).await {
                Ok(true) => applied += 1,
                Ok(false) => {}
                Err(e) => errors.push(json!({ "scope": scope, "kind": kind, "error": e })),
            }
        }
    }
    // Advance even when the page was empty: `after` may have moved past a prune
    // mark this round, and a stale cursor would re-walk (and re-report) the same
    // gap on every following round.
    let conn = datasets::dictation::open_app_db(settings.inner())?;
    ensure_sync_tables(&conn)?;
    let _ = set_app_cursor(&conn, hub_seq.max(after));
    // Stamp the `@app` upstream from the connected host (sync-v2.md §6.1). Only
    // fills an empty origin — see [`stamp_app_provenance`].
    let (k, o) = upstream_of(&pc_base(settings.inner()));
    let _ = stamp_app_provenance(&conn, k, &o);
    Ok(AppPull { applied, needs_history, hub_protocol, dropped_pending: dropped })
}

/// Fold the hub's *whole* history for this identity into the local app DB (§3.6).
///
/// The journal pull above is forward-only and retention-pruned, so it can only ever
/// deliver changes newer than the cursor. A device paired after a year of practice —
/// or one that fell through a prune gap — gets the rest from `/app/state`, which
/// ships state rather than deltas: the current progress row per (media, subtitle)
/// pair, plus the XP ledger. Both halves are paged to the end (the hub caps one
/// request, not the history) and land through their ordinary idempotent write paths
/// under an [`crate::sync::change_log::ApplyGuard`], so importing years of history is
/// not immediately re-pushed to the hub as a storm of fresh edits, and the XP total
/// is derived from the merged ledger instead of accumulated.
///
/// `Ok(None)` means "nothing to do": already stamped (unless `force`) or the hub
/// predates the endpoint. Neither is an error worth warning every round about.
///
/// `hub_lifetime_xp` may come out **ahead** of the local `lifetime_xp` and that is
/// not a bug: the hub's ledger can hold two rows for one award (written before
/// dedup, or pushed from two devices), while importing the second is correctly
/// skipped. The gap is reported instead of hidden, and only the deduped local total
/// is what the user sees.
pub(crate) async fn backfill_app_history(
    settings: &SettingsState,
    cluster: &Option<crate::sync::ClusterInfo>,
    hub_protocol: Option<u32>,
    force: bool,
) -> Result<Option<Value>, String> {
    let scope = crate::sync::change_log::APP_SCOPE;
    let proto = match hub_protocol.or_else(|| cluster.as_ref().map(|c| c.protocol_version)) {
        Some(p) => Some(p),
        None => hub_protocol_version(settings).await,
    };
    // Capability gate (INV-20): only a host that advertises the app journal can
    // serve `/app/state`. Replaces the old website-only origin test; the website
    // (no `/cluster`, no `/status`) resolves to `None` and skips as before.
    if !app_journal_advertised(cluster, &pc_base(settings), proto) {
        log::info!("[sync] host advertises no app_journal — history backfill not available");
        return Ok(None);
    }
    let conn = datasets::dictation::open_app_db(settings)?;
    ensure_sync_tables(&conn)?;
    if !matches!(proto, Some(p) if p >= HISTORY_PROTOCOL) {
        log::info!("[sync] hub has no /app/state (protocol {proto:?}) — history backfill skipped");
        return Ok(None);
    }
    // Do the dictation/xp half unless already settled; do the practice-event half
    // only against a v6 hub whose events haven't yet been folded (`want_events`). A
    // device whose ledger already backfilled still runs when the event half is owed,
    // so a hub upgrading past v6 recovers practice history (INV-21) — and against an
    // older hub `want_events` stays false, so nothing re-runs and nothing is stamped.
    let want_events =
        matches!(proto, Some(p) if p >= EVENTS_PROTOCOL) && app_events_history_backfilled(&conn).is_none();
    if !force && app_history_backfilled(&conn).is_some() && !want_events {
        return Ok(None);
    }

    let user_key = crate::auth::workspace_identity(settings);

    // Page both tables to the end: the hub caps one *request*, not the history, so a
    // large ledger arrives over several GETs. The two cursors advance independently
    // because the lists differ in length and end on different pages.
    let mut after_dictation = 0i64;
    let mut after_xp = 0i64;
    let mut after_reviews = 0i64;
    let mut after_attempts = 0i64;
    let mut pages = 0i64;
    let mut progress_rows = 0i64;
    let mut xp_row_count = 0i64;
    let mut progress_applied = 0i64;
    let mut xp_applied = 0i64;
    let mut review_rows = 0i64;
    let mut attempt_rows = 0i64;
    let mut reviews_applied = 0i64;
    let mut attempts_applied = 0i64;
    let mut skipped = 0i64;
    let mut hub_total: Option<i64> = None;
    let mut unfinished = false;
    let mut failures: Vec<String> = Vec::new();
    let _guard = crate::sync::change_log::ApplyGuard::new();
    loop {
        pages += 1;
        let mut params = vec![
            ("user_key", user_key.clone()),
            ("after_dictation", after_dictation.to_string()),
            ("after_xp", after_xp.to_string()),
        ];
        if want_events {
            params.push(("after_reviews", after_reviews.to_string()));
            params.push(("after_attempts", after_attempts.to_string()));
        }
        let page = match pc_get_json(settings, "/api/v1/app/state", &params)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                failures.push(e);
                unfinished = true;
                break;
            }
        };
        hub_total = page["lifetime_xp"].as_i64().or(hub_total);
        let dictation = page["dictation"].as_array().cloned().unwrap_or_default();
        let xp_rows = page["xp"].as_array().cloned().unwrap_or_default();
        progress_rows += dictation.len() as i64;
        xp_row_count += xp_rows.len() as i64;
        for row in &dictation {
            // An un-pushed local edit to this pair beats the hub's stored state:
            // an import must never undo practice done here while offline.
            let object_id = crate::sync::change_log::object_id_for("dictation", row, &user_key);
            if has_pending_local_edit(&conn, scope, "dictation", &object_id) {
                skipped += 1;
                continue;
            }
            let Ok(d) = serde_json::from_value::<datasets::dictation::ListenDictation>(row.clone())
            else {
                skipped += 1;
                continue;
            };
            match datasets::dictation::listen_save_dictation_as(settings, "", &d, &user_key).await {
                Ok(_) => progress_applied += 1,
                Err(e) => failures.push(format!("dictation {}: {e}", d.media_uuid)),
            }
        }
        for row in &xp_rows {
            let source = row["source"].as_str().unwrap_or("");
            let reference = row["reference_id"].as_str().unwrap_or("");
            let amount = row["amount"].as_i64().unwrap_or(0);
            if source.is_empty() || amount == 0 {
                continue;
            }
            let ds = row["dataset_uuid"].as_str().filter(|s| !s.is_empty());
            // No pending-edit guard on this half: the award path dedups on exactly
            // (user, source, reference), so re-importing an award made here and not
            // yet pushed is a no-op rather than a double count.
            match crate::xp::xp_award_internal(settings, &user_key, amount, source, reference, ds) {
                Ok(r) if r.is_new => xp_applied += 1,
                Ok(_) => skipped += 1,
                Err(e) => failures.push(format!("xp {source}/{reference}: {e}")),
            }
        }
        if want_events {
            let reviews = page["reviews"].as_array().cloned().unwrap_or_default();
            let attempts = page["attempts"].as_array().cloned().unwrap_or_default();
            review_rows += reviews.len() as i64;
            attempt_rows += attempts.len() as i64;
            // No pending-edit guard on either stream: replay is INSERT OR IGNORE by
            // event uuid (INV-23), so a locally-authored-but-unpushed event and the
            // hub's copy converge to one row — the same reason the xp half skips it.
            for row in &reviews {
                match datasets::cards::review::apply_remote(settings, "", row, &user_key) {
                    Ok(true) => reviews_applied += 1,
                    Ok(false) => skipped += 1,
                    Err(e) => failures.push(format!("review: {e}")),
                }
            }
            for row in &attempts {
                match datasets::read_aloud::apply_remote_save(settings, "", row, &user_key) {
                    Ok(true) => attempts_applied += 1,
                    Ok(false) => skipped += 1,
                    Err(e) => failures.push(format!("attempt: {e}")),
                }
            }
        }
        let next_dictation = page["next_dictation_after"].as_i64();
        let next_xp = page["next_xp_after"].as_i64();
        let next_reviews = if want_events { page["next_reviews_after"].as_i64() } else { None };
        let next_attempts = if want_events { page["next_attempts_after"].as_i64() } else { None };
        if next_dictation.is_none()
            && next_xp.is_none()
            && next_reviews.is_none()
            && next_attempts.is_none()
        {
            break;
        }
        if pages >= HISTORY_PAGE_BUDGET {
            log::warn!("[sync] app history hit the {pages}-page budget with rows still pending");
            unfinished = true;
            break;
        }
        after_dictation = next_dictation.unwrap_or(after_dictation);
        after_xp = next_xp.unwrap_or(after_xp);
        after_reviews = next_reviews.unwrap_or(after_reviews);
        after_attempts = next_attempts.unwrap_or(after_attempts);
    }
    // The ledger is now the union of local and imported awards, so read the total
    // back off it. Falls back to the hub's figure if the local recompute failed —
    // this number is reporting only, and a report of 0 would look like data loss.
    let lifetime_xp = crate::xp::recompute_totals(settings, &user_key).unwrap_or_else(|e| {
        log::warn!("[sync] could not recompute XP total after backfill: {e}");
        hub_total.unwrap_or(0)
    });
    // Only stamp a *complete* import: a failed row or an interrupted page walk means
    // the next round should retry rather than treat the history as settled.
    let complete = failures.is_empty() && !unfinished;
    if complete {
        set_app_history_backfilled(&conn)?;
        if want_events {
            set_app_events_history_backfilled(&conn)?;
        }
    }

    let result = json!({
        "user_key": user_key,
        "progress_rows": progress_rows,
        "progress_applied": progress_applied,
        "xp_rows": xp_row_count,
        "xp_applied": xp_applied,
        "review_rows": review_rows,
        "reviews_applied": reviews_applied,
        "attempt_rows": attempt_rows,
        "attempts_applied": attempts_applied,
        "skipped": skipped,
        "pages": pages,
        "lifetime_xp": lifetime_xp,
        "hub_lifetime_xp": hub_total,
        "complete": complete,
        "events_available": matches!(proto, Some(p) if p >= EVENTS_PROTOCOL),
        "events_backfilled": app_events_history_backfilled(&conn).is_some(),
        "failures": failures.iter().take(5).cloned().collect::<Vec<_>>(),
    });
    log::info!("[sync] app history backfill: {result}");
    Ok(Some(result))
}



/// Pull and apply one dataset-scope **row journal** from the connected hub
/// (sync-v2.md §3.2/§3.5, phase 7b). This is the single journal client shared by
/// the LAN branch and — now that phase 7b made the website a dataset-scope journal
/// authority — the cloud-hub branch of [`sync_round_inner`]. The body is identical
/// to the in-line LAN code it was extracted from: incremental `pc_changes_since`,
/// the `hub_seq`-below-cursor regression alarm, a `resync_required` snapshot, the
/// `apply_change` loop under `ApplyGuard` with the INV-14 last-write-wins
/// pending-edit guard, then the cursor advance and first-time provenance stamp.
///
/// Returns `true` when the copy was resolved with a full snapshot (regression or
/// `resync_required`) or otherwise consumed, so the caller must skip the non-DB
/// file-drift check — mirroring the `continue`s this replaced. Accumulators are
/// passed by `&mut` so both hub branches merge into one round result.
async fn pull_and_apply_dataset_journal(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
    cursor_opt: Option<i64>,
    connected_kind: &str,
    connected_origin: &str,
    applied: &mut i64,
    dropped_pending: &mut i64,
    resynced: &mut Vec<String>,
    errors: &mut Vec<Value>,
) -> bool {
    let after = cursor_opt.unwrap_or(-1);
    let page = match pc_changes_since(settings.clone(), uuid.clone(), after).await {
        Ok(p) => p,
        Err(e) => {
            errors.push(json!({ "uuid": uuid, "stage": "changes", "error": e }));
            return true;
        }
    };
    let resync_required = page["resync_required"].as_bool().unwrap_or(true);
    let hub_seq = page["hub_seq"].as_i64().unwrap_or(after);
    // Safety alarm (§3.5): a hub seq *below* our cursor means its journal
    // regressed (a reset / re-install / different cluster answering the same
    // address). Its deltas can no longer be trusted to be the continuation
    // of what we already applied, so resync rather than advance backwards.
    if let Some(c) = cursor_opt {
        if hub_seq < c {
            log::error!(
                "[sync] {uuid}: hub_seq {hub_seq} < local cursor {c} — journal regressed, forcing resync"
            );
            errors.push(json!({ "uuid": &uuid, "stage": "alarm", "error": format!("hub seq {hub_seq} regressed below cursor {c}") }));
            // Record why this copy is about to be re-pulled (§19.2); a
            // successful snapshot below clears it again.
            if let Ok(conn) = datasets::dictation::open_app_db(&settings) {
                if ensure_sync_tables(&conn).is_ok() {
                    let _ = set_resync_reason(&conn, &uuid, "journal_regressed");
                }
            }
            match dataset_sync_snapshot(app.clone(), settings.clone(), uuid.clone()).await {
                Ok(r) => {
                    if r.updated {
                        resynced.push(uuid);
                    }
                }
                Err(e) => errors.push(json!({ "uuid": uuid, "stage": "resync", "error": e })),
            }
            return true;
        }
    }
    if resync_required {
        // NULL/gap cursor: take one fresh snapshot (sets cursor + hash). The
        // hub raised `resync_required` because our cursor sits below its
        // prune watermark — record that cause (§19.2) before healing it.
        if let Ok(conn) = datasets::dictation::open_app_db(&settings) {
            if ensure_sync_tables(&conn).is_ok() {
                let _ = set_resync_reason(&conn, &uuid, "retention_gap");
            }
        }
        match dataset_sync_snapshot(app.clone(), settings.clone(), uuid.clone()).await {
            Ok(r) => {
                if r.updated {
                    resynced.push(uuid);
                }
            }
            Err(e) => errors.push(json!({ "uuid": uuid, "stage": "resync", "error": e })),
        }
        return true;
    }

    let entries = page["entries"].as_array().cloned().unwrap_or_default();
    if !entries.is_empty() {
        let _guard = crate::sync::change_log::ApplyGuard::new();
        let conn = match datasets::dictation::open_app_db(&settings) {
            Ok(c) => c,
            Err(e) => {
                errors.push(json!({ "uuid": uuid, "stage": "changes", "error": e }));
                return true;
            }
        };
        if let Err(e) = ensure_sync_tables(&conn) {
            errors.push(json!({ "uuid": uuid, "stage": "changes", "error": e }));
            return true;
        }
        for ent in &entries {
            let kind = ent["kind"].as_str().unwrap_or("");
            let object_id = ent["object_id"].as_str().unwrap_or("");
            let user_key = ent["user_key"].as_str().unwrap_or("");
            let payload = ent["payload"].clone();
            // INV-14: an un-pushed local edit to the same row wins unless the
            // pulled entry is provably newer, in which case the pending edit
            // is dropped (LWW) and the loss counted + surfaced (§19.2).
            if let Some(pe) = pending_local_edit(&conn, &uuid, kind, object_id) {
                if lww_outcome(ent["edit_time"].as_str(), pe.as_deref()) == LwwOutcome::Skip {
                    continue; // local edit is newer/equal (or unstamped) — protect it
                }
                let _ = drop_pending_local_edit(&conn, &uuid, kind, object_id);
                *dropped_pending += 1;
            }
            match apply_change(&settings, &uuid, kind, &payload, user_key).await {
                Ok(true) => *applied += 1,
                Ok(false) => {}
                Err(e) => errors.push(json!({ "uuid": uuid, "kind": kind, "error": e })),
            }
        }
        drop(_guard);
        // Advance the row cursor to the hub's high-water mark, and stamp
        // provenance if the row does not yet have it (a pre-migration copy
        // reaching its first incremental round against a real host). We do
        // *not* overwrite provenance on a partial round — only a snapshot
        // re-establishes a copy from scratch (INV-7).
        let conn = match datasets::dictation::open_app_db(&settings) {
            Ok(c) => c,
            Err(e) => {
                errors.push(json!({ "uuid": uuid, "stage": "changes", "error": e }));
                return true;
            }
        };
        let _ = ensure_sync_tables(&conn);
        let _ = conn.execute(
            "UPDATE dataset_sync_state SET \"cursor\" = ?2 WHERE dataset_uuid = ?1",
            params![uuid, hub_seq],
        );
        let _ = conn.execute(
            "UPDATE dataset_sync_state \
             SET upstream_kind = ?2, upstream_origin = ?3 \
             WHERE dataset_uuid = ?1 AND (upstream_origin IS NULL OR upstream_origin = '')",
            params![uuid, connected_kind, connected_origin],
        );
    }
    false
}

/// Non-DB file drift for one dataset (media/vtt/etc. change the manifest hash,
/// which the row log does not cover): fall back to one snapshot re-pull
/// (§4.1). Shared by the LAN and cloud-hub branches after the journal pull.
async fn check_file_drift(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
    stored_hash: &str,
    resynced: &mut Vec<String>,
    errors: &mut Vec<Value>,
) {
    match fetch_manifest_hash(&settings, &uuid).await {
        Ok(h) if !h.is_empty() && h != stored_hash => {
            match dataset_sync_snapshot(app.clone(), settings.clone(), uuid.clone()).await {
                Ok(_) => resynced.push(uuid),
                Err(e) => errors.push(json!({ "uuid": uuid, "stage": "files", "error": e })),
            }
        }
        Err(e) => errors.push(json!({ "uuid": uuid, "stage": "manifest", "error": e })),
        _ => {}
    }
}

/// The counts of one manual, single-deck journal sync (the card-sync surface).
/// Mirrors the old bespoke `SyncResult`: `pulled` = rows applied from the hub,
/// `pushed` = this deck's writeback rows acknowledged, `conflicts` = pending local
/// edits dropped by the INV-14 last-write-wins guard.
pub(crate) struct DeckJournalSync {
    pub pulled: i64,
    pub pushed: i64,
    pub conflicts: i64,
}

/// One manual dataset-scope journal round for a single cloud-hub deck: push its
/// pending writeback rows, then pull + apply its row journal, and report the
/// counts. The card-sync command and MCP tools ride this now that card rows travel
/// through the journal (sync-v2.md §8.3, phase 7b) — the bespoke `/api/card/sync/*`
/// round trip and its second conflict engine are gone.
///
/// Returns `Ok(None)` when this deck is *not* journal-synced against the connected
/// host — the host does not advertise `dataset_journal`, or the deck's recorded
/// upstream is a desktop hub (its rows belong to the LAN journal) — matching the
/// old skip path. `Err` only on a real provenance conflict (`card_sync_target`,
/// INV-19) or a transport failure on the pull.
pub(crate) async fn sync_one_deck_journal(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    dataset_uuid: &str,
) -> Result<Option<DeckJournalSync>, String> {
    let base = pc_base(&settings);
    if base.is_empty() {
        return Err("no hub configured; card sync needs an upstream".to_string());
    }
    // Provenance gate (INV-19): route by the deck's recorded upstream, refusing a
    // disagreement with the connected host rather than silently switching targets.
    match crate::sync::client::card_sync_target(settings.inner(), dataset_uuid)? {
        crate::sync::client::CardSyncTarget::Skip { reason } => {
            log::info!("[CardSync] skipping {dataset_uuid}: {reason}");
            return Ok(None);
        }
        crate::sync::client::CardSyncTarget::Website { .. } => {}
    }
    // The connected host must actually serve the dataset journal for this account
    // (a mirror-only website does not; `dataset_journal_advertised` default-denies).
    let cluster = fetch_cluster(&settings).await;
    if !dataset_journal_advertised(&cluster, &base) {
        return Ok(None);
    }
    let (connected_kind, connected_origin) = upstream_of(&base);

    // The deck's cursor drives the incremental pull; count its queued writeback
    // rows before and after the push so `pushed` reflects this deck's acks (the
    // flush is queue-wide — it carries every scope the host accepts).
    let (cursor_opt, before) = {
        let conn = datasets::dictation::open_app_db(&settings)?;
        ensure_sync_tables(&conn)?;
        let cursor = conn
            .query_row(
                "SELECT \"cursor\" FROM dataset_sync_state WHERE dataset_uuid = ?1",
                params![dataset_uuid],
                |r| r.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten();
        (cursor, pending_dataset_rows(&conn, dataset_uuid))
    };

    // 1. Push this deck's pending rows (and any other scope the host takes).
    let _ = writeback_flush_inner(&settings, &cluster).await.unwrap_or_else(|e| {
        log::warn!("[CardSync] writeback flush failed for {dataset_uuid}: {e}");
        0usize
    });
    let after = {
        let conn = datasets::dictation::open_app_db(&settings)?;
        ensure_sync_tables(&conn)?;
        pending_dataset_rows(&conn, dataset_uuid)
    };
    let pushed = before.saturating_sub(after);

    // 2. Pull + apply this deck's row journal (the shared 7b client).
    let mut applied = 0i64;
    let mut dropped = 0i64;
    let mut resynced: Vec<String> = Vec::new();
    let mut errs: Vec<Value> = Vec::new();
    pull_and_apply_dataset_journal(
        app,
        settings.clone(),
        dataset_uuid.to_string(),
        cursor_opt,
        connected_kind,
        connected_origin.as_str(),
        &mut applied,
        &mut dropped,
        &mut resynced,
        &mut errs,
    )
    .await;
    // A transport/resync failure is a real error the caller must see; a per-kind
    // apply failure (`"kind"`) is tolerable — the row simply reconciles next round.
    if let Some(e) = errs
        .iter()
        .find(|e| e["stage"].as_str().map(|s| !s.is_empty()).unwrap_or(false))
    {
        return Err(
            e["error"]
                .as_str()
                .unwrap_or("dataset journal sync failed")
                .to_string(),
        );
    }
    let _ = resynced;
    Ok(Some(DeckJournalSync {
        pulled: applied,
        pushed,
        conflicts: dropped,
    }))
}

/// Count this device's still-queued writeback rows for one dataset scope (the
/// dataset-scope rows carry the deck uuid in `dataset_uuid`; `@app` rows do not).
fn pending_dataset_rows(conn: &Connection, dataset_uuid: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM writeback_queue WHERE dataset_uuid = ?1",
        params![dataset_uuid],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
}

/// Run one full incremental sync round against the connected hub. Triggers:
/// app start, after a local write, a timer, or a network change (§3.3).
pub async fn sync_round_inner(
    app: AppHandle,
    settings: State<'_, SettingsState>,
) -> Result<Value, String> {
    if pc_base(&settings).is_empty() {
        return Err("No PC configured. Use Discover PC or set the PC address.".into());
    }
    // Website hub (T4): it serves no row journal (`/changes`, `/app/changes`,
    // `/app/state` — dev guide §6), so one round is file-level only: catalog ->
    // per-dataset manifest hash -> snapshot re-pull on drift, plus prune-by-
    // absence. Card datasets additionally run their own Bearer pull/push.
    let base = pc_base(&settings);
    let is_website = crate::sync::hub_kind(&base) == crate::sync::HubKind::Website;
    // Provenance of the host this round is talking to (sync-v2.md §6.1). Every
    // write to `dataset_sync_state` / `app_sync_state` in this round stamps
    // these two values; every prune-by-absence checks against them (INV-7).
    let (connected_kind, connected_origin) = upstream_of(&base);
    let connected = (connected_kind, connected_origin.as_str());
    if is_website {
        log::info!(
            "[sync] website hub contract v{} round",
            crate::sync::WEBSITE_CONTRACT_VERSION
        );
    }
    // Phase-1 migration: pre-provenance rows carry a cursor from an unknown
    // host's seq space. NULL them once so the normal "NULL cursor → snapshot
    // resync" path re-establishes each row against whichever host is connected
    // now. Idempotent — after the first round every row has a real origin.
    {
        let conn = datasets::dictation::open_app_db(&settings)?;
        ensure_sync_tables(&conn)?;
        match reset_cursors_without_provenance(&conn) {
            Ok(0) => {}
            Ok(n) => log::info!(
                "[sync] phase-1 migration: reset {n} cursor(s) with unknown provenance"
            ),
            Err(e) => log::warn!("[sync] phase-1 cursor reset failed: {e}"),
        }
    }
    let mut errors: Vec<Value> = Vec::new();
    let mut resynced: Vec<String> = Vec::new();
    let mut pruned: Vec<String> = Vec::new();
    let mut applied = 0i64;
    let mut dropped_pending = 0i64;
    let mut history: Option<Value> = None;
    // Card rows now arrive through the dataset-scope journal (folded into
    // `applied`/`resynced`), so the bespoke per-deck card round trip is gone. The
    // field is kept in the result shape (always empty) for status-surface compat.
    let cards: Vec<Value> = Vec::new();

    // Capability negotiation (sync-v2.md §7, INV-12): read the host's `/cluster`
    // page once per round. `None` (today's website, or an old/unreachable hub)
    // advertises nothing, so every app-plane gate below default-denies and the
    // round degrades to file-level + card sync — unchanged behavior. `app_ok`
    // drives the INV-20 "held" report; each plane call re-checks for itself.
    let cluster = fetch_cluster(&settings).await;
    let cluster_protocol = match cluster.as_ref() {
        Some(c) => Some(c.protocol_version),
        None => hub_protocol_version(&settings).await,
    };
    let app_ok = app_journal_advertised(&cluster, &base, cluster_protocol);
    // Phase-7b dataset-scope journal gate: a LAN hub always serves it, a website
    // only when the account advertises `dataset_journal`. Decides whether a
    // cloud-hub dataset rides the row journal or keeps the file-drift-only path.
    let ds_ok = dataset_journal_advertised(&cluster, &base);

    // 1. Push local edits first so the hub holds our changes before we pull.
    let pushed = writeback_flush_inner(&settings, &cluster).await.unwrap_or_else(|e| {
        errors.push(json!({ "stage": "push", "error": e }));
        0
    });
    // INV-20: a non-empty queue against a host that does not advertise the app
    // journal is no longer silent — report it as held so the status surface can
    // show "N changes waiting for a hub that accepts them" instead of nothing.
    let held = if app_ok {
        0i64
    } else {
        let n = pending_writeback_count(&settings);
        if n > 0 {
            errors.push(json!({
                "stage": "push",
                "scope": "@app",
                "error": format!("host does not advertise app_journal; {n} change(s) held locally"),
            }));
        }
        n
    };

    // 2. Refresh the hub catalog (uuid -> type) with the cheap `lite` list.
    let mut catalog_ok = true;
    let catalog = fetch_catalog(&settings).await.unwrap_or_else(|e| {
        catalog_ok = false;
        errors.push(json!({ "stage": "catalog", "error": e }));
        Vec::new()
    });
    let mut catalog_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for d in &catalog {
        let u = d["uuid"].as_str().unwrap_or("").to_string();
        let t = d["dataset_type"].as_str().unwrap_or("").to_string();
        if !u.is_empty() {
            catalog_map.insert(u, t);
        }
    }

    // 3. Per already-synced dataset: incremental apply, or a full resync when
    //    the cursor is unknown / a prune gap is reported / files drifted.
    let local: Vec<(String, Option<i64>, String, String)> = {
        let conn = datasets::dictation::open_app_db(&settings)?;
        ensure_sync_tables(&conn)?;
        let mut stmt = conn
            .prepare(
                "SELECT dataset_uuid, \"cursor\", overall_hash, COALESCE(upstream_kind, '') \n                 FROM dataset_sync_state",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.filter_map(|x| x.ok()).collect()
    };

    for (uuid, cursor_opt, stored_hash, upstream_kind) in local {
        if !catalog_map.contains_key(&uuid) {
            if catalog_ok {
                let conn = datasets::dictation::open_app_db(&settings)?;
                match prune_local_dataset(&settings, &conn, &uuid, connected) {
                    PruneOutcome::Deleted => pruned.push(uuid),
                    PruneOutcome::SkippedProvenance | PruneOutcome::SkippedWithdrawn => {
                        // Not an error and not a prune: the copy stays, the row
                        // stays, and the next round against the recorded upstream
                        // is the one that can act on it (INV-7, INV-22).
                    }
                    PruneOutcome::Missing => {}
                }
            }
            continue;
        }

        let cursor_is_hash = cursor_opt.is_some() && !stored_hash.is_empty();
        if is_website {
            // Phase 7b: a dataset this cloud hub is the *authority* over syncs its
            // rows through the dataset-scope journal exactly like a LAN hub, so it
            // runs the shared pull+apply client then the non-DB file-drift check.
            // Provenance decides (INV-19): `cloud_hub`, or unstamped (`''`, which
            // the pull adopts and stamps). A deck upstreamed by a *desktop* hub
            // keeps its rows on the LAN journal and must not be pulled from here.
            let journal_synced =
                ds_ok && (upstream_kind == "cloud_hub" || upstream_kind.is_empty());
            if journal_synced {
                let consumed = pull_and_apply_dataset_journal(
                    app.clone(),
                    settings.clone(),
                    uuid.clone(),
                    cursor_opt,
                    connected_kind,
                    connected_origin.as_str(),
                    &mut applied,
                    &mut dropped_pending,
                    &mut resynced,
                    &mut errors,
                )
                .await;
                if !consumed {
                    check_file_drift(
                        app.clone(),
                        settings.clone(),
                        uuid.clone(),
                        &stored_hash,
                        &mut resynced,
                        &mut errors,
                    )
                    .await;
                }
                continue;
            }

            // File drift only (mirror-only account, or a non-cloud-hub upstream):
            // the website's manifest hash covers the same frozen non-DB recipe
            // (§4.1). DB rows for a cloud-hub deck went through the journal above;
            // a desktop-hub deck's rows go through the LAN journal in its own round.
            if !cursor_is_hash {
                match dataset_sync_snapshot(app.clone(), settings.clone(), uuid.clone()).await {
                    Ok(_) => resynced.push(uuid.clone()),
                    Err(e) => errors.push(json!({ "uuid": uuid, "stage": "snapshot", "error": e })),
                }
            } else {
                match fetch_manifest_hash(&settings, &uuid).await {
                    Ok(h) if !h.is_empty() && h != stored_hash => {
                        match dataset_sync_snapshot(app.clone(), settings.clone(), uuid.clone()).await
                        {
                            Ok(_) => resynced.push(uuid.clone()),
                            Err(e) => {
                                errors.push(json!({ "uuid": uuid, "stage": "snapshot", "error": e }))
                            }
                        }
                    }
                    Err(e) => {
                        errors.push(json!({ "uuid": uuid, "stage": "manifest", "error": e }))
                    }
                    _ => {}
                }
            }
            // Catalog rows absent locally are left to the explicit per-dataset
            // Download — a round never auto-pulls media the user hasn't adopted.
            continue;
        }

        // LAN hub: pull the dataset-scope row journal, then the file-drift check.
        let consumed = pull_and_apply_dataset_journal(
            app.clone(),
            settings.clone(),
            uuid.clone(),
            cursor_opt,
            connected_kind,
            connected_origin.as_str(),
            &mut applied,
            &mut dropped_pending,
            &mut resynced,
            &mut errors,
        )
        .await;
        if !consumed {
            // 4. Non-DB file drift (media/vtt/etc. change the manifest hash, which
            //    the row log does not cover): fall back to one snapshot re-pull.
            check_file_drift(
                app.clone(),
                settings.clone(),
                uuid.clone(),
                &stored_hash,
                &mut resynced,
                &mut errors,
            )
            .await;
        }
    }

    // 5. Per-user app data (`dictation` progress + `xp` awards) journals under its
    //    own scope, so it is pulled once per round regardless of which datasets
    //    this device subscribes to (see [`pull_app_scope`]). The plane runs only
    //    against a host that advertises `app_journal` (INV-20); `pull_app_scope`
    //    re-checks and skips cleanly when the host does not.
    let app = pull_app_scope(&settings, &cluster, cluster_protocol, &mut errors)
        .await
        .unwrap_or_else(|e| {
            errors.push(json!({ "scope": "@app", "stage": "app_changes", "error": e }));
            AppPull { applied: 0, needs_history: false, hub_protocol: None, dropped_pending: 0 }
        });
    applied += app.applied;
    // INV-14: total un-pushed local edits dropped to a newer hub entry this round,
    // across both journal scopes. A nonzero value is a red flag, not a statistic
    // (§19.2), so log it per scope (§19.1) and carry it on the round result.
    let dropped_total = dropped_pending + app.dropped_pending;
    if dropped_total > 0 {
        log::warn!(
            "[sync] INV-14: dropped {dropped_total} pending local edit(s) to newer hub entries (app {}, dataset {})",
            app.dropped_pending,
            dropped_pending
        );
    }

    // 6. History backfill (§3.6): the journal is forward-only and pruned, so a
    //    device that has never completed a state read — new, or behind a gap — is
    //    still missing everything older than its cursor. One extra GET until it
    //    succeeds, then the stamp in `app_sync_state` keeps it out of every
    //    following round.
    //
    //    A hub that grew the practice-event streams (v6) but whose events this
    //    device has not yet folded is a *separate* reason to backfill: a phone whose
    //    dictation/xp already settled still owes reviews/attempts once its PC
    //    upgrades. `backfill_app_history` re-checks the protocol and the stamp
    //    internally, so this only decides whether to make the call.
    let hub_proto = app.hub_protocol.or_else(|| cluster.as_ref().map(|c| c.protocol_version));
    let events_owed = matches!(hub_proto, Some(p) if p >= EVENTS_PROTOCOL)
        && datasets::dictation::open_app_db(settings.inner())
            .map(|conn| {
                let _ = ensure_sync_tables(&conn);
                app_events_history_backfilled(&conn).is_none()
            })
            .unwrap_or(false);
    if app.needs_history || events_owed {
        match backfill_app_history(settings.inner(), &cluster, app.hub_protocol, false).await {
            Ok(Some(v)) => history = Some(v),
            // `Ok(None)` = hub predates the endpoint, already stamped. Expected,
            // not a fault — `pull_app_scope` logged the reason at info level.
            Ok(None) => {}
            Err(e) => errors.push(json!({ "scope": "@app", "stage": "history", "error": e })),
        }
    }

    let result = json!({
        "status": if errors.is_empty() { "ok" } else { "partial" },
        "hub_kind": if is_website { "website" } else { "lan" },
        "pushed": pushed,
        "held": held,
        "applied": applied,
        "dropped_pending": dropped_total,
        "resynced": resynced,
        "pruned": pruned,
        "cards": cards,
        "history": history,
        "errors": errors,
    });
    // Persist for the status surface (§19.2): a failure to record must not fail
    // the round, so log at most and still return the result.
    if let Ok(conn) = datasets::dictation::open_app_db(settings.inner()) {
        let _ = ensure_sync_tables(&conn);
        if let Err(e) = set_last_round(&conn, &result) {
            log::warn!("[sync] could not persist last-round result: {e}");
        }
    }
    Ok(result)
}


