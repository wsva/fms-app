//! XP (Experience Points) system — tracks permanent learning progress.
//!
//! XP is earned through learning activities (dictation, reading) and is
//! non-spendable, non-transferable, and permanent.
//!
//! Tables stored in the app-level SQLite (`{workspace_dir}/app.sqlite3`).

use rusqlite::Connection;
use serde::Serialize;
use std::path::Path;
use tauri::{Emitter, State};

use crate::auth::workspace_identity;
use crate::settings::SettingsState;

// ============================================================
// Types
// ============================================================

#[derive(Debug, Clone, Serialize)]
pub struct XpUser {
    pub user_id: String,
    pub lifetime_xp: i64,
    pub level: i64,
    pub updated_at: String,
    /// §3.6 counter display: the XP earned on this device but not yet confirmed
    /// by the hub (still in the writeback queue). Always 0 on the hub. The local
    /// `lifetime_xp` already includes it (a follower bakes the delta at award
    /// time); this is only the "syncing" overlay the UI/status surface reads.
    #[serde(default)]
    pub pending_xp: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct XpLedgerEntry {
    pub id: i64,
    pub amount: i64,
    pub source: String,
    pub reference_id: Option<String>,
    pub dataset_uuid: Option<String>,
    pub created_at: String,
}

/// Result of an XP award operation.
#[derive(Debug, Clone, Serialize)]
pub struct XpAwardResult {
    /// XP actually awarded (0 if duplicate).
    pub xp_awarded: i64,
    /// Total lifetime XP after the award.
    pub lifetime_xp: i64,
    /// Level after the award.
    pub level: i64,
    /// Whether this was a new award (false = already earned).
    pub is_new: bool,
}

// ============================================================
// Helpers
// ============================================================

/// Open the app-level SQLite and ensure XP tables exist.
/// Uses the current workspace directory if set, otherwise falls back to global app data dir.
fn open_app_db(settings: &SettingsState) -> Result<Connection, String> {
    let db_dir = match settings.workspace_dir.lock().unwrap().as_ref() {
        Some(ws_dir) => ws_dir.clone(),
        None => crate::app_paths::data_root(),
    };
    std::fs::create_dir_all(&db_dir).map_err(|e| e.to_string())?;
    let db_path = db_dir.join("app.sqlite3");
    log::debug!("[XP] Opening app DB: {}", db_path.display());
    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS xp_user (
            user_id     TEXT PRIMARY KEY,
            lifetime_xp INTEGER NOT NULL DEFAULT 0,
            level       INTEGER NOT NULL DEFAULT 1,
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS xp_ledger (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id      TEXT NOT NULL,
            amount       INTEGER NOT NULL,
            source       TEXT NOT NULL,
            reference_id TEXT,
            dataset_uuid TEXT,
            created_at   TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS xp_earned (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id      TEXT NOT NULL,
            source       TEXT NOT NULL,
            reference_id TEXT NOT NULL,
            dataset_uuid TEXT,
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, source, reference_id, dataset_uuid)
        );

        CREATE INDEX IF NOT EXISTS idx_xp_ledger_user ON xp_ledger(user_id, created_at DESC);
        ",
    )
    .map_err(|e| e.to_string())?;
    Ok(conn)
}

/// Ensure a user row exists in xp_user.
fn ensure_user_row(conn: &Connection, user_id: &str) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO xp_user (user_id, lifetime_xp, level, updated_at) \
         VALUES (?1, 0, 1, datetime('now'))",
        [user_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ============================================================
// Internal award function (called from other modules)
// ============================================================

/// Award XP for a learning activity.
///
/// Returns `XpAwardResult` with `is_new = false` if this exact activity has
/// already earned XP (prevents duplicates via UNIQUE constraint).
///
/// This function is called internally by dictation and reading commands.
pub fn xp_award_internal(
    settings: &SettingsState,
    user_id: &str,
    amount: i64,
    source: &str,
    reference_id: &str,
    dataset_uuid: Option<&str>,
) -> Result<XpAwardResult, String> {
    let conn = open_app_db(settings)?;
    ensure_user_row(&conn, user_id)?;

    // Try to insert into xp_earned (unique constraint prevents duplicates).
    let inserted = conn
        .execute(
            "INSERT OR IGNORE INTO xp_earned (user_id, source, reference_id, dataset_uuid) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![user_id, source, reference_id, dataset_uuid],
        )
        .map_err(|e| e.to_string())?;

    if inserted == 0 {
        // Already earned — return current state without awarding.
        let (lifetime_xp, level) = get_user_xp_from_db(&conn, user_id)?;
        return Ok(XpAwardResult {
            xp_awarded: 0,
            lifetime_xp,
            level,
            is_new: false,
        });
    }

    // Award XP: update xp_user.
    // Compute level in Rust (SQLite may not have sqrt/floor).
    let new_xp = get_user_xp_from_db(&conn, user_id).unwrap_or((0, 1)).0 + amount;
    let new_level = (new_xp as f64 / 100.0).sqrt().floor() as i64 + 1;
    conn.execute(
        "UPDATE xp_user SET lifetime_xp = ?1, level = ?2, updated_at = datetime('now') WHERE user_id = ?3",
        rusqlite::params![new_xp, new_level, user_id],
    )
    .map_err(|e| {
        log::error!("[XP] Failed to update xp_user for {}: {}", user_id, e);
        e.to_string()
    })?;

    // Log to ledger.
    conn.execute(
        "INSERT INTO xp_ledger (user_id, amount, source, reference_id, dataset_uuid) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![user_id, amount, source, reference_id, dataset_uuid],
    )
    .map_err(|e| e.to_string())?;

    let (lifetime_xp, level) = get_user_xp_from_db(&conn, user_id)?;

    {
        // Queue/record the award: a follower enqueues it for the next push;
        // the hub appends it to `sync_log` as a counter delta (§3.6). PC-side
        // deduplication is by (user_id, source, reference_id).
        let payload = serde_json::json!({
            "user_id": user_id,
            "amount": amount,
            "source": source,
            "reference_id": reference_id,
        });
        let _ = crate::sync_log::commit_change(settings, "xp", dataset_uuid.unwrap_or(""), &payload);
    }

    Ok(XpAwardResult {
        xp_awarded: amount,
        lifetime_xp,
        level,
        is_new: true,
    })
}

fn get_user_xp_from_db(conn: &Connection, user_id: &str) -> Result<(i64, i64), String> {
    conn.query_row(
        "SELECT lifetime_xp, level FROM xp_user WHERE user_id = ?1",
        [user_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .map_err(|e| e.to_string())
}

/// Fold progress/XP rows recorded under a transient identity (`""` or `"local"`)
/// into the permanent owner identity. Called when a workspace is claimed so that
/// practice done while logged out is neither lost nor left invisible.
///
/// Tolerant of missing tables (each statement's error is ignored), and safe to
/// call repeatedly (idempotent once no transient rows remain).
pub(crate) fn migrate_identity_to_owner(ws_dir: &Path, owner_id: &str) {
    // Nothing permanent to fold into for an unclaimed identity.
    if owner_id.is_empty() || owner_id == "local" {
        return;
    }
    let db_path = ws_dir.join("app.sqlite3");
    if !db_path.exists() {
        return;
    }
    let conn = match Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => {
            log::warn!(
                "[XP] migrate_identity_to_owner: cannot open {}: {}",
                db_path.display(),
                e
            );
            return;
        }
    };
    log::info!(
        "[XP] Migrating transient progress/XP identity (''/'local') -> '{}' in {}",
        owner_id,
        db_path.display()
    );

    // listen_dictation: UNIQUE(user_id, media_uuid, subtitle_uuid) — keep the owner row on conflict.
    let _ = conn.execute(
        "UPDATE OR IGNORE listen_dictation SET user_id = ?1 WHERE user_id IN ('', 'local')",
        [owner_id],
    );
    let _ = conn.execute(
        "DELETE FROM listen_dictation WHERE user_id IN ('', 'local')",
        [],
    );

    // xp_ledger: append-only, no uniqueness — plain reassign.
    let _ = conn.execute(
        "UPDATE xp_ledger SET user_id = ?1 WHERE user_id IN ('', 'local')",
        [owner_id],
    );

    // xp_earned: UNIQUE(user_id, source, reference_id, dataset_uuid) — keep the owner row on conflict.
    let _ = conn.execute(
        "UPDATE OR IGNORE xp_earned SET user_id = ?1 WHERE user_id IN ('', 'local')",
        [owner_id],
    );
    let _ = conn.execute(
        "DELETE FROM xp_earned WHERE user_id IN ('', 'local')",
        [],
    );

    // xp_user: user_id is PRIMARY KEY — merge lifetime XP and recompute level in Rust.
    let transient_xp: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(lifetime_xp), 0) FROM xp_user WHERE user_id IN ('', 'local')",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if transient_xp > 0 {
        let _ = conn.execute(
            "INSERT OR IGNORE INTO xp_user (user_id, lifetime_xp, level, updated_at) \
             VALUES (?1, 0, 1, datetime('now'))",
            [owner_id],
        );
        let owner_xp: i64 = conn
            .query_row(
                "SELECT lifetime_xp FROM xp_user WHERE user_id = ?1",
                [owner_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let new_xp = owner_xp + transient_xp;
        let new_level = (new_xp as f64 / 100.0).sqrt().floor() as i64 + 1;
        let _ = conn.execute(
            "UPDATE xp_user SET lifetime_xp = ?1, level = ?2, updated_at = datetime('now') WHERE user_id = ?3",
            rusqlite::params![new_xp, new_level, owner_id],
        );
    }
    let _ = conn.execute("DELETE FROM xp_user WHERE user_id IN ('', 'local')", []);
}

// ============================================================
// Tauri commands
// ============================================================

/// Get the current user's XP summary.
#[tauri::command]
pub async fn xp_get_user(
    _settings: State<'_, SettingsState>,
) -> Result<Option<XpUser>, String> {
    let user_id = workspace_identity(&_settings);
    if user_id.is_empty() {
        return Ok(None);
    }
    let conn = open_app_db(&_settings)?;
    ensure_user_row(&conn, &user_id)?;

    let result = conn
        .query_row(
            "SELECT user_id, lifetime_xp, level, updated_at FROM xp_user WHERE user_id = ?1",
            [&user_id],
            |row| {
                Ok(XpUser {
                    user_id: row.get(0)?,
                    lifetime_xp: row.get(1)?,
                    level: row.get(2)?,
                    updated_at: row.get(3)?,
                    pending_xp: 0,
                })
            },
        )
        .map_err(|e| e.to_string())?;

    // On a follower, overlay how much of the displayed total the hub has not
    // confirmed yet (§3.6 reconcile-on-ack). The hub role has no queue.
    let pending_xp = if _settings.role() != "hub" {
        crate::sync::pending_xp_delta(&_settings, &user_id).unwrap_or(0)
    } else {
        0
    };
    let result = XpUser { pending_xp, ..result };

    Ok(Some(result))
}

/// Get recent XP history for the current user.
#[tauri::command]
pub async fn xp_get_history(
    _settings: State<'_, SettingsState>,
    limit: Option<i64>,
) -> Result<Vec<XpLedgerEntry>, String> {
    let user_id = workspace_identity(&_settings);
    if user_id.is_empty() {
        return Ok(Vec::new());
    }
    let conn = open_app_db(&_settings)?;
    let limit = limit.unwrap_or(50).min(200);

    let mut stmt = conn
        .prepare(
            "SELECT id, amount, source, reference_id, dataset_uuid, created_at \
             FROM xp_ledger WHERE user_id = ?1 ORDER BY created_at DESC LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;

    let entries = stmt
        .query_map(rusqlite::params![&user_id, limit], |row| {
            Ok(XpLedgerEntry {
                id: row.get(0)?,
                amount: row.get(1)?,
                source: row.get(2)?,
                reference_id: row.get(3)?,
                dataset_uuid: row.get(4)?,
                created_at: row.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(entries)
}

/// Award XP for a dictation cue completion.
/// Called from the frontend after successfully completing a cue dictation.
#[tauri::command]
pub async fn xp_award_dictation_cue(
    app: tauri::AppHandle,
    _settings: State<'_, SettingsState>,
    cue_id: String,
    dataset_uuid: String,
) -> Result<XpAwardResult, String> {
    let user_id = workspace_identity(&_settings);
    log::info!("xp_award_dictation_cue: user={}, cue={}", user_id, cue_id);
    let result = xp_award_internal(&_settings, &user_id, 1, "dictation_cue", &cue_id, Some(dataset_uuid.as_str()))?;
    if result.is_new {
        let _ = app.emit("xp-earned", &result);
    }
    Ok(result)
}

/// Award XP for completing all cues in a subtitle.
#[tauri::command]
pub async fn xp_award_dictation_subtitle(
    app: tauri::AppHandle,
    _settings: State<'_, SettingsState>,
    subtitle_id: String,
    dataset_uuid: String,
) -> Result<XpAwardResult, String> {
    let user_id = workspace_identity(&_settings);
    log::info!("xp_award_dictation_subtitle: user={}, subtitle={}", user_id, subtitle_id);
    let result = xp_award_internal(&_settings, &user_id, 2, "dictation_subtitle", &subtitle_id, Some(dataset_uuid.as_str()))?;
    if result.is_new {
        let _ = app.emit("xp-earned", &result);
    }
    Ok(result)
}

/// Award XP for completing all subtitles of a media file.
#[tauri::command]
pub async fn xp_award_dictation_media(
    app: tauri::AppHandle,
    _settings: State<'_, SettingsState>,
    media_id: String,
    dataset_uuid: String,
) -> Result<XpAwardResult, String> {
    let user_id = workspace_identity(&_settings);
    log::info!("xp_award_dictation_media: user={}, media={}", user_id, media_id);
    let result = xp_award_internal(&_settings, &user_id, 5, "dictation_media", &media_id, Some(dataset_uuid.as_str()))?;
    if result.is_new {
        let _ = app.emit("xp-earned", &result);
    }
    Ok(result)
}

/// Award XP for typing a sentence in reading.
#[tauri::command]
pub async fn xp_award_reading_sentence(
    app: tauri::AppHandle,
    _settings: State<'_, SettingsState>,
    sentence_id: String,
) -> Result<XpAwardResult, String> {
    let user_id = workspace_identity(&_settings);
    log::info!("xp_award_reading_sentence: user={}, sentence={}", user_id, sentence_id);
    let result = xp_award_internal(&_settings, &user_id, 1, "reading_sentence", &sentence_id, None)?;
    if result.is_new {
        let _ = app.emit("xp-earned", &result);
    }
    Ok(result)
}

/// Award XP for completing all sentences in a chapter.
#[tauri::command]
pub async fn xp_award_reading_chapter(
    app: tauri::AppHandle,
    _settings: State<'_, SettingsState>,
    chapter_id: String,
) -> Result<XpAwardResult, String> {
    let user_id = workspace_identity(&_settings);
    log::info!("xp_award_reading_chapter: user={}, chapter={}", user_id, chapter_id);
    let result = xp_award_internal(&_settings, &user_id, 5, "reading_chapter", &chapter_id, None)?;
    if result.is_new {
        let _ = app.emit("xp-earned", &result);
    }
    Ok(result)
}
