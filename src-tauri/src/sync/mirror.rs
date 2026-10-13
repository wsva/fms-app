//! Desktop-hub → website **mirror push** (sync-v2.md §8.2, phase 4).
//!
//! The desktop hub is the dataset-scope **authority**: every shared-content
//! mutation lands in its `sync_log` (app DB) via [`change_log::commit_change`].
//! The website keeps a read-only *mirror* of that journal so it can render and
//! re-render the owner's real state — but a mirror is display material, never a
//! merge input (§2, INV-1). This module is the missing app-side half: it pages
//! the hub's dataset-scope journal and pushes it verbatim to the website's
//! already-shipped `POST /api/v1/changes` (`learning/fms/src/lib/sync/mirror.ts`).
//!
//! Contract facts this file relies on (verified against the website route):
//! * **Target** is the account website (`crate::auth::base_url()`), *not* the LAN
//!   `pc_url`. Auth is the OAuth Bearer token — `resolveDatasetWriter` accepts it
//!   and sets `cluster_id = email`. Same credential `sync/upload.rs` uses.
//! * **Body** is `{ origin: "mirror", entries: [MirrorEntry] }`; the route refuses
//!   any other `origin` and refuses the `@app` scope (that leg is the cloud
//!   `@app` authority, phase 7a, a different route).
//! * The route is **not** capability-gated (`features[]`): a mirror-ingest is not
//!   an authority write (INV-20 note in the route), so we push and map 4xx rather
//!   than probing `/cluster` for `dataset_journal`.
//! * `sync_log` has **no `change_id` column** (§6.2: it is a wire-only dedup key),
//!   so we synthesize a stable `mirror:{scope}:{seq}` — deterministic across
//!   re-push and app restart, and aligned with the website's own
//!   `(cluster, scope, seq)` uniqueness.
//!
//! Pure addition: nothing here is reached by the round, writeback, pull, or
//! upload paths. Desktop-only (a hub is desktop-only), so the `mod` declaration
//! in `sync/mod.rs` carries the `desktop` gate.

use std::time::Duration;

use rusqlite::{params, Connection};
use serde_json::{json, Value};
use tauri::State;

use crate::datasets;
use crate::settings::SettingsState;
use crate::sync::change_log;

/// Website route that ingests mirrored journal entries (§7).
const MIRROR_PATH: &str = "/api/v1/changes";

/// Entries per POST. Well under the website's `MAX_MIRROR_PUSH_ROWS` (20000) so
/// a batch bounds memory and the cursor advances incrementally; a 413 still
/// halves it and retries once (see [`push_scope`]).
const MIRROR_BATCH: usize = 1000;

// ---------------------------------------------------------------------------
// Cursor state (desktop-only app-DB table)
// ---------------------------------------------------------------------------

