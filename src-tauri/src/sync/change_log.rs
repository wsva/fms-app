//! Hub-side change machinery (`docs/my_sync_design.md` §3.2, §3.6).
//!
//! The hub records every dataset-row mutation in an append-only `sync_log`
//! living in the **app DB** (`<workspace>/app.sqlite3`), so followers can pull
//! row-level deltas (`GET /datasets/{uuid}/changes?after=SEQ`) instead of
//! re-downloading a whole-directory snapshot for one edited subtitle.
//!
//! Key rules baked in here:
//! * **Write order is dataset DB first, `sync_log` second** — see
//!   [`commit_change`], which is called only *after* the row has landed. The
//!   two live in different files, so they are not one transaction; the logical
//!   drift hash (§3.2, checked in a later phase) catches a crash in between.
//! * **Coalescing / later-wins is only valid for `state` kinds** — captured by
//!   [`ChangeClass`]. Append-only (history) and counter (XP) kinds keep distinct
//!   rows and are never folded together.
//! * Deletes are recorded in a per-dataset-DB `tombstones` table (not a
//!   `deleted_at` column on every synced table), which travels with snapshots.
//!
//! Compiled on all platforms: the tables live in the app DB (present
//! everywhere), but only a workspace designated `role = "hub"` actually appends
//! log rows. Every other writer — a phone or a `role = "follower"` desktop —
//! funnels its mutations through [`commit_change`] into `sync.rs::enqueue_change`
//! for writeback, so both follower kinds share one capture path (§3.4).

use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use crate::datasets;
use crate::settings::SettingsState;

/// Set while a **follower is applying changes pulled from the hub** (§3.3 apply
/// rules: "apply path writes rows without enqueue/log"). The pull-apply reuses
/// the same write commands the UI does, so without this they would re-enqueue
/// (mobile) or re-log (hub-role) a change that is only arriving *from* the hub —
/// an echo loop. [`commit_change`] short-circuits while it is set. RAII-scoped
/// via [`ApplyGuard`] so it is always cleared, even on early return / panic.
static APPLYING_REMOTE: AtomicBool = AtomicBool::new(false);

/// RAII guard suppressing change capture for the duration of a follower apply.
#[must_use]
pub struct ApplyGuard;

impl ApplyGuard {
    pub fn new() -> Self {
        APPLYING_REMOTE.store(true, Ordering::SeqCst);
        ApplyGuard
    }
}

impl Drop for ApplyGuard {
    fn drop(&mut self) {
        APPLYING_REMOTE.store(false, Ordering::SeqCst);
    }
}

/// Whether change capture is currently suppressed by a follower apply.
pub fn is_applying_remote() -> bool {
    APPLYING_REMOTE.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// Schema (hub-only tables, app DB)
// ---------------------------------------------------------------------------

/// Create the hub-side sync tables idempotently. `seq` is `AUTOINCREMENT` so a
/// pruned row can never let its number be reused (§3.2: a bare rowid alias
/// would fire the "hub seq < cursor" alarm falsely after a prune + delete).
pub fn ensure_hub_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_log (
            seq          INTEGER PRIMARY KEY AUTOINCREMENT,
            dataset_uuid TEXT NOT NULL,
            object_id    TEXT NOT NULL,
            kind         TEXT NOT NULL,
            op           TEXT NOT NULL,   /* upsert | delete */
            edit_time    TEXT NOT NULL,
            user_key     TEXT NOT NULL,
            payload      TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_sync_log_ds_seq ON sync_log(dataset_uuid, seq);

        CREATE TABLE IF NOT EXISTS sync_prune_marks (
            dataset_uuid  TEXT PRIMARY KEY,
            pruned_up_to  INTEGER NOT NULL DEFAULT 0
        );

        /* Replay dedup ledger (§3.6): a lost ack response makes the follower
           re-send the same change_id; the hub must apply it once. Pruned by
           `applied_at` (hub wall time), never by `edit_time`. */
        CREATE TABLE IF NOT EXISTS applied_changes (
            change_id  TEXT PRIMARY KEY,
            applied_at TEXT NOT NULL
        );

        /* Per-file content hash cache for the manifest (§3.2): re-hashing
           multi-GB media on every manifest call is prohibitive, so a hash is
           reused while its (size, mtime) tuple is unchanged. */
        CREATE TABLE IF NOT EXISTS file_hash_cache (
            dataset_uuid TEXT NOT NULL,
            path         TEXT NOT NULL,
            size         INTEGER NOT NULL,
            mtime        INTEGER NOT NULL,
            sha256       TEXT NOT NULL,
            PRIMARY KEY (dataset_uuid, path)
        );",
    )
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Change classification (§3.6)
// ---------------------------------------------------------------------------

/// How a writeback `kind` behaves under coalescing and conflict resolution.
/// Getting this wrong silently loses data, so every kind is classified
/// explicitly (defaulting to the safest `AppendOnly` for unknown kinds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeClass {
    /// A mutable row keyed by `object_id`; the queue coalesces to the latest
    /// and conflicts resolve later-`edit_time`-wins.
    State,
    /// History rows inserted by a client uuid PK; never coalesced, always kept.
    AppendOnly,
    /// A value accumulated as deltas (XP); never coalesced, deltas are summed.
    Counter,
}

