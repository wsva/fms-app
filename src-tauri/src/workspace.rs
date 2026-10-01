use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Workspace {
    pub uuid: String,
    pub name: String,
    /// Email address of the user who owns this workspace.
    /// Empty or "local" means unclaimed.
    pub user_id: String,
    /// Emoji or short string for visual identification.
    pub avatar: String,
    pub created_at: String,
    pub last_accessed: String,
    /// If true, auto-select this workspace on next launch (skip chooser).
    pub auto_login: bool,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WorkspaceRegistry {
    pub workspaces: Vec<Workspace>,
    /// UUID of the last active workspace (for auto-select on launch).
    pub last_active: Option<String>,
}

impl Default for WorkspaceRegistry {
    fn default() -> Self {
        Self {
            workspaces: Vec::new(),
            last_active: None,
        }
    }
}

pub struct WorkspaceState {
    pub current: Mutex<Option<Workspace>>,
    pub registry: Mutex<WorkspaceRegistry>,
}

impl WorkspaceState {
    pub fn new() -> Self {
        let registry = Self::load_registry().unwrap_or_default();
        Self {
            current: Mutex::new(None),
            registry: Mutex::new(registry),
        }
    }
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Base data directory: `{data_dir}/fms-app/`
fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("fms-app")
}

/// Workspace root: `{data_dir}/fms-app/workspaces/`
pub fn workspace_base_dir() -> PathBuf {
    app_data_dir().join("workspaces")
}

/// Specific workspace directory: `{data_dir}/fms-app/workspaces/{uuid}/`
pub fn workspace_dir(uuid: &str) -> PathBuf {
    workspace_base_dir().join(uuid)
}

/// Registry file path: `{data_dir}/fms-app/workspaces.json`
fn registry_path() -> PathBuf {
    app_data_dir().join("workspaces.json")
}

// ---------------------------------------------------------------------------
// Registry I/O
// ---------------------------------------------------------------------------

impl WorkspaceState {
    pub fn load_registry() -> Option<WorkspaceRegistry> {
        let path = registry_path();
        log::debug!("[Workspace] Loading registry from: {}", path.display());
        let data = fs::read_to_string(&path).ok()?;
        // Tolerate a UTF-8 BOM (Windows editors/PowerShell often add one).
        let data = data.trim_start_matches('\u{FEFF}');
        let registry: WorkspaceRegistry =
            serde_json::from_str(data).ok()?;
        log::debug!("[Workspace] Loaded {} workspaces", registry.workspaces.len());
        Some(registry)
    }

    pub fn save_registry(registry: &WorkspaceRegistry) -> Result<(), String> {
        let path = registry_path();
        log::debug!("[Workspace] Saving registry to: {}", path.display());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                log::error!("[Workspace] Failed to create registry directory: {}", e);
                e.to_string()
            })?;
        }
        let data = serde_json::to_string_pretty(registry).map_err(|e| {
            log::error!("[Workspace] Failed to serialize registry: {}", e);
            e.to_string()
        })?;
        fs::write(&path, data).map_err(|e| {
            log::error!("[Workspace] Failed to write registry: {}", e);
            e.to_string()
        })?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Workspace file I/O
// ---------------------------------------------------------------------------

pub fn save_workspace_json(ws: &Workspace) -> Result<(), String> {
    let dir = workspace_dir(&ws.uuid);
    fs::create_dir_all(&dir).map_err(|e| {
        log::error!("[Workspace] Failed to create workspace dir '{}': {}", ws.uuid, e);
        e.to_string()
    })?;
    let path = dir.join("workspace.json");
    let data = serde_json::to_string_pretty(ws).map_err(|e| {
        log::error!("[Workspace] Failed to serialize workspace.json: {}", e);
        e.to_string()
    })?;
    fs::write(&path, data).map_err(|e| {
        log::error!("[Workspace] Failed to write workspace.json: {}", e);
        e.to_string()
    })?;
    log::debug!("[Workspace] Saved workspace.json: {}", path.display());
    Ok(())
}

