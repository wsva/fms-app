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
//! Pure addition: nothing here is reached by the writeback, pull, or upload
//! paths — the round's step-7 auto-cadence drain is the one caller besides the
//! command/MCP entry points. Desktop-only (a hub is desktop-only), so the `mod`
//! declaration in `sync/mod.rs` carries the `desktop` gate.

use std::time::Duration;

use rusqlite::{params, Connection};
use serde_json::{json, Value};
use tauri::State;

use crate::datasets;
use crate::settings::SettingsState;
use crate::sync::change_log;

/// Website route that ingests mirrored journal entries (§7).
const MIRROR_PATH: &str = "/api/v1/changes";

/// Website route that flips a scope's journal role between this hub and the cloud
/// authority (`learning/fms/src/app/api/v1/handover/route.ts`, sync-v2.md §9 flow 8,
/// INV-13/INV-15). Body `{ uuid, role }` under the OAuth Bearer, owner-only; echoes
/// `{ uuid, changed, journal_role, journal_epoch }`.
const HANDOVER_PATH: &str = "/api/v1/handover";

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

// ---------------------------------------------------------------------------
// Handover state (desktop-only app-DB table)
// ---------------------------------------------------------------------------

/// Create the handover state table idempotently — the status surface for the
/// desktop→cloud authority handover, kept alongside (and out of) the mirror
/// cursor for the same desktop-only reason `mirror_push_state` is.
fn ensure_handover_table(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS journal_handover_state (
            scope          TEXT PRIMARY KEY,
            journal_epoch  INTEGER NOT NULL DEFAULT 0,
            journal_role   TEXT,
            handed_over_at TEXT,
            last_error     TEXT
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Record the cloud's post-handover role/epoch for `scope`, clearing any prior
/// error. Best-effort (a write failure only costs a stale status read).
fn set_handover_state(conn: &Connection, scope: &str, epoch: i64, role: &str, err: Option<&str>) {
    let _ = conn.execute(
        "INSERT INTO journal_handover_state (scope, journal_epoch, journal_role, handed_over_at, last_error) \
         VALUES (?1, ?2, ?3, datetime('now'), ?4) \
         ON CONFLICT(scope) DO UPDATE SET \
             journal_epoch = ?2, journal_role = ?3, handed_over_at = datetime('now'), last_error = ?4",
        params![scope, epoch, role, err],
    );
}

/// Every recorded handover state `(scope, role, epoch, last_error)` for the
/// status surface (sync-v2 finish 7b). `journal_handover_state` is a write-only
/// table until now — this is its reader. Sorted by scope so the UI order is
/// stable, and `ensure`d first so a hub that never handed anything over reads
/// an empty vec instead of tripping over the missing table.
pub(crate) fn handover_states(
    conn: &Connection,
) -> Result<Vec<(String, Option<String>, i64, Option<String>)>, String> {
    ensure_handover_table(conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT scope, journal_role, journal_epoch, last_error FROM journal_handover_state ORDER BY scope",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Persist a failed handover attempt for `scope` without clobbering the last
/// known-good epoch/role (only the error note moves).
fn record_handover_error(settings: &SettingsState, scope: &str, err: &str) {
    if let Ok(conn) = datasets::dictation::open_app_db(settings) {
        let _ = ensure_handover_table(&conn);
        let _ = conn.execute(
            "INSERT INTO journal_handover_state (scope, journal_epoch, journal_role, handed_over_at, last_error) \
             VALUES (?1, 0, NULL, datetime('now'), ?2) \
             ON CONFLICT(scope) DO UPDATE SET last_error = ?2",
            params![scope, err],
        );
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

/// Send one batch. On 401 it renews the stored token (see
/// [`crate::auth::token_rejected`]) so the next run carries a live credential;
/// the session itself is never dropped over an aged token. The batch in flight
/// still fails, since its bearer is already on the wire.
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
        crate::auth::token_rejected(settings).await;
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
/// `auto: true` marks the round's cadence drain: with no logged-in account it
/// skips silently instead of erroring, because a sync round must not fail over
/// an optional pusher leg.
#[tauri::command]
pub async fn sync_mirror_push(
    settings: State<'_, SettingsState>,
    scope: Option<String>,
    auto: Option<bool>,
) -> Result<Value, String> {
    if auto == Some(true) && crate::auth::get_access_token(&settings).is_err() {
        return Ok(json!({ "skipped": "not_logged_in" }));
    }
    mirror_push_inner(settings.inner(), scope.as_deref(), &settings.role()).await
}

/// The command body, reachable from the sync round (step 7) without a `State`
/// wrapper. All callees already take `&SettingsState`.
pub(crate) async fn mirror_push_inner(
    settings: &SettingsState,
    scope: Option<&str>,
    role: &str,
) -> Result<Value, String> {
    let base = crate::auth::base_url();
    // A missing token is a hard, actionable error for a write.
    let token = crate::auth::access_token(settings).await?;
    let url = format!("{base}{MIRROR_PATH}");
    let client = crate::auth::hub_client_builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;

    let scopes = {
        let conn = datasets::dictation::open_app_db(settings)?;
        ensure_mirror_table(&conn)?;
        change_log::ensure_hub_tables(&conn)?;
        match scope.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => vec![s.to_string()],
            None => mirror_scopes(&conn)?,
        }
    };

    let role = role.to_string();
    let mut scope_results: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    let (mut total_pushed, mut total_deduped, mut total_refused) = (0i64, 0i64, 0i64);

    for sc in &scopes {
        match push_scope(settings, &client, &url, &token, sc).await {
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

// ---------------------------------------------------------------------------
// Authority handover (desktop hub -> cloud, sync-v2.md §9 flow 8 / INV-13)
// ---------------------------------------------------------------------------

/// Normalize and validate the requested handover role. The website requires an
/// explicit `authority`|`mirror` member and never defaults it, so a malformed
/// request must fail here rather than silently hand a scope to the cloud (§15 q4:
/// the operation stays deliberate). Absent/empty means the common desktop→cloud case.
fn normalize_handover_role(role: Option<&str>) -> Result<&'static str, String> {
    match role.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("authority") {
        "authority" => Ok("authority"),
        "mirror" => Ok("mirror"),
        other => Err(format!("role must be 'authority' or 'mirror' (got '{other}')")),
    }
}

/// True when a mirror export fully drained the scope: no refusals and no transport
/// errors. A not-drained export must block a desktop→cloud **authority** handover —
/// INV-1 continuity: the cloud self-seeds its seq space above the journal it already
/// holds, so handing over before every row lands would leave the new authority
/// missing rows. A `mirror` (cloud→desktop) handover returns authority to this hub and
/// does not depend on the export having drained, so the caller only gates on this for
/// `authority`.
fn export_is_complete(export: &Value) -> bool {
    let refused = export["refused"].as_i64().unwrap_or(0);
    let errors_empty = export["errors"].as_array().map(|a| a.is_empty()).unwrap_or(true);
    refused == 0 && errors_empty
}

/// Map a failed handover response to an actionable message, house style of
/// [`mirror_http_error`] (the route's 4xx set is owner/auth-shaped, not batch-shaped).
async fn handover_http_error(resp: reqwest::Response) -> String {
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let msg = body["error"].as_str().unwrap_or("");
    let suffix = |s: &str| if s.is_empty() { String::new() } else { format!(" — {s}") };
    match status {
        401 => "sign in again — the website rejected or did not receive your access token (401)".to_string(),
        403 => format!("the website rejected this handover; the credential is read-only (403){}", suffix(msg)),
        404 => "the website has no catalog row for this dataset uuid — publish it before handing its journal over (404)".to_string(),
        409 => format!("you do not own this dataset uuid on the website (409){}", suffix(msg)),
        c if c >= 500 => format!("the website is failing right now (HTTP {c}) — retryable{}", suffix(msg)),
        c => format!("handover failed (HTTP {c}): {msg}"),
    }
}

/// Hand one dataset scope's journal authority between this desktop hub and the cloud
/// (sync-v2.md §9 flow 8 / INV-13) — the **export half** of the website's already-shipped
/// `POST /api/v1/handover`. `role = "authority"` (the default) hands the scope TO the
/// cloud; `"mirror"` hands it back to this hub.
///
/// The sequence is what makes it safe, not just the flip:
/// 1. Fully **export** the scope's journal to the cloud mirror via the existing
///    [`push_scope`] so every row is present BEFORE the role moves. For an `authority`
///    handover a not-drained export is refused here (INV-1).
/// 2. `POST /api/v1/handover { uuid, role }` under the OAuth Bearer, owner-only.
/// 3. **Record** the epoch the cloud echoes back into this hub's `sync_scope_epoch`
///    (so we stop presenting/refusing against a stale number) and into
///    `journal_handover_state` for the status surface.
///
/// Never automatic (INV-13 forbids failover) and hub-only (a follower has no authority
/// to hand over) — the same guards as `sync_handover_epoch`. Website target, needs a
/// logged-in account.
#[tauri::command]
pub async fn sync_handover_to_cloud(
    settings: State<'_, SettingsState>,
    uuid: String,
    role: Option<String>,
) -> Result<Value, String> {
    if !settings.is_hub() {
        return Err("only a hub holds journal authority to hand a scope over".to_string());
    }
    let uuid = uuid.trim().to_string();
    if uuid.is_empty() {
        return Err("uuid is required (the dataset scope to hand over)".to_string());
    }
    let role = normalize_handover_role(role.as_deref())?;

    let base = crate::auth::base_url();
    let token = crate::auth::access_token(&settings).await?;
    let client = crate::auth::hub_client_builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;

    // 1. Export: fully drain this scope's journal to the cloud mirror before the flip.
    let mirror_url = format!("{base}{MIRROR_PATH}");
    let export = push_scope(&settings, &client, &mirror_url, &token, &uuid).await?;
    if role == "authority" && !export_is_complete(&export) {
        let note = "journal has not fully mirrored to the cloud; handover blocked (INV-1)".to_string();
        record_handover_error(&settings, &uuid, &note);
        return Err(format!(
            "cannot hand scope '{uuid}' to the cloud yet: its journal has not fully mirrored \
             (refused={}). Resolve the reported rows, run sync_mirror_push until the scope \
             drains, then retry the handover.",
            export["refused"].as_i64().unwrap_or(0)
        ));
    }

    // 2. Flip the role on the cloud.
    let handover_url = format!("{base}{HANDOVER_PATH}");
    let body = json!({ "uuid": uuid, "role": role });
    let resp = client
        .post(&handover_url)
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("cannot reach the website at {handover_url}: {e}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        crate::auth::token_rejected(&settings).await;
    }
    if !status.is_success() {
        let msg = handover_http_error(resp).await;
        record_handover_error(&settings, &uuid, &msg);
        return Err(msg);
    }
    let rj: Value = resp.json().await.map_err(|e| format!("unexpected handover response: {e}"))?;

    let changed = rj["changed"].as_bool().unwrap_or(false);
    let journal_role = rj["journal_role"].as_str().unwrap_or(role).to_string();
    let journal_epoch = rj["journal_epoch"].as_i64().unwrap_or(0);

    // 3. Record the epoch the cloud now holds, and the handover state for status.
    if let Ok(conn) = datasets::dictation::open_app_db(&settings) {
        let _ = ensure_handover_table(&conn);
        let _ = change_log::set_scope_epoch(&conn, &uuid, journal_epoch);
        set_handover_state(&conn, &uuid, journal_epoch, &journal_role, None);
    }

    log::warn!(
        "[sync] handover to cloud: scope '{uuid}' role='{journal_role}' epoch={journal_epoch} changed={changed} (export pushed={})",
        export["pushed"].as_i64().unwrap_or(0)
    );

    Ok(json!({
        "uuid": uuid,
        "target": base,
        "role_requested": role,
        "changed": changed,
        "journal_role": journal_role,
        "journal_epoch": journal_epoch,
        "export": {
            "pushed": export["pushed"],
            "deduped": export["deduped"],
            "refused": export["refused"],
            "cursor_after": export["cursor_after"],
        },
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

    // The role the website insists on being explicit: absent defaults to the
    // desktop→cloud case, `mirror` hands back, anything else is refused (never
    // silently defaulted so a typo can't accidentally hand a scope over).
    #[test]
    fn normalize_handover_role_defaults_and_rejects_unknown() {
        assert_eq!(normalize_handover_role(None).unwrap(), "authority");
        assert_eq!(normalize_handover_role(Some("")).unwrap(), "authority");
        assert_eq!(normalize_handover_role(Some("  mirror ")).unwrap(), "mirror");
        assert_eq!(normalize_handover_role(Some("authority")).unwrap(), "authority");
        assert!(normalize_handover_role(Some("leader")).is_err());
    }

    // A not-drained export blocks an authority handover (INV-1: the cloud can't
    // self-seed above a journal with holes). Refusals or transport errors both trip it.
    #[test]
    fn export_is_complete_requires_no_refusals_and_no_errors() {
        assert!(export_is_complete(&json!({ "refused": 0, "errors": [] })));
        // Missing fields default safe-ways: absent errors = complete.
        assert!(export_is_complete(&json!({})));
        assert!(!export_is_complete(&json!({ "refused": 2, "errors": [] })));
        assert!(!export_is_complete(&json!({ "refused": 0, "errors": ["boom"] })));
    }

    #[test]
    fn handover_state_round_trips_and_records_error() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_handover_table(&conn).unwrap();
        let read = |conn: &Connection| {
            conn.query_row(
                "SELECT journal_epoch, journal_role, last_error FROM journal_handover_state WHERE scope = 'ds'",
                [],
                |r| Ok::<(i64, Option<String>, Option<String>), rusqlite::Error>((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok()
        };
        assert!(read(&conn).is_none());
        set_handover_state(&conn, "ds", 4, "authority", None);
        assert_eq!(read(&conn), Some((4, Some("authority".to_string()), None)));
        // A re-handover updates epoch/role in place.
        set_handover_state(&conn, "ds", 5, "mirror", None);
        assert_eq!(read(&conn), Some((5, Some("mirror".to_string()), None)));
    }

    // The status read seam (finish 7b): every row back, scope-sorted, NULL role
    // preserved for a recorded failure, and a never-handed-over hub reads empty
    // instead of faulting on the missing table.
    #[test]
    fn handover_states_reads_every_row_sorted_and_tolerates_empty() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(handover_states(&conn).unwrap().is_empty());

        set_handover_state(&conn, "ds-b", 3, "authority", None);
        let err = "journal has not fully mirrored to the cloud; handover blocked (INV-1)";
        conn.execute(
            "INSERT INTO journal_handover_state (scope, journal_epoch, journal_role, handed_over_at, last_error) VALUES ('ds-a', 0, NULL, datetime('now'), ?1)",
            params![err],
        )
        .unwrap();

        assert_eq!(
            handover_states(&conn).unwrap(),
            vec![
                ("ds-a".to_string(), None, 0, Some(err.to_string())),
                ("ds-b".to_string(), Some("authority".to_string()), 3, None),
            ]
        );
    }
}
