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
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;
use tauri::State;

use crate::datasets::cards::open_card_db;
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
        None => crate::app_paths::data_root().join("simple_words.json"),
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

/// The config file's modification stamp, `None` while it does not exist.
/// Compared with the copy in memory to tell "nothing changed" from "another
/// workspace, or a file written by an agent".
fn config_stamp(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

// ============================================================
// State
// ============================================================

pub struct SimpleWordsState {
    /// Words for the currently active language only.
    pub words: Mutex<HashSet<String>>,
    /// Current config (mirrors persisted JSON)
    pub config: Mutex<SimpleWordsConfig>,
    /// The file that config came from and its stamp when it was read. The
    /// config lives in the *workspace* directory, and a workspace can be picked
    /// long after startup read it (the chooser), so the memory copy has to be
    /// able to notice that it is describing a different workspace now.
    source: Mutex<Option<(PathBuf, Option<SystemTime>)>>,
}

impl SimpleWordsState {
    pub fn new() -> Self {
        Self {
            words: Mutex::new(HashSet::new()),
            config: Mutex::new(SimpleWordsConfig::default()),
            source: Mutex::new(None),
        }
    }
}

// ============================================================
// Active-language resolution
// ============================================================

/// Make sure a configured language is the active one. Configs that name
/// datasets but no active language are exactly what "Save without ever pressing
/// Load" leaves behind, and an empty active language loads nothing at startup:
/// the page then reports 0 words next to a dataset selection that looks right.
/// Returns whether the config was changed (so the caller can persist an
/// adoption rather than re-deriving it on every launch).
fn resolve_active_language(cfg: &mut SimpleWordsConfig) -> bool {
    if !cfg.active_language.is_empty() && cfg.languages.contains_key(&cfg.active_language) {
        return false;
    }
    // Deterministic pick (the alphabetically first language) so two runs over
    // the same config cannot disagree about which language is loaded.
    let picked = cfg.languages.keys().min().cloned().unwrap_or_default();
    if picked == cfg.active_language {
        return false;
    }
    cfg.active_language = picked;
    true
}

// ============================================================
// Helpers
// ============================================================

/// Load card questions from the selected datasets for a single language.
fn load_words_for_language(
    settings: &SettingsState,
    lang: &str,
    lang_cfg: &LanguageConfig,
) -> HashSet<String> {
    let all_datasets = crate::datasets::cards::list_card_datasets(settings);
    let mut words = HashSet::new();
    let mut found: HashSet<&str> = HashSet::new();

    for ds in &all_datasets {
        if !lang_cfg.dataset_uuids.contains(&ds.info.uuid) {
            continue;
        }
        found.insert(ds.info.uuid.as_str());
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

    // A configured dataset that resolves to nothing is the other way this can
    // load 0 words with no error: the dataset was re-imported (new uuid), moved
    // out of a linked directory, or deleted. Say so — the page only has room to
    // show the uuid it could not resolve.
    let missing: Vec<&str> = lang_cfg
        .dataset_uuids
        .iter()
        .map(String::as_str)
        .filter(|uuid| !found.contains(uuid))
        .collect();
    if !missing.is_empty() {
        log::warn!(
            "[SimpleWords] {} configured dataset(s) for '{}' not found: {:?}",
            missing.len(),
            lang,
            missing
        );
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

    let words = load_words_for_language(settings, lang, lang_cfg);
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

/// Initialize state from persisted config (called at startup, and again
/// whenever the active workspace changes).
pub fn init_simple_words(
    settings: &SettingsState,
    state: &SimpleWordsState,
) -> Result<usize, String> {
    let path = config_path(settings);
    let mut cfg = load_config(settings);
    if resolve_active_language(&mut cfg) {
        // Persist the adoption, so the next launch loads the same language
        // instead of re-deriving it (and so the file says what the app does).
        // A failed write is no reason to load nothing: the adoption still holds
        // for this session.
        if let Err(e) = save_config(settings, &cfg) {
            log::warn!("[SimpleWords] Failed to persist the active language: {}", e);
        }
    }
    *state.config.lock().unwrap() = cfg;
    *state.source.lock().unwrap() = Some((path.clone(), config_stamp(&path)));
    load_active_language(settings, state)
}

/// Bring the in-memory config back in line with the file before answering a
/// read. Cheap in the common case (one `metadata` call): the word set is only
/// rebuilt when the file, or the workspace it lives in, actually changed.
fn sync_config(settings: &SettingsState, state: &SimpleWordsState) -> Result<usize, String> {
    let path = config_path(settings);
    let stamp = config_stamp(&path);
    let unchanged = {
        let source = state.source.lock().unwrap();
        match source.as_ref() {
            Some((loaded_path, loaded_stamp)) => loaded_path == &path && *loaded_stamp == stamp,
            None => false,
        }
    };
    if unchanged {
        return Ok(state.words.lock().unwrap().len());
    }
    init_simple_words(settings, state)
}

/// The config the page renders, plus the size of the set actually in memory.
fn config_info(state: &SimpleWordsState) -> SimpleWordsConfigInfo {
    let cfg = state.config.lock().unwrap().clone();
    let word_count = state.words.lock().unwrap().len();
    SimpleWordsConfigInfo {
        languages: cfg.languages,
        active_language: cfg.active_language,
        word_count,
    }
}

// ============================================================
// Tauri Commands
// ============================================================

/// Get the full config (languages, datasets, defaults) and active language info.
///
/// The file is the truth: this re-reads it when the workspace changed under us
/// or something else wrote it, and adopts a configured language when none was
/// active. A page opened on the fallback path (no workspace selected yet) would
/// otherwise keep showing an empty config for the rest of the session.
#[tauri::command]
pub fn simple_words_get_config(
    settings: State<'_, SettingsState>,
    sw_state: State<'_, SimpleWordsState>,
) -> Result<SimpleWordsConfigInfo, String> {
    sync_config(&settings, &sw_state)?;
    Ok(config_info(&sw_state))
}

/// Save the full config (all languages), persist, and rebuild the word set for
/// whatever language is now active — saving a selection and then still seeing
/// "0 words" is the confusion this avoids. Returns the effective config, since
/// an empty `active_language` is resolved to a configured one here.
#[tauri::command]
pub fn simple_words_save_config(
    config: SimpleWordsConfig,
    settings: State<'_, SettingsState>,
    sw_state: State<'_, SimpleWordsState>,
) -> Result<SimpleWordsConfigInfo, String> {
    let mut config = config;
    resolve_active_language(&mut config);
    save_config(&settings, &config)?;
    *sw_state.config.lock().unwrap() = config;
    let path = config_path(&settings);
    *sw_state.source.lock().unwrap() = Some((path.clone(), config_stamp(&path)));
    load_active_language(&settings, &sw_state)?;
    Ok(config_info(&sw_state))
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
    // We just wrote the file: record it, so the next read does not treat our own
    // write as somebody else's change and reload everything again.
    let path = config_path(&settings);
    *sw_state.source.lock().unwrap() = Some((path.clone(), config_stamp(&path)));
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

    let all_datasets = crate::datasets::cards::list_card_datasets(&settings);
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
    settings: State<'_, SettingsState>,
    sw_state: State<'_, SimpleWordsState>,
) -> Result<Vec<String>, String> {
    sync_config(&settings, &sw_state)?;
    let words = sw_state.words.lock().unwrap();
    let mut sorted: Vec<String> = words.iter().cloned().collect();
    sorted.sort();
    Ok(sorted)
}

/// Check if a single word is in the loaded simple_words set.
#[tauri::command]
pub fn simple_words_contains(
    word: String,
    settings: State<'_, SettingsState>,
    sw_state: State<'_, SimpleWordsState>,
) -> Result<bool, String> {
    sync_config(&settings, &sw_state)?;
    let word = word.trim().to_lowercase();
    let words = sw_state.words.lock().unwrap();
    Ok(words.contains(word.as_str()))
}

/// Filter out all simple words from a list (uses active language).
#[tauri::command]
pub fn simple_words_filter(
    words: Vec<String>,
    settings: State<'_, SettingsState>,
    sw_state: State<'_, SimpleWordsState>,
) -> Result<Vec<String>, String> {
    // A filter is an agent's read as much as the page's: answer from the set the
    // file describes, not from a snapshot taken before the workspace changed.
    sync_config(&settings, &sw_state)?;
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

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn language(uuid: &str) -> LanguageConfig {
        LanguageConfig {
            dataset_uuids: vec![uuid.to_string()],
            default_dataset_uuid: uuid.to_string(),
        }
    }

    /// The state a "Save without ever pressing Load" leaves behind: a dataset
    /// selection that nothing loads, because no language is active.
    #[test]
    fn a_configured_language_becomes_active() {
        let mut cfg = SimpleWordsConfig {
            languages: HashMap::from([("de".to_string(), language("card_a"))]),
            active_language: String::new(),
        };
        assert!(resolve_active_language(&mut cfg));
        assert_eq!(cfg.active_language, "de");
    }

    #[test]
    fn an_active_language_is_left_alone() {
        let mut cfg = SimpleWordsConfig {
            languages: HashMap::from([
                ("de".to_string(), language("card_a")),
                ("en".to_string(), language("card_b")),
            ]),
            active_language: "en".to_string(),
        };
        assert!(!resolve_active_language(&mut cfg));
        assert_eq!(cfg.active_language, "en");
    }

    /// A language deleted from under the active one must not leave it pointing
    /// at nothing.
    #[test]
    fn an_unknown_active_language_is_replaced() {
        let mut cfg = SimpleWordsConfig {
            languages: HashMap::from([("de".to_string(), language("card_a"))]),
            active_language: "fr".to_string(),
        };
        assert!(resolve_active_language(&mut cfg));
        assert_eq!(cfg.active_language, "de");
    }

    /// Nothing configured: report no change, so a startup with no config never
    /// creates one.
    #[test]
    fn an_empty_config_is_not_touched() {
        let mut cfg = SimpleWordsConfig::default();
        assert!(!resolve_active_language(&mut cfg));
        assert!(cfg.active_language.is_empty());
    }
}
