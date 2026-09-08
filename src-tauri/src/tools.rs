use std::process::Command;

use rusqlite::Connection;
use tauri::{AppHandle, Manager, State};

use crate::dataset::find_dataset_dir;
use crate::settings::SettingsState;

// ============================================================
// Helpers
// ============================================================

/// Resolve the path to a bundled Python script in the resources directory.
fn resolve_script(app: &AppHandle, script_name: &str) -> Result<std::path::PathBuf, String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|e| format!("Failed to resolve resource directory: {}", e))?;

    let script_path = resource_dir.join("tools").join(script_name);
    if script_path.exists() {
        Ok(script_path)
    } else {
        Err(format!(
            "Script not found: {}. Expected in the resources/tools/ directory.",
            script_path.display()
        ))
    }
}

/// Run a Python script and return its stdout.
fn run_python_script(script_path: &std::path::Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("python")
        .arg(script_path)
        .args(args)
        .output()
        .map_err(|e| format!("Failed to execute python: {}. Is Python installed?", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Python script failed: {}", stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

// ============================================================
// Tauri commands
// ============================================================

/// Write transcript files to the listen_transcript table.
#[tauri::command]
pub async fn dataset_write_transcripts(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<String, String> {
    let dataset_dir = find_dataset_dir(&settings, &dataset_uuid)?;
    let db_path = dataset_dir.join("data.sqlite3");
    let transcript_dir = dataset_dir.join("transcript");

    if !db_path.exists() {
        return Err("Database file not found. Please generate the database first.".into());
    }
    if !transcript_dir.exists() {
        return Err("No transcript directory found in dataset.".into());
    }

    let script = resolve_script(&app, "write_transcripts.py")?;
    run_python_script(
        &script,
        &[
            db_path.to_str().unwrap_or(""),
            transcript_dir.to_str().unwrap_or(""),
        ],
    )
}

/// Validate book.txt exists and can be parsed.
#[tauri::command]
pub async fn dataset_parse_book(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<String, String> {
    let dataset_dir = find_dataset_dir(&settings, &dataset_uuid)?;
    let book_path = dataset_dir.join("book.txt");
    let db_path = dataset_dir.join("data.sqlite3");

    if !book_path.exists() {
        return Err("book.txt not found in dataset directory.".into());
    }
    if !db_path.exists() {
        return Err("Database file not found. Please generate the database first.".into());
    }

    let script = resolve_script(&app, "split_book.py")?;
    run_python_script(
        &script,
        &[
            db_path.to_str().unwrap_or(""),
            book_path.to_str().unwrap_or(""),
        ],
    )
}

/// Align subtitle cues with reference text from book.txt.
/// Reads book.txt directly, writes matched text to listen_subtitle_cue.reference.
#[tauri::command]
pub async fn dataset_align_cues(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<String, String> {
    let dataset_dir = find_dataset_dir(&settings, &dataset_uuid)?;
    let db_path = dataset_dir.join("data.sqlite3");

    if !db_path.exists() {
        return Err("Database file not found. Please generate the database first.".into());
    }

    let script = resolve_script(&app, "align_cue.py")?;

    // Auto-discover all subtitle UUIDs from the database
    let subtitle_uuids: Vec<String> = {
        let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT DISTINCT subtitle_uuid FROM listen_subtitle_cue")
            .map_err(|e| e.to_string())?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut uuids = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            uuids.push(row.get::<_, String>(0).map_err(|e| e.to_string())?);
        }
        uuids
    };

    if subtitle_uuids.is_empty() {
        return Err("No subtitles found in database. Generate subtitles first.".into());
    }

    let mut results = Vec::new();
    for subtitle_uuid in &subtitle_uuids {
        let output = run_python_script(
            &script,
            &[db_path.to_str().unwrap_or(""), subtitle_uuid],
        )?;
        results.push(format!("{}: {}", &subtitle_uuid[..8], output.trim()));
    }

    Ok(results.join("\n"))
}
