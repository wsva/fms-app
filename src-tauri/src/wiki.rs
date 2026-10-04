use serde::{Deserialize, Serialize};
use tauri::State;

use crate::settings::SettingsState;

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------
// These are shared between the local desktop implementation and the Android
// thin client, which deserializes exactly these shapes from the PC's
// `/api/v1/wiki/*` REST responses. They therefore stay feature-independent.

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WikiEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_linked: bool,
    pub modified: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WikiSearchResult {
    pub file_path: String,
    pub file_name: String,
    pub relative_path: String,
    pub snippet: String,
    pub rank: f64,
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------
// Every command dispatches by build feature:
//   * desktop  -> the local filesystem + SQLite implementation in `local_impl`;
//   * Android  -> the read/search commands proxy to the paired PC over REST
//                 (`crate::sync`), and the mutating commands are rejected so the
//                 wiki stays read-only on the phone (nothing is saved locally).

/// List top-level wiki roots (default wiki directory + linked directories).
#[tauri::command]
pub async fn wiki_list_dirs(
    state: State<'_, SettingsState>,
) -> Result<Vec<WikiEntry>, String> {
    #[cfg(feature = "desktop")]
    {
        local_impl::wiki_list_dirs_local(state.inner())
    }
    #[cfg(not(feature = "desktop"))]
    {
        crate::sync::wiki_remote_list_dirs(state.inner()).await
    }
}

/// List contents of a specific wiki directory.
#[tauri::command]
pub async fn wiki_list_dir(
    state: State<'_, SettingsState>,
    path: String,
) -> Result<Vec<WikiEntry>, String> {
    #[cfg(feature = "desktop")]
    {
        let _ = &state; // desktop listing is purely path-based
        local_impl::wiki_list_dir_local(&path)
    }
    #[cfg(not(feature = "desktop"))]
    {
        crate::sync::wiki_remote_list_dir(state.inner(), &path).await
    }
}

/// Read markdown file content.
#[tauri::command]
pub async fn wiki_read_file(
    state: State<'_, SettingsState>,
    path: String,
) -> Result<String, String> {
    #[cfg(feature = "desktop")]
    {
        let _ = &state; // desktop read is purely path-based
        local_impl::wiki_read_file_local(&path)
    }
    #[cfg(not(feature = "desktop"))]
    {
        crate::sync::wiki_remote_read_file(state.inner(), &path).await
    }
}

/// Write/create a markdown file at the specified path (desktop only).
#[tauri::command]
pub async fn wiki_write_file(
    state: State<'_, SettingsState>,
    path: String,
    content: String,
) -> Result<(), String> {
    #[cfg(feature = "desktop")]
    {
        let _ = &state;
        local_impl::wiki_write_file_local(&path, &content)
    }
    #[cfg(not(feature = "desktop"))]
    {
        let _ = (&state, &path, &content);
        Err("Wiki editing is available on the PC".to_string())
    }
}

/// Delete a markdown file, moving it to trash (desktop only).
#[tauri::command]
pub async fn wiki_delete_file(
    state: State<'_, SettingsState>,
    path: String,
) -> Result<(), String> {
    #[cfg(feature = "desktop")]
    {
        let _ = &state;
        local_impl::wiki_delete_file_local(&path)
    }
    #[cfg(not(feature = "desktop"))]
    {
        let _ = (&state, &path);
        Err("Wiki editing is available on the PC".to_string())
    }
}

/// Index all markdown files for search (desktop only; the PC indexes on demand).
#[tauri::command]
pub async fn wiki_index(state: State<'_, SettingsState>) -> Result<u32, String> {
    #[cfg(feature = "desktop")]
    {
        local_impl::wiki_index_local(state.inner())
    }
    #[cfg(not(feature = "desktop"))]
    {
        let _ = &state;
        Err("Wiki indexing is available on the PC".to_string())
    }
}

/// Search markdown files (local FTS on desktop, remote on Android).
#[tauri::command]
pub async fn wiki_search(
    state: State<'_, SettingsState>,
    keyword: String,
) -> Result<Vec<WikiSearchResult>, String> {
    #[cfg(feature = "desktop")]
    {
        local_impl::wiki_search_local(state.inner(), &keyword)
    }
    #[cfg(not(feature = "desktop"))]
    {
        crate::sync::wiki_remote_search(state.inner(), &keyword).await
    }
}

/// Add a linked directory to the wiki (desktop only).
#[tauri::command]
pub async fn wiki_add_dir(
    state: State<'_, SettingsState>,
    name: String,
    path: String,
) -> Result<(), String> {
    #[cfg(feature = "desktop")]
    {
        local_impl::wiki_add_dir_local(state.inner(), &name, &path)
    }
    #[cfg(not(feature = "desktop"))]
    {
        let _ = (&state, &name, &path);
        Err("Wiki directory management is available on the PC".to_string())
    }
}

/// Remove a linked directory from the wiki (desktop only).
#[tauri::command]
pub async fn wiki_remove_dir(
    state: State<'_, SettingsState>,
    path: String,
) -> Result<(), String> {
    #[cfg(feature = "desktop")]
    {
        local_impl::wiki_remove_dir_local(state.inner(), &path)
    }
    #[cfg(not(feature = "desktop"))]
    {
        let _ = (&state, &path);
        Err("Wiki directory management is available on the PC".to_string())
    }
}

// ---------------------------------------------------------------------------
// Desktop local implementation (filesystem + SQLite FTS).
//
// Compiled only under the `desktop` feature so none of its filesystem/DB code
// (or imports) is dead weight in the Android thin build. The REST handlers in
// `rest.rs` (also desktop-only) call these directly.
// ---------------------------------------------------------------------------
#[cfg(feature = "desktop")]
pub(crate) mod local_impl {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use rusqlite::{params, Connection};
    use serde::{Deserialize, Serialize};

    use crate::settings::SettingsState;
    use super::{WikiEntry, WikiSearchResult};

    #[derive(Clone, Serialize, Deserialize, Debug)]
    pub struct WikiLinkedDir {
        pub name: String,
        pub path: String,
    }

    #[derive(Clone, Serialize, Deserialize, Debug, Default)]
    struct WikiMeta {
        #[serde(default)]
        linked_dirs: Vec<WikiLinkedDir>,
    }

    // ---- Helpers --------------------------------------------------------

    fn get_modified_time(path: &Path) -> Option<String> {
        fs::metadata(path)
            .ok()?
            .modified()
            .ok()
            .map(|t| {
                let duration = t.duration_since(UNIX_EPOCH).unwrap_or_default();
                chrono::DateTime::from_timestamp(duration.as_secs() as i64, 0)
                    .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                    .unwrap_or_default()
            })
    }

    fn wiki_meta_path(wiki_dir: &str) -> PathBuf {
        PathBuf::from(wiki_dir).join("meta.json")
    }

    fn read_wiki_meta(wiki_dir: &str) -> WikiMeta {
        let meta_path = wiki_meta_path(wiki_dir);
        if !meta_path.exists() {
            return WikiMeta::default();
        }
        fs::read_to_string(&meta_path)
            .ok()
            .map(|data| data.trim_start_matches('\u{FEFF}').to_string())
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or_default()
    }

    fn write_wiki_meta(wiki_dir: &str, meta: &WikiMeta) -> Result<(), String> {
        let meta_path = wiki_meta_path(wiki_dir);
        let data = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
        fs::write(&meta_path, data).map_err(|e| e.to_string())
    }

    fn list_directory(dir_path: &Path) -> Result<Vec<WikiEntry>, String> {
        let entries = fs::read_dir(dir_path).map_err(|e| e.to_string())?;
        let mut result = Vec::new();

        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let metadata = entry.metadata().map_err(|e| e.to_string())?;

            // Skip hidden files and the wiki database
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') || name == "wiki.sqlite3" {
                    continue;
                }
            }

            let wiki_entry = WikiEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: path.to_string_lossy().into_owned(),
                is_dir: metadata.is_dir(),
                is_linked: false,
                modified: get_modified_time(&path),
            };
            result.push(wiki_entry);
        }

        // Sort: directories first, then files, both alphabetically
        result.sort_by(|a, b| {
            match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });

        Ok(result)
    }

    // ---- Database -------------------------------------------------------

    fn wiki_db_path(wiki_dir: &str) -> PathBuf {
        PathBuf::from(wiki_dir).join("wiki.sqlite3")
    }

    fn init_wiki_db(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(
            r#"
            -- File metadata
            CREATE TABLE IF NOT EXISTS wiki_files (
                path TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                relative_path TEXT NOT NULL,
                content TEXT,
                modified INTEGER,
                indexed_at INTEGER
            );

            -- Full-text search index
            CREATE VIRTUAL TABLE IF NOT EXISTS wiki_fts USING fts5(
                path,
                name,
                content,
                content='wiki_files',
                content_rowid='rowid'
            );

            -- Triggers to keep FTS in sync
            CREATE TRIGGER IF NOT EXISTS wiki_fts_update AFTER UPDATE ON wiki_files BEGIN
                DELETE FROM wiki_fts WHERE rowid = old.rowid;
                INSERT INTO wiki_fts (rowid, path, name, content) VALUES (new.rowid, new.path, new.name, new.content);
            END;

            CREATE TRIGGER IF NOT EXISTS wiki_fts_insert AFTER INSERT ON wiki_files BEGIN
                INSERT INTO wiki_fts (rowid, path, name, content) VALUES (new.rowid, new.path, new.name, new.content);
            END;

            CREATE TRIGGER IF NOT EXISTS wiki_fts_delete AFTER DELETE ON wiki_files BEGIN
                DELETE FROM wiki_fts WHERE rowid = old.rowid;
            END;
            "#,
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    fn open_wiki_db(wiki_dir: &str) -> Result<Connection, String> {
        let db_path = wiki_db_path(wiki_dir);

        // Ensure wiki directory exists
        fs::create_dir_all(wiki_dir).map_err(|e| e.to_string())?;

        let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
        init_wiki_db(&conn)?;
        Ok(conn)
    }

    fn collect_markdown_files(dir: &Path, wiki_root: &Path) -> Result<Vec<(PathBuf, String, String)>, String> {
        let mut files = Vec::new();
        let entries = fs::read_dir(dir).map_err(|e| e.to_string())?;

        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let metadata = entry.metadata().map_err(|e| e.to_string())?;

            // Skip hidden files and wiki database
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') || name == "wiki.sqlite3" {
                    continue;
                }
            }

            if metadata.is_dir() {
                files.extend(collect_markdown_files(&path, wiki_root)?);
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                let relative_path = path
                    .strip_prefix(wiki_root)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .into_owned();
                files.push((path.clone(), path.file_name().unwrap().to_string_lossy().into_owned(), relative_path));
            }
        }

        Ok(files)
    }

    /// Canonicalized absolute roots that wiki browsing and search may access:
    /// the default wiki directory plus every linked directory recorded in
    /// `meta.json`. The PC REST handlers use this to reject request paths that
    /// escape the wiki (path traversal), so a paired device cannot read
    /// arbitrary files on the PC.
    pub(crate) fn wiki_root_paths(settings: &SettingsState) -> Vec<PathBuf> {
        let wiki_dir = settings.wiki_dir().to_string_lossy().into_owned();
        let mut roots = Vec::new();
        if let Ok(p) = PathBuf::from(&wiki_dir).canonicalize() {
            roots.push(p);
        }
        for linked in read_wiki_meta(&wiki_dir).linked_dirs {
            if let Ok(p) = PathBuf::from(&linked.path).canonicalize() {
                roots.push(p);
            }
        }
        roots
    }

    // ---- Command implementations ---------------------------------------

    /// List top-level wiki roots (default wiki directory + linked directories).
    pub(crate) fn wiki_list_dirs_local(state: &SettingsState) -> Result<Vec<WikiEntry>, String> {
        let wiki_dir = state.wiki_dir().to_string_lossy().into_owned();

        let wiki_path = PathBuf::from(&wiki_dir);
        let mut entries = Vec::new();

        // Add default wiki directory as "wiki" entry
        if wiki_path.exists() && wiki_path.is_dir() {
            entries.push(WikiEntry {
                name: "wiki".to_string(),
                path: wiki_dir.clone(),
                is_dir: true,
                is_linked: false,
                modified: get_modified_time(&wiki_path),
            });
        }

        // Add linked directories
        let meta = read_wiki_meta(&wiki_dir);
        for linked in meta.linked_dirs {
            let linked_path = PathBuf::from(&linked.path);
            if linked_path.exists() && linked_path.is_dir() {
                entries.push(WikiEntry {
                    name: linked.name,
                    path: linked.path,
                    is_dir: true,
                    is_linked: true,
                    modified: get_modified_time(&linked_path),
                });
            }
        }

        // Sort alphabetically
        entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        Ok(entries)
    }

    /// List contents of a specific wiki directory.
    pub(crate) fn wiki_list_dir_local(path: &str) -> Result<Vec<WikiEntry>, String> {
        let dir_path = PathBuf::from(path);
        if !dir_path.exists() {
            return Err(format!("Directory does not exist: {}", path));
        }
        if !dir_path.is_dir() {
            return Err(format!("Path is not a directory: {}", path));
        }

        list_directory(&dir_path)
    }

    /// Read markdown file content.
    pub(crate) fn wiki_read_file_local(path: &str) -> Result<String, String> {
        let file_path = PathBuf::from(path);
        if !file_path.exists() {
            return Err(format!("File does not exist: {}", path));
        }
        if !file_path.is_file() {
            return Err(format!("Path is not a file: {}", path));
        }

        fs::read_to_string(&file_path).map_err(|e| e.to_string())
    }

    /// Write/create a markdown file at the specified path.
    pub(crate) fn wiki_write_file_local(path: &str, content: &str) -> Result<(), String> {
        let file_path = PathBuf::from(path);

        // Ensure parent directory exists
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {}", e))?;
        }

        fs::write(&file_path, content).map_err(|e| format!("Failed to write file: {}", e))?;
        log::info!("[Wiki] Wrote file: {}", path);
        Ok(())
    }

    /// Delete a markdown file (moves to trash).
    pub(crate) fn wiki_delete_file_local(path: &str) -> Result<(), String> {
        let file_path = PathBuf::from(path);
        if !file_path.exists() {
            return Err(format!("File does not exist: {}", path));
        }
        if !file_path.is_file() {
            return Err(format!("Path is not a file: {}", path));
        }

        // Move to trash instead of deleting
        let data_dir = crate::app_paths::data_subdir("trash");
        fs::create_dir_all(&data_dir).map_err(|e| format!("Failed to create trash directory: {}", e))?;

        let file_name = file_path
            .file_name()
            .ok_or("Invalid file name")?
            .to_string_lossy();
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let trash_name = format!("{}_{}", timestamp, file_name);
        let trash_path = data_dir.join(&trash_name);

        fs::rename(&file_path, &trash_path).map_err(|e| format!("Failed to move to trash: {}", e))?;
        log::info!("[Wiki] Deleted file (moved to trash): {} -> {}", path, trash_path.display());
        Ok(())
    }

    /// Add a linked directory to the wiki.
    pub(crate) fn wiki_add_dir_local(state: &SettingsState, name: &str, path: &str) -> Result<(), String> {
        let wiki_dir = state.wiki_dir().to_string_lossy().into_owned();

        // Verify the path exists and is a directory
        let target_path = PathBuf::from(path);
        if !target_path.exists() {
            return Err(format!("Directory does not exist: {}", path));
        }
        if !target_path.is_dir() {
            return Err(format!("Path is not a directory: {}", path));
        }

        let mut meta = read_wiki_meta(&wiki_dir);

        // Check if already linked
        if meta.linked_dirs.iter().any(|d| d.path == path) {
            return Err(format!("Directory already linked: {}", path));
        }

        meta.linked_dirs.push(WikiLinkedDir {
            name: name.to_string(),
            path: path.to_string(),
        });

        write_wiki_meta(&wiki_dir, &meta)?;
        log::info!("[Wiki] Linked directory '{}' -> {}", name, path);
        Ok(())
    }

    /// Remove a linked directory from the wiki.
    pub(crate) fn wiki_remove_dir_local(state: &SettingsState, path: &str) -> Result<(), String> {
        let wiki_dir = state.wiki_dir().to_string_lossy().into_owned();

        let mut meta = read_wiki_meta(&wiki_dir);
        let initial_len = meta.linked_dirs.len();
        meta.linked_dirs.retain(|d| d.path != path);

        if meta.linked_dirs.len() == initial_len {
            return Err(format!("Directory not found in linked directories: {}", path));
        }

        write_wiki_meta(&wiki_dir, &meta)?;
        log::info!("[Wiki] Unlinked directory: {}", path);
        Ok(())
    }

    /// Index all markdown files in the wiki directory and linked directories.
    pub(crate) fn wiki_index_local(state: &SettingsState) -> Result<u32, String> {
        let wiki_dir = state.wiki_dir().to_string_lossy().into_owned();

        let wiki_path = PathBuf::from(&wiki_dir);
        if !wiki_path.exists() {
            return Err(format!("Wiki directory does not exist: {}", wiki_dir));
        }

        let conn = open_wiki_db(&wiki_dir)?;

        // Collect roots: main wiki dir + linked dirs
        let mut roots: Vec<(PathBuf, PathBuf)> = vec![(wiki_path.clone(), wiki_path.clone())];

        let meta = read_wiki_meta(&wiki_dir);
        for linked in &meta.linked_dirs {
            let linked_path = PathBuf::from(&linked.path);
            if linked_path.exists() && linked_path.is_dir() {
                roots.push((linked_path.clone(), linked_path.clone()));
            }
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let mut count = 0u32;
        let mut all_files: Vec<(PathBuf, String, String)> = Vec::new();

        // Collect files from all roots
        for (dir, root) in &roots {
            let files = collect_markdown_files(dir, root)?;
            all_files.extend(files);
        }

        // Use transaction for bulk insert
        {
            let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;

            for (file_path, file_name, relative_path) in &all_files {
                let content = fs::read_to_string(file_path).unwrap_or_default();
                let modified = fs::metadata(file_path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64);

                let path_str = file_path.to_string_lossy();

                // Check if file already exists
                let exists: bool = tx
                    .query_row(
                        "SELECT COUNT(*) FROM wiki_files WHERE path = ?1",
                        params![path_str],
                        |row| row.get::<_, i64>(0),
                    )
                    .map(|n| n > 0)
                    .unwrap_or(false);

                if exists {
                    // Update existing entry
                    tx.execute(
                        "UPDATE wiki_files SET name = ?2, relative_path = ?3, content = ?4, modified = ?5, indexed_at = ?6 WHERE path = ?1",
                        params![path_str, file_name, relative_path, content, modified, now],
                    )
                    .map_err(|e| e.to_string())?;
                } else {
                    // Insert new entry
                    tx.execute(
                        "INSERT INTO wiki_files (path, name, relative_path, content, modified, indexed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![path_str, file_name, relative_path, content, modified, now],
                    )
                    .map_err(|e| e.to_string())?;
                }
                count += 1;
            }

            // Remove entries for files that no longer exist
            let current_paths: Vec<String> = all_files
                .iter()
                .map(|(p, _, _)| p.to_string_lossy().into_owned())
                .collect();

            if current_paths.is_empty() {
                tx.execute("DELETE FROM wiki_files", []).map_err(|e| e.to_string())?;
            } else {
                // Build a query to delete files not in the current list
                let placeholders: Vec<String> = current_paths.iter().map(|_| "?".to_string()).collect();
                let delete_query = format!(
                    "DELETE FROM wiki_files WHERE path NOT IN ({})",
                    placeholders.join(",")
                );
                let params: Vec<&dyn rusqlite::ToSql> = current_paths
                    .iter()
                    .map(|s| s as &dyn rusqlite::ToSql)
                    .collect();
                tx.execute(&delete_query, params.as_slice())
                    .map_err(|e| e.to_string())?;
            }

            tx.commit().map_err(|e| e.to_string())?;
        }

        log::info!("[Wiki] Indexed {} markdown files from {} roots", count, roots.len());
        Ok(count)
    }

    /// Search markdown files using FTS5, indexing first if the DB is empty.
    pub(crate) fn wiki_search_local(state: &SettingsState, keyword: &str) -> Result<Vec<WikiSearchResult>, String> {
        let wiki_dir = state.wiki_dir().to_string_lossy().into_owned();

        let wiki_path = PathBuf::from(&wiki_dir);
        if !wiki_path.exists() {
            return Ok(Vec::new());
        }

        // Open database (will be created if it doesn't exist)
        let conn = open_wiki_db(&wiki_dir)?;

        // Check if database has any content
        let file_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM wiki_files", [], |row| row.get(0))
            .unwrap_or(0);

        // If no files indexed yet, run indexing first
        if file_count == 0 {
            drop(conn);
            wiki_index_local(state)?;
            // Reopen database after indexing
            return wiki_search_inner(&wiki_dir, keyword);
        }

        wiki_search_inner(&wiki_dir, keyword)
    }

    fn wiki_search_inner(wiki_dir: &str, keyword: &str) -> Result<Vec<WikiSearchResult>, String> {
        let conn = open_wiki_db(wiki_dir)?;

        // Build FTS5 query from user input
        // - If query contains quotes, pass through (user wants phrase search)
        // - Single word: prefix matching (word*)
        // - Multiple words: AND search with prefix matching on each word
        // - FTS5 operators (OR, AND, NOT, NEAR) are preserved as-is
        let trimmed = keyword.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        let fts_query = if trimmed.contains('"') {
            // User included quotes, pass through as-is for phrase search
            trimmed.to_string()
        } else {
            // FTS5 reserved operators should not get prefix matching
            let fts_operators = ["OR", "AND", "NOT", "NEAR", "or", "and", "not", "near"];
            let words: Vec<String> = trimmed
                .split_whitespace()
                .filter(|w| !w.is_empty())
                .map(|w| {
                    if fts_operators.contains(&w) {
                        w.to_uppercase()
                    } else {
                        format!("{}*", w)
                    }
                })
                .collect();

            if words.is_empty() {
                return Ok(Vec::new());
            }

            // Clean up the query: remove trailing operators and consecutive operators
            let mut cleaned_words: Vec<String> = Vec::new();
            for word in &words {
                if fts_operators.contains(&word.as_str()) {
                    // Only add operator if there's already a search term and last item wasn't an operator
                    if !cleaned_words.is_empty() {
                        let last = cleaned_words.last().unwrap();
                        if !fts_operators.contains(&last.as_str()) {
                            cleaned_words.push(word.clone());
                        }
                    }
                } else {
                    cleaned_words.push(word.clone());
                }
            }

            // Remove trailing operator if present
            while let Some(last) = cleaned_words.last() {
                if fts_operators.contains(&last.as_str()) {
                    cleaned_words.pop();
                } else {
                    break;
                }
            }

            if cleaned_words.is_empty() {
                return Ok(Vec::new());
            }

            cleaned_words.join(" ")
        };

        let mut stmt = conn
            .prepare(
                r#"
                SELECT 
                    wf.path,
                    wf.name,
                    wf.relative_path,
                    snippet(wiki_fts, 2, '<mark>', '</mark>', '...', 20) as snippet,
                    bm25(wiki_fts) as rank
                FROM wiki_fts
                JOIN wiki_files wf ON wiki_fts.path = wf.path
                WHERE wiki_fts MATCH ?1
                ORDER BY bm25(wiki_fts)
                LIMIT 50
                "#,
            )
            .map_err(|e| e.to_string())?;

        let results = stmt
            .query_map(params![fts_query], |row| {
                Ok(WikiSearchResult {
                    file_path: row.get(0)?,
                    file_name: row.get(1)?,
                    relative_path: row.get(2)?,
                    snippet: row.get(3)?,
                    rank: row.get(4)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        Ok(results)
    }
}
