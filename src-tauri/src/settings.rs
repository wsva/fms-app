use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

// ---------------------------------------------------------------------------
// Settings struct
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize, Debug, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ModelUnloadTimeout {
    /// Unload immediately after transcription completes.
    Immediately,
    /// Never unload automatically.
    Never,
    /// Unload after a number of minutes.
    Minutes(u32),
}

impl Default for ModelUnloadTimeout {
    fn default() -> Self {
        Self::Never
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(alias = "stt_model_dir")]
    pub model_dir: String,
    pub recordings_dir: String,
    pub datasets_dir: String,
    /// Root directory of the reading library (each book is a sub-directory).
    #[serde(default)]
    pub books_dir: String,
    /// Root directory of the wiki (markdown documents).
    #[serde(default)]
    pub wiki_dir: String,
    /// Preferred default STT model ID (auto-loaded when nothing is loaded).
    #[serde(default)]
    pub selected_model: String,
    /// When to unload the model after inactivity.
    #[serde(default)]
    pub model_unload_timeout: ModelUnloadTimeout,
    /// Whether the user has completed onboarding.
    #[serde(default)]
    pub onboarding_completed: bool,
    /// Ollama API base URL (global setting, not workspace-scoped).
    #[serde(default = "default_ollama_url")]
    pub ollama_url: String,
    /// LLM inference provider for the chat page and other LLM features.
    /// One of: `ollama` (default), `openai`, `anthropic`, `groq`, `databricks`.
    #[serde(default = "default_llm_provider")]
    pub llm_provider: String,
    /// API key for cloud LLM providers (empty for local Ollama).
    #[serde(default)]
    pub llm_api_key: String,
    /// Default model name for cloud providers / free-text model entry.
    #[serde(default)]
    pub llm_model: String,
    /// goose ACP server WebSocket URL (`goose serve`). Global, desktop-only.
    #[serde(default = "default_goose_acp_url")]
    pub goose_acp_url: String,
    /// Secret key sent as `X-Secret-Key` when connecting to `goose serve`.
    /// Empty means the server runs unauthenticated.
    #[serde(default)]
    pub goose_acp_secret: String,
    /// Whether the Agent (goose ACP) integration is enabled.
    #[serde(default)]
    pub goose_acp_enabled: bool,
    /// PC snapshot server base URL (e.g. `http://192.168.1.20:35711`). Global,
    /// not workspace-scoped. On Android this is a remembered/manual fallback
    /// filled by `pc_discover`; left empty until discovery or manual entry.
    #[serde(default)]
    pub pc_url: String,
    /// Optional shared token sent as `x-fms-token` to the PC REST API.
    /// DEPRECATED: replaced by device-signature pairing (see `pairing.rs` /
    /// `sync.rs`). Kept only so old settings JSON keeps parsing.
    #[serde(default)]
    pub pc_token: String,
    /// This device's pairing identity: first 16 hex chars of SHA-256(pubkey).
    /// Generated lazily on the phone (never entered by the user).
    #[serde(default)]
    pub device_id: String,
    /// Hex-encoded PKCS#8 v1 Ed25519 keypair (private + public) backing
    /// `device_id`. The pubkey is derived from it at load — the PC trusts only
    /// the key, never a claimed identity.
    #[serde(default)]
    pub device_seed: String,
}

fn default_ollama_url() -> String {
    "http://localhost:11434".to_string()
}

fn default_llm_provider() -> String {
    "ollama".to_string()
}

fn default_goose_acp_url() -> String {
    "ws://127.0.0.1:3284/acp".to_string()
}

