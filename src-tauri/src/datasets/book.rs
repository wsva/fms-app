use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use crate::datasets::{
    assert_uuid_free, dataset_roots, fork_base_to_overlay, is_under_download_root, move_to_trash,
    now_stamp, read_info, read_info_opt, write_info, DatasetInfo, DatasetType, FORMAT_BOOK,
    INFO_FILE,
};
use crate::settings::SettingsState;

// ============================================================
// Types
// ============================================================

/// A book in the reading library. Each book is a self-contained directory.
#[derive(Clone, Serialize, Deserialize)]
pub struct BookMeta {
    pub uuid: String,
    /// Display name — the `name` field of the dataset's `info.json`.
    pub name: String,
    /// Absolute filesystem path of the book directory.
    pub path: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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

// ============================================================
// Helpers
// ============================================================

/// Resolve all book root directories: default `<datasets_dir>/book` + linked dirs from meta.json.
fn book_roots(settings: &SettingsState) -> Vec<PathBuf> {
    dataset_roots(settings, DatasetType::Book)
}

/// Find a book directory by UUID across all configured locations.
fn find_book_dir(settings: &SettingsState, uuid: &str) -> Result<PathBuf, String> {
    for root in book_roots(settings) {
        if !root.exists() {
            continue;
        }
        let entries = fs::read_dir(&root).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || !path.join(INFO_FILE).exists() {
                continue;
            }
            if read_info_opt(&path).map(|i| i.uuid).as_deref() == Some(uuid) {
                return Ok(path);
            }
        }
    }
    Err(format!("Book with UUID {} not found", uuid))
}

/// Write-resolution: forks a read-only base under the download root into a
/// writable overlay (dataset-layers.md §6); reads keep using [`find_book_dir`].
fn find_book_dir_write(settings: &SettingsState, uuid: &str) -> Result<PathBuf, String> {
    let dir = find_book_dir(settings, uuid)?;
    if is_under_download_root(settings, &dir) {
        return fork_base_to_overlay(settings, &dir, DatasetType::Book);
    }
    Ok(dir)
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
    // §3.4: a per-dataset tombstone table (travels with snapshots) so a delete
    // is durable and can collide with a resurrected offline edit.
    crate::sync::change_log::ensure_tombstones(&conn)?;
    Ok(conn)
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

/// List all books in the reading library across all configured locations.
#[tauri::command]
pub async fn book_list(settings: State<'_, SettingsState>) -> Result<Vec<BookMeta>, String> {
    Ok(list_books(&settings))
}

/// Core book listing logic, reusable without Tauri `State` (e.g. web service).
pub(crate) fn list_books(settings: &SettingsState) -> Vec<BookMeta> {
    let roots = book_roots(settings);
    let mut books = Vec::new();
    for root in roots {
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
                if info.dataset_type != DatasetType::Book {
                    continue;
                }
                books.push(BookMeta {
                    uuid: info.uuid,
                    name: info.name,
                    path: path.to_string_lossy().into_owned(),
                    created_at: info.created_at,
                    updated_at: info.updated_at,
                });
            }
        }
    }
    books.sort_by(|a, b| a.name.cmp(&b.name));
    books
}

/// Create a new book directory with an initialized database.
#[tauri::command]
pub async fn book_create(
    settings: State<'_, SettingsState>,
    name: String,
) -> Result<BookMeta, String> {
    if name.trim().is_empty() {
        return Err("Book name is required".into());
    }
    let root = book_roots(&settings)
        .into_iter()
        .next()
        .ok_or_else(|| "No book location configured".to_string())?;
    fs::create_dir_all(&root).map_err(|e| format!("Failed to create books dir: {}", e))?;

    let uuid = Uuid::new_v4().to_string();
    assert_uuid_free(&settings, &uuid)?;
    let base_name = sanitize_dir_name(&name);

    let mut dst = root.join(&base_name);
    if dst.exists() {
        dst = root.join(format!("{}-{}", base_name, &uuid[..8]));
    }

    fs::create_dir_all(dst.join("media")).map_err(|e| e.to_string())?;

    let info = DatasetInfo::new(uuid.clone(), DatasetType::Book, FORMAT_BOOK, name.clone());
    write_info(&dst, &info)?;

    // Initialize the database schema.
    open_book_db(&dst)?;

    Ok(BookMeta {
        uuid,
        name,
        path: dst.to_string_lossy().into_owned(),
        created_at: info.created_at.clone(),
        updated_at: info.updated_at,
    })
}

