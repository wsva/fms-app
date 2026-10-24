//! "Read aloud" datasets — a text is shown, the user records themselves reading
//! it, STT transcribes the take, and the transcript is scored against the
//! reference text. Passing scores earn XP.
//!
//! Storage mirrors the book/dictation dataset layout: each read-aloud dataset is
//! a self-contained directory under `<datasets_dir>/read_aloud/` with an
//! `info.json`, a `data.sqlite3` (texts + attempts) and a `media/` folder that
//! holds every recorded take.
//!
//! Scoring uses the word-alignment ASR metric from `asr_metric` —
//! `score = WIP × 100` with H/S/D/I counts and alignment chunks (0-100) — plus
//! two new axes built on the same take: fluency (VAD + word timings) and
//! pronunciation (CTC phoneme GOP). `assess.rs` is the single seam that turns a
//! take into that three-axis picture; this file owns storage, sync and commands.

pub(crate) mod assess;
pub(crate) mod fluency;
pub(crate) mod gop;
#[cfg(feature = "phoneme")]
pub(crate) mod phoneme;
pub(crate) mod tts_ref;
pub(crate) mod vad;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{Emitter, State};
use uuid::Uuid;

use crate::auth::workspace_identity;
use crate::datasets::{
    assert_uuid_free, canonical_stamp, dataset_roots, fork_base_to_overlay, is_under_download_root,
    move_to_trash, now_row_stamp, now_stamp, read_info, read_info_opt, touch_info, write_info,
    DatasetInfo, DatasetType, FORMAT_READ_ALOUD, INFO_FILE,
};
use crate::settings::SettingsState;
use crate::datasets::asr_metric::{score_asr, score_value, AsrScoreSummary};
use crate::xp::{xp_award_internal, XpAwardResult};

use assess::{Assessment, PronEvidence, PronReason};
use fluency::TimedWord;

/// Score thresholds for the scaled XP reward.
const XP_PASS_SCORE: f64 = 60.0;
const XP_GOOD_SCORE: f64 = 80.0;

/// Edge TTS voice used for the canonical pronunciation reference when the user
/// has not chosen one. Multilingual so the same default covers English, German
/// and the rest of the European set the TTS page offers.
pub(crate) const DEFAULT_TTS_VOICE: &str = "en-US-EmmaMultilingualNeural";

// ============================================================
// Types
// ============================================================

/// A read-aloud dataset (a collection of texts to practise).
#[derive(Clone, Serialize, Deserialize)]
pub struct ReadAloudMeta {
    pub uuid: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Absolute filesystem path of the dataset directory.
    pub path: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A single text to read aloud.
#[derive(Clone, Serialize, Deserialize)]
pub struct ReadText {
    pub uuid: String,
    pub dataset_uuid: String,
    pub order_num: i64,
    pub title: String,
    /// Free-form note (source, location, etc.).
    pub note: String,
    /// The reference text the user reads.
    pub content: String,
    /// LLM-normalized reference for scoring (numbers, abbreviations spelled
    /// out so they match what STT transcribes). Derived and disposable:
    /// `save_text_row` resets it whenever `content` changes; scoring falls back
    /// to `content` while it is null.
    #[serde(default)]
    pub normalized_content: Option<String>,
    /// Best score across all attempts (computed/stored, null if never attempted).
    pub best_score: Option<f64>,
    /// Number of recorded attempts (computed on read, not persisted).
    #[serde(default)]
    pub attempt_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// One recorded attempt at a text.
#[derive(Clone, Serialize, Deserialize)]
pub struct ReadAttempt {
    pub uuid: String,
    pub text_uuid: String,
    #[serde(default)]
    pub user_id: String,
    /// Audio path relative to the dataset dir (e.g. `media/<uuid>.wav`).
    pub audio_path: Option<String>,
    /// Resolved absolute audio path for playback (computed on read, not stored).
    #[serde(default)]
    pub audio_url: Option<String>,
    /// STT transcript of the recording.
    pub recognized: String,
    /// WIP-based ASR score 0-100 against the reference text.
    pub score: f64,
    /// The stored assessment (accuracy + fluency + pronunciation), when one was
    /// computed. `None` for takes recorded before assessment existed and for
    /// takes whose recording is gone. Additive: peers that do not know the key
    /// simply drop it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Value>,
    pub created_at: String,
}

/// Result of submitting a recorded attempt.
#[derive(Clone, Serialize)]
pub struct ReadAloudSubmitResult {
    pub score: f64,
    /// The full scoring picture (WER/MER, H/S/D/I, alignment chunks) for the
    /// submitted transcript.
    pub summary: AsrScoreSummary,
    pub attempt: ReadAttempt,
    /// Accuracy (unchanged WIP), fluency and pronunciation for this take. The
    /// accuracy number is byte-identical to `score` — see `assess.rs`.
    pub assessment: Assessment,
    /// XP awarded for this submission (0 when below threshold or already earned).
    pub xp_awarded: i64,
    pub lifetime_xp: i64,
    pub level: i64,
}

// ============================================================
// Helpers
// ============================================================

/// Resolve the directory to create new datasets in: the first configured
/// read-aloud root, or `<datasets_dir>/read_aloud` (created on demand).
fn primary_root(settings: &SettingsState) -> Result<PathBuf, String> {
    if let Some(root) = dataset_roots(settings, DatasetType::Read).into_iter().next() {
        return Ok(root);
    }
    let dir = settings.datasets_dir().join(DatasetType::Read.as_str());
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create read-aloud dir: {}", e))?;
    Ok(dir)
}

/// Find a read-aloud dataset directory by UUID across all configured locations.
fn find_dataset_dir(settings: &SettingsState, uuid: &str) -> Result<PathBuf, String> {
    for root in dataset_roots(settings, DatasetType::Read) {
        if !root.exists() {
            continue;
        }
        let entries = fs::read_dir(&root).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || !path.join(INFO_FILE).exists() {
                continue;
            }
            if let Some(info) = read_info_opt(&path) {
                if info.uuid == uuid {
                    return Ok(path);
                }
            }
        }
    }
    Err(format!("Read-aloud dataset with UUID {} not found", uuid))
}

/// Write-resolution: forks a read-only base under the download root into a
/// writable overlay (dataset-layers.md §6). Read commands keep using
/// [`find_dataset_dir`].
fn find_dataset_dir_write(settings: &SettingsState, uuid: &str) -> Result<PathBuf, String> {
    let dir = find_dataset_dir(settings, uuid)?;
    if is_under_download_root(settings, &dir) {
        return fork_base_to_overlay(settings, &dir, DatasetType::Read);
    }
    Ok(dir)
}

/// Open (and initialize) a dataset's SQLite database.
fn open_db(dir: &Path) -> Result<Connection, String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let conn = Connection::open(dir.join("data.sqlite3")).map_err(|e| e.to_string())?;
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS read_text (
            uuid         TEXT PRIMARY KEY,
            dataset_uuid TEXT NOT NULL,
            order_num    INTEGER NOT NULL DEFAULT 0,
            title        TEXT NOT NULL DEFAULT '',
            note         TEXT NOT NULL DEFAULT '',
            content      TEXT NOT NULL DEFAULT '',
            normalized_content TEXT,
            best_score   REAL,
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS read_attempt (
            uuid       TEXT PRIMARY KEY,
            text_uuid  TEXT NOT NULL,
            user_id    TEXT NOT NULL DEFAULT '',
            audio_path TEXT,
            recognized TEXT NOT NULL DEFAULT '',
            score      REAL NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_read_attempt_text
            ON read_attempt(text_uuid, created_at DESC);
        ",
    )
    .map_err(|e| e.to_string())?;
    // Idempotent migration: datasets created before the normalization column.
    let has_normalized: bool = conn
        .prepare("SELECT 1 FROM PRAGMA_table_info('read_text') WHERE name = 'normalized_content'")
        .and_then(|mut s| s.exists([]))
        .unwrap_or(false);
    if !has_normalized {
        conn.execute("ALTER TABLE read_text ADD COLUMN normalized_content TEXT", [])
            .map_err(|e| e.to_string())?;
    }
    // §3.4: a per-dataset tombstone table (travels with snapshots) so a delete
    // is durable and can collide with a resurrected offline edit.
    crate::sync::change_log::ensure_tombstones(&conn)?;
    Ok(conn)
}

/// The reference text scoring compares against: the LLM-normalized variant when
/// present (non-empty), the raw content otherwise. NULL/empty means "not
/// normalized yet" — the fallback is what makes the column disposable.
fn effective_reference(content: &str, normalized: Option<&str>) -> String {
    match normalized {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ => content.to_string(),
    }
}

/// Resolve a stored audio path to an absolute path for playback. Two bases are
/// tried: the per-user workspace audio dir (a new take, stored as a bare file
/// name) and the dataset dir (a legacy take migrated in place with its original
/// dataset-relative `media/…` path). A take recorded on another device resolves
/// to neither and yields `None` — recordings are device-local (§17).
fn resolve_audio_url(dataset_dir: &Path, audio_dir: &Path, audio_path: &Option<String>) -> Option<String> {
    let rel = audio_path.as_ref().filter(|p| !p.is_empty())?;
    if let Some(name) = Path::new(rel).file_name() {
        let local = audio_dir.join(name);
        if local.exists() {
            return Some(local.to_string_lossy().into_owned());
        }
    }
    let legacy = dataset_dir.join(rel);
    if legacy.exists() {
        return Some(legacy.to_string_lossy().into_owned());
    }
    None
}

// ============================================================
// Attempt events — per-user practice history in the app DB (§17)
// ============================================================
//
// A recorded take is an *event*, not a mutation of the dataset. Rows live in the
// app DB (`read_attempt_event`), keyed by their own uuid and owned by `user_id`,
// and the recording itself lives in a per-user workspace dir — never inside the
// dataset. So practising a read-only adopted base needs no overlay, never
// dirties a published hash or a `read_text` row (INV-9), and one user's scores
// and recordings no longer ride the content channel to everyone who syncs the
// dataset. Replay is `INSERT OR IGNORE` by event uuid; convergence is set
// equality, exactly like card reviews (`datasets/cards/review.rs`).

/// One recorded attempt as a per-user event.
#[derive(Clone, Debug)]
struct AttemptEvent {
    uuid: String,
    user_id: String,
    dataset_uuid: String,
    text_uuid: String,
    /// Device-local recording reference: a bare file name under [`audio_dir`] for
    /// a new take, or a legacy dataset-relative path for a migrated one.
    audio_path: Option<String>,
    recognized: String,
    score: f64,
    /// Compact assessment JSON (`assess::Assessment::metrics_json`). Capped at
    /// roughly 2 KB: aggregates, the twelve weakest words, at most forty pauses.
    /// Per-frame and per-phone arrays are never stored — they are re-derived
    /// from the device-local recording on demand.
    metrics: Option<String>,
    created_at: String,
}

/// Create the event table + migration marker in the app DB. Idempotent, and safe
/// to call on any connection to the app DB.
fn ensure_attempt_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS read_attempt_event (
            uuid         TEXT PRIMARY KEY,
            user_id      TEXT NOT NULL,
            dataset_uuid TEXT NOT NULL,
            text_uuid    TEXT NOT NULL,
            audio_path   TEXT,
            recognized   TEXT NOT NULL DEFAULT '',
            score        REAL NOT NULL DEFAULT 0,
            metrics_json TEXT,
            created_at   TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_rae_owner_text
            ON read_attempt_event(user_id, dataset_uuid, text_uuid, created_at);

        /* One row per (dataset, user) already folded out of the legacy in-dataset
           `read_attempt` table, so the seed costs one indexed lookup per open. */
        CREATE TABLE IF NOT EXISTS read_attempt_migrated (
            dataset_uuid TEXT NOT NULL,
            user_id      TEXT NOT NULL,
            migrated_at  TEXT NOT NULL,
            PRIMARY KEY (dataset_uuid, user_id)
        );",
    )
    .map_err(|e| e.to_string())?;

