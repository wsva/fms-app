use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use chrono::Utc;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use crate::settings::SettingsState;

// ============================================================
// Types
// ============================================================

/// A book in the reading library. Each book is a self-contained directory.
#[derive(Clone, Serialize, Deserialize)]
pub struct BookMeta {
    pub uuid: String,
    pub title: String,
    /// Absolute filesystem path of the book directory.
    pub path: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Contents of a book directory's `info.json`.
#[derive(Clone, Serialize, Deserialize)]
struct BookInfoFile {
    uuid: String,
    title: String,
    structure: String,
    created_at: String,
    updated_at: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BookChapter {
    pub uuid: String,
    pub book_uuid: String,
    pub parent_uuid: Option<String>,
    pub order_num: i64,
    pub title: String,
    pub status: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BookSentence {
    pub uuid: String,
    pub chapter_uuid: String,
    #[serde(default)]
    pub user_id: String,
    pub order_num: i64,
    pub content: String,
    pub sentence_type: String,
    /// Audio path relative to the book directory (e.g. `media/<uuid>.wav`).
    pub audio_path: Option<String>,
    /// Resolved absolute audio path for playback (computed on read, not stored).
    #[serde(default)]
    pub audio_url: Option<String>,
    pub recognized: Option<String>,
    pub bg_color: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BookSentenceWord {
    pub uuid: String,
    pub sentence_uuid: String,
    pub word: String,
    #[serde(default)]
    pub word_type: String,
    #[serde(default)]
    pub note: String,
}

/// Result of writing an audio file into a book's `media/` directory.
#[derive(Clone, Serialize)]
pub struct AudioWriteResult {
    /// File name only (e.g. `<uuid>.wav`).
    pub name: String,
    /// Path relative to the book directory (e.g. `media/<uuid>.wav`) — store this.
    pub rel_path: String,
    /// Absolute path — use for playback via convertFileSrc.
    pub abs_path: String,
}

const BOOK_STRUCTURE: &str = "reading-v1";

// ============================================================
// Helpers
// ============================================================

/// Root directory of the reading library.
/// Workspace-derived: `<workspace>/books` when a workspace is selected.
fn books_root(settings: &SettingsState) -> PathBuf {
    settings.books_dir()
}

/// Find a book directory by UUID.
fn find_book_dir(settings: &SettingsState, uuid: &str) -> Result<PathBuf, String> {
    let root = books_root(settings);
    if !root.exists() {
        return Err(format!("Book with UUID {} not found", uuid));
    }
    let entries = fs::read_dir(&root).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let info_path = path.join("info.json");
        if !info_path.exists() {
            continue;
        }
        let data = match fs::read_to_string(&info_path) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let info: BookInfoFile = match serde_json::from_str(&data) {
            Ok(i) => i,
            Err(_) => continue,
        };
        if info.uuid == uuid {
            return Ok(path);
        }
    }
    Err(format!("Book with UUID {} not found", uuid))
}

/// Open (and initialize) a book's SQLite database.
fn open_book_db(book_dir: &Path) -> Result<Connection, String> {
    fs::create_dir_all(book_dir).map_err(|e| e.to_string())?;
    let conn = Connection::open(book_dir.join("data.sqlite3")).map_err(|e| e.to_string())?;
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS book_chapter (
            uuid        TEXT PRIMARY KEY,
            book_uuid   TEXT NOT NULL,
            parent_uuid TEXT,
            order_num   INTEGER NOT NULL DEFAULT 0,
            title       TEXT NOT NULL DEFAULT '',
            status      TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS book_sentence (
            uuid          TEXT PRIMARY KEY,
            chapter_uuid  TEXT NOT NULL,
            user_id       TEXT NOT NULL DEFAULT '',
            order_num     INTEGER NOT NULL DEFAULT 0,
            content       TEXT NOT NULL DEFAULT '',
            sentence_type TEXT NOT NULL DEFAULT 'text',
            audio_path    TEXT,
            recognized    TEXT,
            bg_color      TEXT,
            created_at    TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS book_sentence_word (
            uuid          TEXT PRIMARY KEY,
            sentence_uuid TEXT NOT NULL,
            word          TEXT NOT NULL DEFAULT '',
            word_type     TEXT NOT NULL DEFAULT '',
            note          TEXT NOT NULL DEFAULT ''
        );
        ",
    )
    .map_err(|e| e.to_string())?;
    Ok(conn)
}

fn read_book_info(book_dir: &Path) -> Result<BookInfoFile, String> {
    let data = fs::read_to_string(book_dir.join("info.json")).map_err(|e| e.to_string())?;
    serde_json::from_str(&data).map_err(|e| e.to_string())
}

/// Resolve a stored relative audio path to an absolute path for playback.
fn resolve_audio_url(book_dir: &Path, audio_path: &Option<String>) -> Option<String> {
    audio_path
        .as_ref()
        .filter(|p| !p.is_empty())
        .map(|rel| book_dir.join(rel).to_string_lossy().into_owned())
}

/// Build a filesystem-safe directory name from a title.
fn sanitize_dir_name(title: &str) -> String {
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
        "book".to_string()
    } else {
        trimmed.to_string()
    }
}

// ============================================================
// Book commands
// ============================================================

/// List all books in the reading library.
#[tauri::command]
pub async fn book_list(settings: State<'_, SettingsState>) -> Result<Vec<BookMeta>, String> {
    let root = books_root(&settings);
    let mut books = Vec::new();
    if !root.exists() {
        return Ok(books);
    }
    let entries = fs::read_dir(&root).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || !path.join("info.json").exists() {
            continue;
        }
        if let Ok(info) = read_book_info(&path) {
            if info.structure != BOOK_STRUCTURE {
                continue;
            }
            books.push(BookMeta {
                uuid: info.uuid,
                title: info.title,
                path: path.to_string_lossy().into_owned(),
                created_at: info.created_at,
                updated_at: info.updated_at,
            });
        }
    }
    books.sort_by(|a, b| a.title.cmp(&b.title));
    Ok(books)
}

/// Create a new book directory with an initialized database.
#[tauri::command]
pub async fn book_create(
    settings: State<'_, SettingsState>,
    title: String,
) -> Result<BookMeta, String> {
    if title.trim().is_empty() {
        return Err("Title is required".into());
    }
    let root = books_root(&settings);
    fs::create_dir_all(&root).map_err(|e| format!("Failed to create books dir: {}", e))?;

    let uuid = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let base_name = sanitize_dir_name(&title);

    let mut dst = root.join(&base_name);
    if dst.exists() {
        dst = root.join(format!("{}-{}", base_name, &uuid[..8]));
    }

    fs::create_dir_all(dst.join("media")).map_err(|e| e.to_string())?;

    let info = BookInfoFile {
        uuid: uuid.clone(),
        title: title.clone(),
        structure: BOOK_STRUCTURE.into(),
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    let data = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
    fs::write(dst.join("info.json"), data).map_err(|e| e.to_string())?;

    // Initialize the database schema.
    open_book_db(&dst)?;

    Ok(BookMeta {
        uuid,
        title,
        path: dst.to_string_lossy().into_owned(),
        created_at: now.clone(),
        updated_at: now,
    })
}

/// Rename a book (updates info.json title; directory name is unchanged).
#[tauri::command]
pub async fn book_rename(
    settings: State<'_, SettingsState>,
    uuid: String,
    title: String,
) -> Result<(), String> {
    if title.trim().is_empty() {
        return Err("Title is required".into());
    }
    let dir = find_book_dir(&settings, &uuid)?;
    let mut info = read_book_info(&dir)?;
    info.title = title;
    info.updated_at = Utc::now().to_rfc3339();
    let data = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
    fs::write(dir.join("info.json"), data).map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a book and its entire directory.
#[tauri::command]
pub async fn book_delete(settings: State<'_, SettingsState>, uuid: String) -> Result<(), String> {
    let dir = find_book_dir(&settings, &uuid)?;
    fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(())
}

// ============================================================
// Chapter commands
// ============================================================

/// List all chapters (flat) for a book.
#[tauri::command]
pub async fn book_list_chapters(
    settings: State<'_, SettingsState>,
    book_uuid: String,
) -> Result<Vec<BookChapter>, String> {
    let dir = find_book_dir(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    let mut stmt = conn
        .prepare(
            "SELECT uuid, book_uuid, parent_uuid, order_num, title, status, created_at, updated_at \
             FROM book_chapter ORDER BY order_num",
        )
        .map_err(|e| e.to_string())?;
    let items = stmt
        .query_map([], |row| {
            Ok(BookChapter {
                uuid: row.get(0)?,
                book_uuid: row.get(1)?,
                parent_uuid: row.get(2)?,
                order_num: row.get(3)?,
                title: row.get(4)?,
                status: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(items)
}

/// Insert or update a chapter.
#[tauri::command]
pub async fn book_save_chapter(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    chapter: BookChapter,
) -> Result<(), String> {
    let dir = find_book_dir(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    conn.execute(
        "INSERT OR REPLACE INTO book_chapter \
         (uuid, book_uuid, parent_uuid, order_num, title, status, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            chapter.uuid,
            chapter.book_uuid,
            chapter.parent_uuid,
            chapter.order_num,
            chapter.title,
            chapter.status,
            chapter.created_at,
            chapter.updated_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a chapter (and its sentences/words).
#[tauri::command]
pub async fn book_delete_chapter(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    uuid: String,
) -> Result<(), String> {
    let dir = find_book_dir(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    // Delete words belonging to sentences of this chapter, then sentences, then chapter.
    conn.execute(
        "DELETE FROM book_sentence_word WHERE sentence_uuid IN \
         (SELECT uuid FROM book_sentence WHERE chapter_uuid = ?1)",
        [&uuid],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM book_sentence WHERE chapter_uuid = ?1",
        [&uuid],
    )
    .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM book_chapter WHERE uuid = ?1", [&uuid])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ============================================================
// Sentence commands
// ============================================================

/// List all sentences for a chapter, ordered, with resolved audio URLs.
#[tauri::command]
pub async fn book_list_sentences(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    chapter_uuid: String,
) -> Result<Vec<BookSentence>, String> {
    let dir = find_book_dir(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    let mut stmt = conn
        .prepare(
            "SELECT uuid, chapter_uuid, user_id, order_num, content, sentence_type, \
             audio_path, recognized, bg_color, created_at, updated_at \
             FROM book_sentence WHERE chapter_uuid = ?1 ORDER BY order_num",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([&chapter_uuid], |row| {
            let audio_path: Option<String> = row.get(6)?;
            Ok(BookSentence {
                uuid: row.get(0)?,
                chapter_uuid: row.get(1)?,
                user_id: row.get(2)?,
                order_num: row.get(3)?,
                content: row.get(4)?,
                sentence_type: row.get(5)?,
                audio_url: None,
                audio_path,
                recognized: row.get(7)?,
                bg_color: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut items: Vec<BookSentence> = Vec::new();
    for row in rows {
        let mut s = row.map_err(|e| e.to_string())?;
        s.audio_url = resolve_audio_url(&dir, &s.audio_path);
        items.push(s);
    }
    Ok(items)
}

fn save_sentence_row(conn: &Connection, s: &BookSentence) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO book_sentence \
         (uuid, chapter_uuid, user_id, order_num, content, sentence_type, \
          audio_path, recognized, bg_color, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            s.uuid,
            s.chapter_uuid,
            s.user_id,
            s.order_num,
            s.content,
            s.sentence_type,
            s.audio_path,
            s.recognized,
            s.bg_color,
            s.created_at,
            s.updated_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Insert or update a single sentence.
#[tauri::command]
pub async fn book_save_sentence(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    sentence: BookSentence,
) -> Result<(), String> {
    let dir = find_book_dir(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    save_sentence_row(&conn, &sentence)
}

/// Insert or update many sentences in a transaction (used for reordering).
#[tauri::command]
pub async fn book_save_sentences(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    sentences: Vec<BookSentence>,
) -> Result<(), String> {
    let dir = find_book_dir(&settings, &book_uuid)?;
    let mut conn = open_book_db(&dir)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for s in &sentences {
        save_sentence_row(&tx, s)?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a sentence (and its words + audio file).
#[tauri::command]
pub async fn book_delete_sentence(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    uuid: String,
) -> Result<(), String> {
    let dir = find_book_dir(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    // Best-effort audio file removal.
    if let Ok(audio_path) = conn.query_row(
        "SELECT audio_path FROM book_sentence WHERE uuid = ?1",
        [&uuid],
        |row| row.get::<_, Option<String>>(0),
    ) {
        if let Some(rel) = audio_path {
            let _ = fs::remove_file(dir.join(rel));
        }
    }
    conn.execute(
        "DELETE FROM book_sentence_word WHERE sentence_uuid = ?1",
        [&uuid],
    )
    .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM book_sentence WHERE uuid = ?1", [&uuid])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ============================================================
// Word commands
// ============================================================

/// List vocabulary words for a sentence.
#[tauri::command]
pub async fn book_list_words(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    sentence_uuid: String,
) -> Result<Vec<BookSentenceWord>, String> {
    let dir = find_book_dir(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    let mut stmt = conn
        .prepare(
            "SELECT uuid, sentence_uuid, word, word_type, note \
             FROM book_sentence_word WHERE sentence_uuid = ?1",
        )
        .map_err(|e| e.to_string())?;
    let items = stmt
        .query_map([&sentence_uuid], |row| {
            Ok(BookSentenceWord {
                uuid: row.get(0)?,
                sentence_uuid: row.get(1)?,
                word: row.get(2)?,
                word_type: row.get(3)?,
                note: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(items)
}

/// Insert or update a vocabulary word.
#[tauri::command]
pub async fn book_save_word(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    word: BookSentenceWord,
) -> Result<(), String> {
    let dir = find_book_dir(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    conn.execute(
        "INSERT OR REPLACE INTO book_sentence_word (uuid, sentence_uuid, word, word_type, note) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            word.uuid,
            word.sentence_uuid,
            word.word,
            word.word_type,
            word.note
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a vocabulary word.
#[tauri::command]
pub async fn book_delete_word(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    uuid: String,
) -> Result<(), String> {
    let dir = find_book_dir(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    conn.execute("DELETE FROM book_sentence_word WHERE uuid = ?1", [&uuid])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ============================================================
// Audio commands
// ============================================================

/// Write base64-encoded WAV audio into a book's media directory.
#[tauri::command]
pub async fn book_write_audio(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    name: String,
    wav_base64: String,
) -> Result<AudioWriteResult, String> {
    let dir = find_book_dir(&settings, &book_uuid)?;
    let media_dir = dir.join("media");
    fs::create_dir_all(&media_dir).map_err(|e| e.to_string())?;

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&wav_base64)
        .map_err(|e| format!("Invalid base64: {}", e))?;

    let file_name = sanitize_dir_name(&name);
    fs::write(media_dir.join(&file_name), bytes).map_err(|e| e.to_string())?;

    let rel_path = format!("media/{}", file_name);
    let abs_path = dir.join(&rel_path).to_string_lossy().into_owned();
    Ok(AudioWriteResult {
        name: file_name,
        rel_path,
        abs_path,
    })
}

/// Import an existing audio file (copy) into a book's media directory.
#[tauri::command]
pub async fn book_import_audio(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    name: String,
    source_path: String,
) -> Result<AudioWriteResult, String> {
    let dir = find_book_dir(&settings, &book_uuid)?;
    let media_dir = dir.join("media");
    fs::create_dir_all(&media_dir).map_err(|e| e.to_string())?;

    let src = Path::new(&source_path);
    if !src.exists() {
        return Err("Source audio file does not exist".into());
    }

    // Preserve the source extension if the provided name has none.
    let file_name = sanitize_dir_name(&name);
    fs::copy(src, media_dir.join(&file_name)).map_err(|e| e.to_string())?;

    let rel_path = format!("media/{}", file_name);
    let abs_path = dir.join(&rel_path).to_string_lossy().into_owned();
    Ok(AudioWriteResult {
        name: file_name,
        rel_path,
        abs_path,
    })
}

/// Delete an audio file from a book's media directory.
#[tauri::command]
pub async fn book_delete_audio(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    rel_path: String,
) -> Result<(), String> {
    let dir = find_book_dir(&settings, &book_uuid)?;
    let path = dir.join(&rel_path);
    if path.exists() {
        fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}