impl ChangeClass {
    pub fn of(kind: &str) -> Self {
        match kind {
            // state: cue/card/tag/book structure rows, editable in place.
            "cue_save"
            | "cue_delete"
            | "card_save"
            | "card_delete"
            | "card_tag_save"
            | "card_tag_delete"
            | "card_set_tags"
            | "book_chapter_save"
            | "book_chapter_delete"
            | "book_sentence_save"
            | "book_sentences_save"
            | "book_sentence_delete"
            | "book_word_save"
            | "book_word_delete"
            | "read_text_save"
            | "read_text_delete"
            | "dictation" => ChangeClass::State,
            // counter: XP is enqueued as an amount delta (§3.6).
            "xp" => ChangeClass::Counter,
            // append-only: review history + anything unrecognised (safe: keep).
            "card_review" | "read_attempt_save" | "read_attempt_delete" => ChangeClass::AppendOnly,
            _ => ChangeClass::AppendOnly,
        }
    }

    /// Whether repeated edits to one `object_id` may be folded to the latest.
    pub fn coalescible(&self) -> bool {
        matches!(self, ChangeClass::State)
    }
}

// ---------------------------------------------------------------------------
// Journal scopes
// ---------------------------------------------------------------------------

/// Change-log scope for **per-user app data** (`dictation` progress, `xp`
/// awards). Those tables live in the app DB, not in any dataset DB, so binding
/// their journal rows to a dataset uuid made delivery depend on the receiver
/// having subscribed to that dataset — and awards with no dataset at all (book
/// reading XP) landed under the empty scope, which no route served and no puller
/// ever read. They all share one named scope the hub serves from its own
/// endpoint (`GET /api/v1/app/changes`) and every device pulls once per round,
/// dataset subscriptions or not.
///
/// The scope is a *journal* key only: the dataset a change refers to stays inside
/// the payload (`xp` carries `dataset_uuid`; `dictation` needs none, its media +
/// subtitle uuids are globally unique and the app-DB row is keyed by them).
pub const APP_SCOPE: &str = "@app";

/// Whether `kind` mutates per-user app data and therefore journals under
/// [`APP_SCOPE`] instead of under a dataset uuid.
pub fn is_app_scope_kind(kind: &str) -> bool {
    matches!(kind, "dictation" | "xp")
}

/// The journal scope a change of `kind` belongs to: [`APP_SCOPE`] for per-user
/// app data, the dataset uuid for shared dataset content. Both sides of the wire
/// (hub append, follower enqueue, hub conflict lookup) must resolve the scope
/// through this one function, or a change gets logged where no puller looks.
pub fn scope_for<'a>(kind: &str, dataset_uuid: &'a str) -> &'a str {
    if is_app_scope_kind(kind) {
        APP_SCOPE
    } else {
        dataset_uuid
    }
}