/// Global-only settings that are NOT workspace-scoped.
/// These are always stored in the global config path.
#[derive(Clone, Serialize, Deserialize)]
pub struct GlobalSettings {
    /// Ollama API base URL.
    #[serde(default = "default_ollama_url")]
    pub ollama_url: String,
    /// LLM inference provider (`ollama`/`openai`/`anthropic`/`groq`/`databricks`).
    #[serde(default = "default_llm_provider")]
    pub llm_provider: String,
    /// API key for cloud LLM providers.
    #[serde(default)]
    pub llm_api_key: String,
    /// Default model name for cloud providers / free-text entry.
    #[serde(default)]
    pub llm_model: String,
    /// goose ACP server WebSocket URL (`goose serve`).
    #[serde(default = "default_goose_acp_url")]
    pub goose_acp_url: String,
    /// Secret key sent as `X-Secret-Key` when connecting to `goose serve`.
    #[serde(default)]
    pub goose_acp_secret: String,
    /// Whether the Agent (goose ACP) integration is enabled.
    #[serde(default)]
    pub goose_acp_enabled: bool,
    /// PC snapshot server base URL (global fallback / remembered value).
    #[serde(default)]
    pub pc_url: String,
    /// Optional shared token for the PC REST API. DEPRECATED (pairing now).
    #[serde(default)]
    pub pc_token: String,
    /// Device pairing identity (see `AppSettings::device_id`).
    #[serde(default)]
    pub device_id: String,
    /// Hex PKCS#8 Ed25519 keypair (see `AppSettings::device_seed`).
    #[serde(default)]
    pub device_seed: String,
    /// Preferred default STT model ID (set from the Models page).
    #[serde(default)]
    pub selected_model: String,
    /// STT model directory (shared across workspaces).
    #[serde(default)]
    pub model_dir: String,
    /// When to unload the model after inactivity.
    #[serde(default)]
    pub model_unload_timeout: ModelUnloadTimeout,
    /// Whether the user has completed onboarding.
    #[serde(default)]
    pub onboarding_completed: bool,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        let data_dir = crate::app_paths::data_root();
        Self {
            ollama_url: default_ollama_url(),
            llm_provider: default_llm_provider(),
            llm_api_key: String::new(),
            llm_model: String::new(),
            goose_acp_url: default_goose_acp_url(),
            goose_acp_secret: String::new(),
            goose_acp_enabled: false,
            pc_url: String::new(),
            pc_token: String::new(),
            device_id: String::new(),
            device_seed: String::new(),
            selected_model: String::new(),
            model_dir: data_dir.join("models").to_string_lossy().into_owned(),
            model_unload_timeout: ModelUnloadTimeout::default(),
            onboarding_completed: false,
        }
    }
}

impl GlobalSettings {
    fn from_settings(s: &AppSettings) -> Self {
        Self {
            ollama_url: s.ollama_url.clone(),
            llm_provider: s.llm_provider.clone(),
            llm_api_key: s.llm_api_key.clone(),
            llm_model: s.llm_model.clone(),
            goose_acp_url: s.goose_acp_url.clone(),
            goose_acp_secret: s.goose_acp_secret.clone(),
            goose_acp_enabled: s.goose_acp_enabled,
            pc_url: s.pc_url.clone(),
            pc_token: s.pc_token.clone(),
            device_id: s.device_id.clone(),
            device_seed: s.device_seed.clone(),
            selected_model: s.selected_model.clone(),
            model_dir: s.model_dir.clone(),
            model_unload_timeout: s.model_unload_timeout,
            onboarding_completed: s.onboarding_completed,
        }
    }

    fn apply_to(&self, s: &mut AppSettings) {
        s.ollama_url = self.ollama_url.clone();
        s.llm_provider = self.llm_provider.clone();
        s.llm_api_key = self.llm_api_key.clone();
        s.llm_model = self.llm_model.clone();
        s.goose_acp_url = self.goose_acp_url.clone();
        s.goose_acp_secret = self.goose_acp_secret.clone();
        s.goose_acp_enabled = self.goose_acp_enabled;
        s.pc_url = self.pc_url.clone();
        s.pc_token = self.pc_token.clone();
        s.device_id = self.device_id.clone();
        s.device_seed = self.device_seed.clone();
        s.selected_model = self.selected_model.clone();
        s.model_dir = self.model_dir.clone();
        s.model_unload_timeout = self.model_unload_timeout;
        s.onboarding_completed = self.onboarding_completed;
    }
}

