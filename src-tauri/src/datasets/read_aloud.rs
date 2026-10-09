//! "Read aloud" datasets — a text is shown, the user records themselves reading
//! it, STT transcribes the take, and the transcript is scored against the
//! reference text. Passing scores earn XP.
//!
//! Storage mirrors the book/dictation dataset layout: each read-aloud dataset is
//! a self-contained directory under `<datasets_dir>/read_aloud/` with an
//! `info.json`, a `data.sqlite3` (texts + attempts) and a `media/` folder that
//! holds every recorded take.
//!
//! Scoring reuses the shared Ratcliff-Obershelp `similarity_score` (0-100) from
//! `textsim`, the same metric used for cue alignment.

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use chrono::Utc;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, State};
use uuid::Uuid;

use crate::auth::workspace_identity;
use crate::datasets::{
    assert_uuid_free, dataset_roots, move_to_trash, now_stamp, read_info, read_info_opt, touch_info,
    write_info, DatasetInfo, DatasetType, FORMAT_READ_ALOUD, INFO_FILE,
};
use crate::settings::SettingsState;
use crate::datasets::textsim::similarity_score;
use crate::xp::{xp_award_internal, XpAwardResult};

/// Score thresholds for the scaled XP reward.
const XP_PASS_SCORE: f64 = 60.0;
const XP_GOOD_SCORE: f64 = 80.0;

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
    /// Similarity score 0-100 against the reference text.
    pub score: f64,
    pub created_at: String,
}

/// Result of submitting a recorded attempt.
#[derive(Clone, Serialize)]
pub struct ReadAloudSubmitResult {
    pub score: f64,
    pub attempt: ReadAttempt,
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
    // §3.4: a per-dataset tombstone table (travels with snapshots) so a delete
    // is durable and can collide with a resurrected offline edit.
    crate::sync::change_log::ensure_tombstones(&conn)?;
    Ok(conn)
}

