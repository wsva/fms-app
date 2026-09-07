use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::State;
use uuid::Uuid;

use crate::auth::get_current_user_email;
use crate::dataset::find_dataset_dir;
use crate::settings::SettingsState;

// ============================================================
// Shared types
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenMedia {
    pub uuid: String,
    pub title: String,
    pub source: String,
    pub note: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenSubtitle {
    pub uuid: String,
    pub media_uuid: String,
    pub name: String,
    pub note: String,
    pub created_at: String,
    pub updated_at: String,
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
    Connection::open(&db_path).map_err(|e| e.to_string())
}

/// Open the app-level database (for dictation progress, etc.).
pub(crate) fn open_app_db() -> Result<Connection, String> {
    let db_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("fms-app");
    std::fs::create_dir_all(&db_dir).map_err(|e| e.to_string())?;
    let conn = Connection::open(db_dir.join("app.sqlite3")).map_err(|e| e.to_string())?;
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
    let conn = open_db(&settings, &dataset_uuid)?;
    let mut stmt = conn
        .prepare("SELECT uuid, title, source, note, created_at, updated_at FROM listen_media ORDER BY title")
        .map_err(|e| e.to_string())?;
    let items = stmt
        .query_map([], |row| {
            Ok(ListenMedia {
                uuid: row.get(0)?,
                title: row.get(1)?,
                source: row.get(2)?,
                note: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
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
    let conn = open_db(&settings, &dataset_uuid)?;
    let mut stmt = conn
        .prepare("SELECT uuid, title, source, note, created_at, updated_at FROM listen_media WHERE uuid = ?1")
        .map_err(|e| e.to_string())?;
    stmt.query_row([&media_uuid], |row| {
        Ok(ListenMedia {
            uuid: row.get(0)?,
            title: row.get(1)?,
            source: row.get(2)?,
            note: row.get(3)?,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
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
    let conn = open_db(&settings, &dataset_uuid)?;
    let mut stmt = conn
        .prepare("SELECT uuid, media_uuid, name, note, created_at, updated_at FROM listen_subtitle WHERE media_uuid = ?1")
        .map_err(|e| e.to_string())?;
    let items = stmt
        .query_map([&media_uuid], |row| {
            Ok(ListenSubtitle {
                uuid: row.get(0)?,
                media_uuid: row.get(1)?,
                name: row.get(2)?,
                note: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(items)
}

/// Get all cues for a subtitle.
#[tauri::command]
pub async fn listen_get_cues(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    subtitle_uuid: String,
) -> Result<Vec<ListenCue>, String> {
    let conn = open_db(&settings, &dataset_uuid)?;
    let mut stmt = conn
        .prepare(
            "SELECT uuid, subtitle_uuid, order_num, start_ms, end_ms, content, reference \
             FROM listen_subtitle_cue WHERE subtitle_uuid = ?1 ORDER BY order_num",
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
    let conn = open_app_db()?;
    let user_id = get_current_user_email();
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
    let conn = open_db(&settings, &dataset_uuid)?;
    let user_id = get_current_user_email();
    conn.execute(
        "INSERT OR REPLACE INTO listen_media (uuid, user_id, title, source, note, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            media.uuid, user_id, media.title, media.source, media.note,
            media.created_at, media.updated_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Save/update a subtitle cue.
#[tauri::command]
pub async fn listen_save_cue(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    cue: ListenCue,
) -> Result<(), String> {
    let conn = open_db(&settings, &dataset_uuid)?;
    conn.execute(
        "INSERT OR REPLACE INTO listen_subtitle_cue \
         (uuid, subtitle_uuid, order_num, start_ms, end_ms, content, reference) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            cue.uuid, cue.subtitle_uuid, cue.order_num, cue.start_ms, cue.end_ms,
            cue.content, cue.reference
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a subtitle cue.
#[tauri::command]
pub async fn listen_delete_cue(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    cue_uuid: String,
) -> Result<(), String> {
    let conn = open_db(&settings, &dataset_uuid)?;
    conn.execute("DELETE FROM listen_subtitle_cue WHERE uuid = ?1", [&cue_uuid])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Save dictation progress (upsert).
#[tauri::command]
pub async fn listen_save_dictation(
    _settings: State<'_, SettingsState>,
    _dataset_uuid: String,
    dictation: ListenDictation,
) -> Result<(), String> {
    let conn = open_app_db()?;
    let user_id = get_current_user_email();
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
    let dataset_dir = find_dataset_dir(&settings, &dataset_uuid)?;
    let stem = Path::new(&source)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let waveform_path = dataset_dir.join("waveform").join(format!("{}.json", stem));

    if !waveform_path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&waveform_path)
        .map_err(|e| format!("Failed to read waveform: {}", e))?;
    let data: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse waveform: {}", e))?;
    Ok(Some(data))
}
