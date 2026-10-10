//! File IO for a workflow run: run-directory resolution, YAML/JSON parsing,
//! atomic `state.json` rewrites, and append-only `events.jsonl` writes.
//!
//! One directory per run holds three plain-text files (see the design doc):
//! `workflow.yaml` (definition), `state.json` (snapshot), `events.jsonl`
//! (audit log). Everything is diffable, agent-readable and sync-friendly.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{Local, Utc};

use crate::settings::SettingsState;

use super::{Definition, Event, RunState};

/// Name of the workspace sub-directory that holds every run.
const RUNS_SUBDIR: &str = "workflows";
const DEFINITION_FILE: &str = "workflow.yaml";
const STATE_FILE: &str = "state.json";
const EVENTS_FILE: &str = "events.jsonl";

/// Root directory holding every workflow run: `<workspace>/workflows`.
/// Falls back to `<data_root>/workflows` when no workspace is selected.
pub(super) fn runs_root(settings: &SettingsState) -> PathBuf {
    settings.workspace_subdir(RUNS_SUBDIR, "")
}

/// Validate and normalize a run id so it is safe to use as a single directory
/// name. Rejects empty ids, path separators and `..` traversal.
pub(super) fn sanitize_run_id(run_id: &str) -> Result<String, String> {
    let id = run_id.trim();
    if id.is_empty() {
        return Err("run_id must not be empty".into());
    }
    if id.contains('/') || id.contains('\\') || id.contains("..") || id == "." {
        return Err(format!(
            "run_id '{id}' is not a valid single directory name (no path separators or '..')"
        ));
    }
    Ok(id.to_string())
}

/// Resolve a run's directory, ensuring it exists as a directory.
pub(super) fn run_dir(settings: &SettingsState, run_id: &str) -> Result<PathBuf, String> {
    let id = sanitize_run_id(run_id)?;
    let dir = runs_root(settings).join(&id);
    if !dir.is_dir() {
        return Err(format!(
            "workflow run '{id}' not found at {}. Create it with workflow_create, or list runs with workflow_list.",
            dir.display()
        ));
    }
    Ok(dir)
}

/// Create a fresh, empty run directory. Fails if it already exists.
pub(super) fn create_run_dir(settings: &SettingsState, run_id: &str) -> Result<PathBuf, String> {
    let id = sanitize_run_id(run_id)?;
    let dir = runs_root(settings).join(&id);
    if dir.exists() {
        return Err(format!(
            "workflow run '{id}' already exists at {}. Pick a different run_id or load it with workflow_status.",
            dir.display()
        ));
    }
    fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create run directory {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Write the raw definition YAML text to `workflow.yaml`.
pub(super) fn write_definition_text(dir: &Path, yaml_text: &str) -> Result<(), String> {
    let path = dir.join(DEFINITION_FILE);
    fs::write(&path, yaml_text)
        .map_err(|e| format!("failed to write {}: {e}", path.display()))
}

/// Read the raw `workflow.yaml` text of a run (for round-tripping into the
/// page's editor, preserving UI-only fields like `title` that a re-serialize
/// from the parsed [`Definition`] would drop).
pub(super) fn read_definition_text(dir: &Path) -> Result<String, String> {
    let path = dir.join(DEFINITION_FILE);
    fs::read_to_string(&path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))
}

/// Parse `workflow.yaml` into a [`Definition`].
pub(super) fn load_definition(dir: &Path) -> Result<Definition, String> {
    let path = dir.join(DEFINITION_FILE);
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    serde_yaml::from_str::<Definition>(&text)
        .map_err(|e| format!("invalid workflow definition in {}: {e}", path.display()))
}

/// Parse `state.json` into a [`RunState`].
pub(super) fn load_state(dir: &Path) -> Result<RunState, String> {
    let path = dir.join(STATE_FILE);
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    serde_json::from_str::<RunState>(&text)
        .map_err(|e| format!("corrupt state in {}: {e}", path.display()))
}

/// Atomically rewrite `state.json`: serialize to a temp file then rename, so a
/// crash mid-write never leaves a half-written snapshot.
pub(super) fn write_state_atomic(dir: &Path, state: &RunState) -> Result<(), String> {
    let final_path = dir.join(STATE_FILE);
    let tmp_path = dir.join(format!("{STATE_FILE}.tmp"));
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| format!("failed to serialize state: {e}"))?;
    fs::write(&tmp_path, json)
        .map_err(|e| format!("failed to write {}: {e}", tmp_path.display()))?;
    fs::rename(&tmp_path, &final_path).map_err(|e| {
        format!(
            "failed to atomically replace {}: {e}",
            final_path.display()
        )
    })
}

/// Append events to `events.jsonl` (one JSON object per line) and flush.
pub(super) fn append_events(dir: &Path, events: &[Event]) -> Result<(), String> {
    if events.is_empty() {
        return Ok(());
    }
    let path = dir.join(EVENTS_FILE);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    for ev in events {
        let line = serde_json::to_string(ev)
            .map_err(|e| format!("failed to serialize event: {e}"))?;
        writeln!(file, "{line}")
            .map_err(|e| format!("failed to append to {}: {e}", path.display()))?;
    }
    file.flush().map_err(|e| format!("failed to flush {}: {e}", path.display()))?;
    Ok(())
}

/// Human-readable local timestamp, e.g. `2026-10-01 10:00:00`.
pub(super) fn now_string() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Unix epoch seconds (UTC) — used for lease heartbeats so expiry is a cheap
/// integer comparison independent of local timezone formatting.
pub(super) fn now_ts() -> i64 {
    Utc::now().timestamp()
}

/// List existing run directory names (directories containing a `workflow.yaml`).
pub(super) fn list_run_ids(settings: &SettingsState) -> Vec<String> {
    let root = runs_root(settings);
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir() && e.path().join(DEFINITION_FILE).exists())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    ids.sort();
    ids
}