/// Derive the mutation op from the kind name (mirrors the naming convention:
/// every delete kind ends in `_delete`).
pub fn op_for(kind: &str) -> &'static str {
    if kind.ends_with("_delete") {
        "delete"
    } else {
        "upsert"
    }
}

/// Stable per-row identifier for conflict coalescing / later-wins. Extracted
/// from the change payload so call sites need not pass it. `book_sentences_save`
/// (a bulk insert) has no single natural id and gets an empty one, which keeps it
/// out of both coalescing and the pull-side pending-edit guard (an empty id never
/// matches). Per-user kinds must fold `user_key` into the id: the scope they
/// journal under is shared by every user, so an id derived from content alone
/// would let one user's push lose a later-wins contest against another user's row
/// (see [`APP_SCOPE`]).
pub fn object_id_for(kind: &str, payload: &Value, user_key: &str) -> String {
    let get = |k: &str| payload.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    match kind {
        "cue_save" => get("uuid"),
        "cue_delete" => {
            let u = get("cue_uuid");
            if u.is_empty() { get("uuid") } else { u }
        }
        // Progress rows are keyed by (user, media, subtitle) — the same triple
        // the app-DB upsert deletes on.
        "dictation" => {
            let m = get("media_uuid");
            let s = get("subtitle_uuid");
            format!("{user_key}:{m}:{s}")
        }
        "card_save" => get("uuid"),
        "card_delete" | "card_review" | "card_set_tags" => get("card_uuid"),
        "card_tag_save" => get("uuid"),
        "card_tag_delete" => get("tag_uuid"),
        "book_chapter_save" | "book_chapter_delete"
        | "book_sentence_save" | "book_sentence_delete"
        | "book_word_save" | "book_word_delete" => get("uuid"),
        "book_sentences_save" => String::new(),
        // Read-aloud texts are editable rows; attempts are insert-once history.
        // Both carry their own uuid, as does each delete payload.
        "read_text_save" | "read_text_delete"
        | "read_attempt_save" | "read_attempt_delete" => get("uuid"),
        // A counter delta is never folded, but it still needs an id that is
        // unique per award: an empty one would collide with every other pending
        // delta in the pull-side guard and silently suppress inbound XP rows.
        "xp" => format!("{user_key}:{}", get("reference_id")),
        _ => get("uuid"),
    }
}

// ---------------------------------------------------------------------------
// Append / read
// ---------------------------------------------------------------------------

