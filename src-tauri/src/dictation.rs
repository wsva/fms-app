use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::auth::workspace_identity;
use crate::dataset::find_dataset_dir;
use crate::settings::SettingsState;

// ============================================================
// Shared types
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenMedia {
    pub uuid: String,
    pub source: String,
    pub duration_ms: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenSubtitle {
    pub uuid: String,
    pub media_uuid: String,
    pub name: String,
    pub track_type: Option<String>,
    pub model_uuid: Option<String>,
    pub version: i64,
    pub is_active: bool,
    pub created_at: String,
    pub updated_at: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenCue {
    pub uuid: String,
    pub subtitle_uuid: String,
    pub order_num: i64,
    pub start_ms: i64,
    pub end_ms: i64,
    pub content: String,
    pub reference: Option<String>,
    // The frontend `Cue` type omits these; they round-trip for existing cues but
    // are absent for newly inserted ones, so default them during deserialization.
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub version_created: i64,
    #[serde(default)]
    pub version_superseded: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenDictation {
    pub media_uuid: String,
    pub subtitle_uuid: String,
    pub status: String,
    pub completed: String,
}

// ============================================================
// Helpers
// ============================================================

/// Open the SQLite database for a given dataset.
fn open_db(settings: &SettingsState, dataset_uuid: &str) -> Result<Connection, String> {
    let dataset_dir = find_dataset_dir(settings, dataset_uuid)?;
    let db_path = dataset_dir.join("data.sqlite3");
    if !db_path.exists() {
        return Err("Database file not found. Please generate the database first.".into());
    }
    log::debug!("Opening dataset DB: {}", db_path.display());
    Connection::open(&db_path).map_err(|e| e.to_string())
}

/// Open the app-level database (for dictation progress, etc.).
/// Uses the current workspace directory if set, otherwise falls back to global app data dir.
pub(crate) fn open_app_db(settings: &SettingsState) -> Result<Connection, String> {
    let db_dir = match settings.workspace_dir.lock().unwrap().as_ref() {
        Some(ws_dir) => ws_dir.clone(),
        None => dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("fms-app"),
    };
    std::fs::create_dir_all(&db_dir).map_err(|e| e.to_string())?;
    let db_path = db_dir.join("app.sqlite3");
    log::debug!("[Dictation] Opening app DB: {}", db_path.display());
    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS listen_dictation (
            uuid          TEXT PRIMARY KEY,
            user_id       TEXT NOT NULL,
            media_uuid    TEXT NOT NULL,
            subtitle_uuid TEXT NOT NULL,
            status        TEXT NOT NULL DEFAULT '',
            completed     TEXT NOT NULL DEFAULT '',
            created_at    TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at    TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, media_uuid, subtitle_uuid)
        );",
    )
    .map_err(|e| e.to_string())?;
    Ok(conn)
}

// ============================================================
// Read commands
// ============================================================

/// List all media in a dataset.
#[tauri::command]
pub async fn listen_list_media(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<Vec<ListenMedia>, String> {
    log::debug!("listen_list_media: dataset={}", dataset_uuid);
    let conn = open_db(&settings, &dataset_uuid)?;
    let mut stmt = conn
        .prepare("SELECT uuid, source, duration_ms, created_at, updated_at FROM listen_media ORDER BY source")
        .map_err(|e| e.to_string())?;
    let items = stmt
        .query_map([], |row| {
            Ok(ListenMedia {
                uuid: row.get(0)?,
                source: row.get(1)?,
                duration_ms: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(items)
}

/// Get a single media by UUID.
#[tauri::command]
pub async fn listen_get_media(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    media_uuid: String,
) -> Result<ListenMedia, String> {
    log::debug!("listen_get_media: dataset={}, media={}", dataset_uuid, media_uuid);
    let conn = open_db(&settings, &dataset_uuid)?;
    let mut stmt = conn
        .prepare("SELECT uuid, source, duration_ms, created_at, updated_at FROM listen_media WHERE uuid = ?1")
        .map_err(|e| e.to_string())?;
    stmt.query_row([&media_uuid], |row| {
        Ok(ListenMedia {
            uuid: row.get(0)?,
            source: row.get(1)?,
            duration_ms: row.get(2)?,
            created_at: row.get(3)?,
            updated_at: row.get(4)?,
        })
    })
    .map_err(|e| format!("Media not found: {}", e))
}

/// Get all subtitles for a media.
#[tauri::command]
pub async fn listen_get_subtitles(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    media_uuid: String,
) -> Result<Vec<ListenSubtitle>, String> {
    log::debug!("listen_get_subtitles: dataset={}, media={}", dataset_uuid, media_uuid);
    let conn = open_db(&settings, &dataset_uuid)?;
    let mut stmt = conn
        .prepare("SELECT uuid, media_uuid, name, track_type, model_uuid, version, is_active, created_at, updated_at, note FROM listen_subtitle WHERE media_uuid = ?1")
        .map_err(|e| e.to_string())?;
    let items = stmt
        .query_map([&media_uuid], |row| {
            Ok(ListenSubtitle {
                uuid: row.get(0)?,
                media_uuid: row.get(1)?,
                name: row.get(2)?,
                track_type: row.get(3)?,
                model_uuid: row.get(4)?,
                version: row.get(5)?,
                is_active: row.get::<_, i64>(6)? != 0,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
                note: row.get(9)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(items)
}

/// Get current cues for a subtitle (version_superseded IS NULL).
#[tauri::command]
pub async fn listen_get_cues(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    subtitle_uuid: String,
) -> Result<Vec<ListenCue>, String> {
    log::debug!("listen_get_cues: dataset={}, subtitle={}", dataset_uuid, subtitle_uuid);
    let conn = open_db(&settings, &dataset_uuid)?;
    let mut stmt = conn
        .prepare(
            "SELECT uuid, subtitle_uuid, order_num, start_ms, end_ms, content, reference, confidence, version_created, version_superseded \
             FROM listen_subtitle_cue WHERE subtitle_uuid = ?1 AND version_superseded IS NULL ORDER BY order_num",
        )
        .map_err(|e| e.to_string())?;
    let items = stmt
        .query_map([&subtitle_uuid], |row| {
            Ok(ListenCue {
                uuid: row.get(0)?,
                subtitle_uuid: row.get(1)?,
                order_num: row.get(2)?,
                start_ms: row.get(3)?,
                end_ms: row.get(4)?,
                content: row.get(5)?,
                reference: row.get(6)?,
                confidence: row.get(7)?,
                version_created: row.get(8)?,
                version_superseded: row.get(9)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(items)
}

/// Get dictation progress for a media+subtitle pair.
#[tauri::command]
pub async fn listen_get_dictation(
    _settings: State<'_, SettingsState>,
    _dataset_uuid: String,
    media_uuid: String,
    subtitle_uuid: String,
) -> Result<Option<ListenDictation>, String> {
    log::debug!("listen_get_dictation: media={}, subtitle={}", media_uuid, subtitle_uuid);
    let conn = open_app_db(&_settings)?;
    let user_id = workspace_identity(&_settings);
    let mut stmt = conn
        .prepare("SELECT media_uuid, subtitle_uuid, status, completed FROM listen_dictation WHERE user_id = ?1 AND media_uuid = ?2 AND subtitle_uuid = ?3")
        .map_err(|e| e.to_string())?;
    let result = stmt
        .query_row([&user_id, &media_uuid, &subtitle_uuid], |row| {
            Ok(ListenDictation {
                media_uuid: row.get(0)?,
                subtitle_uuid: row.get(1)?,
                status: row.get(2)?,
                completed: row.get(3)?,
            })
        })
        .ok();
    Ok(result)
}

/// Get dictation status for all media in a dataset (returns media_uuids with complete status).
#[tauri::command]
pub async fn listen_get_dataset_dictation_status(
    _settings: State<'_, SettingsState>,
    _dataset_uuid: String,
) -> Result<Vec<String>, String> {
    log::debug!("listen_get_dataset_dictation_status");
    let conn = open_app_db(&_settings)?;
    let user_id = workspace_identity(&_settings);
    let mut stmt = conn
        .prepare("SELECT DISTINCT media_uuid FROM listen_dictation WHERE user_id = ?1 AND status = 'complete'")
        .map_err(|e| e.to_string())?;
    let media_uuids: Vec<String> = stmt
        .query_map([&user_id], |row| row.get(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(media_uuids)
}

// ============================================================
// Write commands
// ============================================================

/// Save/update media.
#[tauri::command]
pub async fn listen_save_media(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    media: ListenMedia,
) -> Result<(), String> {
    log::info!("listen_save_media: dataset={}, media={}, source={}", dataset_uuid, media.uuid, media.source);
    let conn = open_db(&settings, &dataset_uuid)?;
    conn.execute(
        "INSERT OR REPLACE INTO listen_media (uuid, source, duration_ms, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            media.uuid, media.source, media.duration_ms,
            media.created_at, media.updated_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Rename a media file and all related files (subtitle, waveform, transcript).
///
/// Updates the `source` field in the database and renames the corresponding files
/// on disk in the media/, subtitle/, waveform/, and transcript/ directories.
#[tauri::command]
pub async fn listen_rename_media(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    media_uuid: String,
    new_source: String,
) -> Result<(), String> {
    log::info!("listen_rename_media: dataset={}, media={}, new_source={}", dataset_uuid, media_uuid, new_source);
    let dataset_dir = find_dataset_dir(&settings, &dataset_uuid)?;
    let db_path = dataset_dir.join("data.sqlite3");
    if !db_path.exists() {
        return Err("Database file not found. Please generate the database first.".into());
    }
    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;

    // Get the old source filename
    let old_source: String = conn
        .query_row(
            "SELECT source FROM listen_media WHERE uuid = ?1",
            [&media_uuid],
            |row| row.get(0),
        )
        .map_err(|e| format!("Media not found: {}", e))?;

    if old_source == new_source {
        return Ok(()); // No change needed
    }

    // Check if new source already exists
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM listen_media WHERE source = ?1 AND uuid != ?2",
            rusqlite::params![&new_source, &media_uuid],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .map_err(|e| e.to_string())?;

    if exists {
        return Err(format!("A media with source '{}' already exists", new_source));
    }

    // Rename files on disk
    let old_rel = Path::new(&old_source);
    let new_rel = Path::new(&new_source);

    let files_to_rename = [
        ("media", old_rel.to_path_buf(), new_rel.to_path_buf()),
        ("subtitle", old_rel.with_extension("vtt"), new_rel.with_extension("vtt")),
        ("waveform", old_rel.with_extension("json"), new_rel.with_extension("json")),
        ("transcript", old_rel.with_extension("txt"), new_rel.with_extension("txt")),
    ];

    for (subdir, old_file, new_file) in files_to_rename {
        let old_path = dataset_dir.join(subdir).join(&old_file);
        let new_path = dataset_dir.join(subdir).join(&new_file);

        if old_path.exists() {
            // Ensure parent directory exists
            if let Some(parent) = new_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {}", e))?;
            }
            std::fs::rename(&old_path, &new_path)
                .map_err(|e| format!("Failed to rename {}/{}: {}", subdir, old_file.display(), e))?;
        }
    }

    // Update database
    conn.execute(
        "UPDATE listen_media SET source = ?1, updated_at = ?2 WHERE uuid = ?3",
        rusqlite::params![&new_source, chrono::Utc::now().to_rfc3339(), &media_uuid],
    )
    .map_err(|e| format!("Failed to update database: {}", e))?;

    Ok(())
}

/// Save/update a subtitle cue.
///
/// Uses an UPSERT so that editing an existing cue preserves its version
/// metadata (`version_created`, `confidence`, `version_superseded`, `created_at`),
/// while a brand-new cue is inserted at version 1. A plain `INSERT OR REPLACE`
/// would drop `version_created` (NOT NULL, no default) and fail the constraint.
#[tauri::command]
pub async fn listen_save_cue(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    cue: ListenCue,
) -> Result<(), String> {
    log::debug!("listen_save_cue: dataset={}, cue={}, order={}", dataset_uuid, cue.uuid, cue.order_num);
    let conn = open_db(&settings, &dataset_uuid)?;
    conn.execute(
        "INSERT INTO listen_subtitle_cue \
         (uuid, subtitle_uuid, order_num, start_ms, end_ms, content, reference, confidence, version_created, version_superseded) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, NULL) \
         ON CONFLICT(uuid) DO UPDATE SET \
         subtitle_uuid = excluded.subtitle_uuid, \
         order_num = excluded.order_num, \
         start_ms = excluded.start_ms, \
         end_ms = excluded.end_ms, \
         content = excluded.content, \
         reference = excluded.reference",
        rusqlite::params![
            cue.uuid, cue.subtitle_uuid, cue.order_num, cue.start_ms, cue.end_ms,
            cue.content, cue.reference, cue.confidence
        ],
    )
    .map_err(|e| e.to_string())?;
    #[cfg(not(feature = "desktop"))]
    {
        let _ = crate::sync::enqueue_change(
            &settings,
            "cue_save",
            &dataset_uuid,
            &serde_json::to_value(&cue).unwrap_or_default(),
        );
    }
    Ok(())
}

/// Delete a subtitle cue.
#[tauri::command]
pub async fn listen_delete_cue(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    cue_uuid: String,
) -> Result<(), String> {
    log::info!("listen_delete_cue: dataset={}, cue={}", dataset_uuid, cue_uuid);
    let conn = open_db(&settings, &dataset_uuid)?;
    conn.execute("DELETE FROM listen_subtitle_cue WHERE uuid = ?1", [&cue_uuid])
        .map_err(|e| e.to_string())?;
    #[cfg(not(feature = "desktop"))]
    {
        let _ = crate::sync::enqueue_change(
            &settings,
            "cue_delete",
            &dataset_uuid,
            &serde_json::json!({ "cue_uuid": cue_uuid }),
        );
    }
    Ok(())
}

/// Delete a media and all related data and files.
///
/// Removes the media's subtitle cues, subtitles, transcript and note rows from
/// the dataset database, its dictation progress from the app-level database,
/// and the associated files on disk (media, subtitle VTT, waveform JSON,
/// transcript).
#[tauri::command]
pub async fn listen_delete_media(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    media_uuid: String,
) -> Result<(), String> {
    log::info!("listen_delete_media: dataset={}, media={}", dataset_uuid, media_uuid);
    let dataset_dir = find_dataset_dir(&settings, &dataset_uuid)?;
    let db_path = dataset_dir.join("data.sqlite3");
    if !db_path.exists() {
        return Err("Database file not found. Please generate the database first.".into());
    }
    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;

    // Resolve the media source file name before deleting the row.
    let source: Option<String> = conn
        .query_row(
            "SELECT source FROM listen_media WHERE uuid = ?1",
            [&media_uuid],
            |row| row.get(0),
        )
        .ok();

    // Collect subtitle uuids so we can delete their cues.
    let subtitle_uuids: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT uuid FROM listen_subtitle WHERE media_uuid = ?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([&media_uuid], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok()).collect()
    };

    // Delete related rows inside a transaction.
    conn.execute_batch("BEGIN").map_err(|e| e.to_string())?;
    for su in &subtitle_uuids {
        conn.execute("DELETE FROM listen_subtitle_cue WHERE subtitle_uuid = ?1", [su])
            .map_err(|e| e.to_string())?;
    }
    conn.execute("DELETE FROM listen_subtitle WHERE media_uuid = ?1", [&media_uuid])
        .map_err(|e| e.to_string())?;
    // listen_transcript / listen_note may be absent in older databases; ignore errors.
    let _ = conn.execute("DELETE FROM listen_transcript WHERE media_uuid = ?1", [&media_uuid]);
    let _ = conn.execute("DELETE FROM listen_note WHERE media_uuid = ?1", [&media_uuid]);
    conn.execute("DELETE FROM listen_media WHERE uuid = ?1", [&media_uuid])
        .map_err(|e| e.to_string())?;
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    drop(conn);

    // Delete dictation progress from the app-level database.
    if let Ok(app_conn) = open_app_db(&settings) {
        let _ = app_conn.execute(
            "DELETE FROM listen_dictation WHERE media_uuid = ?1",
            [&media_uuid],
        );
    }

    // Delete related files on disk.
    if let Some(source) = source {
        // `source` is the media-relative path; sibling artefacts mirror it.
        let rel = Path::new(&source);
        let files = [
            dataset_dir.join("media").join(rel),
            dataset_dir.join("subtitle").join(rel.with_extension("vtt")),
            dataset_dir.join("waveform").join(rel.with_extension("json")),
            dataset_dir.join("transcript").join(rel.with_extension("txt")),
        ];
        for path in files {
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    Ok(())
}

/// Save dictation progress (upsert).
#[tauri::command]
pub async fn listen_save_dictation(
    _settings: State<'_, SettingsState>,
    _dataset_uuid: String,
    dictation: ListenDictation,
) -> Result<(), String> {
    log::info!("listen_save_dictation: media={}, subtitle={}, status={}", dictation.media_uuid, dictation.subtitle_uuid, dictation.status);
    let conn = open_app_db(&_settings)?;
    let user_id = workspace_identity(&_settings);
    // Delete existing row first, then insert fresh
    conn.execute(
        "DELETE FROM listen_dictation WHERE user_id = ?1 AND media_uuid = ?2 AND subtitle_uuid = ?3",
        rusqlite::params![&user_id, &dictation.media_uuid, &dictation.subtitle_uuid],
    )
    .map_err(|e| e.to_string())?;
    let new_uuid = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO listen_dictation (uuid, user_id, media_uuid, subtitle_uuid, status, completed, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'), datetime('now'))",
        rusqlite::params![
            new_uuid, user_id, &dictation.media_uuid,
            &dictation.subtitle_uuid, &dictation.status, &dictation.completed,
        ],
    )
    .map_err(|e| e.to_string())?;
    #[cfg(not(feature = "desktop"))]
    {
        let _ = crate::sync::enqueue_change(
            &_settings,
            "dictation",
            &_dataset_uuid,
            &serde_json::to_value(&dictation).unwrap_or_default(),
        );
    }
    Ok(())
}

// ============================================================
// Legacy commands (kept for backward compat with dictation page)
// ============================================================

/// List all media files in a dataset that have subtitles (for dictation selection).
#[tauri::command]
pub async fn dictation_list_media(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<Vec<(String, String)>, String> {
    log::debug!("dictation_list_media: dataset={}", dataset_uuid);
    let conn = open_db(&settings, &dataset_uuid)?;
    let mut stmt = conn
        .prepare(
            "SELECT m.uuid, m.title \
             FROM listen_media m \
             INNER JOIN listen_subtitle s ON s.media_uuid = m.uuid \
             ORDER BY m.title",
        )
        .map_err(|e| e.to_string())?;
    let media: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(media)
}

/// Get dictation data (media info + cues) for a specific media in a dataset.
#[tauri::command]
pub async fn dictation_get_data(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    media_uuid: String,
) -> Result<DictationData, String> {
    log::debug!("dictation_get_data: dataset={}, media={}", dataset_uuid, media_uuid);
    let dataset_dir = find_dataset_dir(&settings, &dataset_uuid)?;
    let db_path = dataset_dir.join("data.sqlite3");
    if !db_path.exists() {
        return Err("Database file not found. Please generate the database first.".into());
    }
    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare("SELECT uuid, title, source FROM listen_media WHERE uuid = ?1")
        .map_err(|e| e.to_string())?;
    let media = stmt
        .query_row([&media_uuid], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })
        .map_err(|e| format!("Media not found: {}", e))?;
    let (media_uuid, media_title, media_source) = media;

    let media_dir = dataset_dir.join("media");
    let audio_path = media_dir.join(&media_source);
    let audio_path_str = audio_path.to_str().unwrap_or("").to_string();

    let subtitle_uuid: Option<String> = {
        let mut stmt = conn
            .prepare("SELECT uuid FROM listen_subtitle WHERE media_uuid = ?1")
            .map_err(|e| e.to_string())?;
        stmt.query_row([&media_uuid], |row| row.get(0)).ok()
    };

    let cues: Vec<DictationCue> = match subtitle_uuid {
        Some(ref su) => {
            let mut stmt = conn
                .prepare(
                    "SELECT uuid, order_num, start_ms, end_ms, content, reference \
                     FROM listen_subtitle_cue WHERE subtitle_uuid = ?1 ORDER BY order_num",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([su.as_str()], |row| {
                    Ok(DictationCue {
                        uuid: row.get(0)?,
                        order_num: row.get(1)?,
                        start_ms: row.get(2)?,
                        end_ms: row.get(3)?,
                        content: row.get(4)?,
                        reference: row.get(5)?,
                    })
                })
                .map_err(|e| e.to_string())?;
            let mut result = Vec::new();
            for row in rows {
                result.push(row.map_err(|e| e.to_string())?);
            }
            result
        }
        None => Vec::new(),
    };

    Ok(DictationData {
        media_uuid,
        media_title,
        audio_path: audio_path_str,
        cues,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct DictationCue {
    pub uuid: String,
    pub order_num: i64,
    pub start_ms: i64,
    pub end_ms: i64,
    pub content: String,
    pub reference: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DictationData {
    pub media_uuid: String,
    pub media_title: String,
    pub audio_path: String,
    pub cues: Vec<DictationCue>,
}

// ============================================================
// Waveform
// ============================================================

/// Load waveform JSON data for a media file.
/// Returns the parsed JSON object, or None if the waveform file doesn't exist.
#[tauri::command]
pub async fn listen_get_waveform(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    source: String,
) -> Result<Option<serde_json::Value>, String> {
    log::debug!("listen_get_waveform: dataset={}, source={}", dataset_uuid, source);
    let dataset_dir = find_dataset_dir(&settings, &dataset_uuid)?;
    // `source` is the media-relative path; the waveform mirrors it under waveform/.
    let waveform_path = dataset_dir
        .join("waveform")
        .join(Path::new(&source).with_extension("json"));

    if !waveform_path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&waveform_path)
        .map_err(|e| format!("Failed to read waveform: {}", e))?;
    let data: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse waveform: {}", e))?;
    Ok(Some(data))
}

// ============================================================
// Version Management
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleVersion {
    pub uuid: String,
    pub subtitle_uuid: String,
    pub version: i64,
    pub change_type: Option<String>,
    pub description: Option<String>,
    pub cues_added: i64,
    pub cues_modified: i64,
    pub cues_deleted: i64,
    pub created_at: String,
    pub created_by: Option<String>,
}

/// Create a new version for a subtitle, returning the new version number.
///
/// This function:
/// 1. Increments the subtitle's version counter
/// 2. Creates a version log entry
/// 3. Returns the new version number for use in cue updates
#[tauri::command]
pub async fn subtitle_create_version(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    subtitle_uuid: String,
    change_type: String,
    description: String,
    created_by: String,
) -> Result<i64, String> {
    log::info!("subtitle_create_version: subtitle={}, change_type={}, by={}", subtitle_uuid, change_type, created_by);
    let conn = open_db(&settings, &dataset_uuid)?;
    
    // Get current version
    let current_version: i64 = conn
        .query_row(
            "SELECT version FROM listen_subtitle WHERE uuid = ?1",
            [&subtitle_uuid],
            |row| row.get(0),
        )
        .map_err(|e| format!("Subtitle not found: {}", e))?;
    
    let new_version = current_version + 1;
    let now = chrono::Utc::now().to_rfc3339();
    
    // Update subtitle version counter
    conn.execute(
        "UPDATE listen_subtitle SET version = ?1, updated_at = ?2 WHERE uuid = ?3",
        rusqlite::params![new_version, now, subtitle_uuid],
    )
    .map_err(|e| e.to_string())?;
    
    // Insert version log entry
    let version_uuid = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO listen_subtitle_version (uuid, subtitle_uuid, version, change_type, description, created_by, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![version_uuid, subtitle_uuid, new_version, change_type, description, created_by, now],
    )
    .map_err(|e| e.to_string())?;
    
    Ok(new_version)
}

/// Finalize a version by updating the version log with change statistics.
///
/// Call this after all cue modifications are complete.
#[tauri::command]
pub async fn subtitle_finalize_version(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    subtitle_uuid: String,
    version: i64,
    cues_added: i64,
    cues_modified: i64,
    cues_deleted: i64,
) -> Result<(), String> {
    log::info!("subtitle_finalize_version: subtitle={}, v={}, +{} ~{} -{}", subtitle_uuid, version, cues_added, cues_modified, cues_deleted);
    let conn = open_db(&settings, &dataset_uuid)?;
    
    conn.execute(
        "UPDATE listen_subtitle_version SET cues_added = ?1, cues_modified = ?2, cues_deleted = ?3 \
         WHERE subtitle_uuid = ?4 AND version = ?5",
        rusqlite::params![cues_added, cues_modified, cues_deleted, subtitle_uuid, version],
    )
    .map_err(|e| e.to_string())?;
    
    Ok(())
}

/// Get the version history for a subtitle.
#[tauri::command]
pub async fn subtitle_get_versions(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    subtitle_uuid: String,
) -> Result<Vec<SubtitleVersion>, String> {
    log::debug!("subtitle_get_versions: subtitle={}", subtitle_uuid);
    let conn = open_db(&settings, &dataset_uuid)?;
    let mut stmt = conn
        .prepare(
            "SELECT uuid, subtitle_uuid, version, change_type, description, cues_added, cues_modified, cues_deleted, created_at, created_by \
             FROM listen_subtitle_version WHERE subtitle_uuid = ?1 ORDER BY version DESC",
        )
        .map_err(|e| e.to_string())?;
    
    let items = stmt
        .query_map([&subtitle_uuid], |row| {
            Ok(SubtitleVersion {
                uuid: row.get(0)?,
                subtitle_uuid: row.get(1)?,
                version: row.get(2)?,
                change_type: row.get(3)?,
                description: row.get(4)?,
                cues_added: row.get(5)?,
                cues_modified: row.get(6)?,
                cues_deleted: row.get(7)?,
                created_at: row.get(8)?,
                created_by: row.get(9)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(items)
}

/// Get cues as they existed at a specific version.
#[tauri::command]
pub async fn subtitle_get_cues_at_version(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    subtitle_uuid: String,
    version: i64,
) -> Result<Vec<ListenCue>, String> {
    log::debug!("subtitle_get_cues_at_version: subtitle={}, version={}", subtitle_uuid, version);
    let conn = open_db(&settings, &dataset_uuid)?;
    let mut stmt = conn
        .prepare(
            "SELECT uuid, subtitle_uuid, order_num, start_ms, end_ms, content, reference, confidence, version_created, version_superseded \
             FROM listen_subtitle_cue \
             WHERE subtitle_uuid = ?1 AND version_created <= ?2 AND (version_superseded IS NULL OR version_superseded > ?2) \
             ORDER BY order_num",
        )
        .map_err(|e| e.to_string())?;
    
    let items = stmt
        .query_map(rusqlite::params![subtitle_uuid, version], |row| {
            Ok(ListenCue {
                uuid: row.get(0)?,
                subtitle_uuid: row.get(1)?,
                order_num: row.get(2)?,
                start_ms: row.get(3)?,
                end_ms: row.get(4)?,
                content: row.get(5)?,
                reference: row.get(6)?,
                confidence: row.get(7)?,
                version_created: row.get(8)?,
                version_superseded: row.get(9)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(items)
}

/// Rollback a subtitle to a previous version.
///
/// This creates a NEW version that restores the cues from the target version.
/// It does NOT delete any history.
#[tauri::command]
pub async fn subtitle_rollback_to_version(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    subtitle_uuid: String,
    target_version: i64,
) -> Result<i64, String> {
    log::info!("subtitle_rollback_to_version: subtitle={}, target_v={}", subtitle_uuid, target_version);
    let conn = open_db(&settings, &dataset_uuid)?;
    
    // Get current version
    let current_version: i64 = conn
        .query_row(
            "SELECT version FROM listen_subtitle WHERE uuid = ?1",
            [&subtitle_uuid],
            |row| row.get(0),
        )
        .map_err(|e| format!("Subtitle not found: {}", e))?;
    
    if target_version >= current_version {
        return Err("Target version must be less than current version".to_string());
    }
    
    let new_version = current_version + 1;
    let now = chrono::Utc::now().to_rfc3339();
    
    // Get cues at target version
    let mut stmt = conn
        .prepare(
            "SELECT order_num, start_ms, end_ms, content, reference, confidence \
             FROM listen_subtitle_cue \
             WHERE subtitle_uuid = ?1 AND version_created <= ?2 AND (version_superseded IS NULL OR version_superseded > ?2) \
             ORDER BY order_num",
        )
        .map_err(|e| e.to_string())?;
    
    let target_cues: Vec<(i64, i64, i64, String, Option<String>, Option<f64>)> = stmt
        .query_map(rusqlite::params![subtitle_uuid, target_version], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    
    let target_cue_count = target_cues.len() as i64;
    
    // Mark all current cues as superseded
    conn.execute(
        "UPDATE listen_subtitle_cue SET version_superseded = ?1 \
         WHERE subtitle_uuid = ?2 AND version_superseded IS NULL",
        rusqlite::params![new_version, subtitle_uuid],
    )
    .map_err(|e| e.to_string())?;
    
    // Insert restored cues with new version
    for (order_num, start_ms, end_ms, content, reference, confidence) in target_cues {
        let cue_uuid = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO listen_subtitle_cue (uuid, subtitle_uuid, order_num, start_ms, end_ms, content, reference, confidence, version_created) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![cue_uuid, subtitle_uuid, order_num, start_ms, end_ms, content, reference, confidence, new_version],
        )
        .map_err(|e| e.to_string())?;
    }
    
    // Update subtitle version counter
    conn.execute(
        "UPDATE listen_subtitle SET version = ?1, updated_at = ?2 WHERE uuid = ?3",
        rusqlite::params![new_version, now, subtitle_uuid],
    )
    .map_err(|e| e.to_string())?;
    
    // Insert version log entry
    let version_uuid = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO listen_subtitle_version (uuid, subtitle_uuid, version, change_type, description, cues_added, created_by, created_at) \
         VALUES (?1, ?2, ?3, 'rollback', ?4, ?5, 'user', ?6)",
        rusqlite::params![version_uuid, subtitle_uuid, new_version, format!("Rolled back to version {}", target_version), target_cue_count, now],
    )
    .map_err(|e| e.to_string())?;
    
    Ok(new_version)
}

/// Cut a cue's audio range from its source media into a WAV clip and add it to
/// the special "Favorites" dataset (created on demand). Registers matching
/// media/subtitle/cue rows plus a waveform so the clip is immediately playable
/// and dictatable. Returns JSON describing the newly created rows.
#[tauri::command]
pub async fn dictation_add_cue_to_favorites(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    media_uuid: String,
    cue_uuid: String,
    padding_ms: Option<i64>,
) -> Result<serde_json::Value, String> {
    log::info!(
        "dictation_add_cue_to_favorites: dataset={}, media={}, cue={}, padding={:?}",
        dataset_uuid,
        media_uuid,
        cue_uuid,
        padding_ms
    );

    // 1. Read the source cue + media from the source dataset DB.
    let src_dir = find_dataset_dir(&settings, &dataset_uuid)?;
    let src_db_path = src_dir.join("data.sqlite3");
    if !src_db_path.exists() {
        return Err("Source dataset database not found. Please generate the database first.".into());
    }
    let src_conn = Connection::open(&src_db_path).map_err(|e| e.to_string())?;

    let (start_ms, end_ms, content, reference): (i64, i64, String, Option<String>) = src_conn
        .query_row(
            "SELECT start_ms, end_ms, content, reference FROM listen_subtitle_cue WHERE uuid = ?1",
            [&cue_uuid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|e| format!("Cue not found ({}): {}", cue_uuid, e))?;

    let source_rel: String = src_conn
        .query_row(
            "SELECT source FROM listen_media WHERE uuid = ?1",
            [&media_uuid],
            |row| row.get(0),
        )
        .map_err(|e| format!("Media not found ({}): {}", media_uuid, e))?;

    // 2. Resolve the absolute source media path (`source` is relative to media/).
    let src_media_path = src_dir.join("media").join(&source_rel);
    if !src_media_path.exists() {
        return Err(format!(
            "Source media file not found: {}",
            src_media_path.display()
        ));
    }

    // 3. Resolve (or create) the Favorites dataset and open its DB + schema.
    let (fav_dir, fav_uuid) = crate::dataset::ensure_favorites_dataset(&settings)?;
    let fav_db_path = fav_dir.join("data.sqlite3");
    let db_existed = fav_db_path.exists();
    let fav_conn = Connection::open(&fav_db_path).map_err(|e| e.to_string())?;
    if !db_existed {
        crate::dataset::create_db_schema(&fav_conn)?;
    }

    // 4. De-duplicate. The favorite cue REUSES the source cue's uuid as its primary
    //    key, so a row with that uuid already present means this cue was favorited
    //    before. Deleting the clip removes that row, which re-enables adding it.
    let already: bool = fav_conn
        .query_row(
            "SELECT 1 FROM listen_subtitle_cue WHERE uuid = ?1",
            [&cue_uuid],
            |_| Ok(()),
        )
        .is_ok();
    if already {
        log::info!(
            "Cue {} already in Favorites (dataset {}); skipping duplicate",
            cue_uuid,
            fav_uuid
        );
        return Ok(serde_json::json!({
            "status": "duplicate",
            "favorites_dataset_uuid": fav_uuid,
            "cue_uuid": cue_uuid,
            "message": "This cue is already in your Favorites dataset.",
        }));
    }

    // 5. Compute the padded clip range.
    let pad = padding_ms.unwrap_or(150).max(0);
    let clip_start = (start_ms - pad).max(0);
    let clip_end = end_ms + pad;

    // 6. Decode + slice the clip at the source's native rate/channels.
    let pcm = crate::audio::decode_range_native(&src_media_path, clip_start, clip_end)?;
    if pcm.samples.is_empty() {
        return Err("Decoded clip is empty (range may be beyond the end of the media).".into());
    }
    let channels = pcm.channels.max(1) as usize;
    let frames = pcm.samples.len() / channels;
    let clip_dur_ms = if pcm.sample_rate > 0 {
        (frames as i64 * 1000) / pcm.sample_rate as i64
    } else {
        0
    };

    // 7. Write the WAV clip into the Favorites media/ dir.
    let new_media_uuid = Uuid::new_v4().to_string();
    let wav_file_name = format!("{}.wav", new_media_uuid);
    let fav_media_dir = fav_dir.join("media");
    let wav_path = fav_media_dir.join(&wav_file_name);
    crate::audio::write_wav(&wav_path, &pcm.samples, pcm.sample_rate, pcm.channels)?;

    // 8. Insert media/subtitle/cue rows. `source` is media-relative (matches the
    //    waveform lookup and the frontend `${dataset}/media/${source}` playback URL).
    //    The favorite CUE reuses the source `cue_uuid` as its PK for de-duplication.
    let now = chrono::Utc::now().to_rfc3339();
    let subtitle_uuid = Uuid::new_v4().to_string();

    fav_conn
        .execute(
            "INSERT OR REPLACE INTO listen_media (uuid, source, duration_ms, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![new_media_uuid, wav_file_name, clip_dur_ms, now, now],
        )
        .map_err(|e| format!("Failed to insert favorite media: {}", e))?;

    fav_conn
        .execute(
            "INSERT INTO listen_subtitle (uuid, media_uuid, name, track_type, version, is_active, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, 1, 1, ?5, ?6)",
            rusqlite::params![subtitle_uuid, new_media_uuid, "Favorite", "stt", now, now],
        )
        .map_err(|e| format!("Failed to insert favorite subtitle: {}", e))?;

    fav_conn
        .execute(
            "INSERT INTO listen_subtitle_cue (uuid, subtitle_uuid, order_num, start_ms, end_ms, content, reference, version_created) \
             VALUES (?1, ?2, 0, 0, ?3, ?4, ?5, 1)",
            rusqlite::params![cue_uuid, subtitle_uuid, clip_dur_ms, content, reference],
        )
        .map_err(|e| format!("Failed to insert favorite cue: {}", e))?;

    // 9. Generate + persist the clip waveform (best-effort; parity with normal media).
    match crate::audio::generate_waveform(&wav_path, 100) {
        Ok(peaks) => {
            let peaks_json = serde_json::json!({
                "version": peaks.version,
                "channels": peaks.channels,
                "sample_rate": peaks.sample_rate,
                "samples_per_pixel": peaks.samples_per_pixel,
                "bits": peaks.bits,
                "length": peaks.data.len() / 2,
                "data": peaks.data,
            })
            .to_string();
            let wf_path = fav_dir
                .join("waveform")
                .join(format!("{}.json", new_media_uuid));
            if let Some(parent) = wf_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(&wf_path, &peaks_json) {
                log::warn!("[favorites] waveform file write failed: {}", e);
            }
            if let Err(e) = crate::dataset::write_waveform_to_db(
                &fav_conn,
                &fav_media_dir,
                &wav_path,
                &peaks_json,
                peaks.sample_rate,
            ) {
                log::warn!("[favorites] waveform DB write failed: {}", e);
            }
        }
        Err(e) => log::warn!("[favorites] waveform generation failed: {}", e),
    }

    // 10. Notify listeners + return the new identifiers.
    let _ = app.emit("dataset-list-changed", ());
    log::info!(
        "Added favorite clip: dataset={}, media={}, cue={}, duration={}ms",
        fav_uuid,
        new_media_uuid,
        cue_uuid,
        clip_dur_ms
    );
    Ok(serde_json::json!({
        "status": "ok",
        "favorites_dataset_uuid": fav_uuid,
        "media_uuid": new_media_uuid,
        "subtitle_uuid": subtitle_uuid,
        "cue_uuid": cue_uuid,
        "duration_ms": clip_dur_ms,
        "wav_rel_path": format!("media/{}", wav_file_name),
    }))
}

/// List the cue UUIDs currently stored in the Favorites dataset.
///
/// Favorite clips reuse their SOURCE cue's uuid as the cue primary key, so the
/// returned set can be matched against any dataset's cues to mark which ones are
/// already favorited. Returns an empty list when no Favorites dataset exists yet
/// (it is created lazily on first add, so this lookup never creates it).
#[tauri::command]
pub async fn dictation_list_favorite_cues(
    settings: State<'_, SettingsState>,
) -> Result<Vec<String>, String> {
    let found = match crate::dataset::find_favorites_dataset(&settings) {
        Some(f) => f,
        None => return Ok(Vec::new()),
    };
    let (fav_dir, _fav_uuid) = found;
    let db_path = fav_dir.join("data.sqlite3");
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT uuid FROM listen_subtitle_cue")
        .map_err(|e| e.to_string())?;
    let uuids = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(uuids)
}
