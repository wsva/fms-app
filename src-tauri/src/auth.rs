use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::settings::SettingsState;
use crate::workspace::WorkspaceState;

const BASE_URL: &str = "https://lusworkshop.site";

/// Deep link scheme used for login callback.
const DEEP_LINK_SCHEME: &str = "fms-app";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
struct AuthTokens {
    access_token: String,
    refresh_token: String,
    user_id: String,
    username: String,
    #[serde(default)]
    email: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct AuthUser {
    pub name: String,
    pub email: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Auth file path: `<workspace>/auth.json` when a workspace is selected,
/// otherwise the legacy global path `{app-data}/fms-app/auth.json`.
fn auth_file_path(settings: &SettingsState) -> Result<PathBuf, String> {
    if let Some(ws_dir) = settings.workspace_dir.lock().unwrap().as_ref() {
        std::fs::create_dir_all(ws_dir)
            .map_err(|e| format!("Failed to create workspace dir: {}", e))?;
        return Ok(ws_dir.join("auth.json"));
    }
    let dir = crate::app_paths::data_root();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create auth dir: {}", e))?;
    Ok(dir.join("auth.json"))
}

/// Legacy global auth file path (used for one-time migration into a workspace).
fn legacy_auth_file_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("fms-app").join("auth.json"))
}

/// Migrate the legacy global auth.json into the workspace (once).
/// The global tokens belonged to the pre-workspace single user, so the first
/// workspace that reads them claims them; later workspaces start logged out.
fn migrate_legacy_auth(settings: &SettingsState) {
    let ws_auth = match auth_file_path(settings) {
        Ok(p) => p,
        Err(_) => return,
    };
    // Only migrate when a workspace is selected and its auth.json is absent.
    if settings.workspace_dir.lock().unwrap().is_none() || ws_auth.exists() {
        return;
    }
    let Some(legacy) = legacy_auth_file_path() else { return; };
    if !legacy.exists() {
        return;
    }
    match std::fs::rename(&legacy, &ws_auth) {
        Ok(_) => log::info!(
            "[Auth] Migrated legacy auth.json into workspace: {}",
            ws_auth.display()
        ),
        Err(e) => log::warn!("[Auth] Failed to migrate legacy auth.json: {}", e),
    }
}

fn read_tokens(settings: &SettingsState) -> Result<Option<AuthTokens>, String> {
    migrate_legacy_auth(settings);
    let path = auth_file_path(settings)?;
    if !path.exists() {
        return Ok(None);
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read auth file: {}", e))?;
    // Tolerate a UTF-8 BOM (Windows editors/PowerShell often add one).
    let content = content.trim_start_matches('\u{FEFF}');
    let tokens: AuthTokens =
        serde_json::from_str(content).map_err(|e| format!("Failed to parse auth file: {}", e))?;
    Ok(Some(tokens))
}

fn write_tokens(settings: &SettingsState, tokens: &AuthTokens) -> Result<(), String> {
    let path = auth_file_path(settings)?;
    let json = serde_json::to_string_pretty(tokens)
        .map_err(|e| format!("Failed to serialize tokens: {}", e))?;
    std::fs::write(&path, json).map_err(|e| format!("Failed to write auth file: {}", e))?;
    log::info!("[Auth] Tokens written to {}", path.display());
    Ok(())
}

fn delete_tokens(settings: &SettingsState) -> Result<(), String> {
    let path = auth_file_path(settings)?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Failed to delete auth file: {}", e))?;
        log::info!("[Auth] Tokens deleted from {}", path.display());
    }
    Ok(())
}