/// Load workspace.json metadata from a workspace directory.
/// Returns None if the file is missing or cannot be parsed.
pub fn load_workspace_json(ws_dir: &Path) -> Option<Workspace> {
    let path = ws_dir.join("workspace.json");
    let data = fs::read_to_string(&path).ok()?;
    // Tolerate a UTF-8 BOM (Windows editors/PowerShell often add one).
    let data = data.trim_start_matches('\u{FEFF}');
    serde_json::from_str(data)
        .map_err(|e| {
            log::warn!("[Workspace] Failed to parse {}: {}", path.display(), e);
            e
        })
        .ok()
}

// ---------------------------------------------------------------------------
// Core operations
// ---------------------------------------------------------------------------

/// Create a new workspace with the given name.
/// Creates the directory structure and default subdirectories.
fn create_workspace_internal(name: &str) -> Result<Workspace, String> {
    let uuid = uuid::Uuid::new_v4().to_string().replace('-', "");
    let now = chrono::Utc::now().to_rfc3339();

    let ws = Workspace {
        uuid: uuid.clone(),
        name: name.to_string(),
        user_id: String::new(),
        avatar: String::new(),
        created_at: now.clone(),
        last_accessed: now,
        auto_login: false,
    };

    // Create workspace directory and default subdirectories
    let ws_dir = workspace_dir(&uuid);
    let subdirs = ["datasets", "recordings", "books", "wiki"];
    for subdir in &subdirs {
        let path = ws_dir.join(subdir);
        fs::create_dir_all(&path).map_err(|e| {
            log::error!("[Workspace] Failed to create subdir '{}': {}", path.display(), e);
            format!("Failed to create subdirectory '{}': {}", subdir, e)
        })?;
    }

    // Save workspace.json
    save_workspace_json(&ws)?;

    log::info!("[Workspace] Created workspace '{}' (uuid={})", name, uuid);
    Ok(ws)
}

/// Select a workspace: set it as current, update last_accessed and last_active.
fn select_workspace_internal(
    registry: &mut WorkspaceRegistry,
    uuid: &str,
) -> Result<Workspace, String> {
    let ws = registry
        .workspaces
        .iter_mut()
        .find(|w| w.uuid == uuid)
        .ok_or_else(|| {
            log::error!("[Workspace] Workspace not found: {}", uuid);
            format!("Workspace not found: {}", uuid)
        })?;

    let now = chrono::Utc::now().to_rfc3339();
    ws.last_accessed = now;
    registry.last_active = Some(uuid.to_string());

    let selected = ws.clone();
    log::info!("[Workspace] Selected workspace '{}' (uuid={})", selected.name, selected.uuid);
    Ok(selected)
}

/// Claim a workspace with a user_id.
/// Only writes user_id if the current value is empty or "local".
fn claim_workspace_internal(
    registry: &mut WorkspaceRegistry,
    uuid: &str,
    user_id: &str,
) -> Result<(), String> {
    let ws = registry
        .workspaces
        .iter_mut()
        .find(|w| w.uuid == uuid)
        .ok_or_else(|| {
            log::error!("[Workspace] Workspace not found for claim: {}", uuid);
            format!("Workspace not found: {}", uuid)
        })?;

    if ws.user_id.is_empty() || ws.user_id == "local" {
        log::info!(
            "[Workspace] Claiming workspace '{}' for user '{}'",
            uuid, user_id
        );
        ws.user_id = user_id.to_string();
        Ok(())
    } else {
        log::warn!(
            "[Workspace] Workspace '{}' already claimed by user '{}', ignoring claim from '{}'",
            uuid, ws.user_id, user_id
        );
        Err(format!(
            "Workspace already belongs to user '{}'. Cannot claim for '{}'.",
            ws.user_id, user_id
        ))
    }
}

