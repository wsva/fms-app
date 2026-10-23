//! Tesseract OCR. Desktop-only (see the `desktop` feature gate in `lib.rs`).
//!
//! Two recognition paths share one Tesseract install:
//! - `ocr_recognize` dumps plain text (`<base>.txt`). Layout is lost there, so
//!   re-joining broken lines is left to an LLM pass in the UI.
//! - `ocr_recognize_tsv` asks Tesseract for its TSV layout dump and rebuilds the
//!   paragraphs in Rust (`tsv`): words → lines → paragraphs. Line-break hyphens
//!   are then decided by `hyphen` against the downloaded Hunspell dictionaries
//!   of the page's language set (`deu+eng` = German + English). No model
//!   involved.
//!
//! The dictionaries are fetched on demand (`ocr_hyphen_dictionary_*`) rather
//! than bundled: they are data under their own licences (0.5–19 MB each), and
//! most users only ever need the languages they read.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use tauri::State;

use hyphen::{HyphenState, Policy};

pub(crate) mod hyphen;
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
    /// Final text: paragraphs grouped, broken lines joined, and line-break
    /// hyphens folded into one word where the decision said so.
    pub text: String,
    /// The same page with every line-break hyphen kept (`Sprach- verarbeitung`).
    /// The UI re-applies decisions to this when the user flips one.
    pub baseline: String,
    /// Raw Tesseract TSV, so the UI can show what the layout looked like.
    pub tsv: String,
    /// One entry per line-break hyphen, in reading order.
    pub hyphens: Vec<HyphenInfo>,
    /// Every dictionary the page's language set maps to, in selection order,
    /// each with its install state to the UI can offer the missing ones.
    pub dictionaries: Vec<OcrDictionaryInfo>,
    /// Selected Tesseract codes with no dictionary at all (CJK, Arabic, …):
    /// those pages are decided by the shape rule alone.
    pub uncovered: Vec<String>,
}

/// One line-break hyphen and how it was decided.
#[derive(serde::Serialize)]
pub struct HyphenInfo {
    /// Text before the hyphen, hyphen stripped.
    pub left: String,
    /// The hyphen character that ended the line (`-`, `‐`, `‑`).
    pub hyphen: String,
    /// First word of the following line.
    pub right: String,
    /// What joining produces: `left` + `right`.
    pub joined: String,
    /// `join` or `keep`.
    pub decision: String,
    /// Evidence: `dictionary`, `unknown-word`, `lowercase`, `uppercase`,
    /// `digit` or `policy`.
    pub reason: String,
}

/// One dictionary the page's language set maps to.
#[derive(serde::Serialize)]
pub struct OcrDictionaryInfo {
    /// Hunspell/npm code, e.g. `de` — what `ocr_hyphen_dictionary_download` takes.
    pub code: String,
    pub label: String,
    /// The Tesseract code in the selection that mapped to this dictionary
    /// (`deu` for `de`), so the UI can say "Deutsch (deu)".
    pub language: String,
    /// `false` when the dictionary could be downloaded but is not installed.
    pub installed: bool,
}

