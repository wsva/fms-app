//! Tesseract OCR. Desktop-only (see the `desktop` feature gate in `lib.rs`).
//!
//! Two recognition paths share one Tesseract install:
//! - `ocr_recognize` dumps plain text (`<base>.txt`). Layout is lost there, so
//!   re-joining broken lines is left to an LLM pass in the UI.
//! - `ocr_recognize_tsv` asks Tesseract for its TSV layout dump and rebuilds the
//!   paragraphs in Rust (`tsv`): words → lines → paragraphs, plus line-break
//!   hyphen repair against the user's word list. No model involved.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use tauri::State;

use crate::simple_words::SimpleWordsState;

pub(crate) mod tsv;

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
    let tesseract_exe = tesseract_exe(&state)?;

    // Run `tesseract --list-langs`.
    let langs = tokio::task::spawn_blocking({
        let tesseract_exe = tesseract_exe.clone();
        move || {
            let output = crate::no_window(Command::new(&tesseract_exe))
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

/// Recognize text in a base64-encoded image using Tesseract OCR (plain text).
#[tauri::command]
pub async fn ocr_recognize(
    state: State<'_, OcrState>,
    image_base64: String,
    lang: Option<String>,
) -> Result<String, String> {
    let tesseract_exe = tesseract_exe(&state)?;

    // Decode base64 image and save to a unique temp file.
    let image_data = decode_base64_image(&image_base64)?;
    let (input_path, output_base) = temp_paths();
    std::fs::write(&input_path, &image_data)
        .map_err(|e| format!("Failed to write temp file: {}", e))?;

    let lang_arg = lang.as_deref().unwrap_or("eng").to_string();

    // Run tesseract (default output: `<base>.txt`).
    let result = tokio::task::spawn_blocking({
        let tesseract_exe = tesseract_exe.clone();
        let input_path = input_path.clone();
        let output_base = output_base.clone();
        move || {
            run_tesseract(&tesseract_exe, &input_path, &output_base, &lang_arg, None)?;
            std::fs::read_to_string(output_base.with_extension("txt"))
                .map_err(|e| format!("Failed to read output: {}", e))
        }
    })
    .await
    .map_err(|e| format!("Task failed: {}", e))?;

    cleanup(&input_path, &output_base, &["txt"]);

    result
}

/// What `ocr_recognize_tsv` returns: the reconstructed text plus the raw dump.
#[derive(serde::Serialize)]
pub struct OcrTsvResult {
    /// Final text: paragraphs grouped, broken lines joined, hyphenation repaired.
    pub text: String,
    /// Raw Tesseract TSV, so the UI can show what the layout looked like.
    pub tsv: String,
}

/// Recognize text via Tesseract's TSV layout output and rebuild it in Rust.
///
/// The reconstruction groups words into lines, lines into paragraphs (gap,
/// Tesseract ids, indentation) and repairs line-break hyphenation against the
/// words the user has configured for the active language — so a page comes out
/// as clean paragraphs without an LLM.
#[tauri::command]
pub async fn ocr_recognize_tsv(
    state: State<'_, OcrState>,
    sw_state: State<'_, SimpleWordsState>,
    image_base64: String,
    lang: Option<String>,
) -> Result<OcrTsvResult, String> {
    let tesseract_exe = tesseract_exe(&state)?;
    let known_words = known_words(&sw_state);

    // Decode base64 image and save to a unique temp file.
    let image_data = decode_base64_image(&image_base64)?;
    let (input_path, output_base) = temp_paths();
    std::fs::write(&input_path, &image_data)
        .map_err(|e| format!("Failed to write temp file: {}", e))?;

    let lang_arg = lang.as_deref().unwrap_or("eng").to_string();

    let result = tokio::task::spawn_blocking({
        let tesseract_exe = tesseract_exe.clone();
        let input_path = input_path.clone();
        let output_base = output_base.clone();
        let known_words = known_words.clone();
        move || {
            run_tesseract(
                &tesseract_exe,
                &input_path,
                &output_base,
                &lang_arg,
                Some("tsv"),
            )?;

            let tsv = std::fs::read_to_string(output_base.with_extension("tsv"))
                .map_err(|e| format!("Failed to read TSV output: {}", e))?;

            let text = tsv::reconstruct_paragraphs(&tsv, &known_words)
                .map_err(|e| format!("Failed to rebuild text from TSV: {}", e))?;

            log::info!(
                "[OCR] tsv: {} TSV rows, {} output chars, {} dictionary words",
                tsv.lines().count(),
                text.chars().count(),
                known_words.len()
            );

            Ok(OcrTsvResult { text, tsv })
        }
    })
    .await
    .map_err(|e| format!("Task failed: {}", e))?;

    cleanup(&input_path, &output_base, &["tsv"]);

    result
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve (and cache) the tesseract executable path.
fn tesseract_exe(state: &OcrState) -> Result<String, String> {
    let mut path_guard = state.tesseract_path.lock().unwrap();
    if let Some(ref path) = *path_guard {
        return Ok(path.clone());
    }
    let path = find_tesseract()?;
    *path_guard = Some(path.clone());
    Ok(path)
}

/// Run tesseract on `input`, writing `<output_base>.<ext>` for each format
/// (`None` = Tesseract's default plain text).
fn run_tesseract(
    tesseract_exe: &str,
    input: &Path,
    output_base: &Path,
    lang: &str,
    config: Option<&str>,
) -> Result<(), String> {
    let mut cmd = crate::no_window(Command::new(tesseract_exe));
    cmd.arg(input).arg(output_base).arg("-l").arg(lang);
    if let Some(config) = config {
        cmd.arg(config);
    }

    let output = cmd
        .output()
        .map_err(|e| format!("Failed to run tesseract: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Tesseract failed: {}", stderr.trim()));
    }

    Ok(())
}

/// Fresh scratch names for one recognition run.
///
/// Unique per call: fixed names made a plain-text and a TSV run in the same
/// temp dir overwrite each other's input/output.
fn temp_paths() -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    (
        dir.join(format!("fms_ocr_{stamp}_input.png")),
        dir.join(format!("fms_ocr_{stamp}")),
    )
}

/// Remove the scratch input plus any `<base>.<ext>` output files.
fn cleanup(input_path: &Path, output_base: &Path, extensions: &[&str]) {
    let _ = std::fs::remove_file(input_path);
    for extension in extensions {
        let _ = std::fs::remove_file(output_base.with_extension(extension));
    }
}

/// Lowercase dictionary for hyphen repair: every word the user has configured
/// for the active language (Simple Words). Compound entries are skipped since
/// the repair only ever tests fragment+headword.
fn known_words(sw_state: &SimpleWordsState) -> HashSet<String> {
    match sw_state.words.lock() {
        Ok(words) => words
            .iter()
            .map(|word| word.trim().to_lowercase())
            .filter(|word| !word.is_empty() && !word.contains('-') && !word.contains('\u{00AD}'))
            .collect(),
        Err(_) => HashSet::new(),
    }
}

/// Find the tesseract executable.
fn find_tesseract() -> Result<String, String> {
    // Check common Windows installation paths.
    #[cfg(windows)]
    {
        let program_files =
            std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".to_string());

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
