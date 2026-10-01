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
    /// Use Hugging Face mirror (hf-mirror.com) for faster downloads in China.
    /// false = use huggingface.co, true = use hf-mirror.com.
    #[serde(default)]
    pub hf_mirror: bool,
    /// Currently selected model ID.
    #[serde(default)]
    pub selected_model: String,
    /// When to unload the model after inactivity.
    #[serde(default)]
    pub model_unload_timeout: ModelUnloadTimeout,
    /// Whether the user has completed onboarding.
    #[serde(default)]
    pub onboarding_completed: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("fms-app");

        Self {
            model_dir: data_dir.join("models").to_string_lossy().into_owned(),
            recordings_dir: data_dir.join("recordings").to_string_lossy().into_owned(),
            datasets_dir: data_dir.join("datasets").to_string_lossy().into_owned(),
            books_dir: data_dir.join("books").to_string_lossy().into_owned(),
            wiki_dir: data_dir.join("wiki").to_string_lossy().into_owned(),
            hf_mirror: false,
            selected_model: String::new(),
            model_unload_timeout: ModelUnloadTimeout::default(),
            onboarding_completed: false,
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

    fn config_path(workspace_dir: Option<&PathBuf>) -> PathBuf {
        match workspace_dir {
            Some(dir) => dir.join("settings.json"),
            None => dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("fms-app")
                .join("settings.json"),
        }
    }

    /// Resolve a workspace-scoped data sub-directory: `<workspace>/<name>`.
    /// When a workspace is selected it is authoritative; otherwise fall back
    /// to the configured value (or the global app-data default when empty).
    pub fn workspace_subdir(&self, name: &str, configured: &str) -> PathBuf {
        if let Some(ws_dir) = self.workspace_dir.lock().unwrap().as_ref() {
            return ws_dir.join(name);
        }
        if configured.is_empty() {
            return dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("fms-app")
                .join(name);
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

    fn load(workspace_dir: Option<&PathBuf>) -> Option<AppSettings> {
        let path = Self::config_path(workspace_dir);
        log::debug!("[Settings] Loading settings from: {}", path.display());
        let data = fs::read_to_string(path).ok()?;
        // Tolerate a UTF-8 BOM (Windows editors/PowerShell often add one).
        let data = data.trim_start_matches('\u{FEFF}');
        serde_json::from_str(data).map_err(|e| {
            log::warn!("[Settings] Failed to parse settings.json: {} - using defaults", e)
        }).ok()
    }

    pub fn save(settings: &AppSettings, workspace_dir: Option<&PathBuf>) -> Result<(), String> {
        let path = Self::config_path(workspace_dir);
        log::debug!("[Settings] Saving settings to: {}", path.display());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let data = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
        fs::write(path, data).map_err(|e| e.to_string())?;
        Ok(())
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
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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

    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        // Mobile platforms don't support native folder picker
        Err("Folder selection is not supported on mobile platforms. Please configure paths manually.".into())
    }
}