/// Recognize text via Tesseract's TSV layout output and rebuild it in Rust.
///
/// The reconstruction groups words into lines and lines into paragraphs (gap,
/// Tesseract ids, indentation). Every line-break hyphen comes back as a
/// candidate and is folded into one word when any installed dictionary of the
/// page's language set knows the joined form — so `Sprach-` / `verarbeitung`
/// becomes `Sprachverarbeitung` while `E-` / `Mail` stays as printed. The
/// languages selected for recognition (`deu+eng`) are exactly the dictionaries
/// consulted; a language with no dictionary falls back to the shape rule.
#[tauri::command]
pub async fn ocr_recognize_tsv(
    state: State<'_, OcrState>,
    hyphen_state: State<'_, HyphenState>,
    image_base64: String,
    lang: Option<String>,
    hyphen_policy: Option<String>,
) -> Result<OcrTsvResult, String> {
    let tesseract_exe = tesseract_exe(&state)?;
    let lang_arg = lang.as_deref().unwrap_or("eng").to_string();
    let policy = Policy::parse(hyphen_policy.as_deref());

    // Loading the dictionaries parses them the first time round (over a second
    // for German), so it happens off the async runtime.
    let hyphen_state = hyphen_state.inner().clone();
    let selection = tokio::task::spawn_blocking({
        let hyphen_state = hyphen_state.clone();
        let lang = lang_arg.clone();
        move || hyphen::selection(&lang, &hyphen_state)
    })
    .await
    .map_err(|e| format!("Task failed: {}", e))?;

    // Every dictionary of the language set, installed or not, so the UI can
    // offer the missing ones; the decisions below use all that are there.
    let dictionaries: Vec<OcrDictionaryInfo> = selection
        .entries
        .iter()
        .map(|(source, dictionary)| OcrDictionaryInfo {
            code: source.code.to_string(),
            label: source.label.to_string(),
            language: lang_arg
                .split('+')
                .map(str::trim)
                .find(|code| source.tesseract.contains(code))
                .unwrap_or(source.code)
                .to_string(),
            installed: dictionary.is_some(),
        })
        .collect();
    let uncovered = selection.uncovered.clone();

    // Decode base64 image and save to a unique temp file.
    let image_data = decode_base64_image(&image_base64)?;
    let (input_path, output_base) = temp_paths();
    std::fs::write(&input_path, &image_data)
        .map_err(|e| format!("Failed to write temp file: {}", e))?;

    let result = tokio::task::spawn_blocking({
        let tesseract_exe = tesseract_exe.clone();
        let input_path = input_path.clone();
        let output_base = output_base.clone();
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

            let mut page = tsv::reconstruct(&tsv)
                .map_err(|e| format!("Failed to rebuild text from TSV: {}", e))?;

            // A word known to any dictionary of the set joins, so a German page
            // quoting an English compound needs no second run.
            let loaded = selection.loaded();
            hyphen::resolve(&mut page.candidates, &loaded, policy);
            let text = hyphen::apply(&page.text, &page.candidates);

            let joined = page.candidates.iter().filter(|c| c.join).count();
            let deciding: Vec<&str> = selection
                .entries
                .iter()
                .filter(|(_, dictionary)| dictionary.is_some())
                .map(|(source, _)| source.code)
                .collect();
            log::info!(
                "[OCR] tsv: {} TSV rows, {} output chars, {} line-break hyphens ({} joined, policy {}, dictionaries {})",
                tsv.lines().count(),
                text.chars().count(),
                page.candidates.len(),
                joined,
                policy.as_str(),
                if deciding.is_empty() {
                    "none".to_string()
                } else {
                    deciding.join("+")
                },
            );

            let hyphens = page
                .candidates
                .iter()
                .map(|candidate| HyphenInfo {
                    left: candidate.left.clone(),
                    hyphen: candidate.hyphen.to_string(),
                    right: candidate.right.clone(),
                    joined: candidate.joined.clone(),
                    decision: if candidate.join { "join" } else { "keep" }.to_string(),
                    reason: candidate.reason.to_string(),
                })
                .collect();

            Ok(OcrTsvResult {
                text,
                baseline: page.text,
                tsv,
                hyphens,
                dictionaries,
                uncovered,
            })
        }
    })
    .await
    .map_err(|e| format!("Task failed: {}", e))?;

    cleanup(&input_path, &output_base, &["tsv"]);

    result
}

/// Hunspell dictionaries available for line-break hyphen repair, with their
/// install state and licence.
#[tauri::command]
pub fn ocr_hyphen_dictionary_status() -> Vec<hyphen::DictionaryStatus> {
    hyphen::status_all()
}

/// Download and install the dictionary for `code` (e.g. "de"). Used for
/// line-break hyphen repair in TSV mode; safe to call when already installed.
#[tauri::command]
pub async fn ocr_hyphen_dictionary_download(
    state: State<'_, HyphenState>,
    code: String,
) -> Result<hyphen::DictionaryStatus, String> {
    let status = hyphen::download(&code).await?;
    // Drop the cached copy so the next recognition parses the new files.
    state.forget(&code);
    Ok(status)
}

/// Delete an installed dictionary. It can be downloaded again at any time.
#[tauri::command]
pub fn ocr_hyphen_dictionary_delete(
    state: State<'_, HyphenState>,
    code: String,
) -> Result<(), String> {
    hyphen::delete(&code)?;
    state.forget(&code);
    Ok(())
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
