use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::settings::SettingsState;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WikiEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
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
// Helper functions
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Database functions
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// List contents of the wiki root directory
#[tauri::command]
pub async fn wiki_list_dirs(
    state: State<'_, SettingsState>,
) -> Result<Vec<WikiEntry>, String> {
    let wiki_dir = {
        let settings = state.settings.lock().unwrap();
        settings.wiki_dir.clone()
    };

    let wiki_path = PathBuf::from(&wiki_dir);
    if !wiki_path.exists() {
        return Ok(Vec::new());
    }

    list_directory(&wiki_path)
}

/// List contents of a specific wiki directory
#[tauri::command]
pub async fn wiki_list_dir(path: String) -> Result<Vec<WikiEntry>, String> {
    let dir_path = PathBuf::from(&path);
    if !dir_path.exists() {
        return Err(format!("Directory does not exist: {}", path));
    }
    if !dir_path.is_dir() {
        return Err(format!("Path is not a directory: {}", path));
    }

    list_directory(&dir_path)
}

/// Read markdown file content
#[tauri::command]
pub async fn wiki_read_file(path: String) -> Result<String, String> {
    let file_path = PathBuf::from(&path);
    if !file_path.exists() {
        return Err(format!("File does not exist: {}", path));
    }
    if !file_path.is_file() {
        return Err(format!("Path is not a file: {}", path));
    }

    fs::read_to_string(&file_path).map_err(|e| e.to_string())
}

/// Index all markdown files in the wiki directory
#[tauri::command]
pub async fn wiki_index(
    state: State<'_, SettingsState>,
) -> Result<u32, String> {
    let wiki_dir = {
        let settings = state.settings.lock().unwrap();
        settings.wiki_dir.clone()
    };

    let wiki_path = PathBuf::from(&wiki_dir);
    if !wiki_path.exists() {
        return Err(format!("Wiki directory does not exist: {}", wiki_dir));
    }

    let conn = open_wiki_db(&wiki_dir)?;

    // Collect all markdown files
    let files = collect_markdown_files(&wiki_path, &wiki_path)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let mut count = 0u32;

    // Use transaction for bulk insert
    {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;

        for (file_path, file_name, relative_path) in &files {
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
        let current_paths: Vec<String> = files
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

    log::info!("[Wiki] Indexed {} markdown files", count);
    Ok(count)
}

/// Search markdown files using FTS5
#[tauri::command]
pub async fn wiki_search(
    state: State<'_, SettingsState>,
    keyword: String,
) -> Result<Vec<WikiSearchResult>, String> {
    let wiki_dir = {
        let settings = state.settings.lock().unwrap();
        settings.wiki_dir.clone()
    };

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
        wiki_index(state.clone()).await?;
        // Reopen database after indexing
        return wiki_search_inner(&wiki_dir, &keyword);
    }

    wiki_search_inner(&wiki_dir, &keyword)
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