/// Rename a book (updates info.json `name`; directory name is unchanged).
#[tauri::command]
pub async fn book_rename(
    settings: State<'_, SettingsState>,
    uuid: String,
    name: String,
) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Book name is required".into());
    }
    let dir = find_book_dir_write(&settings, &uuid)?;
    let mut info = read_info(&dir)?;
    info.name = name;
    info.updated_at = now_stamp();
    write_info(&dir, &info)
}

/// Delete a book: its directory moves into the shared trash instead of being
/// destroyed, so the operation stays undoable (see [`move_to_trash`]). Returns the
/// trash path.
#[tauri::command]
pub async fn book_delete(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<String, String> {
    let dir = find_book_dir_write(&settings, &uuid)?;
    log::info!("[Book] Deleting book at '{}'", dir.display());
    let trashed = move_to_trash(&dir)?;
    log::info!("[Book] Book {} moved to trash '{}'", uuid, trashed.display());
    Ok(trashed.to_string_lossy().into_owned())
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
    list_chapters(&settings, &book_uuid)
}

/// Core chapter listing logic, reusable without Tauri `State` (e.g. web service).
pub(crate) fn list_chapters(
    settings: &SettingsState,
    book_uuid: &str,
) -> Result<Vec<BookChapter>, String> {
    let dir = find_book_dir(settings, book_uuid)?;
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

/// Read all chapter rows of an open book db (shared by the list command and
/// the sync, which needs the pre-image to preserve status/created_at).
pub(crate) fn query_chapters(conn: &Connection) -> Result<Vec<BookChapter>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT uuid, book_uuid, parent_uuid, order_num, title, status, created_at, updated_at \
             FROM book_chapter ORDER BY order_num",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
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
    Ok(rows)
}

/// Insert or update a chapter.
#[tauri::command]
pub async fn book_save_chapter(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    chapter: BookChapter,
) -> Result<(), String> {
    let dir = find_book_dir_write(&settings, &book_uuid)?;
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
    {
        let _ = crate::sync::change_log::commit_change(
            &settings,
            "book_chapter_save",
            &book_uuid,
            &serde_json::to_value(&chapter).unwrap_or_default(),
        );
    }
    Ok(())
}

/// Delete a chapter (and its sentences/words).
#[tauri::command]
pub async fn book_delete_chapter(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    uuid: String,
) -> Result<(), String> {
    let dir = find_book_dir_write(&settings, &book_uuid)?;
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
    {
        let _ = crate::sync::change_log::commit_change(
            &settings,
            "book_chapter_delete",
            &book_uuid,
            &serde_json::json!({ "uuid": uuid }),
        );
    }
    Ok(())
}

// ============================================================
// Chapter structure sync (round-trip Markdown editor)
// ============================================================

/// One chapter line coming from the Markdown editor. Carries no timestamps or
/// status: the sync preserves `status`/`created_at` per existing UUID, so text
/// edits can never clobber completion state or row provenance.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncNode {
    pub uuid: String,
    pub parent_uuid: Option<String>,
    pub order_num: i64,
    pub title: String,
}

