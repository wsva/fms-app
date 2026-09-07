use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::State;

use crate::dictation::open_app_db;
use crate::settings::SettingsState;

const BASE_URL: &str = "https://lusworkshop.site";

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
// Credential persistence (app database)
// ---------------------------------------------------------------------------

fn save_credentials(nickname: &str, password: &str) -> Result<(), String> {
    let conn = open_app_db()?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS auth_credentials (
            id       INTEGER PRIMARY KEY CHECK (id = 1),
            nickname TEXT NOT NULL,
            password TEXT NOT NULL
        );",
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO auth_credentials (id, nickname, password) VALUES (1, ?1, ?2)",
        rusqlite::params![nickname, password],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn load_credentials() -> Result<Option<(String, String)>, String> {
    let conn = open_app_db()?;
    // Table might not exist yet
    let exists: bool = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='auth_credentials'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);
    if !exists {
        return Ok(None);
    }
    let result = conn
        .query_row(
            "SELECT nickname, password FROM auth_credentials WHERE id = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .ok();
    Ok(result)
}

fn delete_credentials() -> Result<(), String> {
    let conn = open_app_db()?;
    conn.execute("DELETE FROM auth_credentials", [])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn auth_login(
    _settings: State<'_, SettingsState>,
    nickname: String,
    password: String,
) -> Result<AuthUser, String> {
    let client = reqwest::Client::new();

    let body = serde_json::json!({
        "data": {
            "Nickname": nickname,
            "Password": password
        }
    });

    let resp = client
        .post(format!("{}/api/oauth2/signin", BASE_URL))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Login request failed: {}", e))?;

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse login response: {}", e))?;

    let success = json["success"].as_bool().unwrap_or(false);
    if !success {
        let err = json["message"].as_str().unwrap_or("Login failed");
        return Err(err.to_string());
    }

    let list = json["data"]["list"]
        .as_array()
        .ok_or("Invalid login response: missing list")?;

    if list.len() < 4 {
        return Err("Invalid login response: list too short".to_string());
    }

    let access_token = list[0]
        .as_str()
        .ok_or("Invalid access token")?
        .to_string();
    let user_id = list[1]
        .as_str()
        .ok_or("Invalid user id")?
        .to_string();
    let username = list[2]
        .as_str()
        .ok_or("Invalid username")?
        .to_string();
    let refresh_token = list[3]
        .as_str()
        .ok_or("Invalid refresh token")?
        .to_string();

    let tokens = AuthTokens {
        access_token,
        refresh_token,
        user_id,
        username,
        email: String::new(),
    };
    write_tokens(&tokens)?;

    // Save credentials for auto-re-login
    save_credentials(&nickname, &password)?;

    // Fetch user info
    let user = fetch_user_info(&tokens.access_token).await?;

    // Persist email in tokens
    let tokens = AuthTokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        user_id: tokens.user_id,
        username: tokens.username,
        email: user.email.clone(),
    };
    let _ = write_tokens(&tokens);

    Ok(user)
}

#[tauri::command]
pub async fn auth_get_user(
    _settings: State<'_, SettingsState>,
) -> Result<Option<AuthUser>, String> {
    let tokens = match read_tokens()? {
        Some(t) => t,
        None => {
            // No tokens -- try auto-login from stored credentials
            return try_auto_login().await;
        }
    };

    match fetch_user_info(&tokens.access_token).await {
        Ok(user) => Ok(Some(user)),
        Err(_) => {
            // Token expired -- try auto-login from stored credentials
            try_auto_login().await
        }
    }
}

#[tauri::command]
pub async fn auth_logout(_settings: State<'_, SettingsState>) -> Result<(), String> {
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
    delete_credentials()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal
// ---------------------------------------------------------------------------

/// Try to login using stored credentials (for auto-login on app restart / token expiry).
async fn try_auto_login() -> Result<Option<AuthUser>, String> {
    let (nickname, password) = match load_credentials()? {
        Some(creds) => creds,
        None => return Ok(None),
    };

    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "data": {
            "Nickname": nickname,
            "Password": password
        }
    });

    let resp = match client
        .post(format!("{}/api/oauth2/signin", BASE_URL))
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };

    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(_) => return Ok(None),
    };

    if !json["success"].as_bool().unwrap_or(false) {
        return Ok(None);
    }

    let list = match json["data"]["list"].as_array() {
        Some(l) if l.len() >= 4 => l,
        _ => return Ok(None),
    };

    let access_token = list[0].as_str().unwrap_or("").to_string();
    let user_id = list[1].as_str().unwrap_or("").to_string();
    let username = list[2].as_str().unwrap_or("").to_string();
    let refresh_token = list[3].as_str().unwrap_or("").to_string();

    if access_token.is_empty() {
        return Ok(None);
    }

    let tokens = AuthTokens {
        access_token,
        refresh_token,
        user_id,
        username,
        email: String::new(),
    };
    let _ = write_tokens(&tokens);

    match fetch_user_info(&tokens.access_token).await {
        Ok(user) => {
            // Persist email in tokens
            let tokens = AuthTokens {
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token,
                user_id: tokens.user_id,
                username: tokens.username,
                email: user.email.clone(),
            };
            let _ = write_tokens(&tokens);
            Ok(Some(user))
        }
        Err(_) => Ok(None),
    }
}

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

/// Get the current logged-in user's email (for use by other modules).
pub(crate) fn get_current_user_email() -> String {
    read_tokens()
        .ok()
        .flatten()
        .map(|t| t.email)
        .unwrap_or_default()
}
