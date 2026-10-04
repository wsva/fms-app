use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use crate::dataset::{dataset_roots, DatasetInfo};
use crate::settings::SettingsState;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Card dataset info.json — extends the base DatasetInfo with card-specific fields.
#[derive(Clone, Serialize, Deserialize)]
pub struct CardDatasetInfo {
    pub name: String,
    pub uuid: String,
    pub description: String,
    pub parent_uuid: String,
    pub version: u32,
    pub structure: String,
    pub updated: String,
    #[serde(default)]
    pub sync_url: String,
    #[serde(default = "default_visibility")]
    pub visibility: String,
    #[serde(default)]
    pub owner_id: String,
    #[serde(default)]
    pub subscribers: Vec<String>,
}

fn default_visibility() -> String {
    "private".to_string()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CardDatasetSummary {
    pub info: CardDatasetInfo,
    pub card_count: usize,
    pub path: String,
    pub location: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Card {
    pub uuid: String,
    pub question: String,
    #[serde(default)]
    pub suggestion: String,
    #[serde(default)]
    pub answer: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub familiarity: i32,
    pub question_hash: Option<String>,
    pub source_card_uuid: Option<String>,
    pub source_dataset_uuid: Option<String>,
    pub deleted_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct CardReview {
    pub uuid: String,
    pub card_uuid: String,
    #[serde(default)]
    pub familiarity: i32,
    #[serde(default)]
    pub interval_days: i32,
    #[serde(default = "default_ease_factor")]
    pub ease_factor: i32,
    #[serde(default)]
    pub repetitions: i32,
    pub last_review_at: Option<String>,
    pub next_review_at: Option<String>,
}

fn default_ease_factor() -> i32 {
    250
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Tag {
    pub uuid: String,
    pub name: String,
    pub color: Option<String>,
    pub deleted_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct SyncStatus {
    pub dataset_uuid: String,
    pub last_synced_at: String,
    pub clock_offset_ms: i32,
    pub pending_push: i32,
}

/// Filter for card listing.
#[derive(Clone, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum CardFilter {
    All,
    Normal,
    Easy,
    Incomplete,
}

impl Default for CardFilter {
    fn default() -> Self {
        Self::All
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Find a card dataset directory by UUID across all configured locations.
pub(crate) fn find_card_dataset_dir(settings: &SettingsState, uuid: &str) -> Result<PathBuf, String> {
    for root in dataset_roots(settings, crate::dataset::DatasetType::Card) {
        if !root.exists() {
            continue;
        }
        let entries = match fs::read_dir(&root) {
            Ok(e) => e,
            Err(_) => continue,
        };
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
            // Try parsing as CardDatasetInfo first (has extra fields)
            if let Ok(info) = serde_json::from_str::<CardDatasetInfo>(&data) {
                if info.structure == "cards-v1" && info.uuid == uuid {
                    return Ok(path);
                }
            }
            // Fall back to base DatasetInfo for structure check
            if let Ok(info) = serde_json::from_str::<DatasetInfo>(&data) {
                if info.structure == "cards-v1" && info.uuid == uuid {
                    return Ok(path);
                }
            }
        }
    }
    Err(format!("Card dataset with UUID {} not found", uuid))
}

/// Read card dataset info from a dataset directory.
fn read_card_dataset_info(dataset_dir: &Path) -> Result<CardDatasetInfo, String> {
    let info_path = dataset_dir.join("info.json");
    let data = fs::read_to_string(&info_path).map_err(|e| e.to_string())?;
    serde_json::from_str(&data).map_err(|e| e.to_string())
}

/// Open the card dataset SQLite database, ensuring the schema exists.
pub(crate) fn open_card_db(dataset_dir: &Path) -> Result<Connection, String> {
    let db_path = dataset_dir.join("data.sqlite3");
    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
    create_card_schema(&conn)?;
    Ok(conn)
}

/// Open or create the FTS search database for a location.
pub(crate) fn open_search_db(location: &Path) -> Result<Connection, String> {
    let db_path = location.join("search.sqlite3");
    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
    create_fts_schema(&conn)?;
    Ok(conn)
}

/// Create the FTS5 virtual table schema for card search.
fn create_fts_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        CREATE VIRTUAL TABLE IF NOT EXISTS card_fts USING fts5(
            dataset_uuid,
            card_uuid,
            dataset_name,
            question,
            answer,
            note,
            suggestion
        );
        ",
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Index a card in the FTS search database.
fn index_card_in_fts(
    conn: &Connection,
    dataset_uuid: &str,
    card_uuid: &str,
    dataset_name: &str,
    question: &str,
    answer: &str,
    note: &str,
    suggestion: &str,
) -> Result<(), String> {
    // Delete existing entry first (if any)
    conn.execute(
        "DELETE FROM card_fts WHERE dataset_uuid = ?1 AND card_uuid = ?2",
        rusqlite::params![dataset_uuid, card_uuid],
    )
    .map_err(|e| e.to_string())?;

    // Insert new entry
    conn.execute(
        "INSERT INTO card_fts (dataset_uuid, card_uuid, dataset_name, question, answer, note, suggestion) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![dataset_uuid, card_uuid, dataset_name, question, answer, note, suggestion],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Remove a card from the FTS search database.
fn remove_card_from_fts(conn: &Connection, dataset_uuid: &str, card_uuid: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM card_fts WHERE dataset_uuid = ?1 AND card_uuid = ?2",
        rusqlite::params![dataset_uuid, card_uuid],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Remove all cards from a dataset in the FTS search database.
fn remove_dataset_from_fts(conn: &Connection, dataset_uuid: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM card_fts WHERE dataset_uuid = ?1",
        rusqlite::params![dataset_uuid],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Search cards using FTS5 across all locations.
#[derive(Clone, Serialize, Deserialize)]
pub struct CardSearchResult {
    pub dataset_uuid: String,
    pub dataset_name: String,
    pub card_uuid: String,
    pub question: String,
    pub answer: String,
    pub note: String,
    pub suggestion: String,
    pub location: String,
}

/// Search mode for FTS5 queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchMode {
    /// Search only in questions
    Question,
    /// Search in all fields (question, answer, note, suggestion)
    FullText,
}

impl Default for SearchMode {
    fn default() -> Self {
        Self::FullText
    }
}

pub(crate) fn search_cards_fts(
    settings: &SettingsState,
    query: &str,
    mode: SearchMode,
) -> Result<Vec<CardSearchResult>, String> {
    let mut results = Vec::new();

    for root in dataset_roots(settings, crate::dataset::DatasetType::Card) {
        let search_db_path = root.join("search.sqlite3");
        if !search_db_path.exists() {
            continue;
        }

        let conn = match open_search_db(&root) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("[Cards] Failed to open search DB at {:?}: {}", root, e);
                continue;
            }
        };

        // Use FTS5 match syntax with column-specific search
        // Add prefix matching with * for better search results
        let escaped_query = query.replace("'", "''"); // Escape single quotes
        let fts_query = format!("{}*", escaped_query); // Add prefix matching
        let sql = match mode {
            SearchMode::Question => format!(
                "SELECT dataset_uuid, card_uuid, dataset_name, question, answer, note, suggestion 
                 FROM card_fts 
                 WHERE question MATCH '{}'
                 ORDER BY rank",
                fts_query
            ),
            SearchMode::FullText => format!(
                "SELECT dataset_uuid, card_uuid, dataset_name, question, answer, note, suggestion 
                 FROM card_fts 
                 WHERE card_fts MATCH '{}'
                 ORDER BY rank",
                fts_query
            ),
        };

        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("[Cards] FTS query failed: {}", e);
                continue;
            }
        };

        let rows = match stmt.query_map([], |row| {
            Ok(CardSearchResult {
                dataset_uuid: row.get(0)?,
                card_uuid: row.get(1)?,
                dataset_name: row.get(2)?,
                question: row.get(3)?,
                answer: row.get(4)?,
                note: row.get(5)?,
                suggestion: row.get(6)?,
                location: root.to_string_lossy().into_owned(),
            })
        }) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("[Cards] FTS query map failed: {}", e);
                continue;
            }
        };

        for row in rows {
            match row {
                Ok(result) => results.push(result),
                Err(e) => log::warn!("[Cards] Failed to read FTS result: {}", e),
            }
        }
    }

    Ok(results)
}

/// Rebuild the FTS index for a specific location or all locations.
pub(crate) fn rebuild_fts_index(
    settings: &SettingsState,
    location: Option<String>,
) -> Result<usize, String> {
    let mut total_indexed = 0;

    let roots = if let Some(loc) = location {
        vec![PathBuf::from(loc)]
    } else {
        dataset_roots(settings, crate::dataset::DatasetType::Card)
    };

    for root in roots {
        if !root.exists() {
            continue;
        }

        // Open search DB (creates if needed)
        let search_conn = open_search_db(&root)?;

        // Clear existing index for this location
        search_conn
            .execute("DELETE FROM card_fts", [])
            .map_err(|e| e.to_string())?;

        // Iterate through all datasets in this location
        let entries = match fs::read_dir(&root) {
            Ok(e) => e,
            Err(e) => {
                log::warn!("[Cards] Failed to read directory {:?}: {}", root, e);
                continue;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let info_path = path.join("info.json");
            if !info_path.exists() {
                continue;
            }

            // Read dataset info
            let data = match fs::read_to_string(&info_path) {
                Ok(d) => d,
                Err(_) => continue,
            };

            let info: CardDatasetInfo = match serde_json::from_str(&data) {
                Ok(i) => i,
                Err(_) => continue,
            };

            // Open dataset DB
            let card_conn = match open_card_db(&path) {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("[Cards] Failed to open card DB at {:?}: {}", path, e);
                    continue;
                }
            };

            // Index all cards in this dataset
            let mut stmt = match card_conn.prepare(
                "SELECT uuid, question, answer, note, suggestion FROM card WHERE deleted_at IS NULL",
            ) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("[Cards] Failed to prepare card query: {}", e);
                    continue;
                }
            };

            let cards = match stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            }) {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("[Cards] Failed to query cards: {}", e);
                    continue;
                }
            };

            for card in cards {
                match card {
                    Ok((uuid, question, answer, note, suggestion)) => {
                        if let Err(e) = index_card_in_fts(
                            &search_conn,
                            &info.uuid,
                            &uuid,
                            &info.name,
                            &question,
                            &answer,
                            &note,
                            &suggestion,
                        ) {
                            log::warn!("[Cards] Failed to index card {}: {}", uuid, e);
                        } else {
                            total_indexed += 1;
                        }
                    }
                    Err(e) => log::warn!("[Cards] Failed to read card: {}", e),
                }
            }
        }
    }

    Ok(total_indexed)
}