/// Get the user_id from stored tokens without making a network call.
/// Returns None if not logged in.
pub fn get_stored_user_id(settings: &SettingsState) -> Option<String> {
    read_tokens(settings)
        .ok()
        .flatten()
        .map(|t| t.user_id)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Open the browser to the login page. After login, the server redirects to
/// `fms-app://login?access_token=...&user_id=...&username=...&refresh_token=...`
/// which is handled by the deep link handler in lib.rs.
#[tauri::command]
pub async fn auth_open_login(app: AppHandle) -> Result<String, String> {
    log::info!("auth_open_login: opening browser");
    let callback = format!("{}://login", DEEP_LINK_SCHEME);
    let login_url = format!(
        "{}/oauth2/login?desktop_callback={}",
        BASE_URL,
        urlencoding::encode(&callback)
    );
    log::info!("auth_open_login: url={}", login_url);

    // Open the URL in the default browser.
    open_url(&app, &login_url)?;
    Ok(login_url)
}

/// Format a reqwest/IO error with its full source chain. reqwest's top-level
/// Display is just "error sending request for url (...)"; the actual cause
/// (TLS handshake, unknown issuer, DNS, timeout, no crypto provider) lives in
/// the source chain, which is what we need to diagnose Android network errors.
fn err_chain<E: std::error::Error>(err: &E) -> String {
    let mut msg = err.to_string();
    let mut src: Option<&dyn std::error::Error> = err.source();
    while let Some(s) = src {
        msg.push_str(" | caused by: ");
        msg.push_str(&s.to_string());
        src = s.source();
    }
    msg
}

/// Verify credentials directly against the website's first-party signin API and
/// store the returned tokens. This is the in-app login path (primary on
/// Android): the user never leaves the app for a browser OAuth2 + `fms-app://`
/// deep-link round trip, which Chrome on Android silently blocks because the
/// server-issued custom-scheme redirect has no transient user activation.
///
/// Response contract of `POST /api/oauth2/signin` (no `desktop_callback`):
///   success -> { success: true, data: { list: [accessToken, user_id, username, refreshToken] } }
///   failure -> { success: false, errMsg: "..." }
#[tauri::command]
pub async fn auth_login_password(
    app: AppHandle,
    email: String,
    password: String,
) -> Result<AuthUser, String> {
    log::info!("[Auth] login: verifying credentials for '{}'", email);
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "data": { "Nickname": email, "Email": email, "Password": password }
    });
    let resp = client
        .post(format!("{}/api/oauth2/signin", BASE_URL))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            // Log the full source chain so the underlying cause (TLS handshake,
            // unknown issuer, DNS, timeout, no crypto provider) is visible in
            // the in-app Logs page, not just reqwest's generic top-level text.
            let detail = err_chain(&e);
            log::error!("[Auth] login: signin request to {} failed: {}", BASE_URL, detail);
            format!("Signin request failed: {}", detail)
        })?;

    let status = resp.status();
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| {
            log::error!("[Auth] login: failed to parse signin response (status {}): {}", status, e);
            format!("Failed to parse signin response (status {}): {}", status, e)
        })?;

    if json["success"].as_bool() != Some(true) {
        let msg = json["errMsg"].as_str().unwrap_or("Invalid email or password");
        log::warn!("[Auth] login: signin rejected for '{}': {}", email, msg);
        return Err(msg.to_string());
    }

    let list = json["data"]["list"]
        .as_array()
        .ok_or_else(|| "Malformed signin response (missing data.list)".to_string())?;
    let field = |i: usize| list.get(i).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let access_token = field(0);
    let user_id = field(1);
    let username = field(2);
    let refresh_token = field(3);

    if access_token.is_empty() {
        return Err("Signin response contained no access token".to_string());
    }

    log::info!(
        "[Auth] login: signin succeeded for user_id='{}', username='{}'",
        user_id,
        username
    );
    // Reuse the shared post-login pipeline: persist tokens, fetch user info,
    // claim the current workspace, fold logged-out XP, emit `auth-login-success`.
    auth_process_token(app, access_token, refresh_token, user_id, username).await
}

/// Process tokens received from the deep link callback.
/// Called by the deep link handler when `fms-app://login?access_token=...` is received.
pub async fn auth_process_token(
    app: tauri::AppHandle,
    access_token: String,
    refresh_token: String,
    user_id: String,
    username: String,
) -> Result<AuthUser, String> {
    log::info!("auth_process_token: user={}, id={}", username, user_id);

    // Tokens are stored in the currently selected workspace's auth.json
    // (workspace must be selected before login; otherwise legacy global path).
    let settings = app.state::<SettingsState>();

    let tokens = AuthTokens {
        access_token,
        refresh_token,
        user_id: user_id.clone(),
        username,
        email: String::new(),
    };
    write_tokens(&settings, &tokens)?;

    // Fetch user info (includes email).
    let user = fetch_user_info(&tokens.access_token).await?;
    log::info!("auth_process_token success: user={}, email={}", user.name, user.email);

    // Persist email in tokens.
    let tokens = AuthTokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        user_id: tokens.user_id,
        username: tokens.username,
        email: user.email.clone(),
    };
    let _ = write_tokens(&settings, &tokens);

    // Try to claim the current workspace with the user's email.
    let ws_state = app.state::<WorkspaceState>();
    let ws_current = ws_state.current.lock().unwrap().clone();
    if let Some(ws) = ws_current {
        if ws.user_id.is_empty() || ws.user_id == "local" {
            let email_to_claim = if !user.email.is_empty() {
                user.email.clone()
            } else {
                user_id.clone()
            };
            log::info!("[Auth] Claiming workspace '{}' for user '{}'", ws.uuid, email_to_claim);
            
            // Update registry
            {
                let mut registry = ws_state.registry.lock().unwrap();
                if let Some(ws_mut) = registry.workspaces.iter_mut().find(|w| w.uuid == ws.uuid) {
                    ws_mut.user_id = email_to_claim.clone();
                }
                let _ = crate::workspace::WorkspaceState::save_registry(&registry);
            }
            
            // Update workspace.json
            {
                let registry = ws_state.registry.lock().unwrap();
                if let Some(ws_to_save) = registry.workspaces.iter().find(|w| w.uuid == ws.uuid) {
                    let _ = crate::workspace::save_workspace_json(ws_to_save);
                }
            }
            
            // Fold any progress/XP recorded while logged out ("local"/"") into the owner.
            crate::xp::migrate_identity_to_owner(
                &crate::workspace::workspace_dir(&ws.uuid),
                &email_to_claim,
            );

            // Update current workspace state
            {
                let mut current = ws_state.current.lock().unwrap();
                if let Some(ref mut ws) = *current {
                    ws.user_id = email_to_claim;
                }
            }
        } else if ws.user_id != user.email && ws.user_id != user_id {
            log::warn!(
                "[Auth] Workspace '{}' belongs to '{}', but user '{}' logged in",
                ws.uuid, ws.user_id, user.email
            );
        }
    }

    // Notify frontend that login succeeded.
    let _ = app.emit("auth-login-success", &user);

    Ok(user)
}