/// Resolve a stored relative audio path to an absolute path for playback.
fn resolve_audio_url(dir: &Path, audio_path: &Option<String>) -> Option<String> {
    audio_path
        .as_ref()
        .filter(|p| !p.is_empty())
        .map(|rel| dir.join(rel).to_string_lossy().into_owned())
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
    let dir = find_dataset_dir(&settings, &uuid)?;
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
    let dir = find_dataset_dir(&settings, &uuid)?;
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
    let dir = find_dataset_dir(settings, dataset_uuid)?;
    let conn = open_db(&dir)?;
    let mut stmt = conn
        .prepare(
            "SELECT t.uuid, t.dataset_uuid, t.order_num, t.title, t.note, t.content, t.best_score, \
                    (SELECT COUNT(*) FROM read_attempt a WHERE a.text_uuid = t.uuid) AS attempt_count, \
                    t.created_at, t.updated_at \
             FROM read_text t ORDER BY t.order_num, t.created_at",
        )
        .map_err(|e| e.to_string())?;
    let items = stmt
        .query_map([], |row| {
            Ok(ReadText {
                uuid: row.get(0)?,
                dataset_uuid: row.get(1)?,
                order_num: row.get(2)?,
                title: row.get(3)?,
                note: row.get(4)?,
                content: row.get(5)?,
                best_score: row.get(6)?,
                attempt_count: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
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
pub(crate) fn save_text_row(
    settings: &SettingsState,
    dataset_uuid: &str,
    text: &ReadText,
) -> Result<(), String> {
    let dir = find_dataset_dir(settings, dataset_uuid)?;
    let conn = open_db(&dir)?;
    conn.execute(
        "INSERT INTO read_text \
         (uuid, dataset_uuid, order_num, title, note, content, best_score, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
         ON CONFLICT(uuid) DO UPDATE SET \
           order_num = excluded.order_num, \
           title = excluded.title, \
           note = excluded.note, \
           content = excluded.content, \
           updated_at = excluded.updated_at",
        rusqlite::params![
            text.uuid,
            dataset_uuid,
            text.order_num,
            text.title,
            text.note,
            text.content,
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

/// Delete a text, all its attempts, and their audio files.
#[tauri::command]
pub async fn read_aloud_delete_text(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    uuid: String,
) -> Result<(), String> {
    let dir = find_dataset_dir(&settings, &dataset_uuid)?;
    let conn = open_db(&dir)?;
    // Best-effort removal of every attempt's audio file.
    if let Ok(mut stmt) = conn.prepare("SELECT audio_path FROM read_attempt WHERE text_uuid = ?1") {
        let paths: Vec<Option<String>> = stmt
            .query_map([&uuid], |row| row.get::<_, Option<String>>(0))
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        for rel in paths.into_iter().flatten() {
            let _ = fs::remove_file(dir.join(rel));
        }
    }
    conn.execute("DELETE FROM read_attempt WHERE text_uuid = ?1", [&uuid])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM read_text WHERE uuid = ?1", [&uuid])
        .map_err(|e| e.to_string())?;
    let _ = touch_info(&dir);
    // The attempt rows go with it by cascade, so one change entry suffices — the
    // apply path deletes the same way.
    {
        let _ = crate::sync::change_log::commit_change(
            &settings,
            "read_text_delete",
            &dataset_uuid,
            &serde_json::json!({ "uuid": uuid }),
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
    let dir = find_dataset_dir(&settings, &dataset_uuid)?;
    let conn = open_db(&dir)?;
    let mut stmt = conn
        .prepare(
            "SELECT uuid, text_uuid, user_id, audio_path, recognized, score, created_at \
             FROM read_attempt WHERE text_uuid = ?1 ORDER BY created_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let items = stmt
        .query_map([&text_uuid], |row| {
            let audio_path: Option<String> = row.get(3)?;
            Ok(ReadAttempt {
                uuid: row.get(0)?,
                text_uuid: row.get(1)?,
                user_id: row.get(2)?,
                audio_url: resolve_audio_url(&dir, &audio_path),
                audio_path,
                recognized: row.get(4)?,
                score: row.get(5)?,
                created_at: row.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(items)
}

/// Delete a single attempt and its audio file.
#[tauri::command]
pub async fn read_aloud_delete_attempt(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    uuid: String,
) -> Result<(), String> {
    let dir = find_dataset_dir(&settings, &dataset_uuid)?;
    let conn = open_db(&dir)?;
    if let Ok(Some(rel)) = conn.query_row(
        "SELECT audio_path FROM read_attempt WHERE uuid = ?1",
        [&uuid],
        |row| row.get::<_, Option<String>>(0),
    ) {
        let _ = fs::remove_file(dir.join(rel));
    }
    conn.execute("DELETE FROM read_attempt WHERE uuid = ?1", [&uuid])
        .map_err(|e| e.to_string())?;
    {
        let _ = crate::sync::change_log::commit_change(
            &settings,
            "read_attempt_delete",
            &dataset_uuid,
            &serde_json::json!({ "uuid": uuid }),
        );
    }
    Ok(())
}

/// Persist one attempt row, roll the text's `best_score` forward, and journal the
/// change. Shared by `read_aloud_submit` and both sync apply paths (hub replay,
/// follower pull-apply) so an attempt recorded on any device converges the same
/// way everywhere.
///
/// The recording itself is not part of the change: the WAV lives in the dataset's
/// `media/` and travels as an ordinary file through the manifest (§3.2). Only
/// `audio_path` is stored — `audio_url` in the payload is the sender's absolute
/// path and is ignored, since each device re-resolves it on read. Attempts are
/// per-user history, so an apply path overwrites `user_id` with the identity the
/// hub bound the device to (like the `dictation` kind) rather than trusting it.
pub(crate) fn save_attempt_row(
    settings: &SettingsState,
    dataset_uuid: &str,
    attempt: &ReadAttempt,
) -> Result<(), String> {
    let dir = find_dataset_dir(settings, dataset_uuid)?;
    let conn = open_db(&dir)?;
    conn.execute(
        "INSERT OR REPLACE INTO read_attempt \
         (uuid, text_uuid, user_id, audio_path, recognized, score, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            attempt.uuid,
            attempt.text_uuid,
            attempt.user_id,
            attempt.audio_path,
            attempt.recognized,
            attempt.score,
            attempt.created_at
        ],
    )
    .map_err(|e| e.to_string())?;

    // Track the best score seen for this text.
    conn.execute(
        "UPDATE read_text SET best_score = MAX(COALESCE(best_score, 0), ?1), \
         updated_at = datetime('now') WHERE uuid = ?2",
        rusqlite::params![attempt.score, attempt.text_uuid],
    )
    .map_err(|e| e.to_string())?;
    let _ = touch_info(&dir);

    {
        let _ = crate::sync::change_log::commit_change(
            settings,
            "read_attempt_save",
            dataset_uuid,
            &serde_json::to_value(attempt).unwrap_or_default(),
        );
    }
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

/// Pure similarity score (0-100) between a reference text and a transcript.
/// Exposed so the frontend/MCP can preview a score without persisting anything.
#[tauri::command]
pub async fn read_aloud_score(content: String, recognized: String) -> Result<f64, String> {
    Ok(similarity_score(&content, &recognized))
}

/// Submit a recorded take: persist the audio, score it against the reference
/// text, store the attempt, and award scaled XP when the score passes.
///
/// The frontend records + runs STT (via `model_transcribe`) and passes the WAV
/// (base64) plus the recognized text here so scoring, storage and XP all happen
/// atomically on the backend.
#[tauri::command]
pub async fn read_aloud_submit(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    text_uuid: String,
    wav_base64: String,
    recognized: String,
) -> Result<ReadAloudSubmitResult, String> {
    let dir = find_dataset_dir(&settings, &dataset_uuid)?;

    // Load the reference text. The read handle is scoped to this block so the
    // attempt write below gets a fresh connection instead of two live handles
    // on the same dataset DB.
    let content: String = {
        let conn = open_db(&dir)?;
        conn.query_row(
            "SELECT content FROM read_text WHERE uuid = ?1",
            [&text_uuid],
            |row| row.get(0),
        )
        .map_err(|_| format!("Text {} not found in dataset", text_uuid))?
    };

    let score = similarity_score(&content, &recognized);

    // Persist the recording.
    let attempt_uuid = Uuid::new_v4().to_string();
    let file_name = format!("{}.wav", attempt_uuid);
    let media_dir = dir.join("media");
    fs::create_dir_all(&media_dir).map_err(|e| e.to_string())?;
    let audio_path = if wav_base64.trim().is_empty() {
        None
    } else {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(wav_base64.trim())
            .map_err(|e| format!("Invalid base64 audio: {}", e))?;
        fs::write(media_dir.join(&file_name), &bytes).map_err(|e| e.to_string())?;
        Some(format!("media/{}", file_name))
    };

    let user_id = workspace_identity(&settings);
    let now = Utc::now().to_rfc3339();

    let attempt = ReadAttempt {
        uuid: attempt_uuid,
        text_uuid: text_uuid.clone(),
        user_id: user_id.clone(),
        audio_url: resolve_audio_url(&dir, &audio_path),
        audio_path,
        recognized,
        score,
        created_at: now,
    };
    // Row write + `best_score` roll-up + change journal, through the same path a
    // synced attempt arrives by.
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
        attempt,
        xp_awarded: total_awarded,
        lifetime_xp,
        level,
    })
}
