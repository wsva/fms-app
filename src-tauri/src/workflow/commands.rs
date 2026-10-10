//! Tauri command twins of the `workflow_*` MCP tools ([`crate::mcp::workflow`]).
//!
//! They forward verbatim to the same `crate::workflow::*` core fns the MCP
//! surface calls, so the Workflow page's in-app "Run" button and a goose agent
//! driving the pipeline over `/mcp` reach one shared implementation. The page
//! needs the run-scoped bookkeeping (create/advance/record/intervene) to keep
//! `state.json` in sync, hence these are exposed to the frontend too.
//!
//! Desktop-only, exactly like the engine and the MCP server: registered solely
//! in the desktop `generate_handler!` in `lib.rs`.

use serde_json::Value;
use tauri::State;

use crate::settings::SettingsState;

/// List every workflow run under the workspace's `workflows` directory.
#[tauri::command]
pub async fn workflow_list_runs(
    settings: State<'_, SettingsState>,
) -> Result<Value, String> {
    crate::workflow::list_runs(settings.inner())
}

/// Create a new run from a `workflow.yaml` definition text. `run_id` picks the
/// directory name; omit it to auto-generate one from the definition name.
#[tauri::command]
pub async fn workflow_create_run(
    settings: State<'_, SettingsState>,
    run_id: Option<String>,
    yaml_text: String,
) -> Result<Value, String> {
    crate::workflow::create_run(settings.inner(), run_id, &yaml_text)
}

/// Load a run and return its definition metadata + per-step statuses and what
/// is ready/blocked/failed. Recomputes derived statuses and reaps expired
/// leases, so it is safe to call after a restart to resume.
#[tauri::command]
pub async fn workflow_status(
    settings: State<'_, SettingsState>,
    run_id: String,
) -> Result<Value, String> {
    crate::workflow::status(settings.inner(), &run_id)
}

/// The runnable (ready, unclaimed) steps with their resolved action, params and
/// data-flow inputs.
#[tauri::command]
pub async fn workflow_next(
    settings: State<'_, SettingsState>,
    run_id: String,
) -> Result<Value, String> {
    crate::workflow::next_steps(settings.inner(), &run_id)
}

/// Return the raw `workflow.yaml` text of a run, for loading it back into the
/// page's editor (preserves UI-only fields the parsed status drops).
#[tauri::command]
pub async fn workflow_get_definition(
    settings: State<'_, SettingsState>,
    run_id: String,
) -> Result<String, String> {
    crate::workflow::definition_text(settings.inner(), &run_id)
}

/// Claim a ready step (single-flight via a lease) and hand back its resolved
/// action + inputs. Does NOT execute: report the outcome with `workflow_record`.
#[tauri::command]
pub async fn workflow_advance(
    settings: State<'_, SettingsState>,
    run_id: String,
    step: String,
    agent_id: Option<String>,
) -> Result<Value, String> {
    crate::workflow::advance(settings.inner(), &run_id, &step, agent_id)
}

/// Report the outcome of a claimed step: `event` is one of `completed`,
/// `failed`, `blocked`; `detail` optionally carries `{ outputs, reason }`.
#[tauri::command]
pub async fn workflow_record(
    settings: State<'_, SettingsState>,
    run_id: String,
    step: String,
    event: String,
    detail: Option<Value>,
) -> Result<Value, String> {
    crate::workflow::record(settings.inner(), &run_id, &step, &event, detail)
}

/// Manually recover a step: `op` is one of `retry`, `unblock`, `skip`, `reset`.
#[tauri::command]
pub async fn workflow_intervene(
    settings: State<'_, SettingsState>,
    run_id: String,
    step: String,
    op: String,
) -> Result<Value, String> {
    crate::workflow::intervene(settings.inner(), &run_id, &step, &op)
}