/// Get the currently logged-in user, or None if not logged in.
#[tauri::command]
pub async fn auth_get_user(
    settings: State<'_, SettingsState>,
) -> Result<Option<AuthUser>, String> {
    let tokens = match read_tokens(&settings)? {
        Some(t) => t,
        None => return Ok(None),
    };

    match fetch_user_info(&tokens.access_token).await {
        Ok(user) => Ok(Some(user)),
        Err(e) => {
            log::warn!("auth_get_user: token invalid, clearing: {}", e);
            let _ = delete_tokens(&settings);
            Ok(None)
        }
    }
}

/// Log out: clear tokens and notify the server.
#[tauri::command]
pub async fn auth_logout(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
) -> Result<(), String> {
    log::info!("auth_logout");
    let tokens = read_tokens(&settings)?;

    if let Some(ref t) = tokens {
        let client = reqwest::Client::new();
        let _ = client
            .post(format!(
                "{}/api/oauth2/logout?user_id={}",
                BASE_URL, t.user_id
            ))
            .header("Authorization", format!("Bearer {}", t.access_token))
            .send()
            .await;
    }

    delete_tokens(&settings)?;
    let _ = app.emit("auth-logout", ());
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal
// ---------------------------------------------------------------------------

async fn fetch_user_info(access_token: &str) -> Result<AuthUser, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/api/oauth2/userinfo", BASE_URL))
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await
        .map_err(|e| format!("Userinfo request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err("Failed to fetch user info".to_string());
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse userinfo response: {}", e))?;

    let name = json["name"].as_str().unwrap_or("").to_string();
    let email = json["email"].as_str().unwrap_or("").to_string();

    Ok(AuthUser { name, email })
}

/// Get the current user's identifier (email preferred, fallback to user_id).
/// Used by other modules (e.g., XP, dictation) to identify the user.
///
/// Resolution order:
/// 1. The current workspace's owner (`workspace.json` -> `user_id`), so that
///    per-workspace progress data is always attributed to the workspace owner
///    even if a different user is logged in globally.
/// 2. The workspace's `auth.json` tokens (unclaimed workspace / no workspace).
pub(crate) fn get_current_user_email(settings: &SettingsState) -> String {
    // Prefer the workspace owner from workspace.json.
    if let Some(ws_dir) = settings.workspace_dir.lock().unwrap().as_ref() {
        if let Some(ws) = crate::workspace::load_workspace_json(ws_dir) {
            if !ws.user_id.is_empty() && ws.user_id != "local" {
                return ws.user_id;
            }
        }
    }
    // Fallback: auth tokens of the current workspace (or legacy global).
    let tokens = match read_tokens(settings).ok().flatten() {
        Some(t) => t,
        None => return String::new(),
    };
    // Prefer email, fall back to user_id if email is empty.
    if !tokens.email.is_empty() {
        tokens.email
    } else if !tokens.user_id.is_empty() {
        tokens.user_id
    } else {
        tokens.username
    }
}

/// Stable identity used to key per-workspace progress and XP data.
///
/// Unlike [`get_current_user_email`], this NEVER returns an empty string, so
/// logged-out practice in an unclaimed workspace is still recorded (and earns
/// XP) under a stable per-workspace sentinel instead of being silently dropped.
///
/// Resolution:
/// 1. Claimed workspace (`workspace.json` -> `user_id`) → the owner email. This
///    is stable across login/logout, so progress/XP never move out from under it.
/// 2. Selected but unclaimed workspace → the sentinel `"local"`. On claim,
///    [`crate::xp::migrate_identity_to_owner`] folds these rows into the owner.
/// 3. No workspace selected (legacy global DB) → the auth identity, else `"local"`.
pub(crate) fn workspace_identity(settings: &SettingsState) -> String {
    // Selected workspace: owner email when claimed, else a stable sentinel.
    if let Some(ws_dir) = settings.workspace_dir.lock().unwrap().as_ref() {
        if let Some(ws) = crate::workspace::load_workspace_json(ws_dir) {
            if !ws.user_id.is_empty() && ws.user_id != "local" {
                return ws.user_id;
            }
        }
        return "local".to_string();
    }
    // No workspace selected (legacy global DB): fall back to the auth identity.
    let id = get_current_user_email(settings);
    if id.is_empty() {
        "local".to_string()
    } else {
        id
    }
}

/// Open a URL in the default browser via the opener plugin (cross-platform,
/// works on desktop and Android/iOS).
fn open_url(app: &AppHandle, url: &str) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app
        .opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("Failed to open browser: {}", e))
}
