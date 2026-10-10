//! Workflow state-machine tools: drive a persistent, resumable, file-based DAG
//! run (see `docs/design/workflow/core.md`). Agent-in-the-loop — the framework does
//! bookkeeping + scheduling; the agent performs each step's action and reports
//! the outcome. Every tool returns structured JSON with actionable hints.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use serde::Deserialize;
use serde_json::Value;
use tauri::Manager;

use crate::settings::SettingsState;

use super::{DatasetMcpServer, JsonValue};

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WorkflowCreateParam {
    /// Optional dataset uuid: scope the run inside that dataset's own
    /// `<dataset>/workflows` dir (self-contained, travels with sync/export).
    /// Omit to create a workspace-level job instead.
    #[serde(default)]
    dataset_uuid: Option<String>,
    /// Optional run id (single directory name). Generated from the definition
    /// name when omitted.
    #[serde(default)]
    run_id: Option<String>,
    /// The full `workflow.yaml` definition text.
    definition_yaml: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WorkflowListParam {
    /// Optional dataset uuid: list that dataset's runs under `<dataset>/workflows`.
    /// Omit to list the workspace jobs directory (dataset-less jobs) instead.
    #[serde(default)]
    dataset_uuid: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WorkflowRunParam {
    /// Optional dataset uuid the run lives under (see workflow_create). Omit for
    /// workspace-level jobs.
    #[serde(default)]
    dataset_uuid: Option<String>,
    run_id: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WorkflowAdvanceParam {
    #[serde(default)]
    dataset_uuid: Option<String>,
    run_id: String,
    step: String,
    /// Optional lease owner id; distinguishes concurrent agents.
    #[serde(default)]
    agent_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WorkflowRecordParam {
    #[serde(default)]
    dataset_uuid: Option<String>,
    run_id: String,
    step: String,
    /// One of: `completed`, `failed`, `blocked`.
    event: String,
    /// Optional outcome detail: `{ outputs?: {...}, reason?/error?: "..." }`.
    #[serde(default)]
    detail: JsonValue,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WorkflowInterveneParam {
    #[serde(default)]
    dataset_uuid: Option<String>,
    run_id: String,
    step: String,
    /// One of: `retry`, `unblock`, `skip`, `reset`.
    op: String,
}

fn ok(v: &Value) -> Result<String, String> {
    Ok(serde_json::to_string_pretty(v).unwrap_or_default())
}

#[tool_router(router = workflow_router, vis = "pub(crate)")]
impl DatasetMcpServer {
    #[tool(name = "workflow_list", description = "List workflow runs and their status summaries (id, definition name/version, overall run_status, per-status counts). Pass dataset_uuid to list a dataset's own runs under <dataset>/workflows; omit it to list the workspace jobs directory (dataset-less pipeline templates). Use this to discover runs before loading one.")]
    async fn workflow_list(
        &self,
        Parameters(param): Parameters<WorkflowListParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] workflow_list: dataset_uuid={:?}", param.dataset_uuid);
        let state = self.app.state::<SettingsState>();
        let v = crate::workflow::list_runs(state.inner(), param.dataset_uuid.as_deref())?;
        ok(&v)
    }

    #[tool(name = "workflow_create", description = "Create a new workflow run from a YAML definition. Validates the DAG (unique ids, existing references, acyclic), writes workflow.yaml/state.json/events.jsonl, and returns the initial status with the first ready steps. Pass dataset_uuid to store the run inside that dataset's own workflows dir (so it travels with the dataset); omit for a workspace-level job. Pass run_id to choose the directory name, or omit it to auto-generate one.")]
    async fn workflow_create(
        &self,
        Parameters(param): Parameters<WorkflowCreateParam>,
    ) -> Result<String, String> {
        log::info!(
            "[MCP] workflow_create: dataset_uuid={:?}, run_id={:?}",
            param.dataset_uuid,
            param.run_id
        );
        let state = self.app.state::<SettingsState>();
        let v = crate::workflow::create_run(
            state.inner(),
            param.dataset_uuid.as_deref(),
            param.run_id,
            &param.definition_yaml,
        )?;
        ok(&v)
    }

    #[tool(name = "workflow_status", description = "Load a run and return its definition metadata, per-step statuses, and what is ready/blocked/failed with actionable reasons and hints. Recomputes derived statuses and reaps expired leases, so it is always safe to call after a restart to resume. Pass dataset_uuid when the run is stored inside a dataset; omit for a workspace job.")]
    async fn workflow_status(
        &self,
        Parameters(param): Parameters<WorkflowRunParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] workflow_status: run_id={}", param.run_id);
        let state = self.app.state::<SettingsState>();
        let v = crate::workflow::status(state.inner(), param.dataset_uuid.as_deref(), &param.run_id)?;
        ok(&v)
    }

    #[tool(name = "workflow_next", description = "Return the runnable (ready, unclaimed) steps of a run with their resolved action, static params and data-flow inputs, ready for the agent to execute. Empty when nothing is runnable; the hint explains the current run_status.")]
    async fn workflow_next(
        &self,
        Parameters(param): Parameters<WorkflowRunParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] workflow_next: run_id={}", param.run_id);
        let state = self.app.state::<SettingsState>();
        let v = crate::workflow::next_steps(state.inner(), param.dataset_uuid.as_deref(), &param.run_id)?;
        ok(&v)
    }

    #[tool(name = "workflow_get_definition", description = "Return the raw workflow.yaml text of a run, so you can read or edit the exact definition it was created from (preserves UI-only fields). Errors if the run does not exist.")]
    async fn workflow_get_definition(
        &self,
        Parameters(param): Parameters<WorkflowRunParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] workflow_get_definition: run_id={}", param.run_id);
        let state = self.app.state::<SettingsState>();
        crate::workflow::definition_text(state.inner(), param.dataset_uuid.as_deref(), &param.run_id)
    }

    #[tool(name = "workflow_advance", description = "Claim a ready step (single-flight via a lease) and hand its resolved action + inputs back to you to perform. This does NOT execute anything: after doing the work, report the outcome with workflow_record. Errors if the step is not ready or is already claimed by a live lease.")]
    async fn workflow_advance(
        &self,
        Parameters(param): Parameters<WorkflowAdvanceParam>,
    ) -> Result<String, String> {
        log::info!(
            "[MCP] workflow_advance: run_id={}, step={}",
            param.run_id,
            param.step
        );
        let state = self.app.state::<SettingsState>();
        let v = crate::workflow::advance(
            state.inner(),
            param.dataset_uuid.as_deref(),
            &param.run_id,
            &param.step,
            param.agent_id,
        )?;
        ok(&v)
    }

    #[tool(name = "workflow_record", description = "Report the outcome of a claimed step: event=completed (optionally with detail.outputs), failed (with detail.reason/error; auto-retries while attempts remain), or blocked (waiting on something external). Re-evaluates the DAG, appends events, and atomically rewrites state. Returns the updated status.")]
    async fn workflow_record(
        &self,
        Parameters(param): Parameters<WorkflowRecordParam>,
    ) -> Result<String, String> {
        log::info!(
            "[MCP] workflow_record: run_id={}, step={}, event={}",
            param.run_id,
            param.step,
            param.event
        );
        let state = self.app.state::<SettingsState>();
        let detail: Option<Value> = match Value::from(param.detail) {
            Value::Null => None,
            v => Some(v),
        };
        let v = crate::workflow::record(
            state.inner(),
            param.dataset_uuid.as_deref(),
            &param.run_id,
            &param.step,
            &param.event,
            detail,
        )?;
        ok(&v)
    }

    #[tool(name = "workflow_intervene", description = "Manually recover a step: op=retry (make a failed/blocked step runnable again, resetting its attempt count), unblock (re-derive a blocked step once its condition cleared), skip (mark it skipped and propagate to dependents), or reset (clear all of the step's state back to pending). Returns the updated status.")]
    async fn workflow_intervene(
        &self,
        Parameters(param): Parameters<WorkflowInterveneParam>,
    ) -> Result<String, String> {
        log::info!(
            "[MCP] workflow_intervene: run_id={}, step={}, op={}",
            param.run_id,
            param.step,
            param.op
        );
        let state = self.app.state::<SettingsState>();
        let v = crate::workflow::intervene(
            state.inner(),
            param.dataset_uuid.as_deref(),
            &param.run_id,
            &param.step,
            &param.op,
        )?;
        ok(&v)
    }

    #[tool(name = "workflow_builtin_templates", description = "List the workflow templates shipped built-in with the app, grouped by dataset `category` (dictation, book, card) — a category may carry several. Each entry has its category, a stable `id`, the parsed `name`/`version`/`step_count`, and the full raw `workflow.yaml` text. Use this instead of inventing a definition from scratch: pass the returned `yaml` straight to workflow_create (with a dataset_uuid to scope the run inside that dataset). Only the dictation pipeline is wired for in-app execution; book/card are view/agent-facing (their actions still name real MCP tools). This is the single source of truth shared with the in-app Workflow page — an agent and the UI always start from the identical definition.")]
    async fn workflow_builtin_templates(&self) -> String {
        log::info!("[MCP] workflow_builtin_templates");
        match crate::workflow::builtin_templates() {
            Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_default(),
            Err(e) => format!("{{\"error\": \"{e}\"}}"),
        }
    }
}