/// Workspace-scoped settings that are specific to each workspace.
/// These are stored in the workspace's settings.json.
#[derive(Clone, Serialize, Deserialize)]
pub struct WorkspaceSettings {
    pub recordings_dir: String,
    pub datasets_dir: String,
    /// Root directory of the reading library (each book is a sub-directory).
    #[serde(default)]
    pub books_dir: String,
    /// Root directory of the wiki (markdown documents).
    #[serde(default)]
    pub wiki_dir: String,
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        let data_dir = crate::app_paths::data_root();
        Self {
            recordings_dir: data_dir.join("recordings").to_string_lossy().into_owned(),
            datasets_dir: data_dir.join("datasets").to_string_lossy().into_owned(),
            books_dir: data_dir.join("datasets").join("book").to_string_lossy().into_owned(),
            wiki_dir: data_dir.join("wiki").to_string_lossy().into_owned(),
        }
    }
}

impl WorkspaceSettings {
    fn from_settings(s: &AppSettings) -> Self {
        Self {
            recordings_dir: s.recordings_dir.clone(),
            datasets_dir: s.datasets_dir.clone(),
            books_dir: s.books_dir.clone(),
            wiki_dir: s.wiki_dir.clone(),
        }
    }

    fn apply_to(&self, s: &mut AppSettings) {
        s.recordings_dir = self.recordings_dir.clone();
        s.datasets_dir = self.datasets_dir.clone();
        s.books_dir = self.books_dir.clone();
        s.wiki_dir = self.wiki_dir.clone();
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        let data_dir = crate::app_paths::data_root();

        Self {
            model_dir: data_dir.join("models").to_string_lossy().into_owned(),
            recordings_dir: data_dir.join("recordings").to_string_lossy().into_owned(),
            datasets_dir: data_dir.join("datasets").to_string_lossy().into_owned(),
            books_dir: data_dir.join("datasets").join("book").to_string_lossy().into_owned(),
            wiki_dir: data_dir.join("wiki").to_string_lossy().into_owned(),
            selected_model: String::new(),
            model_unload_timeout: ModelUnloadTimeout::default(),
            onboarding_completed: false,
            ollama_url: default_ollama_url(),
            llm_provider: default_llm_provider(),
            llm_api_key: String::new(),
            llm_model: String::new(),
            goose_acp_url: default_goose_acp_url(),
            goose_acp_secret: String::new(),
            goose_acp_enabled: false,
            pc_url: String::new(),
            pc_token: String::new(),
            device_id: String::new(),
            device_seed: String::new(),
        }
    }
}

pub struct SettingsState {
    pub settings: Mutex<AppSettings>,
    /// Current workspace directory. When set, settings are loaded/saved here.
    /// When None, falls back to the global app data directory.
    pub workspace_dir: Mutex<Option<PathBuf>>,
}

impl SettingsState {
    pub fn new() -> Self {
        let settings = Self::load(None).unwrap_or_default();
        Self {
            settings: Mutex::new(settings),
            workspace_dir: Mutex::new(None),
        }
    }

    /// Set the workspace directory for settings storage.
    /// Reloads settings from the new workspace directory.
    pub fn set_workspace_dir(&self, dir: Option<PathBuf>) {
        log::info!("[Settings] Setting workspace dir: {:?}", dir);
        {
            let mut ws_dir = self.workspace_dir.lock().unwrap();
            *ws_dir = dir.clone();
        }
        // Reload settings from the new location
        if let Some(settings) = Self::load(dir.as_ref()) {
            let mut s = self.settings.lock().unwrap();
            *s = settings;
            log::info!("[Settings] Reloaded settings from workspace directory");
        } else {
            log::info!("[Settings] No settings found in workspace directory, using current settings");
        }
    }

    /// Re-read settings from disk using the (now-initialized) base directories
    /// from [`crate::app_paths`]. Called from the Tauri `setup` hook right after
    /// `app_paths::init`, so that on mobile the in-memory defaults observe the
    /// app-private paths instead of the pre-`init` fallback.
    pub fn reload(&self) {
        let ws_dir = self.workspace_dir.lock().unwrap().clone();
        let settings = Self::load(ws_dir.as_ref()).unwrap_or_default();
        *self.settings.lock().unwrap() = settings;
        log::info!("[Settings] Reloaded settings after app_paths::init");
    }

    fn config_path(workspace_dir: Option<&PathBuf>) -> PathBuf {
        match workspace_dir {
            Some(dir) => dir.join("settings.json"),
            None => Self::global_config_path(),
        }
    }

