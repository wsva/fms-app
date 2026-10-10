//! Persistent workflow state machine — a generic, file-based framework for
//! describing and running multi-step processes that an AI agent can drive and
//! resume after any interruption without relying on conversation history.
//!
//! See `docs/design/workflow.md` for the full specification.
//!
//! # Layout
//!
//! The domain-free framework lives in [`core`]: it knows about *steps*,
//! *dependencies* and *statuses*, never what a step actually does. Actions are
//! symbolic names; in this agent-in-the-loop build every step is handed back to
//! the agent (via the `workflow_*` MCP tools) rather than bound to an executor.
//!
//! This group root is the *binding* layer: it turns a dataset identity into a
//! run location and hands the domain-free [`core`] engine an explicit base
//! directory. A dataset-scoped run lives inside its own dataset at
//! `<dataset>/workflows/<run_id>` (self-contained, travels with sync/export);
//! dataset-less editor "jobs" live in the workspace `<workspace>/workflows`.
//! Binding concrete actions (download, transcribe, …) to the generic engine is
//! still future work; today this root only owns the storage-location seam.
//!
//! Desktop-only: the engine is reached through the desktop-gated MCP server
//! (`mcp/workflow.rs`) and, for the Workflow page, its `workflow_*` Tauri
//! command twins ([`commands`]) — both call the same [`core`] fns via these
//! wrappers.

pub(crate) mod core;
/// Tauri command twins of the `workflow_*` MCP tools, so the Workflow page can
/// drive the same engine in-app. Desktop-only (registered only in the desktop
/// handler list), matching the engine + MCP surface.
pub(crate) mod commands;
/// Built-in workflow templates (e.g. the Dataset Dictation pipeline) baked into
/// the binary via `include_str!` — the single source of truth for what the
/// Workflow page and the `workflow_builtin_templates` MCP tool seed a new run
/// with, replacing the former duplicate `docs/ai/workflow/` + `seed.ts` copies.
pub(crate) mod templates;

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::settings::SettingsState;

/// Sub-directory (inside a dataset, or under a workspace) that holds run dirs.
const RUNS_SUBDIR: &str = "workflows";

/// Resolve the run *base directory* for a workflow call.
///
/// - `dataset_uuid` present → the dataset's own `<dataset>/workflows` dir, so a
///   dataset-scoped run travels with the dataset (self-contained, sync-friendly).
/// - absent → the workspace jobs dir `<workspace>/workflows`, for dataset-less
///   editor "jobs" (reusable pipeline templates).
///
/// The domain-free [`core`] engine only ever sees this base path; it never
/// resolves dataset identity itself.
fn run_base(settings: &SettingsState, dataset_uuid: Option<&str>) -> Result<PathBuf, String> {
    match dataset_uuid.map(str::trim).filter(|s| !s.is_empty()) {
        Some(uuid) => {
            let dir = crate::datasets::find_dataset_dir(settings, uuid)?;
            Ok(dir.join(RUNS_SUBDIR))
        }
        None => Ok(settings.workspace_subdir(RUNS_SUBDIR, "")),
    }
}

// Run-scoped public API. These thin wrappers resolve the run base directory from
// `(settings, dataset_uuid)` and hand the explicit path to the domain-free
// [`core`] engine, so callers reference `crate::workflow::*` and never name the
// `core` child module (which would otherwise shadow Rust's `core` crate).

pub(crate) fn list_runs(
    settings: &SettingsState,
    dataset_uuid: Option<&str>,
) -> Result<Value, String> {
    core::list_runs(&run_base(settings, dataset_uuid)?)
}

pub(crate) fn create_run(
    settings: &SettingsState,
    dataset_uuid: Option<&str>,
    run_id: Option<String>,
    definition_yaml: &str,
) -> Result<Value, String> {
    core::create_run(&run_base(settings, dataset_uuid)?, run_id, definition_yaml)
}

pub(crate) fn status(
    settings: &SettingsState,
    dataset_uuid: Option<&str>,
    run_id: &str,
) -> Result<Value, String> {
    core::status(&run_base(settings, dataset_uuid)?, run_id)
}

pub(crate) fn definition_text(
    settings: &SettingsState,
    dataset_uuid: Option<&str>,
    run_id: &str,
) -> Result<String, String> {
    core::definition_text(&run_base(settings, dataset_uuid)?, run_id)
}

pub(crate) fn next_steps(
    settings: &SettingsState,
    dataset_uuid: Option<&str>,
    run_id: &str,
) -> Result<Value, String> {
    core::next_steps(&run_base(settings, dataset_uuid)?, run_id)
}

pub(crate) fn advance(
    settings: &SettingsState,
    dataset_uuid: Option<&str>,
    run_id: &str,
    step_id: &str,
    agent_id: Option<String>,
) -> Result<Value, String> {
    core::advance(&run_base(settings, dataset_uuid)?, run_id, step_id, agent_id)
}

pub(crate) fn record(
    settings: &SettingsState,
    dataset_uuid: Option<&str>,
    run_id: &str,
    step_id: &str,
    event_kind: &str,
    detail: Option<Value>,
) -> Result<Value, String> {
    core::record(&run_base(settings, dataset_uuid)?, run_id, step_id, event_kind, detail)
}

pub(crate) fn intervene(
    settings: &SettingsState,
    dataset_uuid: Option<&str>,
    run_id: &str,
    step_id: &str,
    op: &str,
) -> Result<Value, String> {
    core::intervene(&run_base(settings, dataset_uuid)?, run_id, step_id, op)
}

// ---------------------------------------------------------------------------
// Built-in templates
// ---------------------------------------------------------------------------

/// List every built-in template shipped in the binary, each parsed once for its
/// `name`/`version` metadata alongside its raw YAML text (so the frontend and
/// an agent get the exact same definition, no second source of truth).
pub(crate) fn builtin_templates() -> Result<Value, String> {
    let mut out = Vec::new();
    for (category, id, yaml) in templates::all() {
        let def: core::Definition = serde_yaml::from_str(yaml)
            .map_err(|e| format!("built-in template '{id}' failed to parse: {e}"))?;
        out.push(json!({
            "category": category,
            "id": id,
            "name": def.name,
            "version": def.version,
            "step_count": def.steps.len(),
            "yaml": yaml,
        }));
    }
    Ok(json!({ "templates": out, "count": out.len() }))
}