/// Append one row-mutation to the hub's `sync_log`, returning its `seq`. Must
/// be called *after* the dataset DB write has succeeded (§3.2 write order).
pub fn append(
    conn: &Connection,
    dataset_uuid: &str,
    object_id: &str,
    kind: &str,
    edit_time: &str,
    user_key: &str,
    payload: &str,
) -> Result<i64, String> {
    ensure_hub_tables(conn)?;
    let op = op_for(kind);
    conn.execute(
        "INSERT INTO sync_log (dataset_uuid, object_id, kind, op, edit_time, user_key, payload) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![dataset_uuid, object_id, kind, op, edit_time, user_key, payload],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

/// One `sync_log` row shipped to a follower for replay.
#[derive(Serialize)]
pub struct SyncEntry {
    pub seq: i64,
    pub object_id: String,
    pub kind: String,
    pub op: String,
    pub edit_time: String,
    pub user_key: String,
    /// The change payload as raw JSON (parsed by the follower's apply path).
    pub payload: Value,
}

/// A page of `sync_log` rows after a cursor, plus the metadata a follower needs
/// to detect a pruned gap (§3.5) and to know the hub's current high-water `seq`
/// for the drift check (§3.2).
#[derive(Serialize)]
pub struct ChangesPage {
    pub dataset_uuid: String,
    pub entries: Vec<SyncEntry>,
    /// Highest pruned `seq` for this dataset; a follower whose cursor is below
    /// it has a gap and must resync (`resync_required`).
    pub pruned_up_to: i64,
    /// The hub's current `seq` at the moment this page was built.
    pub hub_seq: i64,
    pub resync_required: bool,
}

/// Read every `sync_log` row for `dataset_uuid` with `seq > after`, oldest
/// first. Sets `resync_required` when the follower's cursor predates a prune.
pub fn read_changes(conn: &Connection, dataset_uuid: &str, after: i64) -> Result<ChangesPage, String> {
    ensure_hub_tables(conn)?;
    let pruned_up_to: i64 = conn
        .query_row(
            "SELECT pruned_up_to FROM sync_prune_marks WHERE dataset_uuid = ?1",
            params![dataset_uuid],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let hub_seq: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM sync_log WHERE dataset_uuid = ?1",
            params![dataset_uuid],
            |r| r.get(0),
        )
        .unwrap_or(0);
    // A cursor at or below the prune mark means rows it never saw are gone. A
    // negative cursor (`after < 0`) is the follower's NULL/unknown-cursor
    // sentinel (§3.2 upgrade migration) and always forces one resync.
    let resync_required = after < pruned_up_to;

    let mut entries = Vec::new();
    if !resync_required {
        let mut stmt = conn
            .prepare(
                "SELECT seq, object_id, kind, op, edit_time, user_key, payload \
                 FROM sync_log WHERE dataset_uuid = ?1 AND seq > ?2 ORDER BY seq ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![dataset_uuid, after], |r| {
                let payload_str: String = r.get(6)?;
                let payload: Value =
                    serde_json::from_str(&payload_str).unwrap_or(Value::Null);
                Ok(SyncEntry {
                    seq: r.get(0)?,
                    object_id: r.get(1)?,
                    kind: r.get(2)?,
                    op: r.get(3)?,
                    edit_time: r.get(4)?,
                    user_key: r.get(5)?,
                    payload,
                })
            })
            .map_err(|e| e.to_string())?;
        for row in rows.flatten() {
            entries.push(row);
        }
    }

    Ok(ChangesPage {
        dataset_uuid: dataset_uuid.to_string(),
        entries,
        pruned_up_to,
        hub_seq,
        resync_required,
    })
}

/// Days of `sync_log` history the hub keeps before pruning (§3.5 retention).
pub const LOG_RETENTION_DAYS: i64 = 30;

/// Prune the hub's change journal to the last `keep_days` days (§3.5): delete
/// `sync_log` rows older than the cutoff, then raise each touched dataset's
/// `sync_prune_marks.pruned_up_to` to the highest deleted `seq`. That bump is
/// what turns a prune into a correctness signal: a follower still cursor-ing
/// below it has a gap it can never be served, so its next `/changes` pull is
/// told `resync_required` and it takes one fresh snapshot. Also expires the
/// `applied_changes` dedup ledger on the same clock. Pruning is done in
/// SQLite's own `datetime()` domain so RFC-3339 `edit_time` and
/// `datetime('now')`-style `applied_at` are both compared correctly.
/// Returns the per-dataset prune high-water marks that were applied.
pub fn prune_log(conn: &Connection, keep_days: i64) -> Result<Vec<(String, i64)>, String> {
    ensure_hub_tables(conn)?;
    let window = format!("-{keep_days} days");
    // Highest seq that is about to be removed, per dataset, captured *before*
    // the delete so the prune mark lands on the exact gap boundary.
    let mut hi: Vec<(String, i64)> = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT dataset_uuid, MAX(seq) FROM sync_log \
                 WHERE datetime(edit_time) < datetime('now', ?1) GROUP BY dataset_uuid",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![&window], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| e.to_string())?;
        for row in rows.flatten() {
            hi.push(row);
        }
    }
    // The dedup ledger and hash cache age out on the same clock regardless of
    // whether any change rows were dropped this pass.
    let _ = conn.execute(
        "DELETE FROM applied_changes WHERE datetime(applied_at) < datetime('now', ?1)",
        params![&window],
    );
    if hi.is_empty() {
        return Ok(Vec::new());
    }
    conn.execute(
        "DELETE FROM sync_log WHERE datetime(edit_time) < datetime('now', ?1)",
        params![&window],
    )
    .map_err(|e| e.to_string())?;
    for (uuid, max_seq) in &hi {
        // Only ever raise the mark — a later prune must never lower it and let
        // a stale delta slip past a follower that already passed this seq.
        conn.execute(
            "INSERT INTO sync_prune_marks(dataset_uuid, pruned_up_to) VALUES(?1, ?2) \
             ON CONFLICT(dataset_uuid) DO UPDATE SET \
                 pruned_up_to = MAX(pruned_up_to, excluded.pruned_up_to)",
            params![uuid, max_seq],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(hi)
}

/// Unix seconds of the last retention pass; `maybe_prune` no-ops within a day
/// so the journal is compacted roughly once per day, not per request.
static LAST_PRUNE_UNIX: AtomicI64 = AtomicI64::new(0);

/// Opportunistic hub-side retention (§3.5). Cheap enough to call from the hot
/// `/changes` path: it returns immediately until a day has elapsed since the
/// last pass, and a compare-and-swap means concurrent pulls do not all prune.
pub fn maybe_prune(conn: &Connection) {
    let now = chrono::Utc::now().timestamp();
    let last = LAST_PRUNE_UNIX.load(Ordering::Relaxed);
    if now - last < 86_400 {
        return;
    }
    if LAST_PRUNE_UNIX
        .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    match prune_log(conn, LOG_RETENTION_DAYS) {
        Ok(marks) => {
            if !marks.is_empty() {
                log::info!("[sync] change-log retention pruned {} dataset(s)", marks.len());
            }
        }
        Err(e) => log::warn!("[sync] change-log retention failed: {e}"),
    }
}

/// The newest `sync_log` entry for one row (`dataset_uuid` + `object_id`), the
/// basis for hub-side later-wins conflict resolution (§3.4). Because every hub
/// write is DB-first / log-second, the log's latest `edit_time` for an object
/// mirrors the live row's `updated_at`, and a latest `op = "delete"` mirrors a
/// tombstone — so this one query is a faithful, dataset-agnostic stand-in for
/// "compare against the live row / tombstone" without reaching into each table.
pub fn latest_for_object(
    conn: &Connection,
    dataset_uuid: &str,
    object_id: &str,
) -> Result<Option<(i64, String, String, Value)>, String> {
    ensure_hub_tables(conn)?;
    let mut stmt = conn
        .prepare_cached(
            "SELECT seq, edit_time, op, payload FROM sync_log \
             WHERE dataset_uuid = ?1 AND object_id = ?2 \
             ORDER BY seq DESC LIMIT 1",
        )
        .map_err(|e| e.to_string())?;
    let row = stmt
        .query_row(params![dataset_uuid, object_id], |r| {
            let payload: String = r.get(3)?;
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                payload,
            ))
        })
        .ok();
    Ok(row.map(|(seq, edit_time, op, p)| {
        (seq, edit_time, op, serde_json::from_str(&p).unwrap_or(Value::Null))
    }))
}