/// Create the card dataset SQLite schema.
fn create_card_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS card (
            uuid                  TEXT PRIMARY KEY,
            question              TEXT NOT NULL,
            suggestion            TEXT NOT NULL DEFAULT '',
            answer                TEXT NOT NULL DEFAULT '',
            note                  TEXT NOT NULL DEFAULT '',
            familiarity           INTEGER NOT NULL DEFAULT 0,
            question_hash         TEXT,
            source_card_uuid      TEXT,
            source_dataset_uuid   TEXT,
            deleted_at            TEXT,
            created_at            TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at            TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_card_hash ON card(question_hash);
        CREATE INDEX IF NOT EXISTS idx_card_familiarity ON card(familiarity);
        CREATE INDEX IF NOT EXISTS idx_card_deleted ON card(deleted_at);
        CREATE INDEX IF NOT EXISTS idx_card_source ON card(source_card_uuid);

        CREATE TABLE IF NOT EXISTS card_review (
            uuid           TEXT PRIMARY KEY,
            card_uuid      TEXT NOT NULL,
            familiarity    INTEGER NOT NULL DEFAULT 0,
            interval_days  INTEGER NOT NULL DEFAULT 0,
            ease_factor    INTEGER NOT NULL DEFAULT 250,
            repetitions    INTEGER NOT NULL DEFAULT 0,
            last_review_at TEXT,
            next_review_at TEXT,
            FOREIGN KEY (card_uuid) REFERENCES card(uuid) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_review_card ON card_review(card_uuid);
        CREATE INDEX IF NOT EXISTS idx_review_next ON card_review(next_review_at);

        CREATE TABLE IF NOT EXISTS tag (
            uuid         TEXT PRIMARY KEY,
            name         TEXT NOT NULL,
            color        TEXT,
            deleted_at   TEXT,
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS card_tag (
            uuid        TEXT PRIMARY KEY,
            card_uuid   TEXT NOT NULL,
            tag_uuid    TEXT NOT NULL,
            deleted_at  TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (card_uuid) REFERENCES card(uuid) ON DELETE CASCADE,
            FOREIGN KEY (tag_uuid) REFERENCES tag(uuid) ON DELETE CASCADE,
            UNIQUE(card_uuid, tag_uuid)
        );
        CREATE INDEX IF NOT EXISTS idx_card_tag_card ON card_tag(card_uuid);
        CREATE INDEX IF NOT EXISTS idx_card_tag_tag ON card_tag(tag_uuid);

        CREATE TABLE IF NOT EXISTS sync_state (
            dataset_uuid    TEXT PRIMARY KEY,
            last_synced_at  TEXT NOT NULL,
            clock_offset_ms INTEGER NOT NULL DEFAULT 0,
            pending_push    INTEGER NOT NULL DEFAULT 0
        );
        "
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Normalize a question string for hashing (same rules as online version).
fn normalize_question(question: &str) -> String {
    let mut q = question.trim().to_lowercase();
    // Collapse internal whitespace
    q = q.split_whitespace().collect::<Vec<_>>().join(" ");
    // Strip trailing punctuation
    q = q.trim_end_matches(|c: char| matches!(c, '?' | '!' | '.' | ',' | ';' | ':')).to_string();
    // Strip German article/reflexive prefix
    if let Some(rest) = q.strip_prefix("der ") {
        q = rest.to_string();
    } else if let Some(rest) = q.strip_prefix("die ") {
        q = rest.to_string();
    } else if let Some(rest) = q.strip_prefix("das ") {
        q = rest.to_string();
    } else if let Some(rest) = q.strip_prefix("ein ") {
        q = rest.to_string();
    } else if let Some(rest) = q.strip_prefix("eine ") {
        q = rest.to_string();
    } else if let Some(rest) = q.strip_prefix("sich ") {
        q = rest.to_string();
    }
    q
}

/// Compute SHA-256 hash of a normalized question string.
fn compute_question_hash(question: &str) -> String {
    use sha2::{Digest, Sha256};
    let normalized = normalize_question(question);
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// List all card datasets across every configured location.
pub(crate) fn list_card_datasets(settings: &SettingsState) -> Vec<CardDatasetSummary> {
    let roots = dataset_roots(settings, crate::dataset::DatasetType::Card);
    let mut datasets = Vec::new();

    for root in roots {
        if !root.exists() {
            continue;
        }
        let location = root.to_string_lossy().into_owned();
        let entries = match fs::read_dir(&root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let info_path = path.join("info.json");
            if !info_path.is_file() {
                continue;
            }
            let data = match fs::read_to_string(&info_path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let info: CardDatasetInfo = match serde_json::from_str(&data) {
                Ok(i) => i,
                Err(_) => continue,
            };
            if info.structure != "cards-v1" {
                continue;
            }

            // Count cards in database
            let card_count = {
                let db_path = path.join("data.sqlite3");
                if db_path.exists() {
                    Connection::open(&db_path)
                        .ok()
                        .and_then(|conn| {
                            conn.query_row(
                                "SELECT COUNT(*) FROM card WHERE deleted_at IS NULL",
                                [],
                                |row| row.get::<_, i64>(0),
                            )
                            .ok()
                            .map(|c| c as usize)
                        })
                        .unwrap_or(0)
                } else {
                    0
                }
            };

            datasets.push(CardDatasetSummary {
                info,
                card_count,
                path: path.to_string_lossy().into_owned(),
                location: location.clone(),
            });
        }
    }

    datasets.sort_by(|a, b| a.info.name.cmp(&b.info.name));
    datasets
}

// ---------------------------------------------------------------------------
// SM-2 Spaced Repetition Algorithm
// ---------------------------------------------------------------------------

/// SM-2 algorithm: compute the next review parameters based on quality rating.
///
/// Quality: 0-5 (0 = complete failure, 5 = perfect recall)
/// Returns: (interval_days, ease_factor, repetitions, familiarity)
fn sm2_next(
    quality: i32,
    repetitions: i32,
    interval_days: i32,
    ease_factor: i32,
) -> (i32, i32, i32, i32) {
    // Map quality to familiarity level (0-6)
    let familiarity = match quality {
        0..=1 => 1, // Still unfamiliar
        2 => 2,     // Passive understanding
        3 => 3,     // Recognition
        4 => 5,     // Active recall
        5 => 6,     // Natural usage
        _ => 0,
    };

    if quality < 3 {
        // Failed: reset repetitions and interval
        return (1, ease_factor.max(130), 0, familiarity);
    }

    let new_reps = repetitions + 1;
    let new_interval = match new_reps {
        1 => 1,
        2 => 6,
        _ => ((interval_days as f64) * (ease_factor as f64 / 100.0)).round() as i32,
    };

    // Update ease factor: EF' = EF + (0.1 - (5-q) * (0.08 + (5-q)*0.02))
    // Scaled by 100 since we store ease_factor as integer (250 = 2.5)
    let q = quality as f64;
    let ef_delta = (0.1 - (5.0 - q) * (0.08 + (5.0 - q) * 0.02)) * 100.0;
    let new_ef = (ease_factor as f64 + ef_delta).round() as i32;
    let new_ef = new_ef.max(130); // Minimum ease factor

    (new_interval, new_ef, new_reps, familiarity)
}

// ---------------------------------------------------------------------------
// Commands: Dataset Management
// ---------------------------------------------------------------------------

/// List all card datasets across all configured locations.
#[tauri::command]
pub async fn card_dataset_list(
    settings: State<'_, SettingsState>,
) -> Result<Vec<CardDatasetSummary>, String> {
    let datasets = list_card_datasets(&settings);
    log::info!("[Cards] Found {} card dataset(s)", datasets.len());
    Ok(datasets)
}

/// Create a new card dataset.
#[tauri::command]
pub async fn card_dataset_create(
    settings: State<'_, SettingsState>,
    name: String,
    description: Option<String>,
    location: Option<String>,
) -> Result<CardDatasetSummary, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Dataset name must not be empty".into());
    }

    let roots = dataset_roots(&settings, crate::dataset::DatasetType::Card);
    let root = match location {
        Some(loc) if !loc.trim().is_empty() => {
            let p = PathBuf::from(loc.trim());
            if !roots.iter().any(|r| *r == p) {
                return Err("Selected location is not a configured dataset location".into());
            }
            p
        }
        _ => roots
            .into_iter()
            .next()
            .unwrap_or_else(|| settings.datasets_dir()),
    };
    fs::create_dir_all(&root).map_err(|e| format!("Failed to create datasets dir: {}", e))?;

    let uuid = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();

    // Sanitize directory name
    let dir_name: String = trimmed
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect();
    let dir_name = dir_name.trim().trim_end_matches('.').to_string();
    let dir_name = if dir_name.is_empty() { uuid.clone() } else { dir_name };

    let dst = root.join(&dir_name);
    if dst.exists() {
        return Err(format!("Dataset '{}' already exists", dir_name));
    }

    fs::create_dir_all(&dst).map_err(|e| e.to_string())?;

    // Get current user for owner_id
    let owner_id = crate::auth::get_stored_user_id(&settings).unwrap_or_default();

    let info = CardDatasetInfo {
        name: trimmed.to_string(),
        uuid: uuid.clone(),
        description: description.unwrap_or_default(),
        parent_uuid: String::new(),
        version: 1,
        structure: "cards-v1".into(),
        updated: now.clone(),
        sync_url: String::new(),
        visibility: "private".to_string(),
        owner_id,
        subscribers: Vec::new(),
    };

    let data = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
    fs::write(dst.join("info.json"), &data).map_err(|e| e.to_string())?;

    // Create the database with schema
    let _conn = open_card_db(&dst)?;

    log::info!("[Cards] Created dataset '{}' at '{}'", trimmed, dst.display());
    Ok(CardDatasetSummary {
        info,
        card_count: 0,
        path: dst.to_string_lossy().into_owned(),
        location: root.to_string_lossy().into_owned(),
    })
}

/// Update card dataset metadata (name, description, sync_url, visibility).
#[tauri::command]
pub async fn card_dataset_update(
    settings: State<'_, SettingsState>,
    uuid: String,
    name: Option<String>,
    description: Option<String>,
    sync_url: Option<String>,
    visibility: Option<String>,
) -> Result<(), String> {
    let path = find_card_dataset_dir(&settings, &uuid)?;
    let info_path = path.join("info.json");

    let data = fs::read_to_string(&info_path).map_err(|e| e.to_string())?;
    let mut info: CardDatasetInfo = serde_json::from_str(&data).map_err(|e| e.to_string())?;

    if let Some(n) = name {
        info.name = n;
    }
    if let Some(d) = description {
        info.description = d;
    }
    if let Some(s) = sync_url {
        info.sync_url = s;
    }
    if let Some(v) = visibility {
        if !["private", "shared", "public"].contains(&v.as_str()) {
            return Err("Invalid visibility value. Must be: private, shared, or public".into());
        }
        info.visibility = v;
    }
    info.updated = Utc::now().to_rfc3339();

    let data = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
    fs::write(&info_path, data).map_err(|e| e.to_string())?;
    Ok(())
}

/// Add a subscriber to a card dataset.
#[tauri::command]
pub async fn card_dataset_add_subscriber(
    settings: State<'_, SettingsState>,
    uuid: String,
    email: String,
) -> Result<(), String> {
    let path = find_card_dataset_dir(&settings, &uuid)?;
    let info_path = path.join("info.json");

    let data = fs::read_to_string(&info_path).map_err(|e| e.to_string())?;
    let mut info: CardDatasetInfo = serde_json::from_str(&data).map_err(|e| e.to_string())?;

    if !info.subscribers.contains(&email) {
        info.subscribers.push(email.clone());
    }
    info.updated = Utc::now().to_rfc3339();

    let data = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
    fs::write(&info_path, data).map_err(|e| e.to_string())?;
    log::info!("[Cards] Added subscriber '{}' to dataset '{}'", email, uuid);
    Ok(())
}

/// Remove a subscriber from a card dataset.
#[tauri::command]
pub async fn card_dataset_remove_subscriber(
    settings: State<'_, SettingsState>,
    uuid: String,
    email: String,
) -> Result<(), String> {
    let path = find_card_dataset_dir(&settings, &uuid)?;
    let info_path = path.join("info.json");

    let data = fs::read_to_string(&info_path).map_err(|e| e.to_string())?;
    let mut info: CardDatasetInfo = serde_json::from_str(&data).map_err(|e| e.to_string())?;

    info.subscribers.retain(|s| s != &email);
    info.updated = Utc::now().to_rfc3339();

    let data = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
    fs::write(&info_path, data).map_err(|e| e.to_string())?;
    log::info!("[Cards] Removed subscriber '{}' from dataset '{}'", email, uuid);
    Ok(())
}

/// Delete a card dataset by removing its entire directory.
#[tauri::command]
pub async fn card_dataset_delete(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<(), String> {
    let path = find_card_dataset_dir(&settings, &uuid)?;
    log::info!("[Cards] Deleting dataset at '{}'", path.display());
    
    // Remove all cards in this dataset from the FTS index
    for root in dataset_roots(&settings, crate::dataset::DatasetType::Card) {
        if path.starts_with(&root) {
            if let Ok(search_conn) = open_search_db(&root) {
                let _ = remove_dataset_from_fts(&search_conn, &uuid);
            }
            break;
        }
    }
    
    fs::remove_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Move a card dataset to a different location (directory root).
/// The target_location must be one of the configured dataset roots.
#[tauri::command]
pub async fn card_dataset_move(
    settings: State<'_, SettingsState>,
    uuid: String,
    target_location: String,
) -> Result<String, String> {
    let source_path = find_card_dataset_dir(&settings, &uuid)?;
    let target_root = std::path::PathBuf::from(&target_location);

    if !target_root.exists() {
        return Err(format!("Target location does not exist: {}", target_location));
    }

    // Verify target is a known dataset root
    let roots = dataset_roots(&settings, crate::dataset::DatasetType::Card);
    let target_root_canonical = target_root.canonicalize().map_err(|e| e.to_string())?;
    let is_known_root = roots.iter().any(|r| {
        r.canonicalize().map(|c| c == target_root_canonical).unwrap_or(false)
    });
    if !is_known_root {
        return Err(format!("Target location is not a configured dataset root: {}", target_location));
    }

    // Read info.json to verify it's a valid dataset
    let info_path = source_path.join("info.json");
    if !info_path.exists() {
        return Err("Not a valid dataset directory (missing info.json)".into());
    }

    let dir_name = source_path.file_name()
        .ok_or("Cannot determine dataset directory name")?
        .to_string_lossy();
    let dest_path = target_root.join(dir_name.as_ref());

    if dest_path.exists() {
        return Err(format!("A dataset with the same directory already exists at: {}", dest_path.display()));
    }

    log::info!("[Cards] Moving dataset '{}' from '{}' to '{}'", uuid, source_path.display(), dest_path.display());
    fs::rename(&source_path, &dest_path).map_err(|e| e.to_string())?;

    Ok(dest_path.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// Commands: Card CRUD
// ---------------------------------------------------------------------------

/// List cards in a dataset with optional filtering.
#[tauri::command]
pub async fn card_list(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    filter: Option<CardFilter>,
    tag_uuid: Option<String>,
    keyword: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<(Vec<Card>, i64), String> {
    let path = find_card_dataset_dir(&settings, &dataset_uuid)?;
    let conn = open_card_db(&path)?;

    let filter = filter.unwrap_or(CardFilter::All);
    let limit = limit.unwrap_or(100);
    let offset = offset.unwrap_or(0);

    let mut conditions = vec!["c.deleted_at IS NULL".to_string()];
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    // Apply filter
    match filter {
        CardFilter::Normal => {
            conditions.push("length(c.question) > 0".into());
            conditions.push("length(c.answer) > 0".into());
            conditions.push("c.familiarity < 6".into());
        }
        CardFilter::Easy => {
            conditions.push("c.familiarity = 6".into());
        }
        CardFilter::Incomplete => {
            conditions.push("(length(c.question) = 0 OR length(c.answer) = 0)".into());
        }
        CardFilter::All => {}
    }

    // Apply tag filter
    if let Some(ref tag) = tag_uuid {
        if !tag.is_empty() {
            conditions.push(format!(
                "EXISTS (SELECT 1 FROM card_tag ct WHERE ct.card_uuid = c.uuid AND ct.tag_uuid = ?{} AND ct.deleted_at IS NULL)",
                params.len() + 1
            ));
            params.push(Box::new(tag.clone()));
        }
    }

    // Apply keyword filter
    if let Some(ref kw) = keyword {
        let kw = kw.trim();
        if !kw.is_empty() {
            let pattern = format!("%{}%", kw.to_lowercase());
            conditions.push(format!(
                "(lower(c.question) LIKE ?{} OR lower(c.answer) LIKE ?{})",
                params.len() + 1,
                params.len() + 2,
            ));
            params.push(Box::new(pattern.clone()));
            params.push(Box::new(pattern));
        }
    }

    let where_clause = conditions.join(" AND ");

    // Count query
    let count_sql = format!("SELECT COUNT(*) FROM card c WHERE {}", where_clause);
    let count_params: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let total: i64 = conn
        .query_row(&count_sql, count_params.as_slice(), |row| row.get(0))
        .map_err(|e| e.to_string())?;

    // Data query
    let data_sql = format!(
        "SELECT c.* FROM card c WHERE {} ORDER BY c.updated_at DESC LIMIT ?{} OFFSET ?{}",
        where_clause,
        params.len() + 1,
        params.len() + 2,
    );
    let mut data_params: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    data_params.push(&limit);
    data_params.push(&offset);

    let mut stmt = conn.prepare(&data_sql).map_err(|e| e.to_string())?;
    let cards = stmt
        .query_map(data_params.as_slice(), |row| {
            Ok(Card {
                uuid: row.get(0)?,
                question: row.get(1)?,
                suggestion: row.get(2)?,
                answer: row.get(3)?,
                note: row.get(4)?,
                familiarity: row.get(5)?,
                question_hash: row.get(6)?,
                source_card_uuid: row.get(7)?,
                source_dataset_uuid: row.get(8)?,
                deleted_at: row.get(9)?,
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok((cards, total))
}

/// Get a single card by UUID.
#[tauri::command]
pub async fn card_get(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    card_uuid: String,
) -> Result<Card, String> {
    let path = find_card_dataset_dir(&settings, &dataset_uuid)?;
    let conn = open_card_db(&path)?;

    conn.query_row(
        "SELECT * FROM card WHERE uuid = ?1",
        rusqlite::params![card_uuid],
        |row| {
            Ok(Card {
                uuid: row.get(0)?,
                question: row.get(1)?,
                suggestion: row.get(2)?,
                answer: row.get(3)?,
                note: row.get(4)?,
                familiarity: row.get(5)?,
                question_hash: row.get(6)?,
                source_card_uuid: row.get(7)?,
                source_dataset_uuid: row.get(8)?,
                deleted_at: row.get(9)?,
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        },
    )
    .map_err(|_| "Card not found".to_string())
}

/// Create or update a card. If uuid is empty, a new card is created.
#[tauri::command]
pub async fn card_save(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    card: Card,
) -> Result<Card, String> {
    let path = find_card_dataset_dir(&settings, &dataset_uuid)?;
    let conn = open_card_db(&path)?;
    let now = Utc::now().to_rfc3339();

    let uuid = if card.uuid.is_empty() {
        Uuid::new_v4().to_string()
    } else {
        card.uuid.clone()
    };

    let question_hash = if card.question.trim().is_empty() {
        None
    } else {
        Some(compute_question_hash(&card.question))
    };

    conn.execute(
        "INSERT INTO card (uuid, question, suggestion, answer, note, familiarity, \
         question_hash, source_card_uuid, source_dataset_uuid, deleted_at, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, ?11)
         ON CONFLICT(uuid) DO UPDATE SET
            question = ?2, suggestion = ?3, answer = ?4, note = ?5,
            familiarity = ?6, question_hash = ?7, source_card_uuid = ?8,
            source_dataset_uuid = ?9, updated_at = ?11",
        rusqlite::params![
            uuid,
            card.question,
            card.suggestion,
            card.answer,
            card.note,
            card.familiarity,
            question_hash,
            card.source_card_uuid,
            card.source_dataset_uuid,
            now,
            now,
        ],
    )
    .map_err(|e| e.to_string())?;

    // Update dataset timestamp
    update_dataset_timestamp(&path)?;

    // Update FTS index
    if let Ok(info) = read_card_dataset_info(&path) {
        // Find the location (root) for this dataset
        for root in dataset_roots(&settings, crate::dataset::DatasetType::Card) {
            if path.starts_with(&root) {
                if let Ok(search_conn) = open_search_db(&root) {
                    let _ = index_card_in_fts(
                        &search_conn,
                        &dataset_uuid,
                        &uuid,
                        &info.name,
                        &card.question,
                        &card.answer,
                        &card.note,
                        &card.suggestion,
                    );
                }
                break;
            }
        }
    }

    log::info!("[Cards] Saved card '{}' in dataset '{}'", uuid, dataset_uuid);
    let saved = Card {
        uuid,
        question: card.question,
        suggestion: card.suggestion,
        answer: card.answer,
        note: card.note,
        familiarity: card.familiarity,
        question_hash,
        source_card_uuid: card.source_card_uuid,
        source_dataset_uuid: card.source_dataset_uuid,
        deleted_at: None,
        created_at: now.clone(),
        updated_at: now,
    };

    // Thin-client writeback: queue the change for the PC. Mobile only — desktop
    // is the source of truth and already wrote straight to its DB.
    #[cfg(not(feature = "desktop"))]
    {
        let _ = crate::sync::enqueue_change(
            &settings,
            "card_save",
            &dataset_uuid,
            &serde_json::to_value(&saved).unwrap_or_default(),
        );
    }

    Ok(saved)
}

/// Soft-delete a card.
#[tauri::command]
pub async fn card_delete(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    card_uuid: String,
) -> Result<(), String> {
    let path = find_card_dataset_dir(&settings, &dataset_uuid)?;
    let conn = open_card_db(&path)?;
    let now = Utc::now().to_rfc3339();

    conn.execute(
        "UPDATE card SET deleted_at = ?1, updated_at = ?1 WHERE uuid = ?2",
        rusqlite::params![now, card_uuid],
    )
    .map_err(|e| e.to_string())?;

    update_dataset_timestamp(&path)?;

    // Remove from FTS index
    for root in dataset_roots(&settings, crate::dataset::DatasetType::Card) {
        if path.starts_with(&root) {
            if let Ok(search_conn) = open_search_db(&root) {
                let _ = remove_card_from_fts(&search_conn, &dataset_uuid, &card_uuid);
            }
            break;
        }
    }

    log::info!("[Cards] Deleted card '{}' in dataset '{}'", card_uuid, dataset_uuid);
    #[cfg(not(feature = "desktop"))]
    {
        let _ = crate::sync::enqueue_change(
            &settings,
            "card_delete",
            &dataset_uuid,
            &serde_json::json!({ "card_uuid": card_uuid }),
        );
    }
    Ok(())
}

/// Fork a card from one dataset to another.
#[tauri::command]
pub async fn card_fork(
    settings: State<'_, SettingsState>,
    source_dataset_uuid: String,
    card_uuid: String,
    target_dataset_uuid: String,
) -> Result<Card, String> {
    // Get the source card
    let source_card = card_get(settings.clone(), source_dataset_uuid.clone(), card_uuid.clone()).await?;

    // Create a new card in the target dataset with lineage tracking
    let new_card = Card {
        uuid: String::new(), // Will be generated by card_save
        question: source_card.question,
        suggestion: source_card.suggestion,
        answer: source_card.answer,
        note: source_card.note,
        familiarity: source_card.familiarity,
        question_hash: None, // Will be computed by card_save
        source_card_uuid: Some(card_uuid),
        source_dataset_uuid: Some(source_dataset_uuid),
        deleted_at: None,
        created_at: String::new(),
        updated_at: String::new(),
    };

    card_save(settings, target_dataset_uuid, new_card).await
}

// ---------------------------------------------------------------------------
// Commands: FTS Search
// ---------------------------------------------------------------------------

/// Search cards across all datasets using FTS5 full-text search.
#[tauri::command]
pub async fn card_search(
    settings: State<'_, SettingsState>,
    query: String,
    mode: Option<String>,
) -> Result<Vec<CardSearchResult>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let search_mode = match mode.as_deref() {
        Some("question") => SearchMode::Question,
        _ => SearchMode::FullText,
    };
    log::info!("[Cards] Searching for: '{}' (mode: {:?})", query, search_mode);
    search_cards_fts(&settings, &query, search_mode)
}

/// Rebuild the FTS search index for one or all locations.
#[tauri::command]
pub async fn card_fts_rebuild(
    settings: State<'_, SettingsState>,
    location: Option<String>,
) -> Result<usize, String> {
    log::info!("[Cards] Rebuilding FTS index (location: {:?})", location);
    rebuild_fts_index(&settings, location)
}

// ---------------------------------------------------------------------------
// Commands: Tags
// ---------------------------------------------------------------------------

/// List all tags in a dataset.
#[tauri::command]
pub async fn card_tag_list(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<Vec<Tag>, String> {
    let path = find_card_dataset_dir(&settings, &dataset_uuid)?;
    let conn = open_card_db(&path)?;

    let mut stmt = conn
        .prepare("SELECT * FROM tag WHERE deleted_at IS NULL ORDER BY name")
        .map_err(|e| e.to_string())?;

    let tags = stmt
        .query_map([], |row| {
            Ok(Tag {
                uuid: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
                deleted_at: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(tags)
}

/// Create or update a tag.
#[tauri::command]
pub async fn card_tag_save(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    tag: Tag,
) -> Result<Tag, String> {
    let path = find_card_dataset_dir(&settings, &dataset_uuid)?;
    let conn = open_card_db(&path)?;
    let now = Utc::now().to_rfc3339();

    let uuid = if tag.uuid.is_empty() {
        Uuid::new_v4().to_string()
    } else {
        tag.uuid.clone()
    };

    conn.execute(
        "INSERT INTO tag (uuid, name, color, deleted_at, created_at, updated_at) \
         VALUES (?1, ?2, ?3, NULL, ?4, ?5)
         ON CONFLICT(uuid) DO UPDATE SET name = ?2, color = ?3, updated_at = ?5",
        rusqlite::params![uuid, tag.name, tag.color, now, now],
    )
    .map_err(|e| e.to_string())?;

    let saved = Tag {
        uuid,
        name: tag.name,
        color: tag.color,
        deleted_at: None,
        created_at: now.clone(),
        updated_at: now,
    };
    #[cfg(not(feature = "desktop"))]
    {
        let _ = crate::sync::enqueue_change(
            &settings,
            "card_tag_save",
            &dataset_uuid,
            &serde_json::to_value(&saved).unwrap_or_default(),
        );
    }
    Ok(saved)
}

/// Delete a tag (soft delete).
#[tauri::command]
pub async fn card_tag_delete(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    tag_uuid: String,
) -> Result<(), String> {
    let path = find_card_dataset_dir(&settings, &dataset_uuid)?;
    let conn = open_card_db(&path)?;
    let now = Utc::now().to_rfc3339();

    conn.execute(
        "UPDATE tag SET deleted_at = ?1, updated_at = ?1 WHERE uuid = ?2",
        rusqlite::params![now, tag_uuid],
    )
    .map_err(|e| e.to_string())?;

    #[cfg(not(feature = "desktop"))]
    {
        let _ = crate::sync::enqueue_change(
            &settings,
            "card_tag_delete",
            &dataset_uuid,
            &serde_json::json!({ "tag_uuid": tag_uuid }),
        );
    }

    Ok(())
}

/// Set tags for a card (replaces all existing tags).
#[tauri::command]
pub async fn card_set_tags(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    card_uuid: String,
    tag_uuids: Vec<String>,
) -> Result<(), String> {
    let path = find_card_dataset_dir(&settings, &dataset_uuid)?;
    let conn = open_card_db(&path)?;
    let now = Utc::now().to_rfc3339();

    // Soft-delete existing tags
    conn.execute(
        "UPDATE card_tag SET deleted_at = ?1, updated_at = ?1 WHERE card_uuid = ?2 AND deleted_at IS NULL",
        rusqlite::params![now, card_uuid],
    )
    .map_err(|e| e.to_string())?;

    // Insert new tags
    for tag_uuid in &tag_uuids {
        let ct_uuid = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO card_tag (uuid, card_uuid, tag_uuid, deleted_at, created_at, updated_at) \
             VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
            rusqlite::params![ct_uuid, card_uuid, tag_uuid, now, now],
        )
        .map_err(|e| e.to_string())?;
    }

    #[cfg(not(feature = "desktop"))]
    {
        let _ = crate::sync::enqueue_change(
            &settings,
            "card_set_tags",
            &dataset_uuid,
            &serde_json::json!({ "card_uuid": card_uuid, "tag_uuids": tag_uuids }),
        );
    }

    Ok(())
}

/// Get tags for a specific card.
#[tauri::command]
pub async fn card_get_tags(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    card_uuid: String,
) -> Result<Vec<Tag>, String> {
    let path = find_card_dataset_dir(&settings, &dataset_uuid)?;
    let conn = open_card_db(&path)?;

    let mut stmt = conn
        .prepare(
            "SELECT t.* FROM tag t \
             JOIN card_tag ct ON ct.tag_uuid = t.uuid \
             WHERE ct.card_uuid = ?1 AND ct.deleted_at IS NULL AND t.deleted_at IS NULL \
             ORDER BY t.name",
        )
        .map_err(|e| e.to_string())?;

    let tags = stmt
        .query_map(rusqlite::params![card_uuid], |row| {
            Ok(Tag {
                uuid: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
                deleted_at: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(tags)
}

// ---------------------------------------------------------------------------
// Commands: SM-2 Review
// ---------------------------------------------------------------------------

/// Get the next card for review (SM-2 spaced repetition).
#[tauri::command]
pub async fn card_test_get(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<Option<(Card, Option<CardReview>)>, String> {
    let path = find_card_dataset_dir(&settings, &dataset_uuid)?;
    let conn = open_card_db(&path)?;

    // First try to find a card that is due for review
    let due_result = conn.query_row(
        "SELECT c.*, cr.uuid, cr.card_uuid, cr.familiarity, cr.interval_days, \
         cr.ease_factor, cr.repetitions, cr.last_review_at, cr.next_review_at \
         FROM card c \
         JOIN card_review cr ON cr.card_uuid = c.uuid \
         WHERE c.deleted_at IS NULL \
           AND c.familiarity < 6 \
           AND length(c.question) > 0 \
           AND length(c.answer) > 0 \
           AND cr.next_review_at <= datetime('now') \
         ORDER BY RANDOM() * (6 - c.familiarity) DESC \
         LIMIT 1",
        [],
        |row| {
            let card = Card {
                uuid: row.get(0)?,
                question: row.get(1)?,
                suggestion: row.get(2)?,
                answer: row.get(3)?,
                note: row.get(4)?,
                familiarity: row.get(5)?,
                question_hash: row.get(6)?,
                source_card_uuid: row.get(7)?,
                source_dataset_uuid: row.get(8)?,
                deleted_at: row.get(9)?,
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
            };
            let review = CardReview {
                uuid: row.get(12)?,
                card_uuid: row.get(13)?,
                familiarity: row.get(14)?,
                interval_days: row.get(15)?,
                ease_factor: row.get(16)?,
                repetitions: row.get(17)?,
                last_review_at: row.get(18)?,
                next_review_at: row.get(19)?,
            };
            Ok((card, Some(review)))
        },
    );

    if let Ok(result) = due_result {
        return Ok(Some(result));
    }

    // No due reviews — pick from cards that have never been reviewed
    let new_result = conn.query_row(
        "SELECT c.* FROM card c \
         WHERE c.deleted_at IS NULL \
           AND c.familiarity < 6 \
           AND length(c.question) > 0 \
           AND length(c.answer) > 0 \
           AND NOT EXISTS (SELECT 1 FROM card_review cr WHERE cr.card_uuid = c.uuid) \
         ORDER BY RANDOM() * (6 - c.familiarity) DESC \
         LIMIT 1",
        [],
        |row| {
            let card = Card {
                uuid: row.get(0)?,
                question: row.get(1)?,
                suggestion: row.get(2)?,
                answer: row.get(3)?,
                note: row.get(4)?,
                familiarity: row.get(5)?,
                question_hash: row.get(6)?,
                source_card_uuid: row.get(7)?,
                source_dataset_uuid: row.get(8)?,
                deleted_at: row.get(9)?,
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
            };
            Ok((card, None))
        },
    );

    match new_result {
        Ok(result) => Ok(Some(result)),
        Err(_) => Ok(None),
    }
}

/// Submit a review result and update SM-2 state.
#[tauri::command]
pub async fn card_test_submit(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    card_uuid: String,
    quality: i32,
) -> Result<CardReview, String> {
    let path = find_card_dataset_dir(&settings, &dataset_uuid)?;
    let conn = open_card_db(&path)?;
    let now = Utc::now().to_rfc3339();

    // Get existing review or create defaults
    let existing = conn.query_row(
        "SELECT * FROM card_review WHERE card_uuid = ?1",
        rusqlite::params![card_uuid],
        |row| {
            Ok(CardReview {
                uuid: row.get(0)?,
                card_uuid: row.get(1)?,
                familiarity: row.get(2)?,
                interval_days: row.get(3)?,
                ease_factor: row.get(4)?,
                repetitions: row.get(5)?,
                last_review_at: row.get(6)?,
                next_review_at: row.get(7)?,
            })
        },
    )
    .ok();

    let (interval_days, ease_factor, repetitions, familiarity) = if let Some(ref rev) = existing {
        sm2_next(quality, rev.repetitions, rev.interval_days, rev.ease_factor)
    } else {
        sm2_next(quality, 0, 0, 250)
    };

    // Calculate next_review_at
    let next_review = chrono::Utc::now() + chrono::Duration::days(interval_days as i64);
    let next_review_at = next_review.to_rfc3339();

    let review_uuid = existing
        .as_ref()
        .map(|r| r.uuid.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    // Upsert review
    conn.execute(
        "INSERT INTO card_review (uuid, card_uuid, familiarity, interval_days, ease_factor, \
         repetitions, last_review_at, next_review_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(uuid) DO UPDATE SET
            familiarity = ?3, interval_days = ?4, ease_factor = ?5,
            repetitions = ?6, last_review_at = ?7, next_review_at = ?8",
        rusqlite::params![
            review_uuid,
            card_uuid,
            familiarity,
            interval_days,
            ease_factor,
            repetitions,
            now,
            next_review_at,
        ],
    )
    .map_err(|e| e.to_string())?;

    // Update card familiarity
    conn.execute(
        "UPDATE card SET familiarity = ?1, updated_at = ?2 WHERE uuid = ?3",
        rusqlite::params![familiarity, now, card_uuid],
    )
    .map_err(|e| e.to_string())?;

    update_dataset_timestamp(&path)?;

    log::info!(
        "[Cards] Review submitted: card='{}', quality={}, next_interval={}d",
        card_uuid,
        quality,
        interval_days
    );

    #[cfg(not(feature = "desktop"))]
    {
        let _ = crate::sync::enqueue_change(
            &settings,
            "card_review",
            &dataset_uuid,
            &serde_json::json!({ "card_uuid": card_uuid, "quality": quality }),
        );
    }

    Ok(CardReview {
        uuid: review_uuid,
        card_uuid,
        familiarity,
        interval_days,
        ease_factor,
        repetitions,
        last_review_at: Some(now),
        next_review_at: Some(next_review_at),
    })
}

// ---------------------------------------------------------------------------
// Commands: Sync Status
// ---------------------------------------------------------------------------

/// Get sync status for a card dataset.
#[tauri::command]
pub async fn card_sync_status(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<Option<SyncStatus>, String> {
    let path = find_card_dataset_dir(&settings, &dataset_uuid)?;
    let conn = open_card_db(&path)?;

    let result = conn.query_row(
        "SELECT * FROM sync_state WHERE dataset_uuid = ?1",
        rusqlite::params![dataset_uuid],
        |row| {
            Ok(SyncStatus {
                dataset_uuid: row.get(0)?,
                last_synced_at: row.get(1)?,
                clock_offset_ms: row.get(2)?,
                pending_push: row.get(3)?,
            })
        },
    )
    .ok();

    Ok(result)
}

/// Get cards changed since a given timestamp (for sync push).
#[tauri::command]
pub async fn card_sync_get_changes(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    since: String,
) -> Result<Vec<Card>, String> {
    let path = find_card_dataset_dir(&settings, &dataset_uuid)?;
    let conn = open_card_db(&path)?;

    let mut stmt = conn
        .prepare(
            "SELECT * FROM card WHERE updated_at > ?1 ORDER BY updated_at ASC",
        )
        .map_err(|e| e.to_string())?;

    let cards = stmt
        .query_map(rusqlite::params![since], |row| {
            Ok(Card {
                uuid: row.get(0)?,
                question: row.get(1)?,
                suggestion: row.get(2)?,
                answer: row.get(3)?,
                note: row.get(4)?,
                familiarity: row.get(5)?,
                question_hash: row.get(6)?,
                source_card_uuid: row.get(7)?,
                source_dataset_uuid: row.get(8)?,
                deleted_at: row.get(9)?,
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(cards)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Update the dataset's info.json timestamp.
fn update_dataset_timestamp(dataset_dir: &Path) -> Result<(), String> {
    let info_path = dataset_dir.join("info.json");
    let data = fs::read_to_string(&info_path).map_err(|e| e.to_string())?;
    let mut info: CardDatasetInfo = serde_json::from_str(&data).map_err(|e| e.to_string())?;
    info.updated = Utc::now().to_rfc3339();
    let data = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
    fs::write(&info_path, data).map_err(|e| e.to_string())
}