    /// Always returns the global config path (not workspace-scoped).
    /// Uses the platform-aware base from [`crate::app_paths`] (desktop:
    /// `dirs::config_dir()/fms-app`; mobile: app-private data dir).
    fn global_config_path() -> PathBuf {
        crate::app_paths::config_root().join("settings.json")
    }

    /// Resolve a workspace-scoped data sub-directory: `<workspace>/<name>`.
    /// When a workspace is selected it is authoritative; otherwise fall back
    /// to the configured value (or the global app-data default when empty).
    pub fn workspace_subdir(&self, name: &str, configured: &str) -> PathBuf {
        if let Some(ws_dir) = self.workspace_dir.lock().unwrap().as_ref() {
            return ws_dir.join(name);
        }
        if configured.is_empty() {
            return crate::app_paths::data_subdir(name);
        }
        PathBuf::from(configured)
    }

    /// Effective datasets directory (workspace-derived).
    pub fn datasets_dir(&self) -> PathBuf {
        let configured = self.settings.lock().unwrap().datasets_dir.clone();
        self.workspace_subdir("datasets", &configured)
    }

    /// Effective recordings directory (workspace-derived).
    pub fn recordings_dir(&self) -> PathBuf {
        let configured = self.settings.lock().unwrap().recordings_dir.clone();
        self.workspace_subdir("recordings", &configured)
    }

    /// Effective books directory (workspace-derived).
    pub fn books_dir(&self) -> PathBuf {
        let configured = self.settings.lock().unwrap().books_dir.clone();
        self.workspace_subdir("books", &configured)
    }

    /// Effective wiki directory (workspace-derived).
    pub fn wiki_dir(&self) -> PathBuf {
        let configured = self.settings.lock().unwrap().wiki_dir.clone();
        self.workspace_subdir("wiki", &configured)
    }

    /// Currently preferred default STT model ID (global setting; empty = unset).
    pub fn selected_model(&self) -> String {
        self.settings.lock().unwrap().selected_model.clone()
    }

    /// Persist the preferred default STT model ID. Empty clears the preference,
    /// falling back to the app's automatic choice.
    pub fn set_selected_model(&self, version: &str) -> Result<(), String> {
        let ws_dir = self.workspace_dir.lock().unwrap().clone();
        let mut s = self.settings.lock().unwrap().clone();
        s.selected_model = version.to_string();
        SettingsState::save(&s, ws_dir.as_ref())?;
        *self.settings.lock().unwrap() = s;
        Ok(())
    }

    /// Load settings with global+workspace merge.
    /// Global settings are always loaded from the global path.
    /// If a workspace is set, workspace settings are loaded and merged on top.
    fn load(workspace_dir: Option<&PathBuf>) -> Option<AppSettings> {
        // Always load global settings first
        let global = Self::load_global();
        
        // If workspace is set, load workspace settings and merge
        if let Some(ws_dir) = workspace_dir {
            if let Some(mut ws_settings) = Self::load_from_path(&ws_dir.join("settings.json")) {
                // Apply global settings on top (global takes precedence for global fields)
                global.apply_to(&mut ws_settings);
                log::debug!("[Settings] Loaded workspace settings with global overlay");
                return Some(ws_settings);
            }
        }
        
        // No workspace or no workspace settings — load from global path
        if let Some(mut global_settings) = Self::load_from_path(&Self::global_config_path()) {
            global.apply_to(&mut global_settings);
            log::debug!("[Settings] Loaded global settings");
            return Some(global_settings);
        }
        
        None
    }
    
    /// Load only global settings from the global path.
    fn load_global() -> GlobalSettings {
        let path = Self::global_config_path();
        log::debug!("[Settings] Loading global settings from: {}", path.display());
        match fs::read_to_string(&path) {
            Ok(data) => {
                let data = data.trim_start_matches('\u{FEFF}');
                serde_json::from_str(data).unwrap_or_default()
            }
            Err(_) => GlobalSettings::default(),
        }
    }
    
    /// Load settings from a specific path.
    fn load_from_path(path: &std::path::Path) -> Option<AppSettings> {
        log::debug!("[Settings] Loading settings from: {}", path.display());
        let data = fs::read_to_string(path).ok()?;
        let data = data.trim_start_matches('\u{FEFF}');
        serde_json::from_str(data).map_err(|e| {
            log::warn!("[Settings] Failed to parse settings.json: {} - using defaults", e)
        }).ok()
    }

