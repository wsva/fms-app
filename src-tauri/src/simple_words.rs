//! Simple Words — a set of "known simple" words loaded from selected card datasets.
//!
//! Users organize simple words by language. For each language, they select
//! one or more card datasets as sources, and designate one as the "default"
//! where new words are added when marking a word as simple.
//!
//! Only ONE language is loaded into memory at a time (the "active" language),
//! since the user typically works with one language at a time.
//!
//! Config is persisted in `{workspace}/simple_words.json`:
//! ```json
//! {
//!   "languages": {
//!     "de": {
//!       "dataset_uuids": ["uuid1", "uuid2"],
//!       "default_dataset_uuid": "uuid1"
//!     }
//!   },
//!   "active_language": "de"
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::State;

use crate::cards::open_card_db;
use crate::settings::SettingsState;

// ============================================================
// Persistence — per-language config
// ============================================================

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LanguageConfig {
    pub dataset_uuids: Vec<String>,
    pub default_dataset_uuid: String,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct SimpleWordsConfig {
    #[serde(default)]
    pub languages: HashMap<String, LanguageConfig>,
    #[serde(default)]
    pub active_language: String,
}

/// Serializable summary returned by get_config command.
#[derive(Serialize)]
pub struct SimpleWordsConfigInfo {
    pub languages: HashMap<String, LanguageConfig>,
    pub active_language: String,
    pub word_count: usize,
}

fn config_path(settings: &SettingsState) -> PathBuf {
    match settings.workspace_dir.lock().unwrap().as_ref() {
        Some(ws_dir) => ws_dir.join("simple_words.json"),
        None => dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("fms-app")
            .join("simple_words.json"),
    }
}

fn load_config(settings: &SettingsState) -> SimpleWordsConfig {
    let path = config_path(settings);
    match std::fs::read_to_string(&path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => SimpleWordsConfig::default(),
    }
}

fn save_config(settings: &SettingsState, cfg: &SimpleWordsConfig) -> Result<(), String> {
    let path = config_path(settings);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let data = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, data).map_err(|e| e.to_string())
}

// ============================================================
// State
// ============================================================

pub struct SimpleWordsState {
    /// Words for the currently active language only.
    pub words: Mutex<HashSet<String>>,
    /// Current config (mirrors persisted JSON)
    pub config: Mutex<SimpleWordsConfig>,
}

impl SimpleWordsState {
    pub fn new() -> Self {
        Self {
            words: Mutex::new(HashSet::new()),
            config: Mutex::new(SimpleWordsConfig::default()),
        }
    }
}

// ============================================================
// Helpers
// ============================================================

/// Load card questions from the selected datasets for a single language.
fn load_words_for_language(
    settings: &SettingsState,
    lang_cfg: &LanguageConfig,
) -> HashSet<String> {
    let all_datasets = crate::cards::list_card_datasets(settings);
    let mut words = HashSet::new();

    for ds in &all_datasets {
        if !lang_cfg.dataset_uuids.contains(&ds.info.uuid) {
            continue;
        }
        let ds_path = PathBuf::from(&ds.path);
        let conn = match open_card_db(&ds_path) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("[SimpleWords] Failed to open DB for '{}': {}", ds.info.name, e);
                continue;
            }
        };

        let mut stmt = match conn.prepare(
            "SELECT question FROM card WHERE deleted_at IS NULL AND question != ''",
        ) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("[SimpleWords] Failed to prepare query for '{}': {}", ds.info.name, e);
                continue;
            }
        };

        let questions: Vec<String> = match stmt.query_map([], |row| row.get::<_, String>(0)) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                log::warn!("[SimpleWords] Failed to query '{}': {}", ds.info.name, e);
                continue;
            }
        };

        for q in questions {
            let word = q.trim().to_lowercase();
            if !word.is_empty() {
                words.insert(word);
            }
        }
    }

    words
}

/// Load words for the active language into memory.
fn load_active_language(settings: &SettingsState, state: &SimpleWordsState) -> Result<usize, String> {
    let cfg = state.config.lock().unwrap().clone();
    let lang = &cfg.active_language;

    if lang.is_empty() {
        *state.words.lock().unwrap() = HashSet::new();
        return Ok(0);
    }

    let lang_cfg = match cfg.languages.get(lang) {
        Some(lc) => lc,
        None => {
            *state.words.lock().unwrap() = HashSet::new();
            return Ok(0);
        }
    };

    let words = load_words_for_language(settings, lang_cfg);
    let count = words.len();
    log::info!(
        "[SimpleWords] Loaded {} unique words for language '{}' from {} dataset(s)",
        count,
        lang,
        lang_cfg.dataset_uuids.len()
    );
    *state.words.lock().unwrap() = words;
    Ok(count)
}

/// Initialize state from persisted config (called at startup).
pub fn init_simple_words(
    settings: &SettingsState,
    state: &SimpleWordsState,
) -> Result<usize, String> {
    let cfg = load_config(settings);
    *state.config.lock().unwrap() = cfg;
    load_active_language(settings, state)
}

// ============================================================
// Tauri Commands
// ============================================================

