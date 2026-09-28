use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tauri::{AppHandle, Manager, State};

use crate::align::split_book_sentences;
use crate::dataset::find_dataset_dir;
use crate::settings::SettingsState;

// ============================================================
// Conda environment resolution
// ============================================================

/// Conda environment that provides the Python deps (nltk, …) for the bundled
/// dataset tools. Created via `conda env create -f environment.yml`.
const CONDA_ENV: &str = "dataset_studio";

/// Platform-specific path to a conda env's Python interpreter.
fn env_python(env_dir: &Path) -> PathBuf {
    if cfg!(windows) {
        env_dir.join("python.exe")
    } else {
        env_dir.join("bin").join("python")
    }
}

/// Candidate conda *base* directories, most-likely first. A desktop app does not
/// inherit the shell's `conda activate`, and `conda`/`python` may not be on PATH,
/// so we discover the install from env vars and common locations instead.
pub fn conda_base_candidates() -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = Vec::new();

    // CONDA_EXE = <base>/Scripts/conda.exe (win) or <base>/bin/conda (unix).
    if let Ok(exe) = std::env::var("CONDA_EXE") {
        let exe = PathBuf::from(exe);
        if let Some(base) = exe.parent().and_then(Path::parent) {
            bases.push(base.to_path_buf());
        }
    }

    // CONDA_PREFIX = the active env dir (or base). Derive the base from it.
    if let Ok(prefix) = std::env::var("CONDA_PREFIX") {
        let prefix = PathBuf::from(prefix);
        if let Some(parent) = prefix.parent() {
            if parent.file_name().and_then(|s| s.to_str()) == Some("envs") {
                if let Some(base) = parent.parent() {
                    bases.push(base.to_path_buf());
                }
            }
        }
        bases.push(prefix);
    }

    // Common per-user install locations. `.conda` is conda's user config dir and
    // also the default `envs_dirs` target (`~/.conda/envs/<name>`), where envs land
    // when the base install isn't user-writable (a very common Windows setup).
    if let Some(home) = dirs::home_dir() {
        for name in [
            "miniconda3",
            "Miniconda3",
            "anaconda3",
            "Anaconda3",
            "miniforge3",
            "mambaforge",
            ".conda",
        ] {
            bases.push(home.join(name));
        }
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        bases.push(local.join("miniconda3"));
        bases.push(local.join("anaconda3"));
        bases.push(local.join("Continuum").join("miniconda3"));
    }
    if let Ok(program_data) = std::env::var("PROGRAMDATA") {
        let program_data = PathBuf::from(program_data);
        bases.push(program_data.join("miniconda3"));
        bases.push(program_data.join("anaconda3"));
    }

    // System-wide locations.
    if cfg!(windows) {
        bases.push(PathBuf::from("C:\\miniconda3"));
        bases.push(PathBuf::from("C:\\anaconda3"));
    } else {
        bases.push(PathBuf::from("/opt/conda"));
        bases.push(PathBuf::from("/opt/miniconda3"));
        bases.push(PathBuf::from("/opt/anaconda3"));
    }

    bases
}

/// Find a conda env dir by parsing conda's env registry at
/// `~/.conda/environments.txt`. That file lists the absolute prefix of every env
/// conda knows about, so this finds envs under a custom `envs_dirs` (e.g.
/// `~/.conda/envs`) even when `conda` isn't on PATH. Most robust resolver.
fn conda_env_dir_from_registry(env_name: &str) -> Option<PathBuf> {
    let registry = dirs::home_dir()?.join(".conda").join("environments.txt");
    let contents = fs::read_to_string(&registry).ok()?;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let dir = PathBuf::from(line);
        if dir.file_name().and_then(|s| s.to_str()) == Some(env_name) {
            return Some(dir);
        }
    }
    None
}

