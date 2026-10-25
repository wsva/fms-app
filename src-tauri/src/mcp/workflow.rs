//! Workflow state-machine tools: drive a persistent, resumable, file-based DAG
//! run (see `docs/design/workflow/core.md`). Agent-in-the-loop — the framework does
//! bookkeeping + scheduling; the agent performs each step's action and reports
//! the outcome. Every tool returns structured JSON with actionable hints.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::Manager;

use crate::settings::SettingsState;
use crate::ui_state::UiStateHandle;

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
    /// Which dataset the run lives under. Optional: leave it out and the run is
    /// located by `run_id` alone (searched in the workspace jobs dir, then in
    /// every dataset's `workflows/`), so a call that forgets the scope still
    /// reaches the right run — unless two datasets hold the same id, which is
    /// then reported rather than guessed at.
    #[serde(default)]
    dataset_uuid: Option<String>,
    run_id: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WorkflowAdvanceParam {
    /// Which dataset the run lives under; optional, the run is located by its
    /// `run_id` alone when this is omitted (see [`WorkflowRunParam`]).
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
    /// Which dataset the run lives under; optional, the run is located by its
    /// `run_id` alone when this is omitted (see [`WorkflowRunParam`]).
    #[serde(default)]
    dataset_uuid: Option<String>,
    run_id: String,
    step: String,
    /// One of: `completed`, `failed`, `blocked`.
    event: String,
    /// The outcome detail. Outputs go under `detail.outputs` — or straight on
    /// `detail` (`{"model_version": "parakeet-v3"}`), which is accepted as well
    /// — and a `completed` must carry every name the step declares. A
    /// `reason`/`error` key names why a step did not complete.
    #[serde(default)]
    detail: JsonValue,
    /// The `claim_id` [`workflow_advance`] returned for this claim. Optional at
    /// first, but required once a step has been reaped at least once: a report for
    /// a `running` step whose lease was reaped and re-granted must present the live
    /// token, or a stale executor's late completion would clobber the current claim.
    #[serde(default)]
    claim_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WorkflowInterveneParam {
    /// Which dataset the run lives under; optional, the run is located by its
    /// `run_id` alone when this is omitted (see [`WorkflowRunParam`]).
    #[serde(default)]
    dataset_uuid: Option<String>,
    run_id: String,
    step: String,
    /// One of: `retry`, `unblock`, `skip`, `reset`.
    op: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WorkflowVerifyParam {
    /// Required: verification probes the *dataset*, so a workspace-level job
    /// (no dataset) has no subject and cannot be verified.
    dataset_uuid: String,
    run_id: String,
    /// Allow the one-way demotion of `completed` steps the evidence contradicts.
    /// Omit (or false) for a pure read that writes nothing.
    #[serde(default)]
    apply: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WorkflowAdoptParam {
    /// Required: adoption inherits progress from the *dataset* on disk.
    dataset_uuid: String,
    run_id: String,
    /// The `workflow.yaml` text to open the run from, when this dataset does not
    /// have one yet — and to rebind an existing run whose history is nothing but
    /// inherited progress, so a template edit reaches datasets scanned earlier.
    /// Omit it and an absent run is reported rather than invented.
    #[serde(default)]
    definition_yaml: Option<String>,
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

    #[tool(name = "workflow_status", description = "Load a run and return its definition metadata, per-step statuses, and what is ready/blocked/failed with actionable reasons and hints. READ-ONLY: it recomputes derived statuses in memory so the view is accurate after a restart, but it never writes. An expired lease is reported in `expired_claims`/`lease_hint`, not reaped here — the reap lands on the next workflow_advance/workflow_record/workflow_intervene, or call workflow_reap to persist it now. Pass dataset_uuid when you mean a particular dataset; omit it and the run is found by its run_id alone.")]
    async fn workflow_status(
        &self,
        Parameters(param): Parameters<WorkflowRunParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] workflow_status: run_id={}", param.run_id);
        let state = self.app.state::<SettingsState>();
        let v = crate::workflow::status(state.inner(), param.dataset_uuid.as_deref(), &param.run_id)?;
        ok(&v)
    }

    #[tool(name = "workflow_reap", description = "Explicitly persist the reaping of any expired leases in a run, journalling one `interrupted` event per reaped claim (and a `failed` event when a step exhausted its `max_interrupts` budget). This is the mutating home for a reap that workflow_status only reports: call it to clear stalled claims an agent left `running` without advancing or recording anything else. A run with no expired lease is returned unchanged and nothing is written. Pass dataset_uuid to name a particular dataset; omit it and the run is found by its run_id alone.")]
    async fn workflow_reap(
        &self,
        Parameters(param): Parameters<WorkflowRunParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] workflow_reap: run_id={}", param.run_id);
        let state = self.app.state::<SettingsState>();
        let v = crate::workflow::reap(state.inner(), param.dataset_uuid.as_deref(), &param.run_id)?;
        ok(&v)
    }

    #[tool(name = "workflow_next", description = "Return the runnable (ready, unclaimed) steps of a run with their resolved action, static params and data-flow inputs, ready for the agent to execute. Each step carries `lease_secs`: how long a claim on it is honoured before the engine assumes the work died — a step that legitimately runs longer than that (transcribing a dataset) must be given a bigger `timeout:` in the definition. Also returns `claimed`: the steps held by a lease with no recorded outcome, which is why nothing may be runnable; report them with workflow_record before anything else. Empty `steps` with empty `claimed` means genuinely nothing can run; the hint explains the current run_status. Each entry is an executable call: a dataset-scoped run fills in the subject, so `params.uuid` names the dataset its actions work on.")]
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

    #[tool(name = "workflow_advance", description = "Claim a ready step (single-flight via a lease) and hand its resolved action + inputs back to you to perform. This does NOT execute anything: the response is the call to make — `action` plus `params`, whose `uuid` is the dataset this run is filed under — and you must report the outcome with workflow_record before your turn ends. The response carries a `claim_id` token for this claim: pass it back to workflow_record if this step has ever been reaped, so a late report from an executor whose claim died cannot clobber yours. `lease_secs` in the response is how long you have: after that the engine reaps the claim (an `interrupted` event) and it can be re-claimed by anyone — a step reaped past its `max_interrupts` becomes `failed`. A step you claimed and never reported sits `running` under your lease while nothing is running. Errors if the step is not ready — naming what it waits on — or is already claimed by a live lease.")]
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

    #[tool(name = "workflow_record", description = "Report the outcome of a claimed step: event=completed (with the step's declared values under detail.outputs, or directly on detail), failed (with detail.reason/error; auto-retries while attempts remain), or blocked (waiting on something external). A completion that omits a declared output is refused and writes nothing — the step stays running so the corrected call lands on it — because a dependent reading that name through a `when:` guard would otherwise sit blocked forever with no event naming the cause. Pass the `claim_id` from workflow_advance when this step has been reaped before: a report for a `running` step carrying a missing or stale token is rejected and writes nothing, so a dead executor cannot clobber the live claim. Also reaps any expired lease this call touches (journalling an `interrupted` event) and re-evaluates the DAG, appending events and atomically rewriting state. Returns the updated status. Omit dataset_uuid and the run is still found by its run_id.")]
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
            param.claim_id.as_deref(),
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

    #[tool(name = "workflow_verify", description = "Score a dataset-scoped run against the dataset as it is right now: re-run the domain's read-only checks and join them to each step's `verify:` binding, returning a per-step verdict (verified / drifted / unknown), the offending checks with counts, and which steps are gated. Verdicts are a projection of the current probe, never stored in state.json, so calling this changes nothing unless apply=true. apply=true performs the one permitted status change: a `completed` step whose `proves` checks report findings goes back to `ready` and an `invalidated` event is appended - a probe can never promote a step, only retract a claim it no longer supports. Errors for a dataset kind with no check provider yet (only dictation today) or a run whose definition declares no `verify:` blocks. dataset_audit gives the raw subject-side report; this gives what it means for *this* pipeline.")]
    async fn workflow_verify(
        &self,
        Parameters(param): Parameters<WorkflowVerifyParam>,
    ) -> Result<String, String> {
        log::info!(
            "[MCP] workflow_verify: dataset_uuid={}, run_id={}, apply={:?}",
            param.dataset_uuid,
            param.run_id,
            param.apply
        );
        let state = self.app.state::<SettingsState>();
        let v = crate::workflow::verify(
            state.inner(),
            &param.dataset_uuid,
            &param.run_id,
            param.apply.unwrap_or(false),
        )?;
        ok(&v)
    }

    #[tool(name = "workflow_adopt", description = "Record a step as completed because the evidence shows its outcome is already on disk, without running it. The mirror of workflow_verify's apply: verify may only move a step DOWN, this is the only act that moves one UP unrun, and it applies solely to steps that opt in via the definition's `adopt:` list (in the dictation pipeline: init_dataset and sync_media, which claim an agreement between the database and the media folder, plus generate_subtitles / generate_waveforms / adjust_cue_times, which additionally list media_none because a per-file check reads 'zero offenders' vacuously when a dataset has no audio at all). A step whose listed checks were not all measured, or that report any offender, is withheld and named in `withheld` with the reason. Steps already completed, failed, skipped or running are left untouched - a human's intervention outranks the filesystem. Each adoption appends an `adopted` event, so events.jsonl still distinguishes progress the run performed from progress it inherited. Opens the run from definition_yaml if the dataset has none yet; if the run exists, holds nothing but inherited progress, and the passed definition differs semantically from the stored one, the run is rebound to it (statuses of surviving steps carried over, `definition_refreshed` event) so a template edit does not strand older datasets - a run with earned progress keeps the definition it was created from. Returns the full status, with `adoption.refreshed` and `adoption.refresh_error` telling which case happened. Calling it twice with unchanged evidence writes nothing. Use this when a dataset was prepared outside the app (or by an earlier run) and the graph should show the state it is actually in.")]
    async fn workflow_adopt(
        &self,
        Parameters(param): Parameters<WorkflowAdoptParam>,
    ) -> Result<String, String> {
        log::info!(
            "[MCP] workflow_adopt: dataset_uuid={}, run_id={}, defines={}",
            param.dataset_uuid,
            param.run_id,
            param.definition_yaml.is_some()
        );
        let state = self.app.state::<SettingsState>();
        let v = crate::workflow::adopt(
            state.inner(),
            &param.dataset_uuid,
            &param.run_id,
            param.definition_yaml.as_deref(),
        )?;
        ok(&v)
    }

    #[tool(name = "workflow_builtin_templates", description = "List the workflow templates shipped built-in with the app, grouped by dataset `category` (dictation, book, card) — a category may carry several. Each entry has its category, a stable `id`, the parsed `name`/`version`/`step_count`, the full raw `workflow.yaml` text, and the run it keeps its progress in: `run_slug` plus `run_id`, where `<uuid>` in that id stands for the dataset's uuid — pass the same shape to workflow_create, and prefer `dictation-<uuid>` (the category's primary workflow, the id the app's Workflow page reads) unless you specifically mean another template, because two templates over one dataset are two separate histories. Use this instead of inventing a definition from scratch: pass the returned `yaml` straight to workflow_create (with a dataset_uuid to scope the run inside that dataset). Only templates in the `dictation` category are wired for in-app execution; book/card are view/agent-facing (their actions still name real MCP tools). This is the single source of truth shared with the in-app Workflow page — an agent and the UI always start from the identical definition.")]
    async fn workflow_builtin_templates(&self) -> String {
        log::info!("[MCP] workflow_builtin_templates");
        match crate::workflow::builtin_templates() {
            Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_default(),
            Err(e) => format!("{{\"error\": \"{e}\"}}"),
        }
    }

    #[tool(name = "workflow_list_actions", description = "List the step vocabulary an authored workflow.yaml may name: every step id the app can actually run, the Tauri `run_command`/`clear_command` it dispatches, its form fields (name, type, placeholder, closed `options` set) and whether the page re-probes the dataset afterwards, plus the flat `commands` list a custom step's `action:` may take. Call this BEFORE writing a definition so ids and field names validate first try instead of guessing — workflow_validate rejects unknown step ids and adopt:/verify: names. The vocabulary is published by the running app's Workflow page (a frontend registry, so Rust keeps no copy); an empty registry means that page has not been opened this session, and the error says how to make it appear. Read-only.")]
    async fn workflow_list_actions(&self) -> Result<String, String> {
        log::info!("[MCP] workflow_list_actions");
        let ui = self.app.state::<UiStateHandle>();
        let snapshot = ui.workflow_actions();
        let actions = snapshot
            .get("actions")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        if actions.is_empty() {
            return Err("the workflow step vocabulary has not been published yet — the Workflow \
                        tab reports it when the page mounts. Call app_navigate with tab=\"workflow\" \
                        and retry. Until then, workflow_builtin_templates returns definitions that \
                        already name valid steps; you can copy their action names."
                .to_string());
        }

        let mut step_ids: Vec<String> = actions
            .iter()
            .filter_map(|a| a.get("id").and_then(|v| v.as_str()).map(String::from))
            .collect();
        step_ids.sort();

        ok(&json!({
            "status": "ok",
            "count": actions.len(),
            "step_ids": step_ids,
            "commands": snapshot.get("commands").cloned().unwrap_or(Value::Null),
            "actions": actions,
            "published_at": snapshot.get("updated_at").cloned().unwrap_or(Value::Null),
            "note": "`run_command`/`clear_command` are the in-app dispatch; a custom step may instead name any entry of `commands` as its `action:`. Only the dictation category has in-app runnable steps; book/card templates name MCP tools directly.",
        }))
    }
}