/// Get the full config (languages, datasets, defaults) and active language info.
#[tauri::command]
pub fn simple_words_get_config(
    sw_state: State<'_, SimpleWordsState>,
) -> Result<SimpleWordsConfigInfo, String> {
    let cfg = sw_state.config.lock().unwrap().clone();
    let word_count = sw_state.words.lock().unwrap().len();
    Ok(SimpleWordsConfigInfo {
        languages: cfg.languages,
        active_language: cfg.active_language,
        word_count,
    })
}

/// Save the full config (all languages), persist, but do NOT reload active words.
#[tauri::command]
pub fn simple_words_save_config(
    config: SimpleWordsConfig,
    settings: State<'_, SettingsState>,
    sw_state: State<'_, SimpleWordsState>,
) -> Result<(), String> {
    save_config(&settings, &config)?;
    *sw_state.config.lock().unwrap() = config;
    Ok(())
}

/// Load a specific language into memory (switches active language).
#[tauri::command]
pub fn simple_words_load_language(
    language: String,
    settings: State<'_, SettingsState>,
    sw_state: State<'_, SimpleWordsState>,
) -> Result<usize, String> {
    // Update config with new active language
    {
        let mut cfg = sw_state.config.lock().unwrap();
        if !cfg.languages.contains_key(&language) {
            return Err(format!("Language '{}' not configured", language));
        }
        cfg.active_language = language.clone();
    }
    // Persist
    let cfg = sw_state.config.lock().unwrap().clone();
    save_config(&settings, &cfg)?;
    // Load words
    load_active_language(&settings, &sw_state)
}

/// Add a word to the default dataset for the active language.
#[tauri::command]
pub fn simple_words_add_word(
    word: String,
    settings: State<'_, SettingsState>,
    sw_state: State<'_, SimpleWordsState>,
) -> Result<bool, String> {
    let word = word.trim().to_lowercase();
    if word.is_empty() {
        return Err("Word cannot be empty".to_string());
    }

    let cfg = sw_state.config.lock().unwrap().clone();
    let lang = &cfg.active_language;
    if lang.is_empty() {
        return Err("No active language selected".to_string());
    }

    let lang_cfg = cfg
        .languages
        .get(lang)
        .ok_or_else(|| format!("Language '{}' not configured", lang))?;

    let default_uuid = &lang_cfg.default_dataset_uuid;
    if default_uuid.is_empty() {
        return Err(format!("No default dataset set for language '{}'", lang));
    }

    let all_datasets = crate::cards::list_card_datasets(&settings);
    let ds = all_datasets
        .iter()
        .find(|d| d.info.uuid == *default_uuid)
        .ok_or_else(|| {
            format!(
                "Default dataset '{}' for language '{}' not found",
                default_uuid, lang
            )
        })?;

    let ds_path = PathBuf::from(&ds.path);
    let conn = open_card_db(&ds_path)?;

    // Check if word already exists
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM card WHERE question = ?1 AND deleted_at IS NULL",
            [&word],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .map_err(|e| e.to_string())?;

    if exists {
        // Still add to in-memory set in case it was missing
        sw_state.words.lock().unwrap().insert(word);
        return Ok(false);
    }

    // Insert new card
    let uuid = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO card (uuid, question, suggestion, answer, note, familiarity, created_at, updated_at) \
         VALUES (?1, ?2, '', '', '', 0, ?3, ?4)",
        rusqlite::params![uuid, word, now, now],
    )
    .map_err(|e| e.to_string())?;

    // Update in-memory set
    sw_state.words.lock().unwrap().insert(word.clone());

    log::info!(
        "[SimpleWords] Added '{}' to language '{}' (dataset '{}')",
        word,
        lang,
        ds.info.name
    );
    Ok(true)
}

/// List all simple words for the active language (sorted).
#[tauri::command]
pub fn simple_words_list(
    sw_state: State<'_, SimpleWordsState>,
) -> Result<Vec<String>, String> {
    let words = sw_state.words.lock().unwrap();
    let mut sorted: Vec<String> = words.iter().cloned().collect();
    sorted.sort();
    Ok(sorted)
}

/// Check if a single word is in the loaded simple_words set.
#[tauri::command]
pub fn simple_words_contains(
    word: String,
    sw_state: State<'_, SimpleWordsState>,
) -> Result<bool, String> {
    let word = word.trim().to_lowercase();
    let words = sw_state.words.lock().unwrap();
    Ok(words.contains(word.as_str()))
}

/// Filter out all simple words from a list (uses active language).
#[tauri::command]
pub fn simple_words_filter(
    words: Vec<String>,
    sw_state: State<'_, SimpleWordsState>,
) -> Result<Vec<String>, String> {
    let simple = sw_state.words.lock().unwrap();
    let filtered: Vec<String> = words
        .into_iter()
        .filter(|w| !simple.contains(w.trim().to_lowercase().as_str()))
        .collect();
    Ok(filtered)
}

/// Reload words for the active language from disk.
#[tauri::command]
pub fn simple_words_reload(
    settings: State<'_, SettingsState>,
    sw_state: State<'_, SimpleWordsState>,
) -> Result<usize, String> {
    load_active_language(&settings, &sw_state)
}