/// The hub's current `sync_log` cursor for one dataset — the seq a follower
/// records after taking a full snapshot, so its first `/changes` continues from
/// exactly the state the snapshot already contains (§3.2). Reads the live max
/// (not the pruned mark): a snapshot bundles every row up to this seq.
pub fn hub_seq_for(conn: &Connection, dataset_uuid: &str) -> Result<i64, String> {
    ensure_hub_tables(conn)?;
    conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) FROM sync_log WHERE dataset_uuid = ?1",
        params![dataset_uuid],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Replay dedup ledger (§3.6)
// ---------------------------------------------------------------------------

/// Whether `change_id` was already applied (so a re-sent batch is a no-op).
pub fn already_applied(conn: &Connection, change_id: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM applied_changes WHERE change_id = ?1",
        params![change_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// Record `change_id` as applied. Idempotent (`INSERT OR IGNORE`).
pub fn mark_applied(conn: &Connection, change_id: &str) {
    let _ = conn.execute(
        "INSERT OR IGNORE INTO applied_changes (change_id, applied_at) VALUES (?1, datetime('now'))",
        params![change_id],
    );
}

// ---------------------------------------------------------------------------
// Tombstones (per-dataset-DB, §3.4)
// ---------------------------------------------------------------------------

/// Ensure the `tombstones` table exists in a dataset DB. Called from each
/// dataset-type schema initializer so a tombstone travels with the snapshot.
pub fn ensure_tombstones(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tombstones (
            object_id  TEXT PRIMARY KEY,
            table_name TEXT NOT NULL,
            deleted_at TEXT NOT NULL
        );",
    )
    .map_err(|e| e.to_string())
}

/// Record a delete so a later resurrected edit from an offline device collides
/// with it and is dropped rather than silently recreating the row.
pub fn record_tombstone(
    conn: &Connection,
    object_id: &str,
    table_name: &str,
    deleted_at: &str,
) -> Result<(), String> {
    ensure_tombstones(conn)?;
    conn.execute(
        "INSERT OR REPLACE INTO tombstones (object_id, table_name, deleted_at) VALUES (?1, ?2, ?3)",
        params![object_id, table_name, deleted_at],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// The `deleted_at` timestamp for `object_id`, if it was deleted.
pub fn tombstone_of(conn: &Connection, object_id: &str) -> Result<Option<String>, String> {
    ensure_tombstones(conn)?;
    conn.query_row(
        "SELECT deleted_at FROM tombstones WHERE object_id = ?1",
        params![object_id],
        |r| r.get::<_, String>(0),
    )
    .map(Some)
    .or_else(|e| {
        if let rusqlite::Error::QueryReturnedNoRows = e {
            Ok(None)
        } else {
            Err(e.to_string())
        }
    })
}

/// Physical table a change kind mutates, recorded on a tombstone so the delete
/// is self-describing. Best-effort: unknown kinds land under `"row"`.
pub fn table_for_kind(kind: &str) -> &'static str {
    match kind {
        "cue_save" | "cue_delete" => "listen_subtitle_cue",
        "dictation" => "listen_dictation",
        "card_save" | "card_delete" | "card_review" | "card_set_tags" => "cards",
        "card_tag_save" | "card_tag_delete" => "card_tags",
        "book_chapter_save" | "book_chapter_delete" => "book_chapters",
        "book_sentence_save" | "book_sentences_save" | "book_sentence_delete" => "book_sentences",
        "book_word_save" | "book_word_delete" => "book_sentence_words",
        "read_text_save" | "read_text_delete" => "read_text",
        "read_attempt_save" | "read_attempt_delete" => "read_attempt",
        _ => "row",
    }
}

/// Open a dataset's own SQLite DB (`<dataset_dir>/data.sqlite3`) so a hub-side
/// delete can record a tombstone that travels with future snapshots.
#[cfg(feature = "desktop")]
fn open_dataset_db(settings: &SettingsState, dataset_uuid: &str) -> Result<Connection, String> {
    let (dir, _ty) = crate::datasets::find_dataset_dir_typed(settings, dataset_uuid)?;
    let db = dir.join("data.sqlite3");
    if !db.exists() {
        return Err(format!("no dataset db for {dataset_uuid}"));
    }
    Connection::open(&db).map_err(|e| e.to_string())
}

/// Record a delete's tombstone in the dataset DB (hub path). Best-effort: a
/// missing/unresolvable dataset DB must never fail the already-committed write.
#[cfg(feature = "desktop")]
pub fn record_delete_tombstone(
    settings: &SettingsState,
    dataset_uuid: &str,
    object_id: &str,
    kind: &str,
    deleted_at: &str,
) {
    if object_id.is_empty() {
        return;
    }
    if let Ok(conn) = open_dataset_db(settings, dataset_uuid) {
        let _ = record_tombstone(&conn, object_id, table_for_kind(kind), deleted_at);
    }
}

/// Whether `object_id` carries a tombstone at least as new as `deleted_at_str`
/// (RFC3339 UTC). Used as a legacy safety net when a hub has *no* `sync_log`
/// entry for the object — e.g. a row deleted before incremental sync existed —
/// so a resurrected offline edit is dropped against the recorded delete rather
/// than silently recreating the row (§3.4).
#[cfg(feature = "desktop")]
pub fn is_tombstoned_at_least(
    settings: &SettingsState,
    dataset_uuid: &str,
    object_id: &str,
    deleted_at_str: &str,
) -> bool {
    let Ok(conn) = open_dataset_db(settings, dataset_uuid) else {
        return false;
    };
    match tombstone_of(&conn, object_id) {
        Ok(Some(deleted_at)) => deleted_at.as_str() >= deleted_at_str,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Choke point
// ---------------------------------------------------------------------------

/// The single change-capture choke point (§3.2). Every dataset-row mutation
/// funnels here *after* its DB write:
/// * **desktop, `role = "hub"`** → append to `sync_log` (the transport journal
///   followers pull from);
/// * **everything else (followers)** → append to `writeback_queue` to be pushed
///   to the hub.
///
/// The journal is attributed to this machine's own workspace identity. Callers
/// that write *on behalf of another user* — a hub replaying a paired device's
/// writeback — must use [`commit_change_as`] instead, or the row gets logged
/// under the wrong `user_key` and every reader that trusts it mis-attributes it.
///
/// Unknown/unclassified kinds are logged best-effort (object id derived from the
/// payload). Never fails the caller's write: the DB row is already committed,
/// and the drift check (§3.2) reconciles a missed log append.
pub fn commit_change(
    settings: &SettingsState,
    kind: &str,
    dataset_uuid: &str,
    payload: &Value,
) -> Result<(), String> {
    let user_key = crate::auth::workspace_identity(settings);
    commit_change_as(settings, kind, dataset_uuid, payload, &user_key)
}

/// [`commit_change`] with the acting identity supplied explicitly. Used by the
/// per-user write paths (`dictation` progress, XP awards), which know whose row
/// they are touching independently of who happens to be logged in here: a hub
/// replaying a phone's writeback passes the phone's bound identity, so the
/// journal entry — and the per-user `object_id` derived from it — describes the
/// real owner rather than the hub's current workspace user.
pub fn commit_change_as(
    settings: &SettingsState,
    kind: &str,
    dataset_uuid: &str,
    payload: &Value,
    user_key: &str,
) -> Result<(), String> {
    // A row landing because we just *pulled* it from the hub must not be
    // re-queued/re-logged — that would echo the change back to its source.
    if is_applying_remote() {
        return Ok(());
    }
    // Per-user app data journals under one shared scope, not under the dataset
    // that happens to be nearby (see [`APP_SCOPE`]).
    let scope = scope_for(kind, dataset_uuid);
    #[cfg(feature = "desktop")]
    {
        if settings.role() == "hub" {
            let object_id = object_id_for(kind, payload, user_key);
            let edit_time = chrono::Utc::now().to_rfc3339();
            let conn = datasets::dictation::open_app_db(settings)?;
            append(
                &conn,
                scope,
                &object_id,
                kind,
                &edit_time,
                user_key,
                &payload.to_string(),
            )?;
            // Deletes also land in the dataset DB's `tombstones` (§3.4) so a
            // delete survives as an explicit record inside snapshots, not only in
            // the app-level log. Only dataset-scoped kinds can be deletes.
            if op_for(kind) == "delete" {
                record_delete_tombstone(settings, dataset_uuid, &object_id, kind, &edit_time);
            }
            Ok(())
        } else {
            // Fat-follower desktop (Phase 5): a non-hub desktop queues the change
            // for writeback exactly like a phone does — the enqueue gate is the
            // runtime `role`, not a compile-time `cfg`. The hub role never gets
            // here, so it never enqueues to itself.
            crate::sync::client::enqueue_change(settings, kind, scope, payload, user_key)
        }
    }
    #[cfg(not(feature = "desktop"))]
    {
        crate::sync::client::enqueue_change(settings, kind, scope, payload, user_key)
    }
}