/// Delete a workspace by moving it to trash.
fn delete_workspace_internal(
    registry: &mut WorkspaceRegistry,
    uuid: &str,
) -> Result<(), String> {
    let idx = registry
        .workspaces
        .iter()
        .position(|w| w.uuid == uuid)
        .ok_or_else(|| {
            log::error!("[Workspace] Workspace not found for deletion: {}", uuid);
            format!("Workspace not found: {}", uuid)
        })?;

    let ws = &registry.workspaces[idx];
    let ws_dir = workspace_dir(uuid);

    if ws_dir.exists() {
        // Move to trash
        let trash_dir = app_data_dir().join("trash");
        fs::create_dir_all(&trash_dir).map_err(|e| {
            log::error!("[Workspace] Failed to create trash directory: {}", e);
            format!("Failed to create trash directory: {}", e)
        })?;

        let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S");
        let trash_name = format!("{}_{}_{}", timestamp, ws.name, uuid);
        let trash_path = trash_dir.join(&trash_name);

        fs::rename(&ws_dir, &trash_path).map_err(|e| {
            log::error!(
                "[Workspace] Failed to move workspace to trash: {} -> {}: {}",
                ws_dir.display(), trash_path.display(), e
            );
            format!("Failed to move workspace to trash: {}", e)
        })?;

        log::info!(
            "[Workspace] Moved workspace '{}' to trash: {}",
            uuid, trash_path.display()
        );
    }

    // Remove from registry
    let removed = registry.workspaces.remove(idx);

    // Clear last_active if it was the deleted workspace
    if registry.last_active.as_deref() == Some(uuid) {
        registry.last_active = registry.workspaces.first().map(|w| w.uuid.clone());
    }

    log::info!(
        "[Workspace] Deleted workspace '{}' (name='{}')",
        uuid, removed.name
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Data migration
// ---------------------------------------------------------------------------

/// Check if there is legacy data that needs migration and perform it.
/// Returns true if migration was performed.
pub fn migrate_legacy_data(registry: &mut WorkspaceRegistry) -> bool {
    let legacy_settings = app_data_dir().join("settings.json");

    // Only migrate if:
    // 1. Legacy settings.json exists
    // 2. No workspaces exist yet
    if !legacy_settings.exists() {
        log::debug!("[Workspace] No legacy settings.json found, skipping migration");
        return false;
    }

    if !registry.workspaces.is_empty() {
        log::debug!("[Workspace] Workspaces already exist, skipping legacy migration");
        return false;
    }

    log::info!("[Workspace] Detected legacy data, starting migration...");

    // Create a default workspace
    match create_workspace_internal("Local") {
        Ok(ws) => {
            let ws_dir = workspace_dir(&ws.uuid);

            // Move legacy settings.json
            let legacy_settings = app_data_dir().join("settings.json");
            let new_settings = ws_dir.join("settings.json");
            if legacy_settings.exists() {
                if let Err(e) = fs::rename(&legacy_settings, &new_settings) {
                    log::warn!("[Workspace] Failed to move settings.json: {}, copying instead", e);
                    // Try copy if rename fails (cross-device)
                    let _ = fs::copy(&legacy_settings, &new_settings);
                    let _ = fs::remove_file(&legacy_settings);
                }
                log::info!("[Workspace] Migrated settings.json");
            }

            // Move legacy app.sqlite3
            let legacy_db = app_data_dir().join("app.sqlite3");
            let new_db = ws_dir.join("app.sqlite3");
            if legacy_db.exists() {
                if let Err(e) = fs::rename(&legacy_db, &new_db) {
                    log::warn!("[Workspace] Failed to move app.sqlite3: {}, copying instead", e);
                    let _ = fs::copy(&legacy_db, &new_db);
                    let _ = fs::remove_file(&legacy_db);
                }
                log::info!("[Workspace] Migrated app.sqlite3");
            }

            // Move legacy directories
            let dirs_to_migrate = ["datasets", "recordings", "books", "wiki"];
            for dirname in &dirs_to_migrate {
                let legacy_dir = app_data_dir().join(dirname);
                let new_dir = ws_dir.join(dirname);
                if legacy_dir.exists() && !new_dir.exists() {
                    if let Err(e) = fs::rename(&legacy_dir, &new_dir) {
                        log::warn!("[Workspace] Failed to move {}: {}", dirname, e);
                    } else {
                        log::info!("[Workspace] Migrated {} directory", dirname);
                    }
                }
            }

            // Add to registry
            registry.workspaces.push(ws.clone());
            registry.last_active = Some(ws.uuid.clone());

            log::info!(
                "[Workspace] Migration complete. Created default workspace '{}' (uuid={})",
                ws.name, ws.uuid
            );
            true
        }
        Err(e) => {
            log::error!("[Workspace] Failed to create default workspace for migration: {}", e);
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize workspace system on app startup.
/// Returns the number of workspaces (for frontend to decide whether to show chooser).
pub fn init_workspaces(state: &WorkspaceState) -> Result<usize, String> {
    let mut registry = state.registry.lock().unwrap();

    // Attempt legacy data migration
    let migrated = migrate_legacy_data(&mut registry);
    if migrated {
        // Save the updated registry
        WorkspaceState::save_registry(&registry)?;
    }

    // If no workspaces, create a default one
    if registry.workspaces.is_empty() {
        log::info!("[Workspace] No workspaces found, creating default workspace");
        let ws = create_workspace_internal("Local")?;
        registry.workspaces.push(ws);
        registry.last_active = None; // Will be set on first selection
        WorkspaceState::save_registry(&registry)?;
    }

    let count = registry.workspaces.len();
    log::info!("[Workspace] Initialized with {} workspace(s)", count);
    Ok(count)
}

/// Auto-select a workspace if possible (0 or 1 workspace, or last_active with auto_login).
/// Returns the selected workspace, or None if chooser is needed.
pub fn auto_select_workspace(state: &WorkspaceState) -> Option<Workspace> {
    let mut registry = state.registry.lock().unwrap();

    // If only one workspace, auto-select it
    if registry.workspaces.len() == 1 {
        let uuid = registry.workspaces[0].uuid.clone();
        match select_workspace_internal(&mut registry, &uuid) {
            Ok(ws) => {
                let _ = WorkspaceState::save_registry(&registry);
                let mut current = state.current.lock().unwrap();
                *current = Some(ws.clone());
                return Some(ws);
            }
            Err(e) => {
                log::error!("[Workspace] Failed to auto-select single workspace: {}", e);
                return None;
            }
        }
    }

    // If last_active is set and that workspace has auto_login, select it
    if let Some(ref last_uuid) = registry.last_active.clone() {
        if let Some(ws) = registry.workspaces.iter().find(|w| w.uuid == *last_uuid) {
            if ws.auto_login {
                match select_workspace_internal(&mut registry, last_uuid) {
                    Ok(ws) => {
                        let _ = WorkspaceState::save_registry(&registry);
                        let mut current = state.current.lock().unwrap();
                        *current = Some(ws.clone());
                        log::info!(
                            "[Workspace] Auto-selected workspace '{}' (auto_login=true)",
                            ws.name
                        );
                        return Some(ws);
                    }
                    Err(e) => {
                        log::warn!("[Workspace] Failed to auto-select last workspace: {}", e);
                    }
                }
            }
        }
    }

    // Multiple workspaces, no auto_login -> need chooser
    log::info!(
        "[Workspace] {} workspaces available, showing chooser",
        registry.workspaces.len()
    );
    None
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn workspace_list(
    state: State<'_, WorkspaceState>,
) -> Result<Vec<Workspace>, String> {
    let registry = state.registry.lock().unwrap();
    log::debug!("[Workspace] Listing {} workspaces", registry.workspaces.len());
    Ok(registry.workspaces.clone())
}

#[tauri::command]
pub async fn workspace_get_current(
    state: State<'_, WorkspaceState>,
) -> Result<Option<Workspace>, String> {
    let current = state.current.lock().unwrap();
    Ok(current.clone())
}

#[tauri::command]
pub async fn workspace_create(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    name: String,
) -> Result<Workspace, String> {
    let ws = create_workspace_internal(&name)?;

    let mut registry = state.registry.lock().unwrap();
    registry.workspaces.push(ws.clone());
    WorkspaceState::save_registry(&registry)?;

    // Notify frontend
    let _ = app.emit("workspace-changed", &ws);

    Ok(ws)
}

#[tauri::command]
pub async fn workspace_select(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    settings_state: State<'_, crate::settings::SettingsState>,
    uuid: String,
) -> Result<Workspace, String> {
    let ws = {
        let mut registry = state.registry.lock().unwrap();
        let ws = select_workspace_internal(&mut registry, &uuid)?;
        WorkspaceState::save_registry(&registry)?;
        ws
    };

    // Set current workspace
    {
        let mut current = state.current.lock().unwrap();
        *current = Some(ws.clone());
    }

    // Set workspace directory in settings state (this also reloads settings)
    {
        let ws_dir = workspace_dir(&uuid);
        log::info!("[Workspace] Setting workspace dir for settings: {}", ws_dir.display());
        settings_state.set_workspace_dir(Some(ws_dir));
    }

    // Notify frontend
    let _ = app.emit("workspace-selected", &ws);

    Ok(ws)
}

#[tauri::command]
pub async fn workspace_delete(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    uuid: String,
) -> Result<(), String> {
    // Don't allow deleting the currently active workspace
    {
        let current = state.current.lock().unwrap();
        if let Some(ref ws) = *current {
            if ws.uuid == uuid {
                return Err("Cannot delete the currently active workspace. Switch to another workspace first.".to_string());
            }
        }
    }

    {
        let mut registry = state.registry.lock().unwrap();
        delete_workspace_internal(&mut registry, &uuid)?;
        WorkspaceState::save_registry(&registry)?;
    }

    // Notify frontend
    let _ = app.emit("workspace-changed", ());

    Ok(())
}

#[tauri::command]
pub async fn workspace_rename(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    uuid: String,
    name: String,
) -> Result<(), String> {
    let mut registry = state.registry.lock().unwrap();
    let ws = registry
        .workspaces
        .iter_mut()
        .find(|w| w.uuid == uuid)
        .ok_or_else(|| format!("Workspace not found: {}", uuid))?;

    let old_name = ws.name.clone();
    ws.name = name.clone();
    save_workspace_json(ws)?;
    WorkspaceState::save_registry(&registry)?;

    // Update current if this is the active workspace
    {
        let mut current = state.current.lock().unwrap();
        if let Some(ref mut ws) = *current {
            if ws.uuid == uuid {
                ws.name = name.clone();
            }
        }
    }

    log::info!(
        "[Workspace] Renamed workspace '{}' from '{}' to '{}'",
        uuid, old_name, name
    );

    // Notify frontend
    let _ = app.emit("workspace-changed", ());

    Ok(())
}

#[tauri::command]
pub async fn workspace_claim(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    user_id: String,
) -> Result<(), String> {
    let uuid = {
        let current = state.current.lock().unwrap();
        match current.as_ref() {
            Some(ws) => ws.uuid.clone(),
            None => return Err("No workspace selected".to_string()),
        }
    };

    {
        let mut registry = state.registry.lock().unwrap();
        claim_workspace_internal(&mut registry, &uuid, &user_id)?;

        // Update current workspace
        {
            let mut current = state.current.lock().unwrap();
            if let Some(ref mut ws) = *current {
                ws.user_id = user_id.clone();
            }
        }

        // Save workspace.json
        let ws = registry.workspaces.iter().find(|w| w.uuid == uuid).unwrap().clone();
        save_workspace_json(&ws)?;
        WorkspaceState::save_registry(&registry)?;
    }

    log::info!("[Workspace] Claimed workspace '{}' for user '{}'", uuid, user_id);

    // Notify frontend
    let _ = app.emit("workspace-changed", ());

    Ok(())
}

#[tauri::command]
pub async fn workspace_set_auto_login(
    state: State<'_, WorkspaceState>,
    uuid: String,
    auto_login: bool,
) -> Result<(), String> {
    let mut registry = state.registry.lock().unwrap();
    let ws = registry
        .workspaces
        .iter_mut()
        .find(|w| w.uuid == uuid)
        .ok_or_else(|| format!("Workspace not found: {}", uuid))?;

    ws.auto_login = auto_login;
    save_workspace_json(ws)?;
    WorkspaceState::save_registry(&registry)?;

    // Update current if this is the active workspace
    {
        let mut current = state.current.lock().unwrap();
        if let Some(ref mut ws) = *current {
            if ws.uuid == uuid {
                ws.auto_login = auto_login;
            }
        }
    }

    log::info!(
        "[Workspace] Set auto_login={} for workspace '{}'",
        auto_login, uuid
    );

    Ok(())
}