/// Create the mirror-push cursor table idempotently. Kept out of the
/// cross-platform `ensure_sync_tables` on purpose — this is a desktop-hub-only
/// concern, mirroring how `sync/upload.rs` owns `dataset_upload_state`.
fn ensure_mirror_table(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS mirror_push_state (
            scope          TEXT PRIMARY KEY,
            pushed_seq     INTEGER NOT NULL DEFAULT 0,
            last_pushed_at TEXT,
            last_error     TEXT
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// The highest `sync_log` seq already mirrored for `scope` (0 = never pushed).
fn pushed_seq(conn: &Connection, scope: &str) -> i64 {
    conn.query_row(
        "SELECT pushed_seq FROM mirror_push_state WHERE scope = ?1",
        params![scope],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// Record the mirrored high-water `seq` for `scope`, plus an optional
/// last-error note (a refusal or a retention gap) for the status surface.
fn set_pushed_seq(conn: &Connection, scope: &str, seq: i64, err: Option<&str>) {
    let _ = conn.execute(
        "INSERT INTO mirror_push_state (scope, pushed_seq, last_pushed_at, last_error) \
         VALUES (?1, ?2, datetime('now'), ?3) \
         ON CONFLICT(scope) DO UPDATE SET \
             pushed_seq = ?2, last_pushed_at = datetime('now'), last_error = ?3",
        params![scope, seq, err],
    );
}

/// Persist the cursor outside any `.await` (a `Connection` is not `Send`, so it
/// must never be held across the HTTP send). Best-effort: a write failure here
/// only costs a re-push, which `change_id` dedup makes a no-op.
fn persist_cursor(settings: &SettingsState, scope: &str, seq: i64, err: Option<&str>) {
    if let Ok(conn) = datasets::dictation::open_app_db(settings) {
        let _ = ensure_mirror_table(&conn);
        set_pushed_seq(&conn, scope, seq, err);
    }
}

/// Every dataset scope with journal rows, excluding `@app` (which the website
/// mirror route refuses — it belongs to the cloud `@app` authority leg).
fn mirror_scopes(conn: &Connection) -> Result<Vec<String>, String> {
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
// Wire mapping (pure)
// ---------------------------------------------------------------------------

/// Map one hub journal row to the website's `MirrorEntry` shape, synthesizing the
/// stable `change_id` the journal itself does not carry (§6.2). Verbatim
/// otherwise: the mirror reproduces the authority's `seq`/`op`/payload exactly
/// (INV-1, INV-2 — the website mints no seq and rewrites nothing).
fn to_mirror_entry(scope: &str, e: &change_log::SyncEntry) -> Value {
    json!({
        "change_id": format!("mirror:{scope}:{}", e.seq),
        "seq": e.seq,
        "scope": scope,
        "object_id": e.object_id,
        "kind": e.kind,
        "op": e.op,
        "edit_time": e.edit_time,
        "user_key": e.user_key,
        "payload": e.payload,
    })
}

/// The new cursor after a chunk, given the website's `refused[]`.
///
/// No refusals → advance to the chunk's max `seq` (all stored or deduped). Any
/// refusal → advance to just below the **lowest** refused `seq`, so the bad row
/// stalls the scope visibly and retryably rather than being skipped silently
/// (§7: actionable refusals; §19: surfaced, never hidden). An unparseable
/// refusal stalls before the whole chunk. Never moves the cursor backwards.
fn advance_cursor(entries: &[Value], refused: &[Value]) -> i64 {
    let seq_of = |v: &Value| v.get("seq").and_then(|s| s.as_i64());
    let max_seq = entries.iter().filter_map(seq_of).max().unwrap_or(0);
    if refused.is_empty() {
        return max_seq;
    }
    let floor = entries
        .first()
        .and_then(seq_of)
        .map(|s| s - 1)
        .unwrap_or(max_seq);
    let min_refused = refused
        .iter()
        .filter_map(|r| {
            r.get("change_id")
                .and_then(|c| c.as_str())
                // change_id is `mirror:{scope}:{seq}` — the seq is the last segment.
                .and_then(|cid| cid.rsplit(':').next())
                .and_then(|s| s.parse::<i64>().ok())
        })
        .min();
    match min_refused {
        Some(m) => (m - 1).max(floor),
        None => floor,
    }
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

/// Why one POST could not be recorded. `TooLarge` is recoverable by halving the
/// batch; `Fatal` stops the scope.
enum PushFail {
    TooLarge,
    Fatal(String),
}

/// Map a failed mirror response to an actionable message, in the house style of
/// `sync/upload.rs::upload_http_error`.
async fn mirror_http_error(resp: reqwest::Response) -> String {
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let msg = body["error"].as_str().unwrap_or("");
    let suffix = |s: &str| if s.is_empty() { String::new() } else { format!(" — {s}") };
    match status {
        401 => "sign in again — the website rejected or did not receive your access token (401)"
            .to_string(),
        403 => format!(
            "the website rejected this write; the credential is read-only (403){}",
            suffix(msg)
        ),
        404 => "the website does not serve the mirror route (POST /api/v1/changes) — it predates \
                sync-v2 phase 4 (404)"
            .to_string(),
        413 => format!("mirror batch too large for the website's limit (413){}", suffix(msg)),
        c if c >= 500 => format!("the website is failing right now (HTTP {c}) — retryable{}", suffix(msg)),
        c => format!("mirror push failed (HTTP {c}): {msg}"),
    }
}

/// Send one batch. Clears the stored token on 401 (as `upload.rs` does) so the
/// next status read reports "sign in again" rather than retrying a dead token.
async fn post_chunk(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    settings: &SettingsState,
    chunk: &[Value],
) -> Result<Value, PushFail> {
    let body = json!({ "origin": "mirror", "entries": chunk });
    let resp = client
        .post(url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|e| PushFail::Fatal(format!("cannot reach the website at {url}: {e}")))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        crate::auth::token_rejected(settings);
    }
    if status.as_u16() == 413 {
        return Err(PushFail::TooLarge);
    }
    if !status.is_success() {
        return Err(PushFail::Fatal(mirror_http_error(resp).await));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| PushFail::Fatal(format!("unexpected mirror response: {e}")))
}

// ---------------------------------------------------------------------------
// Per-scope push
// ---------------------------------------------------------------------------

/// Mirror one dataset scope's journal, incrementally from its stored cursor.
/// Returns `{ scope, cursor_before, cursor_after, pushed, deduped, refused,
/// gap_skipped, errors[] }`.
async fn push_scope(
    settings: &SettingsState,
    client: &reqwest::Client,
    url: &str,
    token: &str,
    scope: &str,
) -> Result<Value, String> {
    // Read the page with the connection scoped to this synchronous block — it is
    // dropped before any `.await` (a `Connection` is not `Send`).
    let (cursor_before, start_cursor, entries, gap_skipped) = {
        let conn = datasets::dictation::open_app_db(settings)?;
        ensure_mirror_table(&conn)?;
        change_log::ensure_hub_tables(&conn)?;
        let after = pushed_seq(&conn, scope);
        let page = change_log::read_changes(&conn, scope, after)?;
        if page.resync_required {
            // Our cursor predates a retention prune: those rows are gone and can
            // never be mirrored. Skip the gap (the mirror is best-effort display)
            // and continue from the prune watermark, recording the skip.
            let gap = (page.pruned_up_to - after).max(0);
            set_pushed_seq(&conn, scope, page.pruned_up_to, Some("retention gap: pruned rows skipped"));
            let page = change_log::read_changes(&conn, scope, page.pruned_up_to)?;
            let entries = page.entries.iter().map(|e| to_mirror_entry(scope, e)).collect::<Vec<Value>>();
            (after, page.pruned_up_to, entries, gap)
        } else {
            let entries = page.entries.iter().map(|e| to_mirror_entry(scope, e)).collect::<Vec<Value>>();
            (after, after, entries, 0)
        }
    };

    let mut pushed = 0i64;
    let mut deduped = 0i64;
    let mut refused = 0i64;
    let mut cursor_after = start_cursor;
    let mut errors: Vec<String> = Vec::new();

    let mut i = 0usize;
    let mut batch = MIRROR_BATCH;
    while i < entries.len() {
        let end = (i + batch).min(entries.len());
        let chunk = &entries[i..end];
        match post_chunk(client, url, token, settings, chunk).await {
            Ok(rj) => {
                pushed += rj["pushed"].as_i64().unwrap_or(0);
                deduped += rj["deduped"].as_i64().unwrap_or(0);
                let refusals = rj["refused"].as_array().cloned().unwrap_or_default();
                refused += refusals.len() as i64;
                let new_cursor = advance_cursor(chunk, &refusals);
                let note = if refusals.is_empty() {
                    None
                } else {
                    Some(
                        refusals[0]["error"]
                            .as_str()
                            .unwrap_or("entry refused")
                            .to_string(),
                    )
                };
                persist_cursor(settings, scope, new_cursor, note.as_deref());
                cursor_after = new_cursor;
                i = end;
                batch = MIRROR_BATCH;
                if !refusals.is_empty() {
                    errors.push(format!(
                        "{} entr(y/ies) refused at seq {}; paused there (retry after fixing the row)",
                        refusals.len(),
                        new_cursor + 1
                    ));
                    break;
                }
            }
            Err(PushFail::TooLarge) if batch > 1 => batch /= 2,
            Err(PushFail::TooLarge) => {
                errors.push("the website refused even a single-entry batch (413)".to_string());
                break;
            }
            Err(PushFail::Fatal(e)) => {
                errors.push(e);
                break;
            }
        }
    }

    Ok(json!({
        "scope": scope,
        "cursor_before": cursor_before,
        "cursor_after": cursor_after,
        "pushed": pushed,
        "deduped": deduped,
        "refused": refused,
        "gap_skipped": gap_skipped,
        "errors": errors,
    }))
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

/// Push this desktop hub's dataset-scope change journal up to the account
/// website's mirror (`POST /api/v1/changes`). Incremental (per-scope cursor in
/// `mirror_push_state`) and idempotent (the website dedups on `change_id`), so
/// re-running is safe. `scope` limits the push to one dataset uuid; omitted, it
/// mirrors every dataset scope with journal rows. Website-only target, needs a
/// logged-in account. Meaningful on a `role = "hub"` desktop — that is the only
/// node that appends dataset-scope rows — so other roles normally report zero
/// scopes (surfaced in `note`, not an error).
#[tauri::command]
pub async fn sync_mirror_push(
    settings: State<'_, SettingsState>,
    scope: Option<String>,
) -> Result<Value, String> {
    let base = crate::auth::base_url();
    // A missing token is a hard, actionable error for a write.
    let token = crate::auth::get_access_token(&settings)?;
    let url = format!("{base}{MIRROR_PATH}");
    let client = crate::auth::hub_client_builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;

    let scopes = {
        let conn = datasets::dictation::open_app_db(&settings)?;
        ensure_mirror_table(&conn)?;
        change_log::ensure_hub_tables(&conn)?;
        match scope.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => vec![s.to_string()],
            None => mirror_scopes(&conn)?,
        }
    };

    let role = settings.role();
    let mut scope_results: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    let (mut total_pushed, mut total_deduped, mut total_refused) = (0i64, 0i64, 0i64);

    for sc in &scopes {
        match push_scope(&settings, &client, &url, &token, sc).await {
            Ok(v) => {
                total_pushed += v["pushed"].as_i64().unwrap_or(0);
                total_deduped += v["deduped"].as_i64().unwrap_or(0);
                total_refused += v["refused"].as_i64().unwrap_or(0);
                scope_results.push(v);
            }
            Err(e) => errors.push(json!({ "scope": sc, "error": e })),
        }
    }

    // Only a hub journals dataset-scope changes; say so rather than let a
    // zero-scope result read as a silent failure (§19: surfaced, not hidden).
    let note = if role != "hub" {
        Some(
            "this device is not role=hub; only a hub journals dataset-scope changes, \
             so there is normally nothing to mirror",
        )
    } else {
        None
    };

    log::info!(
        "[sync] mirror-push to {base}: {} scope(s), pushed={total_pushed} deduped={total_deduped} refused={total_refused}",
        scope_results.len()
    );

    Ok(json!({
        "target": base,
        "role": role,
        "scopes": scope_results,
        "totals": {
            "pushed": total_pushed,
            "deduped": total_deduped,
            "refused": total_refused,
        },
        "errors": errors,
        "note": note,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use change_log::SyncEntry;

    fn entry(seq: i64, op: &str) -> SyncEntry {
        SyncEntry {
            seq,
            object_id: format!("obj{seq}"),
            kind: "cue_save".to_string(),
            op: op.to_string(),
            edit_time: "2026-01-01T00:00:00Z".to_string(),
            user_key: "me".to_string(),
            payload: json!({ "uuid": format!("obj{seq}") }),
        }
    }

    // The wire shape the website's mirror.ts validates, and the stable change_id
    // the journal itself does not carry.
    #[test]
    fn to_mirror_entry_maps_verbatim_with_synthesized_change_id() {
        let scope = "ds-uuid";
        let v = to_mirror_entry(scope, &entry(7, "delete"));
        assert_eq!(v["change_id"], json!("mirror:ds-uuid:7"));
        assert_eq!(v["seq"], json!(7));
        assert_eq!(v["scope"], json!(scope));
        assert_eq!(v["object_id"], json!("obj7"));
        assert_eq!(v["kind"], json!("cue_save"));
        assert_eq!(v["op"], json!("delete"));
        assert_eq!(v["edit_time"], json!("2026-01-01T00:00:00Z"));
        assert_eq!(v["user_key"], json!("me"));
        assert_eq!(v["payload"], json!({ "uuid": "obj7" }));
    }

    #[test]
    fn advance_cursor_takes_max_seq_when_nothing_refused() {
        let entries: Vec<Value> = (1..=3).map(|s| to_mirror_entry("d", &entry(s, "upsert"))).collect();
        assert_eq!(advance_cursor(&entries, &[]), 3);
    }

    #[test]
    fn advance_cursor_stalls_just_below_the_lowest_refusal() {
        let entries: Vec<Value> = (1..=3).map(|s| to_mirror_entry("d", &entry(s, "upsert"))).collect();
        // A refusal at seq 2 commits seq 1 and stalls at 1.
        let refused = vec![json!({ "change_id": "mirror:d:2", "error": "bad row" })];
        assert_eq!(advance_cursor(&entries, &refused), 1);
    }

    #[test]
    fn advance_cursor_does_not_move_when_the_first_entry_is_refused() {
        let entries: Vec<Value> = (1..=3).map(|s| to_mirror_entry("d", &entry(s, "upsert"))).collect();
        let refused = vec![json!({ "change_id": "mirror:d:1", "error": "bad row" })];
        assert_eq!(advance_cursor(&entries, &refused), 0);
    }

    #[test]
    fn advance_cursor_stalls_before_chunk_on_unparseable_refusal() {
        let entries: Vec<Value> = (5..=7).map(|s| to_mirror_entry("d", &entry(s, "upsert"))).collect();
        let refused = vec![json!({ "change_id": "(missing change_id)", "error": "x" })];
        // floor = first_seq - 1 = 4; nothing committed from this chunk.
        assert_eq!(advance_cursor(&entries, &refused), 4);
    }

    // The `@app` scope is the cloud-authority leg, not the mirror — the query
    // must never hand it to a mirror push (the website route refuses it).
    #[test]
    fn mirror_scopes_excludes_the_app_scope() {
        let conn = Connection::open_in_memory().unwrap();
        change_log::ensure_hub_tables(&conn).unwrap();
        change_log::append(&conn, "dataset-a", "o1", "cue_save", "2026-01-01T00:00:00Z", "me", "{}").unwrap();
        change_log::append(&conn, "dataset-b", "o2", "cue_save", "2026-01-01T00:00:00Z", "me", "{}").unwrap();
        change_log::append(&conn, "dataset-a", "o3", "cue_save", "2026-01-01T00:00:01Z", "me", "{}").unwrap();
        change_log::append(&conn, change_log::APP_SCOPE, "me:o4", "xp", "2026-01-01T00:00:02Z", "me", "{}").unwrap();

        let scopes = mirror_scopes(&conn).unwrap();
        assert_eq!(scopes, vec!["dataset-a".to_string(), "dataset-b".to_string()]);
        assert!(!scopes.iter().any(|s| s == change_log::APP_SCOPE));
    }

    #[test]
    fn cursor_state_round_trips_per_scope() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_mirror_table(&conn).unwrap();
        assert_eq!(pushed_seq(&conn, "dataset-a"), 0);
        set_pushed_seq(&conn, "dataset-a", 42, None);
        assert_eq!(pushed_seq(&conn, "dataset-a"), 42);
        // A second write updates in place and records the error note.
        set_pushed_seq(&conn, "dataset-a", 57, Some("stalled"));
        assert_eq!(pushed_seq(&conn, "dataset-a"), 57);
        // Another scope is independent.
        assert_eq!(pushed_seq(&conn, "dataset-b"), 0);
    }
}