    /// Save settings with global+workspace split.
    /// Global fields are always saved to the global path.
    /// Workspace fields are saved to the workspace path (if set) or global path.
    pub fn save(settings: &AppSettings, workspace_dir: Option<&PathBuf>) -> Result<(), String> {
        // Always save global settings to global path
        let global = GlobalSettings::from_settings(settings);
        Self::save_global(&global)?;
        
        // Save workspace settings to workspace path (or global if no workspace)
        let path = Self::config_path(workspace_dir);
        log::debug!("[Settings] Saving workspace settings to: {}", path.display());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let data = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
        fs::write(path, data).map_err(|e| e.to_string())?;
        Ok(())
    }
    
    /// Save only global settings to the global path.
    fn save_global(global: &GlobalSettings) -> Result<(), String> {
        let path = Self::global_config_path();
        log::debug!("[Settings] Saving global settings to: {}", path.display());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        
        // Load existing settings to merge (preserve workspace fields)
        let mut existing = Self::load_from_path(&path).unwrap_or_default();
        global.apply_to(&mut existing);
        
        let data = serde_json::to_string_pretty(&existing).map_err(|e| e.to_string())?;
        fs::write(path, data).map_err(|e| e.to_string())?;
        Ok(())
    }
    
    /// Load only workspace settings from the workspace path.
    fn load_workspace(workspace_dir: Option<&PathBuf>) -> WorkspaceSettings {
        match workspace_dir {
            Some(ws_dir) => {
                let path = ws_dir.join("settings.json");
                log::debug!("[Settings] Loading workspace settings from: {}", path.display());
                match fs::read_to_string(&path) {
                    Ok(data) => {
                        let data = data.trim_start_matches('\u{FEFF}');
                        // Parse as AppSettings then extract workspace fields
                        match serde_json::from_str::<AppSettings>(data) {
                            Ok(s) => WorkspaceSettings::from_settings(&s),
                            Err(_) => WorkspaceSettings::default(),
                        }
                    }
                    Err(_) => WorkspaceSettings::default(),
                }
            }
            None => WorkspaceSettings::default(),
        }
    }
    
