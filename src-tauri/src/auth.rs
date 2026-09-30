use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{Emitter, State};

use crate::settings::SettingsState;

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

fn auth_file_path() -> Result<PathBuf, String> {
    let dir = dirs::data_dir()
        .ok_or("Could not determine data directory")?
        .join("fms-app");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create auth dir: {}", e))?;
    Ok(dir.join("auth.json"))
}

fn read_tokens() -> Result<Option<AuthTokens>, String> {
    let path = auth_file_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read auth file: {}", e))?;
    let tokens: AuthTokens =
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse auth file: {}", e))?;
    Ok(Some(tokens))
}

fn write_tokens(tokens: &AuthTokens) -> Result<(), String> {
    let path = auth_file_path()?;
    let json = serde_json::to_string_pretty(tokens)
        .map_err(|e| format!("Failed to serialize tokens: {}", e))?;
    std::fs::write(&path, json).map_err(|e| format!("Failed to write auth file: {}", e))?;
    Ok(())
}

fn delete_tokens() -> Result<(), String> {
    let path = auth_file_path()?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Failed to delete auth file: {}", e))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Open the browser to the login page. After login, the server redirects to
/// `fms-app://login?access_token=...&user_id=...&username=...&refresh_token=...`
/// which is handled by the deep link handler in lib.rs.
#[tauri::command]
pub async fn auth_open_login() -> Result<String, String> {
    log::info!("auth_open_login: opening browser");
    let callback = format!("{}://login", DEEP_LINK_SCHEME);
    let login_url = format!(
        "{}/oauth2/login?desktop_callback={}",
        BASE_URL,
        urlencoding::encode(&callback)
    );
    log::info!("auth_open_login: url={}", login_url);

    // Open the URL in the default browser.
    open_url(&login_url)?;
    Ok(login_url)
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

    let tokens = AuthTokens {
        access_token,
        refresh_token,
        user_id,
        username,
        email: String::new(),
    };
    write_tokens(&tokens)?;

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
    let _ = write_tokens(&tokens);

    // Notify frontend that login succeeded.
    let _ = app.emit("auth-login-success", &user);

    Ok(user)
}

/// Get the currently logged-in user, or None if not logged in.
#[tauri::command]
pub async fn auth_get_user(
    _settings: State<'_, SettingsState>,
) -> Result<Option<AuthUser>, String> {
    let tokens = match read_tokens()? {
        Some(t) => t,
        None => return Ok(None),
    };

    match fetch_user_info(&tokens.access_token).await {
        Ok(user) => Ok(Some(user)),
        Err(e) => {
            log::warn!("auth_get_user: token invalid, clearing: {}", e);
            let _ = delete_tokens();
            Ok(None)
        }
    }
}

/// Log out: clear tokens and notify the server.
#[tauri::command]
pub async fn auth_logout(
    app: tauri::AppHandle,
    _settings: State<'_, SettingsState>,
) -> Result<(), String> {
    log::info!("auth_logout");
    let tokens = read_tokens()?;

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

    delete_tokens()?;
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

/// Get the current logged-in user's identifier (email preferred, fallback to user_id).
/// Used by other modules (e.g., XP, dictation) to identify the user.
pub(crate) fn get_current_user_email() -> String {
    let tokens = match read_tokens().ok().flatten() {
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

/// Open a URL in the default browser.
fn open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .map_err(|e| format!("Failed to open browser: {}", e))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|e| format!("Failed to open browser: {}", e))?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|e| format!("Failed to open browser: {}", e))?;
    }
    Ok(())
}