    // Idempotent migration for databases created before assessment existed. An
    // old row simply has no metrics; nothing is back-filled, because the old
    // recording may be gone and a recomputed score would be invented.
    let has_metrics: bool = conn
        .prepare("SELECT 1 FROM PRAGMA_table_info('read_attempt_event') WHERE name = 'metrics_json'")
        .and_then(|mut s| s.exists([]))
        .unwrap_or(false);
    if !has_metrics {
        conn.execute("ALTER TABLE read_attempt_event ADD COLUMN metrics_json TEXT", [])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The per-user, device-local directory attempt recordings live in: beside
/// `app.sqlite3` (same workspace resolution), never inside a dataset dir, so
/// recordings stay out of the shared manifest.
fn audio_dir(settings: &SettingsState) -> PathBuf {
    settings.workspace_subdir("read_aloud_audio", "")
}

/// Open the app DB with the attempt schema ensured, for the event-only paths
/// (append, delete, replay) that need no dataset content.
fn open_app_attempts(settings: &SettingsState) -> Result<Connection, String> {
    let conn = crate::datasets::dictation::open_app_db(settings)?;
    ensure_attempt_schema(&conn)?;
    Ok(conn)
}

/// A read-aloud dataset DB (read-resolved) with the app DB attached, plus the
/// identity whose attempts the queries see and the dirs audio resolves against.
/// Dropping this drops the attachment. Used by the reads that join attempt
/// events onto `read_text` (list texts / list attempts) and by the lazy seed.
struct AttemptCtx {
    conn: Connection,
    me: String,
    dataset_dir: PathBuf,
    audio_dir: PathBuf,
}

/// Open a dataset for practice reads: its own DB for content, the app DB
/// attached for my attempt events, under one connection so the join stays in SQL.
/// Takes the **read** resolved directory on purpose — reading practice state is
/// not editing, so it must not fork a read-only base into an overlay.
fn open_attempts(settings: &SettingsState, dataset_uuid: &str) -> Result<AttemptCtx, String> {
    let dataset_dir = find_dataset_dir(settings, dataset_uuid)?;
    let me = workspace_identity(settings);
    let app_path = crate::datasets::dictation::app_db_path(settings);
    // Open it once for real so the file and its tables exist before the ATTACH.
    {
        let app_conn = crate::datasets::dictation::open_app_db(settings)?;
        ensure_attempt_schema(&app_conn)?;
    }
    let conn = open_db(&dataset_dir)?;
    let _ = conn.busy_timeout(Duration::from_secs(5));
    conn.execute("ATTACH DATABASE ?1 AS app", params![app_path.to_string_lossy().as_ref()])
        .map_err(|e| format!("attaching app DB '{}': {e}", app_path.display()))?;
    let ctx = AttemptCtx { conn, me, dataset_dir, audio_dir: audio_dir(settings) };
    migrate_legacy_attempts(&ctx.conn, &ctx.me, dataset_uuid)?;
    Ok(ctx)
}

/// Fold this dataset's legacy in-dataset `read_attempt` rows into app-DB events
/// for the acting user, once per (dataset, user). Only *their own* rows (or the
/// unowned single-user legacy rows) are seeded, never another user's takes
/// (§17.5). Stamps are canonicalized so ordering matches new takes.
fn migrate_legacy_attempts(conn: &Connection, me: &str, dataset_uuid: &str) -> Result<(), String> {
    let seeded: bool = conn
        .query_row(
            "SELECT 1 FROM app.read_attempt_migrated WHERE dataset_uuid = ?1 AND user_id = ?2",
            params![dataset_uuid, me],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if seeded {
        return Ok(());
    }
    let now = now_row_stamp();
    let rows: Vec<(String, String, Option<String>, String, f64, Option<String>)> = {
        let mut stmt = conn
            .prepare(
                "SELECT uuid, text_uuid, audio_path, recognized, score, created_at FROM read_attempt \
                 WHERE user_id = ?1 OR user_id = '' OR user_id IS NULL",
            )
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map(params![me], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
            })
            .map_err(|e| e.to_string())?;
        mapped.filter_map(|r| r.ok()).collect()
    };
    for (uuid, text_uuid, audio_path, recognized, score, created_at) in rows {
        let created = created_at.map(|c| canonical_stamp(&c)).unwrap_or_else(|| now.clone());
        conn.execute(
            "INSERT OR IGNORE INTO app.read_attempt_event \
             (uuid, user_id, dataset_uuid, text_uuid, audio_path, recognized, score, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![uuid, me, dataset_uuid, text_uuid, audio_path, recognized, score, created],
        )
        .map_err(|e| format!("seeding attempt events: {e}"))?;
    }
    conn.execute(
        "INSERT OR REPLACE INTO app.read_attempt_migrated (dataset_uuid, user_id, migrated_at) VALUES (?1, ?2, ?3)",
        params![dataset_uuid, me, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Store one attempt event. `INSERT OR IGNORE` by uuid, so an author receiving
/// its own echo — or two devices converging on the same take — is a no-op.
fn append_attempt_event(conn: &Connection, ev: &AttemptEvent) -> Result<bool, String> {
    let changed = conn
        .execute(
            "INSERT OR IGNORE INTO read_attempt_event \
             (uuid, user_id, dataset_uuid, text_uuid, audio_path, recognized, score, metrics_json, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![ev.uuid, ev.user_id, ev.dataset_uuid, ev.text_uuid, ev.audio_path, ev.recognized, ev.score, ev.metrics, ev.created_at],
        )
        .map_err(|e| e.to_string())?;
    Ok(changed > 0)
}

/// The journal payload for one attempt event. `audio_url` is deliberately absent:
/// it is the sender's absolute path and is re-resolved per device on read.
fn attempt_payload(ev: &AttemptEvent) -> Value {
    json!({
        "uuid": ev.uuid,
        "user_id": ev.user_id,
        "dataset_uuid": ev.dataset_uuid,
        "text_uuid": ev.text_uuid,
        "audio_path": ev.audio_path,
        "recognized": ev.recognized,
        "score": ev.score,
        // Optional key: an old peer ignores it, a new peer reads it, and a
        // payload without it (`get()` returns None) stays valid — so no
        // PROTOCOL_VERSION bump is needed.
        "metrics": ev.metrics,
        "created_at": ev.created_at,
    })
}

/// Hub-side read behind the practice-history backfill (`GET /api/v1/app/state`,
/// sync-v2.md §17.6 / INV-21): page one identity's attempt events oldest-first so
/// a device that subscribes after the journal was pruned can still recover its
/// takes. Paginated by `rowid` (at most `limit` rows after cursor `after`, `0` =
/// start); the next cursor is `Some` only when the page was full, exactly like
/// [`crate::datasets::dictation::dictation_rows_for_user`]. Page-stable because an
/// event is insert-once — its rowid never moves. Rows carry the [`attempt_payload`]
/// shape so the follower feeds them straight to [`apply_remote_save`]. The
/// `audio_path` stays device-local (a bare file name or a legacy dataset-relative
/// path): the row backfills, the WAV bytes do not — as any synced attempt already
/// behaves.
// Hub-side read: only `sync::rest` calls it, so the mobile build has no caller.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub(crate) fn rows_for_user(
    settings: &SettingsState,
    user_id: &str,
    after: i64,
    limit: i64,
) -> Result<(Vec<Value>, Option<i64>), String> {
    let conn = open_app_attempts(settings)?;
    let mut stmt = conn
        .prepare(
            "SELECT rowid, uuid, user_id, dataset_uuid, text_uuid, audio_path, recognized, score, metrics_json, created_at \
             FROM read_attempt_event WHERE user_id = ?1 AND rowid > ?2 ORDER BY rowid ASC LIMIT ?3",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![user_id, after, limit], |row| {
            let rowid = row.get::<_, i64>(0)?;
            let value = json!({
                "uuid": row.get::<_, String>(1)?,
                "user_id": row.get::<_, String>(2)?,
                "dataset_uuid": row.get::<_, String>(3)?,
                "text_uuid": row.get::<_, String>(4)?,
                "audio_path": row.get::<_, Option<String>>(5)?,
                "recognized": row.get::<_, String>(6)?,
                "score": row.get::<_, f64>(7)?,
                "metrics": row.get::<_, Option<String>>(8)?,
                "created_at": row.get::<_, String>(9)?,
            });
            Ok((rowid, value))
        })
        .map_err(|e| e.to_string())?;
    let mut out: Vec<Value> = Vec::new();
    let mut last_id = after;
    for r in rows.filter_map(|r| r.ok()) {
        last_id = r.0;
        out.push(r.1);
    }
    let next = if out.len() as i64 >= limit { Some(last_id) } else { None };
    Ok((out, next))
}

/// Keep a stored stamp in the one fixed-width UTC shape ordering depends on.
fn canonical_or_now(value: Option<&str>) -> String {
    match value {
        Some(raw) if !raw.is_empty() => canonical_stamp(raw),
        _ => now_row_stamp(),
    }
}

/// Replay one journaled `read_attempt_save`: store the event it describes. The
/// `@app` scope passes no dataset, so it is read out of the payload (as `xp` and
/// `card_review` do). Returns `false` when the event was already present — the
/// normal outcome for the author receiving its own echo, not an error.
pub(crate) fn apply_remote_save(
    settings: &SettingsState,
    dataset_uuid: &str,
    payload: &Value,
    write_identity: &str,
) -> Result<bool, String> {
    let get = |k: &str| payload.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let uuid = get("uuid");
    if uuid.is_empty() {
        return Err("read_attempt_save change carries no uuid".to_string());
    }
    let dataset_uuid = if dataset_uuid.is_empty() { get("dataset_uuid") } else { dataset_uuid.to_string() };
    if dataset_uuid.is_empty() {
        return Err("read_attempt_save change carries no dataset_uuid".to_string());
    }
    let ev = AttemptEvent {
        uuid,
        // Per-user history: attribute to the identity the hub bound the device
        // to, never to the `user_id` the device declared (the `dictation` rule).
        user_id: if write_identity.is_empty() { get("user_id") } else { write_identity.to_string() },
        dataset_uuid: dataset_uuid.clone(),
        text_uuid: get("text_uuid"),
        audio_path: payload.get("audio_path").and_then(|v| v.as_str()).map(|s| s.to_string()),
        recognized: get("recognized"),
        score: payload.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0),
        // Stored as the JSON text it travelled as. A string keeps the exact
        // serialisation, so re-reading it on another device yields the same
        // metrics card instead of a re-typed object.
        metrics: payload.get("metrics").and_then(|v| match v {
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            Value::Null | Value::String(_) => None,
            other => Some(other.to_string()),
        }),
        created_at: canonical_or_now(payload.get("created_at").and_then(|v| v.as_str())),
    };
    let conn = open_app_attempts(settings)?;
    let inserted = append_attempt_event(&conn, &ev)?;
    if inserted {
        // Log it onward so other followers receive the same event uuid; the
        // pull-side `ApplyGuard` suppresses this, so it is how a hub fans out.
        let _ = crate::sync::change_log::commit_change_as(
            settings, "read_attempt_save", &dataset_uuid, &attempt_payload(&ev), &ev.user_id,
        );
    }
    Ok(inserted)
}

/// Replay one journaled `read_attempt_delete`: drop the event for its owner and
/// best-effort remove the local recording. The delete lives only in the `@app`
/// journal — no dataset tombstone (that would open and mutate a downloaded base).
pub(crate) fn apply_remote_delete(
    settings: &SettingsState,
    dataset_uuid: &str,
    payload: &Value,
    write_identity: &str,
) -> Result<bool, String> {
    let get = |k: &str| payload.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let uuid = get("uuid");
    if uuid.is_empty() {
        return Err("read_attempt_delete change carries no uuid".to_string());
    }
    let dataset_uuid = if dataset_uuid.is_empty() { get("dataset_uuid") } else { dataset_uuid.to_string() };
    let me = if write_identity.is_empty() { get("user_id") } else { write_identity.to_string() };
    let conn = open_app_attempts(settings)?;
    if let Ok(Some(rel)) = conn.query_row(
        "SELECT audio_path FROM read_attempt_event WHERE uuid = ?1",
        params![uuid],
        |row| row.get::<_, Option<String>>(0),
    ) {
        if let Some(name) = Path::new(&rel).file_name() {
            let _ = fs::remove_file(audio_dir(settings).join(name));
        }
    }
    let deleted = conn
        .execute("DELETE FROM read_attempt_event WHERE uuid = ?1 AND user_id = ?2", params![uuid, me])
        .map_err(|e| e.to_string())?;
    let _ = crate::sync::change_log::commit_change_as(
        settings, "read_attempt_delete", &dataset_uuid,
        &json!({ "uuid": uuid, "dataset_uuid": dataset_uuid }), &me,
    );
    Ok(deleted > 0)
}

/// Build a filesystem-safe directory/file name from a title.
fn sanitize_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        "read_aloud".to_string()
    } else {
        trimmed.to_string()
    }
}

// ============================================================
// Dataset commands
// ============================================================

/// List all read-aloud datasets across all configured locations.
#[tauri::command]
pub async fn read_aloud_list(
    settings: State<'_, SettingsState>,
) -> Result<Vec<ReadAloudMeta>, String> {
    Ok(list_datasets(&settings))
}

/// Core listing logic, reusable without Tauri `State` (e.g. MCP / web service).
pub(crate) fn list_datasets(settings: &SettingsState) -> Vec<ReadAloudMeta> {
    let mut out = Vec::new();
    for root in dataset_roots(settings, DatasetType::Read) {
        if !root.exists() {
            continue;
        }
        let entries = match fs::read_dir(&root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || !path.join(INFO_FILE).exists() {
                continue;
            }
            if let Some(info) = read_info_opt(&path) {
                if info.dataset_type != DatasetType::Read {
                    continue;
                }
                out.push(ReadAloudMeta {
                    uuid: info.uuid,
                    name: info.name,
                    description: info.description,
                    path: path.to_string_lossy().into_owned(),
                    created_at: info.created_at,
                    updated_at: info.updated_at,
                });
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Create a new read-aloud dataset with an initialized database.
#[tauri::command]
pub async fn read_aloud_create(
    settings: State<'_, SettingsState>,
    name: String,
    description: Option<String>,
) -> Result<ReadAloudMeta, String> {
    create_dataset(&settings, name, description.unwrap_or_default())
}

pub(crate) fn create_dataset(
    settings: &SettingsState,
    name: String,
    description: String,
) -> Result<ReadAloudMeta, String> {
    if name.trim().is_empty() {
        return Err("Name is required".into());
    }
    let root = primary_root(settings)?;

    let uuid = Uuid::new_v4().to_string();
    assert_uuid_free(settings, &uuid)?;
    let base_name = sanitize_name(&name);

    let mut dst = root.join(&base_name);
    if dst.exists() {
        dst = root.join(format!("{}-{}", base_name, &uuid[..8]));
    }
    fs::create_dir_all(dst.join("media")).map_err(|e| e.to_string())?;

    let mut info = DatasetInfo::new(
        uuid.clone(),
        DatasetType::Read,
        FORMAT_READ_ALOUD,
        name.clone(),
    );
    info.description = description.clone();
    write_info(&dst, &info)?;
    open_db(&dst)?;

    Ok(ReadAloudMeta {
        uuid,
        name,
        description,
        path: dst.to_string_lossy().into_owned(),
        created_at: info.created_at.clone(),
        updated_at: info.updated_at,
    })
}

/// Update a dataset's name and/or description.
#[tauri::command]
pub async fn read_aloud_update(
    settings: State<'_, SettingsState>,
    uuid: String,
    name: String,
    description: Option<String>,
) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Name is required".into());
    }
    let dir = find_dataset_dir_write(&settings, &uuid)?;
    let mut info = read_info(&dir)?;
    info.name = name;
    if let Some(d) = description {
        info.description = d;
    }
    info.updated_at = now_stamp();
    write_info(&dir, &info)
}

/// Delete a dataset: its directory moves into the shared trash instead of being
/// destroyed, so the operation stays undoable (see [`move_to_trash`]). Returns the
/// trash path.
#[tauri::command]
pub async fn read_aloud_delete(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<String, String> {
    let dir = find_dataset_dir_write(&settings, &uuid)?;
    log::info!("[ReadAloud] Deleting dataset at '{}'", dir.display());
    let trashed = move_to_trash(&dir)?;
    log::info!(
        "[ReadAloud] Dataset {} moved to trash '{}'",
        uuid,
        trashed.display()
    );
    Ok(trashed.to_string_lossy().into_owned())
}

// ============================================================
// Text commands
// ============================================================

/// List all texts in a dataset, ordered, with best score + attempt count.
#[tauri::command]
pub async fn read_aloud_list_texts(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<Vec<ReadText>, String> {
    list_texts(&settings, &dataset_uuid)
}

pub(crate) fn list_texts(settings: &SettingsState, dataset_uuid: &str) -> Result<Vec<ReadText>, String> {
    // Attempt stats are per-user practice state, so they come from the app DB
    // (attached), never from the shared `read_text.best_score` column — a text
    // downloaded from someone else must not surface their scores as mine (§17).
    let ctx = open_attempts(settings, dataset_uuid)?;
    let mut stmt = ctx
        .conn
        .prepare(
            "SELECT t.uuid, t.dataset_uuid, t.order_num, t.title, t.note, t.content, t.normalized_content, \
                    t.best_score, t.created_at, t.updated_at \
             FROM read_text t ORDER BY t.order_num, t.created_at",
        )
        .map_err(|e| e.to_string())?;
    let mut items: Vec<ReadText> = stmt
        .query_map([], |row| {
            Ok(ReadText {
                uuid: row.get(0)?,
                dataset_uuid: row.get(1)?,
                order_num: row.get(2)?,
                title: row.get(3)?,
                note: row.get(4)?,
                content: row.get(5)?,
                normalized_content: row.get(6)?,
                best_score: row.get(7)?,
                attempt_count: 0,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    // Recompute `best_score` + `attempt_count` from *my* stored transcripts with
    // the current metric, so attempts scored under an older scale never surface.
    let mut count: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let mut best: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    if let Ok(mut stmt) = ctx.conn.prepare(
        "SELECT a.text_uuid, a.recognized, t.content, t.normalized_content \
         FROM app.read_attempt_event a JOIN read_text t ON t.uuid = a.text_uuid \
         WHERE a.user_id = ?1",
    ) {
        if let Ok(rows) = stmt.query_map(params![&ctx.me], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        }) {
            for (text_uuid, recognized, content, normalized) in rows.flatten() {
                *count.entry(text_uuid.clone()).or_insert(0) += 1;
                let reference = effective_reference(&content, normalized.as_deref());
                let s = score_value(&reference, &recognized);
                let e = best.entry(text_uuid).or_insert(s);
                if s > *e {
                    *e = s;
                }
            }
        }
    }
    for text in &mut items {
        text.attempt_count = count.get(&text.uuid).copied().unwrap_or(0);
        // Best is mine alone: a text I have never attempted shows no score, even
        // if the row carried a publisher's cached `best_score`.
        text.best_score = best.get(&text.uuid).copied();
    }
    Ok(items)
}

/// Insert or update a text. `best_score`/`attempt_count` are managed by the
/// backend; any value supplied here for `best_score` is persisted as-is so the
/// frontend can round-trip a text it just read.
#[tauri::command]
pub async fn read_aloud_save_text(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    text: ReadText,
) -> Result<(), String> {
    save_text_row(&settings, &dataset_uuid, &text)
}

/// Write one text row and journal the change (§3.2 choke point). Shared by the
/// command and both sync apply paths (hub replay, follower pull-apply), so an
/// edit made on any device reaches the journal exactly once — `commit_change`
/// self-suppresses while a remote change is being applied.
///
/// Staleness rule: `normalized_content` is derived from `content`, so editing
/// the content resets it to NULL (re-normalize afterwards); a same-content save
/// (a title/order/note edit, or a round-tripped sync apply) preserves it.
pub(crate) fn save_text_row(
    settings: &SettingsState,
    dataset_uuid: &str,
    text: &ReadText,
) -> Result<(), String> {
    let dir = find_dataset_dir_write(settings, dataset_uuid)?;
    let conn = open_db(&dir)?;
    conn.execute(
        "INSERT INTO read_text \
         (uuid, dataset_uuid, order_num, title, note, content, normalized_content, best_score, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
         ON CONFLICT(uuid) DO UPDATE SET \
           order_num = excluded.order_num, \
           title = excluded.title, \
           note = excluded.note, \
           content = excluded.content, \
           normalized_content = CASE WHEN read_text.content = excluded.content \
               THEN COALESCE(excluded.normalized_content, read_text.normalized_content) \
               ELSE NULL END, \
           updated_at = excluded.updated_at",
        rusqlite::params![
            text.uuid,
            dataset_uuid,
            text.order_num,
            text.title,
            text.note,
            text.content,
            text.normalized_content,
            text.best_score,
            text.created_at,
            text.updated_at
        ],
    )
    .map_err(|e| e.to_string())?;
    let _ = touch_info(&dir);
    {
        let _ = crate::sync::change_log::commit_change(
            settings,
            "read_text_save",
            dataset_uuid,
            &serde_json::to_value(text).unwrap_or_default(),
        );
    }
    Ok(())
}

/// Delete a text. Per-user attempt events are left in the app DB (orphaned but
/// harmless — every read joins on `text_uuid`): a content edit never touches
/// practice data, mirroring how deleting a card leaves its review events.
#[tauri::command]
pub async fn read_aloud_delete_text(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    uuid: String,
) -> Result<(), String> {
    let dir = find_dataset_dir_write(&settings, &dataset_uuid)?;
    let conn = open_db(&dir)?;
    conn.execute("DELETE FROM read_text WHERE uuid = ?1", [&uuid])
        .map_err(|e| e.to_string())?;
    let _ = touch_info(&dir);
    {
        let _ = crate::sync::change_log::commit_change(
            &settings,
            "read_text_delete",
            &dataset_uuid,
            &json!({ "uuid": uuid }),
        );
    }
    Ok(())
}

// ============================================================
// Attempt commands
// ============================================================

/// List all recorded attempts for a text (newest first), with resolved audio URLs.
#[tauri::command]
pub async fn read_aloud_list_attempts(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    text_uuid: String,
) -> Result<Vec<ReadAttempt>, String> {
    // Only *my* attempts, read from the app DB and joined onto the dataset's text
    // for re-scoring — practising never wrote into the dataset (§17).
    let ctx = open_attempts(&settings, &dataset_uuid)?;
    let mut stmt = ctx
        .conn
        .prepare(
            "SELECT a.uuid, a.text_uuid, a.user_id, a.audio_path, a.recognized, a.score, a.created_at, \
                    t.content, t.normalized_content, a.metrics_json \
             FROM app.read_attempt_event a LEFT JOIN read_text t ON t.uuid = a.text_uuid \
             WHERE a.text_uuid = ?1 AND a.user_id = ?2 ORDER BY a.created_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let items = stmt
        .query_map(params![&text_uuid, &ctx.me], |row| {
            let audio_path: Option<String> = row.get(3)?;
            let recognized: String = row.get(4)?;
            let stored_score: f64 = row.get(5)?;
            // Re-score against the effective reference (normalized when present)
            // with the current metric so history is comparable across metric
            // changes; fall back to the stored value if the text row is gone.
            let score = match row.get::<_, Option<String>>(7)? {
                Some(content) => score_value(
                    &effective_reference(&content, row.get::<_, Option<String>>(8)?.as_deref()),
                    &recognized,
                ),
                None => stored_score,
            };
            let audio_url = resolve_audio_url(&ctx.dataset_dir, &ctx.audio_dir, &audio_path);
            Ok(ReadAttempt {
                uuid: row.get(0)?,
                text_uuid: row.get(1)?,
                user_id: row.get(2)?,
                audio_url,
                audio_path,
                recognized,
                score,
                metrics: stored_metrics(row.get::<_, Option<String>>(9)?),
                created_at: row.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(items)
}

/// Delete a single attempt (my event) and its device-local recording.
#[tauri::command]
pub async fn read_aloud_delete_attempt(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    uuid: String,
) -> Result<(), String> {
    delete_attempt_inner(&settings, &dataset_uuid, &uuid)
}

/// Delete my attempt event from the app DB, best-effort remove its recording,
/// and journal under `@app`. Shared by the command; the sync apply path uses
/// [`apply_remote_delete`]. Never touches the dataset directory.
fn delete_attempt_inner(settings: &SettingsState, dataset_uuid: &str, uuid: &str) -> Result<(), String> {
    let me = workspace_identity(settings);
    let conn = open_app_attempts(settings)?;
    if let Ok(Some(rel)) = conn.query_row(
        "SELECT audio_path FROM read_attempt_event WHERE uuid = ?1 AND user_id = ?2",
        params![uuid, &me],
        |row| row.get::<_, Option<String>>(0),
    ) {
        if let Some(name) = Path::new(&rel).file_name() {
            let _ = fs::remove_file(audio_dir(settings).join(name));
        }
        // A migrated legacy take still points into the dataset's media dir.
        if let Ok(dir) = find_dataset_dir(settings, dataset_uuid) {
            let _ = fs::remove_file(dir.join(&rel));
        }
    }
    conn.execute(
        "DELETE FROM read_attempt_event WHERE uuid = ?1 AND user_id = ?2",
        params![uuid, &me],
    )
    .map_err(|e| e.to_string())?;
    let _ = crate::sync::change_log::commit_change_as(
        settings,
        "read_attempt_delete",
        dataset_uuid,
        &json!({ "uuid": uuid, "dataset_uuid": dataset_uuid }),
        &me,
    );
    Ok(())
}

/// Re-run recognition for an existing attempt: overwrite its transcript,
/// re-score and re-assess it against the current reference, in place. The
/// recording is untouched — only the transcript (and everything derived from
/// it) changes — so this is the seam the UI's "Recognize again" button (and the
/// matching MCP tool) writes through after it re-transcribes the device-local
/// WAV with the loaded STT model.
#[tauri::command]
pub async fn read_aloud_rerecognize(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    uuid: String,
    recognized: String,
    words: Option<Vec<TimedWord>>,
) -> Result<ReadAttempt, String> {
    update_recognized_inner(&settings, &dataset_uuid, &uuid, &recognized, words.as_deref()).await
}

/// Decode the device-local recording behind an attempt, when it is still here.
/// A take recorded on another device resolves to nothing, and a decode failure
/// is logged rather than surfaced: the transcript can still be updated.
fn stored_recording(
    settings: &SettingsState,
    dataset_uuid: &str,
    audio_path: &Option<String>,
) -> Vec<f32> {
    let Some(rel) = audio_path.as_ref().filter(|p| !p.is_empty()) else {
        return Vec::new();
    };
    let Ok(dataset_dir) = find_dataset_dir(settings, dataset_uuid) else {
        return Vec::new();
    };
    let Some(path) = resolve_audio_url(&dataset_dir, &audio_dir(settings), &Some(rel.clone())) else {
        return Vec::new();
    };
    match crate::audio::decode_to_pcm(Path::new(&path)) {
        Ok(samples) => samples,
        Err(e) => {
            log::warn!("read_aloud: could not decode '{}' for assessment: {}", path, e);
            Vec::new()
        }
    }
}

/// Shared body of [`read_aloud_rerecognize`]: overwrite one of my attempt events'
/// transcript, re-score and re-assess it against the effective reference, and
/// re-journal the save. Never touches the dataset directory or the recording.
///
/// The read and the write take their own short-lived connections: the
/// assessment in between is async, and a `rusqlite::Connection` must not be
/// held across an await point.
async fn update_recognized_inner(
    settings: &SettingsState,
    dataset_uuid: &str,
    uuid: &str,
    recognized: &str,
    words: Option<&[TimedWord]>,
) -> Result<ReadAttempt, String> {
    let me = workspace_identity(settings);

    // Load the event (must belong to me) plus its text's reference for scoring.
    let (text_uuid, audio_path, created_at, content, normalized): (
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
    ) = {
        let ctx = open_attempts(settings, dataset_uuid)?;
        ctx.conn
            .query_row(
                "SELECT a.text_uuid, a.audio_path, a.created_at, t.content, t.normalized_content \
                 FROM app.read_attempt_event a LEFT JOIN read_text t ON t.uuid = a.text_uuid \
                 WHERE a.uuid = ?1 AND a.user_id = ?2",
                params![uuid, &me],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .map_err(|_| format!("Attempt {} not found", uuid))?
    };

    // Score against the effective reference while the text row still exists; a
    // missing row degrades to 0, the same fallback list_attempts applies.
    let reference = content.as_deref().map(|c| effective_reference(c, normalized.as_deref()));
    let summary = reference.as_deref().map(|r| score_asr(r, recognized));
    let score = summary.as_ref().map(|s| s.score).unwrap_or(0.0);

    // Re-derive the metrics from the recording that is still on this device.
    // A missing WAV leaves fluency and pronunciation unmeasured — the score
    // still updates, which is what "recognize again" means.
    let samples = stored_recording(settings, dataset_uuid, &audio_path);
    let metrics = match (summary.as_ref(), reference.as_deref()) {
        (Some(summary), Some(reference)) => {
            let assessment = assess_take(settings, summary, reference, recognized, &samples, words).await;
            stored_metrics(Some(assessment.metrics_json()))
        }
        _ => None,
    };

    {
        let conn = open_app_attempts(settings)?;
        conn.execute(
            "UPDATE read_attempt_event SET recognized = ?1, score = ?2, metrics_json = ?3 \
             WHERE uuid = ?4 AND user_id = ?5",
            params![
                recognized,
                score,
                metrics.as_ref().map(|v| v.to_string()),
                uuid,
                &me
            ],
        )
        .map_err(|e| e.to_string())?;
    }

    let audio_url = find_dataset_dir(settings, dataset_uuid)
        .ok()
        .and_then(|dir| resolve_audio_url(&dir, &audio_dir(settings), &audio_path));
    let ev = AttemptEvent {
        uuid: uuid.to_string(),
        user_id: me.clone(),
        dataset_uuid: dataset_uuid.to_string(),
        text_uuid: text_uuid.clone(),
        audio_path: audio_path.clone(),
        recognized: recognized.to_string(),
        score,
        metrics: metrics.as_ref().map(|v| v.to_string()),
        created_at: canonical_stamp(&created_at),
    };
    // Re-journal the save for the change log. Peers replay events by uuid with
    // INSERT OR IGNORE, so an already-delivered take keeps its own copy — the
    // right outcome, since re-recognition runs against a device-local recording
    // with a device-local model (§17).
    let _ = crate::sync::change_log::commit_change_as(
        settings,
        "read_attempt_save",
        dataset_uuid,
        &attempt_payload(&ev),
        &ev.user_id,
    );

    Ok(ReadAttempt {
        uuid: uuid.to_string(),
        text_uuid,
        user_id: me,
        audio_url,
        audio_path,
        recognized: recognized.to_string(),
        score,
        metrics,
        created_at,
    })
}

/// Persist one attempt as a per-user event in the app DB and journal it under
/// `@app`. Shared by `read_aloud_submit`; the sync apply paths use
/// [`apply_remote_save`]. Nothing is written into the dataset directory — the
/// recording lives in the per-user audio dir and the row in the app DB — so
/// practising never forks an adopted base nor dirties a `read_text` row (INV-9).
/// `audio_url` on the incoming attempt is the sender's absolute path and is
/// ignored; each device re-resolves it on read.
pub(crate) fn save_attempt_row(
    settings: &SettingsState,
    dataset_uuid: &str,
    attempt: &ReadAttempt,
) -> Result<(), String> {
    let conn = open_app_attempts(settings)?;
    let ev = AttemptEvent {
        uuid: attempt.uuid.clone(),
        user_id: attempt.user_id.clone(),
        dataset_uuid: dataset_uuid.to_string(),
        text_uuid: attempt.text_uuid.clone(),
        audio_path: attempt.audio_path.clone(),
        recognized: attempt.recognized.clone(),
        score: attempt.score,
        metrics: attempt.metrics.as_ref().map(|v| v.to_string()),
        created_at: canonical_stamp(&attempt.created_at),
    };
    append_attempt_event(&conn, &ev)?;
    let _ = crate::sync::change_log::commit_change_as(
        settings,
        "read_attempt_save",
        dataset_uuid,
        &attempt_payload(&ev),
        &ev.user_id,
    );
    Ok(())
}

/// Number of texts in a dataset. Used as the read-aloud `media_count` in the
/// REST sync catalog; best-effort, so an unreadable dataset reports 0.
pub(crate) fn count_texts(settings: &SettingsState, dataset_uuid: &str) -> usize {
    find_dataset_dir(settings, dataset_uuid)
        .and_then(|dir| {
            let conn = open_db(&dir)?;
            conn
                .query_row("SELECT COUNT(*) FROM read_text", [], |r| r.get::<_, i64>(0))
                .map(|n| n as usize)
                .map_err(|e| e.to_string())
        })
        .unwrap_or(0)
}

/// Pure ASR score (WIP × 100) plus the word-level alignment picture between a
/// reference text and a transcript. Exposed so the frontend/MCP can preview a
/// score and render the diff without persisting anything.
#[tauri::command]
pub async fn read_aloud_score(content: String, recognized: String) -> Result<AsrScoreSummary, String> {
    Ok(score_asr(&content, &recognized))
}

/// Read the stored metrics text back as a JSON value. Unparseable text is
/// treated as absent rather than failing the whole attempt list.
fn stored_metrics(raw: Option<String>) -> Option<Value> {
    serde_json::from_str(raw.as_deref()?).ok()
}

// ============================================================
// Assessment — the three axes for one take
// ============================================================

/// Pronunciation evidence, gathered before [`assess::assess`] runs.
///
/// `assess` is synchronous and pure, so the model- and network-shaped work has
/// to happen first and be handed in. Bundling it here keeps the borrow of the
/// posteriors alive for exactly the call that needs it.
struct PronRun {
    post: gop::LogPost,
    spans: Vec<Vec<gop::PhoneSpan>>,
    reason: Option<PronReason>,
}

impl PronRun {
    fn unavailable(reason: PronReason) -> Self {
        Self { post: gop::LogPost::new(Vec::new(), 0), spans: Vec::new(), reason: Some(reason) }
    }

    fn evidence(&self) -> PronEvidence<'_> {
        match self.reason {
            Some(reason) => PronEvidence::Unavailable(reason),
            None => PronEvidence::Measured {
                post: &self.post,
                spans: &self.spans,
                tau: assess::GOP_TAU,
            },
        }
    }
}

/// The Edge TTS voice the canonical reference is synthesised with.
fn tts_voice(settings: &SettingsState) -> String {
    let configured = settings
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .read_aloud_tts_voice
        .trim()
        .to_string();
    if configured.is_empty() {
        DEFAULT_TTS_VOICE.to_string()
    } else {
        configured
    }
}

/// Cache location for canonical reference renderings: a workspace dir beside
/// the recordings, never inside a dataset (INV-9).
fn reference_cache_dir(settings: &SettingsState) -> PathBuf {
    settings.workspace_subdir(tts_ref::CACHE_DIR, "")
}

/// Gather pronunciation evidence: build the reference phone timeline from a
/// canonical TTS rendering, then pin it onto the learner's take.
///
/// Every early return names *why* nothing was measured. No proxy score is ever
/// substituted — an honest gap is the point.
#[cfg(feature = "phoneme")]
async fn pronunciation_evidence(
    settings: &SettingsState,
    reference: &str,
    ref_words: usize,
    samples: &[f32],
) -> PronRun {
    if samples.is_empty() || ref_words == 0 {
        return PronRun::unavailable(PronReason::TooShort);
    }
    let model_root = crate::models::index::model_root();
    let Some(dir) = phoneme::find_model_dir(&model_root) else {
        return PronRun::unavailable(PronReason::ModelMissing);
    };

    let voice = tts_voice(settings);
    let canonical = match tts_ref::resolve(&reference_cache_dir(settings), reference, &voice).await {
        Ok(reference) => reference,
        Err(tts_ref::RefError::NoNetwork) => return PronRun::unavailable(PronReason::NoNetwork),
        Err(tts_ref::RefError::Failed(message)) => {
            log::warn!("read_aloud reference unavailable: {}", message);
            return PronRun::unavailable(PronReason::TtsFailed);
        }
    };

    // 1. The reference timeline. Decode the canonical rendering with the same
    //    model, then pin that decode back onto its own posteriors so each phone
    //    has a span (and therefore a time) to be grouped by.
    let decoded = phoneme::with_model(&dir, |model| {
        let post = model.posteriors(&canonical.samples)?;
        let phones = model.collapse(&post);
        Ok((post, phones))
    });
    let (reference_post, phones) = match decoded {
        Ok(value) => value,
        Err(message) => {
            log::warn!("read_aloud phoneme model failed on the reference: {}", message);
            return PronRun::unavailable(PronReason::ModelError);
        }
    };
    if phones.is_empty() {
        log::warn!("read_aloud reference decoded to no phones");
        return PronRun::unavailable(PronReason::TtsFailed);
    }
    let Some(reference_spans) = gop::forced_align(&reference_post, &phones) else {
        log::warn!("read_aloud reference has more phones than frames");
        return PronRun::unavailable(PronReason::TtsFailed);
    };

    let frame_secs = tts_ref::frame_seconds(reference_post.len(), canonical.seconds());
    let groups = tts_ref::group_phones(
        &reference_spans,
        frame_secs,
        &canonical.boundaries,
        ref_words,
    );
    let target = tts_ref::flatten(&groups);
    if target.is_empty() {
        log::warn!("read_aloud reference grouped into no phones");
        return PronRun::unavailable(PronReason::TtsFailed);
    }

    // 2. The learner's posteriors — the same model, so the two sides share one
    //    phone inventory and nothing has to be mapped between them.
    let post = match phoneme::with_model(&dir, |model| model.posteriors(samples)) {
        Ok(post) => post,
        Err(message) => {
            log::warn!("read_aloud phoneme model failed on the take: {}", message);
            return PronRun::unavailable(PronReason::ModelError);
        }
    };

    // 3. Pin the reference phones onto the take.
    let Some(aligned) = gop::forced_align(&post, &target) else {
        return PronRun::unavailable(PronReason::TooShort);
    };
    let spans = tts_ref::split_by_word(&aligned, &groups);

    PronRun { post, spans, reason: None }
}

/// Without the `phoneme` feature nothing can measure pronunciation; say so
/// instead of compiling a stub that pretends.
#[cfg(not(feature = "phoneme"))]
async fn pronunciation_evidence(
    _settings: &SettingsState,
    _reference: &str,
    _ref_words: usize,
    _samples: &[f32],
) -> PronRun {
    PronRun::unavailable(PronReason::ModelMissing)
}

/// Decode a recorder WAV to 16 kHz mono samples. An empty input means "no
/// recording", which is not an error — fluency is simply unavailable.
fn decode_samples(wav_base64: &str) -> Result<Vec<f32>, String> {
    if wav_base64.trim().is_empty() {
        return Ok(Vec::new());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(wav_base64.trim())
        .map_err(|e| format!("Invalid base64 audio: {}", e))?;
    crate::audio::decode_wav_pcm(&bytes)
}

/// The single seam every scoring path goes through: accuracy from the existing
/// metric, fluency from VAD + word timings, pronunciation from GOP.
async fn assess_take(
    settings: &SettingsState,
    summary: &AsrScoreSummary,
    reference: &str,
    recognized: &str,
    samples: &[f32],
    words: Option<&[TimedWord]>,
) -> Assessment {
    let ref_words = crate::datasets::asr_metric::normalize_words(reference).len();
    let pron = pronunciation_evidence(settings, reference, ref_words, samples).await;
    assess::assess(summary, reference, recognized, samples, words, pron.evidence())
}

/// Score one take without persisting anything: accuracy + fluency +
/// pronunciation. The preview twin of [`read_aloud_submit`].
#[tauri::command]
pub async fn read_aloud_assess(
    settings: State<'_, SettingsState>,
    content: String,
    recognized: String,
    wav_base64: String,
    words: Option<Vec<TimedWord>>,
) -> Result<Assessment, String> {
    let samples = decode_samples(&wav_base64)?;
    let summary = score_asr(&content, &recognized);
    Ok(assess_take(&settings, &summary, &content, &recognized, &samples, words.as_deref()).await)
}

/// Submit a recorded take: persist the audio, score it against the reference
/// text, store the attempt, and award scaled XP when the score passes.
///
/// The frontend records + runs STT (via `model_transcribe_detailed`) and passes
/// the WAV (base64), the recognized text and, when the model gave any, the word
/// timings here so scoring, storage and XP all happen atomically on the backend.
///
/// The accuracy score is unchanged by the three-axis work: `score` is still the
/// WIP number XP and `best_score` have always used.
#[tauri::command]
pub async fn read_aloud_submit(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    text_uuid: String,
    wav_base64: String,
    recognized: String,
    words: Option<Vec<TimedWord>>,
) -> Result<ReadAloudSubmitResult, String> {
    let dir = find_dataset_dir(&settings, &dataset_uuid)?;

    // Load the reference text (read-only). Practising never writes to the dataset.
    let content: String = {
        let conn = open_db(&dir)?;
        conn.query_row(
            "SELECT content, normalized_content FROM read_text WHERE uuid = ?1",
            [&text_uuid],
            |row| {
                let content: String = row.get(0)?;
                Ok(effective_reference(&content, row.get::<_, Option<String>>(1)?.as_deref()))
            },
        )
        .map_err(|_| format!("Text {} not found in dataset", text_uuid))?
    };

    let summary = score_asr(&content, &recognized);
    let score = summary.score;

    // The recorder writes 16 kHz mono; a decode failure must not lose the take,
    // so it degrades to "no fluency" instead of failing the submission.
    let samples = decode_samples(&wav_base64).unwrap_or_else(|e| {
        log::warn!("read_aloud: recording could not be decoded for fluency: {}", e);
        Vec::new()
    });

    let assessment = assess_take(
        &settings,
        &summary,
        &content,
        &recognized,
        &samples,
        words.as_deref(),
    )
    .await;

    // Persist the recording in the per-user audio dir — never inside the dataset,
    // so a take stays out of the shared manifest and needs no overlay (§17).
    let attempt_uuid = Uuid::new_v4().to_string();
    let file_name = format!("{}.wav", attempt_uuid);
    let audio_root = audio_dir(&settings);
    let audio_path = if wav_base64.trim().is_empty() {
        None
    } else {
        fs::create_dir_all(&audio_root).map_err(|e| e.to_string())?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(wav_base64.trim())
            .map_err(|e| format!("Invalid base64 audio: {}", e))?;
        fs::write(audio_root.join(&file_name), &bytes).map_err(|e| e.to_string())?;
        Some(file_name)
    };

    let user_id = workspace_identity(&settings);
    let now = now_row_stamp();

    let attempt = ReadAttempt {
        uuid: attempt_uuid,
        text_uuid: text_uuid.clone(),
        user_id: user_id.clone(),
        audio_url: audio_path.as_ref().map(|f| audio_root.join(f).to_string_lossy().into_owned()),
        audio_path,
        recognized,
        score,
        metrics: stored_metrics(Some(assessment.metrics_json())),
        created_at: now,
    };
    // Event write + `@app` change journal, through the same path a synced attempt
    // arrives by.
    save_attempt_row(&settings, &dataset_uuid, &attempt)?;

    // Scaled XP: 1 XP for passing (>=60), +1 more for a strong read (>=80).
    // Each tier is deduped per text via xp_earned, so repeating never farms XP
    // but improving into a higher tier still rewards the extra point.
    let mut total_awarded = 0i64;
    let mut last: Option<XpAwardResult> = None;
    if score >= XP_PASS_SCORE {
        let r = xp_award_internal(
            &settings,
            &user_id,
            1,
            "read_aloud",
            &format!("{}:pass", text_uuid),
            Some(dataset_uuid.as_str()),
        )?;
        total_awarded += r.xp_awarded;
        last = Some(r);
    }
    if score >= XP_GOOD_SCORE {
        let r = xp_award_internal(
            &settings,
            &user_id,
            1,
            "read_aloud",
            &format!("{}:good", text_uuid),
            Some(dataset_uuid.as_str()),
        )?;
        total_awarded += r.xp_awarded;
        last = Some(r);
    }

    let (lifetime_xp, level) = last
        .as_ref()
        .map(|r| (r.lifetime_xp, r.level))
        .unwrap_or((0, 1));

    if total_awarded > 0 {
        let combined = XpAwardResult {
            xp_awarded: total_awarded,
            lifetime_xp,
            level,
            is_new: true,
        };
        let _ = app.emit("xp-earned", &combined);
    }

    Ok(ReadAloudSubmitResult {
        score,
        summary,
        attempt,
        assessment,
        xp_awarded: total_awarded,
        lifetime_xp,
        level,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_settings(tag: &str) -> (SettingsState, PathBuf) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("fms-read-aloud-test-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        let settings = SettingsState::new();
        settings.set_workspace_dir(Some(dir.clone()));
        (settings, dir)
    }

    /// A real read-aloud dataset with one text, built through the production path.
    fn dataset_with_text(settings: &SettingsState, content: &str) -> (String, String) {
        let meta = create_dataset(settings, "Practise".into(), String::new()).unwrap();
        let text_uuid = Uuid::new_v4().to_string();
        let now = now_stamp();
        save_text_row(
            settings,
            &meta.uuid,
            &ReadText {
                uuid: text_uuid.clone(),
                dataset_uuid: meta.uuid.clone(),
                order_num: 0,
                title: "T".into(),
                note: String::new(),
                content: content.into(),
                normalized_content: None,
                best_score: None,
                attempt_count: 0,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .unwrap();
        (meta.uuid, text_uuid)
    }

    fn attempt(text_uuid: &str, user_id: &str, score: f64) -> ReadAttempt {
        ReadAttempt {
            uuid: Uuid::new_v4().to_string(),
            text_uuid: text_uuid.to_string(),
            user_id: user_id.to_string(),
            audio_path: None,
            audio_url: None,
            recognized: "hello world".into(),
            score,
            metrics: None,
            created_at: now_row_stamp(),
        }
    }

    fn event_count(settings: &SettingsState) -> i64 {
        let conn = open_app_attempts(settings).unwrap();
        conn.query_row("SELECT COUNT(*) FROM read_attempt_event", [], |r| r.get(0))
            .unwrap()
    }

    /// A minimal dataset directory (info.json + initialized DB) at an arbitrary
    /// path, so a test can place one under the read-only download root.
    fn write_dataset_at(dir: &Path, uuid: &str, text_uuid: &str) {
        fs::create_dir_all(dir).unwrap();
        let info = DatasetInfo::new(uuid.to_string(), DatasetType::Read, FORMAT_READ_ALOUD, "Base".into());
        write_info(dir, &info).unwrap();
        let conn = open_db(dir).unwrap();
        conn.execute(
            "INSERT INTO read_text (uuid, dataset_uuid, order_num, title, note, content, created_at, updated_at) \
             VALUES (?1, ?2, 0, 'T', '', 'hello world', datetime('now'), datetime('now'))",
            params![text_uuid, uuid],
        )
        .unwrap();
    }

    // -- the core move: attempts are app-DB events, not dataset content --------

    #[test]
    fn attempt_lands_in_app_db_not_the_dataset() {
        let (settings, _dir) = temp_settings("app-db");
        let (ds, text) = dataset_with_text(&settings, "hello world");

        save_attempt_row(&settings, &ds, &attempt(&text, "local", 95.0)).unwrap();

        // The event lives in the app DB.
        assert_eq!(event_count(&settings), 1);

        // The dataset DB is untouched: no legacy attempt row, no best_score bump.
        let dataset_dir = find_dataset_dir(&settings, &ds).unwrap();
        let conn = open_db(&dataset_dir).unwrap();
        let legacy: i64 = conn
            .query_row("SELECT COUNT(*) FROM read_attempt", [], |r| r.get(0))
            .unwrap();
        assert_eq!(legacy, 0, "practising must not write into the dataset DB");
        let best: Option<f64> = conn
            .query_row("SELECT best_score FROM read_text WHERE uuid = ?1", params![text], |r| r.get(0))
            .unwrap();
        assert!(best.is_none(), "read_text.best_score is never touched by practising");

        // list_texts surfaces my attempt, recomputed from the app-DB event.
        let texts = list_texts(&settings, &ds).unwrap();
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0].attempt_count, 1);
        assert!(texts[0].best_score.is_some());
    }

    #[test]
    fn practising_a_download_root_base_does_not_fork() {
        let (settings, _dir) = temp_settings("no-fork");
        let ds = Uuid::new_v4().to_string();
        let text = Uuid::new_v4().to_string();
        let base = crate::datasets::download_root(&settings, DatasetType::Read).join("Base");
        write_dataset_at(&base, &ds, &text);
        assert!(is_under_download_root(&settings, &base), "base must sit under the download root");

        save_attempt_row(&settings, &ds, &attempt(&text, "local", 90.0)).unwrap();

        // No overlay was forked into the writable read-aloud root.
        let writable = settings.datasets_dir().join(DatasetType::Read.as_str());
        let forked = fs::read_dir(&writable).map(|e| e.flatten().count()).unwrap_or(0);
        assert_eq!(forked, 0, "practising a download-root base must not fork an overlay");
        // The attempt still landed, and the base still resolves to itself.
        assert_eq!(event_count(&settings), 1);
        assert_eq!(find_dataset_dir(&settings, &ds).unwrap(), base);
    }

    #[test]
    fn list_texts_counts_only_my_attempts() {
        let (settings, _dir) = temp_settings("owner");
        let (ds, text) = dataset_with_text(&settings, "hello world");
        // Two of mine plus one belonging to someone else, seeded straight into the
        // app DB (as another device's replay would).
        let conn = open_app_attempts(&settings).unwrap();
        for (u, user, day) in [
            ("a", "local", "01"),
            ("b", "local", "02"),
            ("c", "other", "03"),
        ] {
            conn.execute(
                "INSERT INTO read_attempt_event \
                 (uuid, user_id, dataset_uuid, text_uuid, audio_path, recognized, score, created_at) \
                 VALUES (?1, ?2, ?3, ?4, NULL, 'hello world', 90, ?5)",
                params![u, user, ds, text, format!("2026-01-{day}T00:00:00Z")],
            )
            .unwrap();
        }
        drop(conn);

        let texts = list_texts(&settings, &ds).unwrap();
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0].attempt_count, 2, "only my attempts count");
    }

    #[test]
    fn delete_attempt_removes_my_event_and_journals_under_app() {
        let (settings, _dir) = temp_settings("delete");
        let (ds, text) = dataset_with_text(&settings, "hello world");
        let a = attempt(&text, "local", 90.0);
        save_attempt_row(&settings, &ds, &a).unwrap();
        assert_eq!(event_count(&settings), 1);

        delete_attempt_inner(&settings, &ds, &a.uuid).unwrap();
        assert_eq!(event_count(&settings), 0, "my event is gone from the app DB");

        // The delete journaled under the shared @app scope, not the dataset uuid,
        // and wrote no dataset tombstone (that would open a downloaded base).
        let conn = open_app_attempts(&settings).unwrap();
        let scope: String = conn
            .query_row(
                "SELECT dataset_uuid FROM writeback_queue WHERE kind = 'read_attempt_delete' \
                 ORDER BY queued_at DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(scope, crate::sync::change_log::APP_SCOPE);
    }

    #[test]
    fn legacy_attempts_seed_events_once() {
        let (settings, _dir) = temp_settings("migrate");
        let (ds, text) = dataset_with_text(&settings, "hello world");
        let dataset_dir = find_dataset_dir(&settings, &ds).unwrap();
        // A legacy in-dataset attempt, unowned as single-user rows were.
        {
            let conn = open_db(&dataset_dir).unwrap();
            conn.execute(
                "INSERT INTO read_attempt (uuid, text_uuid, user_id, audio_path, recognized, score, created_at) \
                 VALUES ('legacy-1', ?1, '', NULL, 'hello world', 80, '2026-01-01T00:00:00Z')",
                params![text],
            )
            .unwrap();
        }
        // First practice-open seeds it into the app DB.
        { let _ctx = open_attempts(&settings, &ds).unwrap(); }
        assert_eq!(event_count(&settings), 1);

        // A later legacy row is NOT seeded: the migration marker blocks a re-seed.
        {
            let conn = open_db(&dataset_dir).unwrap();
            conn.execute(
                "INSERT INTO read_attempt (uuid, text_uuid, user_id, audio_path, recognized, score, created_at) \
                 VALUES ('legacy-2', ?1, '', NULL, 'hello', 50, '2026-01-02T00:00:00Z')",
                params![text],
            )
            .unwrap();
        }
        { let _ctx = open_attempts(&settings, &ds).unwrap(); }
        assert_eq!(event_count(&settings), 1, "migration marker prevents a second seed");
    }

    // -- replay: a journaled event applied from another device -----------------

    #[test]
    fn apply_remote_save_stores_event_and_is_idempotent() {
        let (settings, _dir) = temp_settings("replay");
        let (ds, text) = dataset_with_text(&settings, "hello world");
        let uuid = Uuid::new_v4().to_string();
        // `@app` passes no dataset, so it travels in the payload.
        let payload = json!({
            "uuid": uuid, "user_id": "", "dataset_uuid": ds, "text_uuid": text,
            "audio_path": null, "recognized": "hello world", "score": 88.0,
            "created_at": "2026-01-01T00:00:00Z",
        });
        let inserted = apply_remote_save(&settings, "", &payload, "alice").unwrap();
        assert!(inserted, "first replay stores the event");
        // Attributed to the identity the hub bound, not the (empty) declared one.
        let conn = open_app_attempts(&settings).unwrap();
        let owner: String = conn
            .query_row("SELECT user_id FROM read_attempt_event WHERE uuid = ?1", params![uuid], |r| r.get(0))
            .unwrap();
        assert_eq!(owner, "alice");
        // Replaying the same event (an echo) is a no-op, not a duplicate.
        let again = apply_remote_save(&settings, "", &payload, "alice").unwrap();
        assert!(!again, "replay is INSERT OR IGNORE by event uuid");
        assert_eq!(event_count(&settings), 1);
    }

    // -- the hub-side state read behind the practice backfill (§17.6 / INV-21) ----

    #[test]
    fn rows_for_user_pages_only_my_attempts() {
        let (settings, _dir) = temp_settings("rows");
        let conn = open_app_attempts(&settings).unwrap();
        for (u, user, day) in [("a", "local", "01"), ("b", "local", "02"), ("c", "local", "03"), ("z", "other", "04")] {
            conn.execute(
                "INSERT INTO read_attempt_event \
                 (uuid, user_id, dataset_uuid, text_uuid, audio_path, recognized, score, created_at) \
                 VALUES (?1, ?2, 'ds-rows', 't', NULL, 'hello world', 90, ?3)",
                params![u, user, format!("2026-01-{day}T00:00:00Z")],
            )
            .unwrap();
        }
        drop(conn);

        // A page larger than the set is the whole owner set, and `next` is None.
        let (all, next) = rows_for_user(&settings, "local", 0, 10).unwrap();
        assert_eq!(all.len(), 3, "only my events, never another user's");
        assert!(next.is_none(), "a short page means the history is exhausted");
        let uuids: Vec<&str> = all.iter().map(|r| r["uuid"].as_str().unwrap()).collect();
        assert_eq!(uuids, vec!["a", "b", "c"], "ordered by rowid (insertion order)");

        // A page exactly the size of a chunk reports a next cursor and continues.
        let (p1, n1) = rows_for_user(&settings, "local", 0, 2).unwrap();
        assert_eq!(p1.len(), 2);
        let c1 = n1.expect("a full page carries a next cursor");
        let (p2, n2) = rows_for_user(&settings, "local", c1, 2).unwrap();
        assert_eq!(p2.len(), 1);
        assert_eq!(p2[0]["uuid"].as_str(), Some("c"));
        assert!(n2.is_none());
    }

    #[test]
    fn rows_for_user_rows_round_trip_through_apply_remote_save() {
        let (settings, _dir) = temp_settings("rows-trip");
        let conn = open_app_attempts(&settings).unwrap();
        conn.execute(
            "INSERT INTO read_attempt_event \
             (uuid, user_id, dataset_uuid, text_uuid, audio_path, recognized, score, created_at) \
             VALUES ('a', 'local', 'ds-trip', 't', NULL, 'hello world', 88, '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        drop(conn);
        let (rows, _) = rows_for_user(&settings, "local", 0, 10).unwrap();
        assert_eq!(rows.len(), 1);

        // A follower feeds a page row straight to `apply_remote_save` — the `@app`
        // state read carries no dataset, so `dataset_uuid` travels in the payload.
        let (fresh, _fdir) = temp_settings("rows-trip-fresh");
        let applied = apply_remote_save(&fresh, "", &rows[0], "carol").unwrap();
        assert!(applied, "a page row is a valid replay payload");
        let conn = open_app_attempts(&fresh).unwrap();
        let owner: String = conn
            .query_row("SELECT user_id FROM read_attempt_event WHERE uuid = 'a'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(owner, "carol", "replay attributes to the hub's bound identity");
        drop(conn);
        // Idempotent: re-running the page stores nothing new (INV-23).
        let again = apply_remote_save(&fresh, "", &rows[0], "carol").unwrap();
        assert!(!again);
        assert_eq!(event_count(&fresh), 1);
    }
}