    /// Save only workspace settings to the workspace path.
    fn save_workspace(workspace: &WorkspaceSettings, workspace_dir: Option<&PathBuf>) -> Result<(), String> {
        match workspace_dir {
            Some(ws_dir) => {
                let path = ws_dir.join("settings.json");
                log::debug!("[Settings] Saving workspace settings to: {}", path.display());
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                
                // Load existing settings to merge (preserve global fields)
                let mut existing = Self::load_from_path(&path).unwrap_or_default();
                workspace.apply_to(&mut existing);
                
                let data = serde_json::to_string_pretty(&existing).map_err(|e| e.to_string())?;
                fs::write(path, data).map_err(|e| e.to_string())?;
                Ok(())
            }
            None => {
                // No workspace selected — save to global path
                let path = Self::global_config_path();
                log::debug!("[Settings] No workspace, saving workspace settings to global: {}", path.display());
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                
                let mut existing = Self::load_from_path(&path).unwrap_or_default();
                workspace.apply_to(&mut existing);
                
                let data = serde_json::to_string_pretty(&existing).map_err(|e| e.to_string())?;
                fs::write(path, data).map_err(|e| e.to_string())?;
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn settings_get(
    state: State<'_, SettingsState>,
) -> Result<AppSettings, String> {
    let mut s = state.settings.lock().unwrap().clone();
    // Report the effective (workspace-derived) data directories so the
    // frontend always sees the paths the backend actually uses.
    s.datasets_dir = state.datasets_dir().to_string_lossy().into_owned();
    s.recordings_dir = state.recordings_dir().to_string_lossy().into_owned();
    s.books_dir = state.books_dir().to_string_lossy().into_owned();
    s.wiki_dir = state.wiki_dir().to_string_lossy().into_owned();
    Ok(s)
}

#[tauri::command]
pub async fn settings_set(
    app: AppHandle,
    state: State<'_, SettingsState>,
    settings: AppSettings,
) -> Result<(), String> {
    let ws_dir = state.workspace_dir.lock().unwrap().clone();
    SettingsState::save(&settings, ws_dir.as_ref())?;
    log::info!("[Settings] Settings updated");
    {
        let mut s = state.settings.lock().unwrap();
        *s = settings;
    }
    // Emit event so other pages can react to settings changes
    let _ = app.emit("settings-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn settings_pick_folder(
    _app: AppHandle,
    field: String,
) -> Result<String, String> {
    log::info!("settings_pick_folder: field={}", field);
    let title = match field.as_str() {
        "model_dir" => "Select Model Directory",
        "recordings_dir" => "Select Recordings Directory",
        "datasets_dir" => "Select Datasets Directory",
        "books_dir" => "Select Books Library Directory",
        "wiki_dir" => "Select Wiki Directory",
        "dataset_location" => "Select Datasets Location",
        _ => return Err(format!("Unknown field: {}", field)),
    };

    // Use rfd for folder picking since tauri-plugin-dialog doesn't support it on mobile
    #[cfg(all(feature = "desktop", not(any(target_os = "android", target_os = "ios"))))]
    {
        use rfd::FileDialog;
        let path = FileDialog::new()
            .set_title(title)
            .pick_folder();

        match path {
            Some(p) => Ok(p.to_string_lossy().into_owned()),
            None => Err("No folder selected".into()),
        }
    }

    #[cfg(any(target_os = "android", target_os = "ios", not(feature = "desktop")))]
    {
        // Mobile platforms (and the feature-gated mobile build) don't support
        // the native rfd folder picker.
        let _ = title;
        Err("Folder selection is not supported on mobile platforms. Please configure paths manually.".into())
    }
}

// ---------------------------------------------------------------------------
// Separate Global / Workspace commands
// ---------------------------------------------------------------------------

/// Get global settings (always from global config path).
#[tauri::command]
pub async fn settings_get_global(
    state: State<'_, SettingsState>,
) -> Result<GlobalSettings, String> {
    let s = state.settings.lock().unwrap().clone();
    Ok(GlobalSettings::from_settings(&s))
}

/// Save global settings (always to global config path).
#[tauri::command]
pub async fn settings_set_global(
    app: AppHandle,
    state: State<'_, SettingsState>,
    global: GlobalSettings,
) -> Result<(), String> {
    SettingsState::save_global(&global)?;
    log::info!("[Settings] Global settings updated");
    
    // Update in-memory state
    {
        let mut s = state.settings.lock().unwrap();
        global.apply_to(&mut *s);
    }
    
    let _ = app.emit("settings-changed", ());
    Ok(())
}

/// Get workspace settings (from workspace config path, or global if no workspace).
#[tauri::command]
pub async fn settings_get_workspace(
    state: State<'_, SettingsState>,
) -> Result<WorkspaceSettings, String> {
    let ws_dir = state.workspace_dir.lock().unwrap().clone();
    let ws = SettingsState::load_workspace(ws_dir.as_ref());
    // Return effective (workspace-derived) paths
    let mut result = ws;
    let s = state.settings.lock().unwrap().clone();
    result.datasets_dir = state.datasets_dir().to_string_lossy().into_owned();
    result.recordings_dir = state.recordings_dir().to_string_lossy().into_owned();
    result.books_dir = state.books_dir().to_string_lossy().into_owned();
    result.wiki_dir = state.wiki_dir().to_string_lossy().into_owned();
    // Suppress unused variable warning
    let _ = s;
    Ok(result)
}

/// Save workspace settings (to workspace config path, or global if no workspace).
#[tauri::command]
pub async fn settings_set_workspace(
    app: AppHandle,
    state: State<'_, SettingsState>,
    workspace: WorkspaceSettings,
) -> Result<(), String> {
    let ws_dir = state.workspace_dir.lock().unwrap().clone();
    SettingsState::save_workspace(&workspace, ws_dir.as_ref())?;
    log::info!("[Settings] Workspace settings updated");
    
    // Update in-memory state
    {
        let mut s = state.settings.lock().unwrap();
        workspace.apply_to(&mut *s);
    }
    
    let _ = app.emit("settings-changed", ());
    Ok(())
}
