//! Stage 3: parse the generated VTT into `data.sqlite3` cues, plus the schema
//! every stage reads and writes.

use std::fs;
use std::path::Path;

use chrono::Utc;
use rusqlite::Connection;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::settings::SettingsState;

use super::touch_info;

use super::{
    DatasetProgress, find_dataset_dir_write, list_media_files, rel_source_string, sibling_path,
};

/// Bootstrap a dictation dataset's `data.sqlite3` so it exists (empty) from
/// creation: open the file and run the schema. **No-op when the DB already
/// exists**, so it never clobbers a fully generated database and is safe on the
/// idempotent init path. Media registration is deliberately NOT done here —
/// [`dataset_sync_media`] owns reconciling `listen_media` with the files on disk,
/// and `dataset_generate_database` rebuilds everything from the VTT subtitles.
pub(crate) fn ensure_dataset_db(dataset_dir: &Path) -> Result<(), String> {
    let db_path = dataset_dir.join("data.sqlite3");
    if db_path.exists() {
        return Ok(());
    }
    let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
    create_db_schema(&conn)?;
    log::info!("Created empty database at '{}'", db_path.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Stage 3: SQLite database generation
// ---------------------------------------------------------------------------
/// A single cue parsed from a VTT file.
#[allow(dead_code)]
pub(crate) struct VttCue {
    pub(crate) start_ms: i64,
    pub(crate) end_ms: i64,
    pub(crate) content: String,
}

/// Parse a VTT file into a list of cues.
#[allow(dead_code)]
pub(crate) fn parse_vtt(path: &Path) -> Result<Vec<VttCue>, String> {
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut cues = Vec::new();

    // Skip the WEBVTT header line
    let mut lines = content.lines().peekable();
    while let Some(line) = lines.peek() {
        if line.starts_with("WEBVTT") || line.is_empty() {
            lines.next();
            continue;
        }
        break;
    }

    // Parse cue blocks
    loop {
        // Skip blank lines
        while let Some(line) = lines.peek() {
            if line.is_empty() {
                lines.next();
            } else {
                break;
            }
        }

        // Look for a timing line: HH:MM:SS.mmm --> HH:MM:SS.mmm
        let timing_line = match lines.next() {
            Some(l) => l,
            None => break,
        };

        if !timing_line.contains("-->") {
            continue; // skip non-timing lines (e.g. NOTE blocks)
        }

        let parts: Vec<&str> = timing_line.split("-->").collect();
        if parts.len() != 2 {
            continue;
        }

        let start_ms = match parse_vtt_timestamp(parts[0].trim()) {
            Some(ms) => ms,
            None => continue,
        };
        let end_ms = match parse_vtt_timestamp(parts[1].trim()) {
            Some(ms) => ms,
            None => continue,
        };

        // Collect text lines until blank line or EOF
        let mut text_lines = Vec::new();
        while let Some(line) = lines.peek() {
            if line.is_empty() {
                break;
            }
            text_lines.push(lines.next().unwrap());
        }

        let content = text_lines.join("\n");
        if !content.is_empty() {
            cues.push(VttCue {
                start_ms,
                end_ms,
                content,
            });
        }
    }

    Ok(cues)
}

/// Parse a VTT timestamp like "00:01:23.456" into milliseconds.
#[allow(dead_code)]
pub(crate) fn parse_vtt_timestamp(s: &str) -> Option<i64> {
    let s = s.split_whitespace().next()?; // ignore any trailing position info
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let hours: i64 = parts[0].parse().ok()?;
    let minutes: i64 = parts[1].parse().ok()?;
    // Seconds may have a decimal point
    let sec_parts: Vec<&str> = parts[2].split('.').collect();
    let seconds: i64 = sec_parts[0].parse().ok()?;
    let millis: i64 = if sec_parts.len() == 2 {
        let ms_str = sec_parts[1];
        // Pad or truncate to 3 digits
        let padded = format!("{:0<3}", ms_str);
        padded[..3].parse().ok()?
    } else {
        0
    };

    Some(hours * 3_600_000 + minutes * 60_000 + seconds * 1_000 + millis)
}

/// Create the SQLite database schema (v2 with versioning).
pub(crate) fn create_db_schema(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        -- Media files (audio/video)
        CREATE TABLE IF NOT EXISTS listen_media (
            uuid        TEXT PRIMARY KEY,
            source      TEXT NOT NULL,
            duration_ms INTEGER,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- Subtitle tracks with versioning
        CREATE TABLE IF NOT EXISTS listen_subtitle (
            uuid        TEXT PRIMARY KEY,
            media_uuid  TEXT NOT NULL,
            name        TEXT NOT NULL,
            track_type  TEXT,
            model_uuid  TEXT,
            version     INTEGER NOT NULL DEFAULT 1,
            is_active   INTEGER NOT NULL DEFAULT 1,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
            note        TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_subtitle_media ON listen_subtitle(media_uuid);
        CREATE INDEX IF NOT EXISTS idx_subtitle_active ON listen_subtitle(media_uuid, is_active);

        -- Cues with efficient versioning (effective range pattern)
        CREATE TABLE IF NOT EXISTS listen_subtitle_cue (
            uuid               TEXT PRIMARY KEY,
            subtitle_uuid      TEXT NOT NULL,
            order_num          INTEGER NOT NULL,
            start_ms           INTEGER NOT NULL,
            end_ms             INTEGER NOT NULL,
            content            TEXT NOT NULL,
            reference          TEXT,
            confidence         REAL,
            version_created    INTEGER NOT NULL,
            version_superseded INTEGER,
            created_at         TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_cue_subtitle ON listen_subtitle_cue(subtitle_uuid);
        CREATE INDEX IF NOT EXISTS idx_cue_current ON listen_subtitle_cue(subtitle_uuid, version_superseded);
        CREATE INDEX IF NOT EXISTS idx_cue_version ON listen_subtitle_cue(subtitle_uuid, version_created);

        -- Version metadata
        CREATE TABLE IF NOT EXISTS listen_subtitle_version (
            uuid           TEXT PRIMARY KEY,
            subtitle_uuid  TEXT NOT NULL,
            version        INTEGER NOT NULL,
            change_type    TEXT,
            description    TEXT,
            cues_added     INTEGER NOT NULL DEFAULT 0,
            cues_modified  INTEGER NOT NULL DEFAULT 0,
            cues_deleted   INTEGER NOT NULL DEFAULT 0,
            created_at     TEXT NOT NULL DEFAULT (datetime('now')),
            created_by     TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_version_subtitle ON listen_subtitle_version(subtitle_uuid);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_version_unique ON listen_subtitle_version(subtitle_uuid, version);

        -- Transcript (per-media)
        CREATE TABLE IF NOT EXISTS listen_transcript (
            uuid       TEXT PRIMARY KEY,
            media_uuid TEXT NOT NULL,
            transcript TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- Notes
        CREATE TABLE IF NOT EXISTS listen_note (
            uuid       TEXT PRIMARY KEY,
            media_uuid TEXT NOT NULL,
            note       TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        "
    )
    .map_err(|e| e.to_string())?;

    // §3.4: a per-dataset tombstone table (travels with snapshots) so a delete
    // is durable and can collide with a resurrected offline edit.
    crate::sync::change_log::ensure_tombstones(conn)?;

    Ok(())
}

/// Delete the SQLite database for a dataset and reset status to with_subtitle.
#[allow(dead_code)]
#[tauri::command]
pub async fn dataset_delete_database(
    _app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<(), String> {
    let dataset_dir = find_dataset_dir_write(&settings, &uuid)?;
    let db_path = dataset_dir.join("data.sqlite3");

    if db_path.exists() {
        log::info!("Deleting database for dataset '{}'", uuid);
        fs::remove_file(&db_path).map_err(|e| format!("Failed to remove database: {}", e))?;
    }

    // Update timestamp
    touch_info(&dataset_dir)?;

    Ok(())
}

/// Generate the SQLite database for a dataset (Stage 2 → Stage 3).
#[allow(dead_code)]
#[tauri::command]
pub async fn dataset_generate_database(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<(), String> {
    let dataset_dir = find_dataset_dir_write(&settings, &uuid)?;
    let media_dir = dataset_dir.join("media");
    let subtitle_dir = dataset_dir.join("subtitle");
    let transcript_dir = dataset_dir.join("transcript");
    let db_path = dataset_dir.join("data.sqlite3");

    // Remove existing DB if present
    if db_path.exists() {
        fs::remove_file(&db_path).map_err(|e| e.to_string())?;
    }

    log::info!("Generating database for dataset '{}'", uuid);
    let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
    create_db_schema(&conn)?;

    let media_files = list_media_files(&media_dir);
    let total = media_files.len();
    let now = Utc::now().to_rfc3339();

    for (i, mf) in media_files.iter().enumerate() {
        let _ = app.emit(
            "dataset-progress",
            DatasetProgress {
                uuid: uuid.clone(),
                current_file: mf.name.clone(),
                file_index: i + 1,
                total_files: total,
                stage: "database".into(),
            },
        );

        let file_path = Path::new(&mf.path);
        let stem = file_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();

        let media_uuid = Uuid::new_v4().to_string();

        // Insert listen_media. `source` stores the media-relative path so nested
        // media resolve for playback and sibling (subtitle/waveform/transcript) files.
        conn.execute(
            "INSERT INTO listen_media (uuid, source, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![media_uuid, rel_source_string(&media_dir, file_path), now, now],
        )
        .map_err(|e| e.to_string())?;

        // Insert listen_transcript if transcript exists (mirrors the media sub-directory).
        let transcript_path = sibling_path(&media_dir, &transcript_dir, file_path, "txt");
        if transcript_path.exists() {
            let transcript_text = fs::read_to_string(&transcript_path).map_err(|e| e.to_string())?;
            let transcript_uuid = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO listen_transcript (uuid, media_uuid, transcript, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![transcript_uuid, media_uuid, transcript_text, now, now],
            )
            .map_err(|e| e.to_string())?;
        }

        // Parse VTT and insert subtitle + cues (mirrors the media sub-directory).
        let vtt_path = sibling_path(&media_dir, &subtitle_dir, file_path, "vtt");
        if vtt_path.exists() {
            let cues = parse_vtt(&vtt_path)?;
            let subtitle_uuid = Uuid::new_v4().to_string();

            // Insert listen_subtitle (version 1, initial STT)
            conn.execute(
                "INSERT INTO listen_subtitle (uuid, media_uuid, name, track_type, version, is_active, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 1, 1, ?5, ?6)",
                rusqlite::params![subtitle_uuid, media_uuid, stem, "stt", now, now],
            )
            .map_err(|e| e.to_string())?;

            // Insert version log entry
            let version_uuid = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO listen_subtitle_version (uuid, subtitle_uuid, version, change_type, description, cues_added, created_by) VALUES (?1, ?2, 1, 'initial', 'Initial STT transcription', ?3, 'system')",
                rusqlite::params![version_uuid, subtitle_uuid, cues.len() as i64],
            )
            .map_err(|e| e.to_string())?;

            // Insert each cue (version 1)
            for (order, cue) in cues.iter().enumerate() {
                let cue_uuid = Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO listen_subtitle_cue (uuid, subtitle_uuid, order_num, start_ms, end_ms, content, version_created) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)",
                    rusqlite::params![cue_uuid, subtitle_uuid, order as i64, cue.start_ms, cue.end_ms, cue.content],
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }

    // Close connection before updating info.json
    drop(conn);

    // Update timestamp
    touch_info(&dataset_dir)?;

    log::info!("Database generated for dataset '{}': {} media, {} total", uuid, total, total);
    Ok(())
}

/// Write VTT subtitles to the existing database (without rebuilding the entire database).
/// Clears existing subtitle data and re-imports from VTT files by walking the
/// subtitle/ directory recursively.
#[allow(dead_code)]
#[tauri::command]
pub async fn dataset_write_subtitles_to_db(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<String, String> {
    let dataset_dir = find_dataset_dir_write(&settings, &uuid)?;
    let subtitle_dir = dataset_dir.join("subtitle");
    let db_path = dataset_dir.join("data.sqlite3");

    if !db_path.exists() {
        return Err("Database file not found. Please build the database first.".into());
    }
    if !subtitle_dir.exists() {
        return Err("No subtitle directory found. Generate subtitles first.".into());
    }

    log::info!("Writing subtitles to database for dataset '{}'", uuid);
    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;

    // Clear existing subtitle data
    conn.execute("DELETE FROM listen_subtitle_cue", [])
        .map_err(|e| format!("Failed to clear subtitle cues: {}", e))?;
    conn.execute("DELETE FROM listen_subtitle", [])
        .map_err(|e| format!("Failed to clear subtitles: {}", e))?;

    // Build a lookup: media source stem -> media_uuid  (e.g. "a/b" -> uuid)
    let mut stmt = conn
        .prepare("SELECT uuid, source FROM listen_media")
        .map_err(|e| e.to_string())?;
    let media_map: std::collections::HashMap<String, String> = stmt
        .query_map([], |row| {
            let media_uuid: String = row.get(0)?;
            let source: String = row.get(1)?;
            Ok((media_uuid, source))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .map(|(media_uuid, source)| {
            let stem = Path::new(&source)
                .with_extension("")
                .to_string_lossy()
                .into_owned();
            (stem, media_uuid)
        })
        .collect();
    drop(stmt);

    // Walk the subtitle directory recursively, collecting .vtt files
    let mut vtt_files: Vec<(String, std::path::PathBuf)> = Vec::new();
    collect_vtt_files(&subtitle_dir, &subtitle_dir, &mut vtt_files);
    vtt_files.sort_by(|a, b| a.0.cmp(&b.0));

    let now = Utc::now().to_rfc3339();
    let mut written = 0usize;
    let mut skipped = 0usize;
    let total = vtt_files.len();
    let mut log_lines: Vec<String> = Vec::new();

    log_lines.push(format!("Media entries in database ({}):", media_map.len()));
    for (stem, media_uuid) in &media_map {
        log_lines.push(format!("  stem='{}'  media_uuid={}", stem, media_uuid));
    }
    log_lines.push(format!("VTT files found on disk ({}):", total));
    for (rel_stem, vtt_path) in &vtt_files {
        log_lines.push(format!("  stem='{}'  path={}", rel_stem, vtt_path.display()));
    }
    log_lines.push(String::new());

    for (rel_stem, vtt_path) in &vtt_files {
        let _ = app.emit(
            "dataset-progress",
            DatasetProgress {
                uuid: uuid.clone(),
                current_file: rel_stem.clone(),
                file_index: written + skipped + 1,
                total_files: total,
                stage: "write_subtitles".into(),
            },
        );

        // Match VTT to a media entry by source stem
        let Some(media_uuid) = media_map.get(rel_stem) else {
            log_lines.push(format!("SKIP '{}' — no matching media entry (stem not found in database)", rel_stem));
            skipped += 1;
            continue;
        };

        let cues = parse_vtt(vtt_path)?;
        let subtitle_uuid = Uuid::new_v4().to_string();
        let name = Path::new(rel_stem)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();

        log_lines.push(format!("WRITE '{}' -> media_uuid={}, {} cue(s)", rel_stem, media_uuid, cues.len()));

        // Insert listen_subtitle (version 1, initial import)
        conn.execute(
            "INSERT INTO listen_subtitle (uuid, media_uuid, name, track_type, version, is_active, created_at, updated_at) VALUES (?1, ?2, ?3, 'stt', 1, 1, ?4, ?5)",
            rusqlite::params![subtitle_uuid, media_uuid, name, now, now],
        )
        .map_err(|e| e.to_string())?;

        // Insert version log entry
        let version_uuid = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO listen_subtitle_version (uuid, subtitle_uuid, version, change_type, description, cues_added, created_by) VALUES (?1, ?2, 1, 'initial', 'Imported from VTT', ?3, 'system')",
            rusqlite::params![version_uuid, subtitle_uuid, cues.len() as i64],
        )
        .map_err(|e| e.to_string())?;

        // Insert each cue (version 1)
        for (order, cue) in cues.iter().enumerate() {
            let cue_uuid = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO listen_subtitle_cue (uuid, subtitle_uuid, order_num, start_ms, end_ms, content, version_created) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)",
                rusqlite::params![cue_uuid, subtitle_uuid, order as i64, cue.start_ms, cue.end_ms, cue.content],
            )
            .map_err(|e| e.to_string())?;
        }

        written += 1;
    }

    drop(conn);

    // Update timestamp
    touch_info(&dataset_dir)?;

    log_lines.push(String::new());
    log_lines.push(format!("Wrote {} subtitle(s) to database.", written));
    if skipped > 0 {
        log_lines.push(format!("Skipped {} VTT file(s) without a matching media entry.", skipped));
    }

    log::info!("Wrote {} subtitle(s) to database for dataset '{}', skipped {}", written, uuid, skipped);
    Ok(log_lines.join("\n"))
}

/// Recursively walk `dir` collecting every `.vtt` file. `root` is the subtitle/
/// directory used to compute each file's relative stem (e.g. `a/b.vtt` → `a/b`).
/// Path separators are normalised to `/` to match the `source` field in the database.
#[allow(dead_code)]
pub(crate) fn collect_vtt_files(root: &Path, dir: &Path, out: &mut Vec<(String, std::path::PathBuf)>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_vtt_files(root, &path, out);
        } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("vtt")) {
            let rel_stem = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .with_extension("")
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel_stem, path));
        }
    }
}
