use std::process::Command;
use std::sync::Mutex;

use tauri::State;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Holds Tesseract configuration.
pub struct OcrState {
    /// Path to tesseract executable (cached after first lookup).
    tesseract_path: Mutex<Option<String>>,
}

impl OcrState {
    pub fn new() -> Self {
        Self {
            tesseract_path: Mutex::new(None),
        }
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// List available Tesseract languages (from `tesseract --list-langs`).
#[tauri::command]
pub async fn ocr_list_languages(state: State<'_, OcrState>) -> Result<Vec<String>, String> {
    // Find tesseract executable.
    let tesseract_exe = {
        let mut path_guard = state.tesseract_path.lock().unwrap();
        if let Some(ref path) = *path_guard {
            path.clone()
        } else {
            let path = find_tesseract()?;
            *path_guard = Some(path.clone());
            path
        }
    };

    // Run `tesseract --list-langs`.
    let langs = tokio::task::spawn_blocking({
        let tesseract_exe = tesseract_exe.clone();
        move || {
            let output = Command::new(&tesseract_exe)
                .arg("--list-langs")
                .output()
                .map_err(|e| format!("Failed to run tesseract: {}", e))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("Tesseract failed: {}", stderr.trim()));
            }

            // Parse output: first line is header, rest are language codes.
            let stdout = String::from_utf8_lossy(&output.stdout);
            let langs: Vec<String> = stdout
                .lines()
                .skip(1) // Skip "List of available languages in traineddata models:"
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();

            Ok(langs)
        }
    })
    .await
    .map_err(|e| format!("Task failed: {}", e))?;

    langs
}

/// Recognize text in a base64-encoded image using Tesseract OCR.
#[tauri::command]
pub async fn ocr_recognize(
    state: State<'_, OcrState>,
    image_base64: String,
    lang: Option<String>,
) -> Result<String, String> {
    // Find tesseract executable.
    let tesseract_exe = {
        let mut path_guard = state.tesseract_path.lock().unwrap();
        if let Some(ref path) = *path_guard {
            path.clone()
        } else {
            let path = find_tesseract()?;
            *path_guard = Some(path.clone());
            path
        }
    };

    // Decode base64 image and save to temp file.
    let image_data = decode_base64_image(&image_base64)?;
    let temp_dir = std::env::temp_dir();
    let input_path = temp_dir.join("fms_ocr_input.png");
    let output_base = temp_dir.join("fms_ocr_output");

    // Write image to temp file.
    std::fs::write(&input_path, &image_data)
        .map_err(|e| format!("Failed to write temp file: {}", e))?;

    // Build tesseract command.
    let lang_arg = lang.as_deref().unwrap_or("eng");
    let output_path = format!("{}.txt", output_base.display());

    // Run tesseract.
    let result = tokio::task::spawn_blocking({
        let tesseract_exe = tesseract_exe.clone();
        let input_path = input_path.clone();
        let output_base = output_base.clone();
        let lang_arg = lang_arg.to_string();
        move || {
            let output = Command::new(&tesseract_exe)
                .arg(&input_path)
                .arg(&output_base)
                .arg("-l")
                .arg(&lang_arg)
                .output()
                .map_err(|e| format!("Failed to run tesseract: {}", e))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("Tesseract failed: {}", stderr.trim()));
            }

            // Read output file.
            std::fs::read_to_string(&format!("{}.txt", output_base.display()))
                .map_err(|e| format!("Failed to read output: {}", e))
        }
    })
    .await
    .map_err(|e| format!("Task failed: {}", e))?;

    // Cleanup temp files.
    let _ = std::fs::remove_file(&input_path);
    let _ = std::fs::remove_file(&output_path);

    result
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Find the tesseract executable.
fn find_tesseract() -> Result<String, String> {
    // Check common Windows installation paths.
    #[cfg(windows)]
    {
        let program_files = std::env::var("ProgramFiles")
            .unwrap_or_else(|_| "C:\\Program Files".to_string());
        
        // Check Tesseract installation paths.
        let paths = [
            format!("{}\\Tesseract-OCR\\tesseract.exe", program_files),
            format!("{} (x86)\\Tesseract-OCR\\tesseract.exe", program_files),
            "C:\\Program Files\\Tesseract-OCR\\tesseract.exe".to_string(),
        ];

        for path in &paths {
            if std::path::Path::new(path).exists() {
                return Ok(path.clone());
            }
        }
    }

    // Fallback: try PATH.
    if cfg!(windows) {
        Ok("tesseract.exe".to_string())
    } else {
        Ok("tesseract".to_string())
    }
}

/// Decode a base64-encoded image (with or without data URL prefix).
fn decode_base64_image(data_url: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;

    // Strip data URL prefix if present.
    let base64_data = if let Some(idx) = data_url.find(",") {
        &data_url[idx + 1..]
    } else {
        data_url
    };

    let bytes = base64::prelude::BASE64_STANDARD
        .decode(base64_data)
        .map_err(|e| format!("Invalid base64: {}", e))?;

    Ok(bytes)
}