/// Parse `conda env list` to find a named env directory. Honours
/// custom `envs_dirs` that the filesystem heuristic would miss.
fn conda_env_dir_from_cli(env_name: &str) -> Option<PathBuf> {
    let conda = if cfg!(windows) { "conda.exe" } else { "conda" };
    let output = Command::new(conda).args(["env", "list"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Rows look like "<name> [*] <path>"; the '*' marks the active env.
        let mut parts = line.split_whitespace();
        let Some(first) = parts.next() else { continue };
        let (name, path) = if first == "*" {
            let Some(path) = parts.next() else { continue };
            ("", path)
        } else {
            let Some(second) = parts.next() else { continue };
            let path = if second == "*" {
                let Some(path) = parts.next() else { continue };
                path
            } else {
                second
            };
            (first, path)
        };
        if name == env_name {
            return Some(PathBuf::from(path));
        }
    }
    None
}

/// Resolve the Python interpreter inside a named conda env.
///
/// Searches for `env_name` using the same three-tier strategy as
/// [`resolve_conda_python`] but allows callers (e.g. the Logics-Parsing OCR
/// engine) to target a different env than the default `dataset_studio`.
pub fn resolve_conda_python_named(env_name: &str) -> Result<PathBuf, String> {
    log::debug!("Resolving conda python for env: '{}'", env_name);
    // 1. conda's own env registry (~/.conda/environments.txt).
    if let Some(env_dir) = conda_env_dir_from_registry(env_name) {
        let py = env_python(&env_dir);
        if py.exists() {
            return Ok(py);
        }
    }
    // 2. Filesystem lookup under candidate bases.
    for base in conda_base_candidates() {
        let py = env_python(&base.join("envs").join(env_name));
        if py.exists() {
            return Ok(py);
        }
    }
    // 3. Fallback: ask conda CLI.
    if let Some(env_dir) = conda_env_dir_from_cli(env_name) {
        let py = env_python(&env_dir);
        if py.exists() {
            return Ok(py);
        }
    }
    Err(format!(
        "Conda environment '{env_name}' not found. Create it, then retry:\n  \
         conda create -n {env_name} python=3.10\n\
         Searched ~/.conda/environments.txt, CONDA_EXE / CONDA_PREFIX, and common \
         Miniconda/Anaconda/.conda locations."
    ))
}

/// Resolve the Python interpreter inside the `dataset_studio` conda env.
///
/// This both *checks the env exists* and *assures scripts run inside it*: we
/// invoke that env's interpreter directly rather than a bare `python` on PATH
/// (which may resolve to a different env without nltk). Returns an actionable
/// error when the env cannot be located.
pub fn resolve_conda_python() -> Result<PathBuf, String> {
    resolve_conda_python_named(CONDA_ENV)
}

// ============================================================
// Helpers
// ============================================================

/// Resolve the path to a bundled Python script in the resources directory.
pub fn resolve_script(app: &AppHandle, script_name: &str) -> Result<PathBuf, String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|e| format!("Failed to resolve resource directory: {}", e))?;

    let script_path = resource_dir.join("tools").join(script_name);
    if script_path.exists() {
        return Ok(script_path);
    }

    // Dev fallback: `bundle.resources` copies the scripts next to the binary, but
    // only on a full build. When they aren't there yet (e.g. a fresh `tauri dev`),
    // resolve straight from the crate source tree. Compiled out of release builds,
    // so production relies solely on the bundled copy.
    #[cfg(debug_assertions)]
    {
        let dev_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("tools")
            .join(script_name);
        if dev_path.exists() {
            return Ok(dev_path);
        }
    }

    Err(format!(
        "Script not found: {}. Expected in the resources/tools/ directory.",
        script_path.display()
    ))
}

/// Run a bundled Python script inside the `dataset_studio` conda env and return
/// its stdout.
pub fn run_python_script(script_path: &Path, args: &[&str]) -> Result<String, String> {
    let python = resolve_conda_python()?;
    log::info!("Running Python script: {} with args: {:?}", script_path.display(), args);
    let output = Command::new(&python)
        .arg(script_path)
        .args(args)
        .output()
        .map_err(|e| format!("Failed to execute {}: {}", python.display(), e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        log::error!("Python script failed (exit {}): {}", output.status, detail);
        return Err(format!("Python script failed: {}", detail));
    }

    log::info!("Python script completed successfully");
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

    log::info!("Writing transcripts for dataset '{}'", dataset_uuid);
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

/// Stage 4a: split `book.txt` into sentences and cache them (one per line) in
/// `book_sentences.txt`. Two engines are offered via `mode`:
///
/// - `"rust"` (default): built-in splitter (`align::split_book_sentences`).
///   Self-contained and fast, but a simple heuristic — lower quality on
///   abbreviations / edge cases. Boundaries match the transcript-align stage.
/// - `"python"`: runs the bundled `split_book.py` (NLTK `sent_tokenize`, with the
///   book's language auto-detected via the stopwords corpus) for higher-quality
///   boundaries. Requires the `dataset_studio` conda env with nltk installed; the
///   interpreter is resolved automatically, so `python` need not be on PATH.
///
/// Returns a short summary for the UI log.
#[tauri::command]
pub async fn dataset_parse_book(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    mode: Option<String>,
) -> Result<String, String> {
    let dataset_dir = find_dataset_dir(&settings, &dataset_uuid)?;
    let book_path = dataset_dir.join("book.txt");
    let db_path = dataset_dir.join("data.sqlite3");

    log::info!("Parsing book for dataset '{}' (mode: {:?})", dataset_uuid, mode);
    if !book_path.exists() {
        return Err("book.txt not found in dataset directory.".into());
    }
    if !db_path.exists() {
        return Err("Database file not found. Please generate the database first.".into());
    }

    let output_path = dataset_dir.join("book_sentences.txt");

    // Python engine: delegate to the bundled NLTK-based script.
    if mode.as_deref() == Some("python") {
        let script = resolve_script(&app, "split_book.py")?;
        return run_python_script(
            &script,
            &[
                book_path.to_str().unwrap_or(""),
                output_path.to_str().unwrap_or(""),
            ],
        );
    }

    // Built-in Rust engine.
    let text = fs::read_to_string(&book_path)
        .map_err(|e| format!("Failed to read book.txt: {}", e))?;
    if text.trim().is_empty() {
        return Err("book.txt is empty.".into());
    }

    let sentences = split_book_sentences(&text);
    if sentences.is_empty() {
        return Err("No sentences found in book.txt.".into());
    }

    // One sentence per line, mirroring split_book.py's cache format.
    let mut out = String::new();
    for sentence in &sentences {
        out.push_str(sentence);
        out.push('\n');
    }
    fs::write(&output_path, &out)
        .map_err(|e| format!("Failed to write book_sentences.txt: {}", e))?;

    Ok(format!(
        "Split {} sentences from book.txt (Rust)\nWritten to: book_sentences.txt",
        sentences.len()
    ))
}