/// A deletion root and the size of the subtree it takes with it.
#[derive(Clone, Debug, Serialize)]
pub struct DeleteImpact {
    pub uuid: String,
    pub title: String,
    /// Chapters removed in total (including the root itself).
    pub chapters: usize,
    /// Sentences removed with them.
    pub sentences: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChapterSyncSummary {
    pub created: usize,
    pub updated: usize,
    pub moved: usize,
    pub reordered: usize,
    pub deleted: usize,
    pub deletions: Vec<DeleteImpact>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SyncResult {
    pub dry_run: bool,
    pub summary: ChapterSyncSummary,
    /// Current tree on dry-run; the reconciled tree after a real apply.
    pub chapters: Vec<BookChapter>,
}

/// Recursively delete a chapter subtree (words → sentences → chapter), leaves
/// first. Used inside the sync transaction; change-log rows are written by the
/// caller once the transaction commits.
fn delete_chapter_subtree_tx(tx: &Connection, uuid: &str) -> Result<Vec<String>, String> {
    let children: Vec<String> = tx
        .prepare("SELECT uuid FROM book_chapter WHERE parent_uuid = ?1")
        .map_err(|e| e.to_string())?
        .query_map([uuid], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    let mut removed = Vec::new();
    for child in children {
        removed.extend(delete_chapter_subtree_tx(tx, &child)?);
    }
    removed.push(uuid.to_string());
    tx.execute(
        "DELETE FROM book_sentence_word WHERE sentence_uuid IN \
         (SELECT uuid FROM book_sentence WHERE chapter_uuid = ?1)",
        [uuid],
    )
    .map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM book_sentence WHERE chapter_uuid = ?1", [uuid])
        .map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM book_chapter WHERE uuid = ?1", [uuid])
        .map_err(|e| e.to_string())?;
    Ok(removed)
}

/// Reconcile a book's chapter tree against `nodes` (the full desired forest,
/// in document order). Embedded UUIDs are identity: a node matching an existing
/// chapter updates it in place (uuid, status and created_at survive), a node
/// with an unknown uuid is created, and an existing chapter whose uuid is
/// absent is deleted together with its whole subtree and its sentences.
/// With `dry_run` nothing is written and the summary describes what *would*
/// happen — this is what the editor's Preview calls.
pub(crate) fn sync_chapters(
    settings: &SettingsState,
    book_uuid: &str,
    nodes: Vec<SyncNode>,
    dry_run: bool,
) -> Result<SyncResult, String> {
    let dir = find_book_dir_write(settings, book_uuid)?;
    let mut conn = open_book_db(&dir)?;

    let existing: Vec<BookChapter> = query_chapters(&conn)?;
    let existing_map: std::collections::HashMap<String, &BookChapter> =
        existing.iter().map(|c| (c.uuid.clone(), c)).collect();
    let incoming_uuids: std::collections::HashSet<String> =
        nodes.iter().map(|n| n.uuid.clone()).collect();

    // Validate: unique uuids, and every parent reference resolves inside the
    // batch (chapters being deleted cannot parent anything).
    let mut seen = std::collections::HashSet::new();
    for n in &nodes {
        if !seen.insert(n.uuid.as_str()) {
            return Err(format!("duplicate chapter uuid in submission: {}", n.uuid));
        }
        if let Some(p) = &n.parent_uuid {
            if !incoming_uuids.contains(p) {
                return Err(format!("chapter \"{}\" references unknown parent", n.title));
            }
        }
    }

    // Deletion roots: existing rows whose uuid was dropped from the document.
    // children_of only needs edges from dropped parents — a child of a kept
    // parent is itself kept (it was in the batch) or belongs to a dropped
    // subtree and is reached through its own ancestor chain.
    let mut children_of: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for c in &existing {
        if let Some(p) = &c.parent_uuid {
            if !incoming_uuids.contains(p) {
                children_of.entry(p.clone()).or_default().push(c.uuid.clone());
            }
        }
    }
    let mut roots: Vec<String> = existing
        .iter()
        .filter(|c| !incoming_uuids.contains(&c.uuid))
        .map(|c| c.uuid.clone())
        .collect();
    let in_roots: std::collections::HashSet<String> = roots.iter().cloned().collect();
    for c in &existing {
        if incoming_uuids.contains(&c.uuid) {
            if let Some(p) = &c.parent_uuid {
                if in_roots.contains(p) {
                    return Err(format!(
                        "chapter \"{}\" is kept but its parent is being deleted",
                        c.title
                    ));
                }
            }
        }
    }
    // Expand each deletion root's subtree, excluding descendants that are
    // themselves deletion roots (they are listed separately).
    let sentence_count = |uuid: &str, conn: &Connection| -> Result<usize, String> {
        conn.query_row(
            "SELECT COUNT(*) FROM book_sentence WHERE chapter_uuid = ?1",
            [uuid],
            |r| r.get::<_, i64>(0),
        )
        .map(|v| v as usize)
        .map_err(|e| e.to_string())
    };
    let mut deletions: Vec<DeleteImpact> = Vec::new();
    for root in &roots {
        let mut stack = vec![root.clone()];
        let mut all: Vec<String> = Vec::new();
        while let Some(u) = stack.pop() {
            all.push(u.clone());
            for ch in children_of.get(&u).cloned().unwrap_or_default() {
                stack.push(ch);
            }
        }
        let own: Vec<String> = all
            .iter()
            .filter(|u| *u != root && !in_roots.contains(u.as_str()))
            .cloned()
            .collect();
        let mut sentences = sentence_count(root, &conn)?;
        for u in &own {
            sentences += sentence_count(u, &conn)?;
        }
        let title = existing_map
            .get(root)
            .map(|c| c.title.clone())
            .unwrap_or_default();
        deletions.push(DeleteImpact {
            uuid: root.clone(),
            title,
            chapters: own.len() + 1,
            sentences,
        });
    }
    // Drop non-minimal roots: a root whose ancestor chain contains another root
    // would double-count; keep only the topmost root per branch.
    {
        let mut covered: std::collections::HashSet<String> = std::collections::HashSet::new();
        for d in &deletions {
            let mut stack = vec![d.uuid.clone()];
            while let Some(u) = stack.pop() {
                for ch in children_of.get(&u).cloned().unwrap_or_default() {
                    if ch != d.uuid {
                        covered.insert(ch.clone());
                        stack.push(ch);
                    }
                }
            }
        }
        deletions.retain(|d| !covered.contains(&d.uuid));
        roots.retain(|r| !covered.contains(r));
        deletions.sort_by_key(|d| d.title.clone());
    }
    let deleted = roots.len();

    // Classify incoming nodes.
    let mut summary = ChapterSyncSummary {
        created: 0,
        updated: 0,
        moved: 0,
        reordered: 0,
        deleted,
        deletions,
    };
    for n in &nodes {
        match existing_map.get(&n.uuid) {
            None => summary.created += 1,
            Some(c) => {
                if c.title != n.title {
                    summary.updated += 1;
                }
                let parent_changed = c.parent_uuid.as_deref().unwrap_or("")
                    != n.parent_uuid.as_deref().unwrap_or("");
                if parent_changed {
                    summary.moved += 1;
                } else if c.order_num != n.order_num {
                    summary.reordered += 1;
                }
            }
        }
    }

    if dry_run {
        return Ok(SyncResult {
            dry_run: true,
            summary,
            chapters: existing,
        });
    }

    let deleted_uuids: Vec<String>;
    {
        let mut tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut deleted: Vec<String> = Vec::new();
        for root in &roots {
            deleted.extend(delete_chapter_subtree_tx(&tx, root)?);
        }
        let now = now_stamp();
        for n in &nodes {
            let (status, created_at) = match existing_map.get(&n.uuid) {
                Some(c) => (c.status.clone(), c.created_at.clone()),
                None => (None, now.clone()),
            };
            tx.execute(
                "INSERT OR REPLACE INTO book_chapter \
                 (uuid, book_uuid, parent_uuid, order_num, title, status, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    n.uuid,
                    book_uuid,
                    n.parent_uuid,
                    n.order_num,
                    n.title,
                    status,
                    created_at,
                    now
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        deleted_uuids = deleted;
    }
    for uuid in &deleted_uuids {
        let _ = crate::sync::change_log::commit_change(
            settings,
            "book_chapter_delete",
            book_uuid,
            &serde_json::json!({ "uuid": uuid }),
        );
    }
    for n in &nodes {
        let _ = crate::sync::change_log::commit_change(
            settings,
            "book_chapter_save",
            book_uuid,
            &serde_json::to_value(n).unwrap_or_default(),
        );
    }

    let chapters = query_chapters(&conn)?;
    Ok(SyncResult {
        dry_run: false,
        summary,
        chapters,
    })
}

/// Reconcile the whole chapter tree of a book in one atomic pass (the
/// round-trip Markdown editor's Apply/Preview backend).
#[tauri::command]
pub async fn book_sync_chapters(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    nodes: Vec<SyncNode>,
    dry_run: bool,
) -> Result<SyncResult, String> {
    sync_chapters(&settings, &book_uuid, nodes, dry_run)
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
    list_sentences(&settings, &book_uuid, &chapter_uuid)
}

/// Core sentence listing logic, reusable without Tauri `State` (e.g. web service).
pub(crate) fn list_sentences(
    settings: &SettingsState,
    book_uuid: &str,
    chapter_uuid: &str,
) -> Result<Vec<BookSentence>, String> {
    let dir = find_book_dir(settings, book_uuid)?;
    let conn = open_book_db(&dir)?;
    let mut stmt = conn
        .prepare(
            "SELECT uuid, chapter_uuid, user_id, order_num, content, sentence_type, \
             audio_path, recognized, bg_color, created_at, updated_at \
             FROM book_sentence WHERE chapter_uuid = ?1 ORDER BY order_num",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([chapter_uuid], |row| {
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
    let dir = find_book_dir_write(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    save_sentence_row(&conn, &sentence)?;
    {
        let _ = crate::sync::change_log::commit_change(
            &settings,
            "book_sentence_save",
            &book_uuid,
            &serde_json::to_value(&sentence).unwrap_or_default(),
        );
    }
    Ok(())
}

/// Insert or update many sentences in a transaction (used for reordering).
#[tauri::command]
pub async fn book_save_sentences(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    sentences: Vec<BookSentence>,
) -> Result<(), String> {
    let dir = find_book_dir_write(&settings, &book_uuid)?;
    let mut conn = open_book_db(&dir)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for s in &sentences {
        save_sentence_row(&tx, s)?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    {
        let _ = crate::sync::change_log::commit_change(
            &settings,
            "book_sentences_save",
            &book_uuid,
            &serde_json::to_value(&sentences).unwrap_or_default(),
        );
    }
    Ok(())
}

/// Delete a sentence (and its words + audio file).
#[tauri::command]
pub async fn book_delete_sentence(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    uuid: String,
) -> Result<(), String> {
    let dir = find_book_dir_write(&settings, &book_uuid)?;
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
    {
        let _ = crate::sync::change_log::commit_change(
            &settings,
            "book_sentence_delete",
            &book_uuid,
            &serde_json::json!({ "uuid": uuid }),
        );
    }
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
    let dir = find_book_dir_write(&settings, &book_uuid)?;
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
    {
        let _ = crate::sync::change_log::commit_change(
            &settings,
            "book_word_save",
            &book_uuid,
            &serde_json::to_value(&word).unwrap_or_default(),
        );
    }
    Ok(())
}

/// Delete a vocabulary word.
#[tauri::command]
pub async fn book_delete_word(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    uuid: String,
) -> Result<(), String> {
    let dir = find_book_dir_write(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    conn.execute("DELETE FROM book_sentence_word WHERE uuid = ?1", [&uuid])
        .map_err(|e| e.to_string())?;
    {
        let _ = crate::sync::change_log::commit_change(
            &settings,
            "book_word_delete",
            &book_uuid,
            &serde_json::json!({ "uuid": uuid }),
        );
    }
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
    let dir = find_book_dir_write(&settings, &book_uuid)?;
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
    let dir = find_book_dir_write(&settings, &book_uuid)?;
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
