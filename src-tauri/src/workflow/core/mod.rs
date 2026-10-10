//! Workflow core: typed schema (`Definition`/`RunState`/`Event`), the pure
//! engine (readiness + transitions, in [`engine`]), file IO ([`persist`]),
//! validation ([`validate`]), and the run-scoped public API the MCP tools call.
//!
//! Domain-free: nothing here knows what a step's `action` actually does.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::settings::SettingsState;

pub(super) mod engine;
mod persist;
mod validate;

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// The seven step states from the design doc.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pending,
    Ready,
    Running,
    Completed,
    Failed,
    Blocked,
    Skipped,
}

impl Default for Status {
    fn default() -> Self {
        Status::Pending
    }
}

impl Status {
    fn as_str(&self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Ready => "ready",
            Status::Running => "running",
            Status::Completed => "completed",
            Status::Failed => "failed",
            Status::Blocked => "blocked",
            Status::Skipped => "skipped",
        }
    }
}

/// What happens to a step's dependents when it fails.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum OnFailure {
    Stop,
    Continue,
    SkipDependents,
}

impl Default for OnFailure {
    fn default() -> Self {
        OnFailure::Stop
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RetryPolicy {
    /// Maximum number of attempts (a first run counts as attempt 1).
    #[serde(default = "default_attempts")]
    pub attempts: u32,
    /// Optional backoff hint (free-form; interpreted by the executor/agent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff: Option<String>,
}

fn default_attempts() -> u32 {
    1
}

/// One node of the workflow DAG. Only `id` and `action` are mandatory.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Step {
    pub id: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    /// Named references to other steps' outputs, e.g. `${steps.x.outputs.y}`.
    #[serde(default)]
    pub inputs: std::collections::BTreeMap<String, String>,
    /// Names of values this step produces (recorded into state by `record`).
    #[serde(default)]
    pub outputs: Vec<String>,
    /// Static arguments passed to the executor/agent.
    #[serde(default)]
    pub params: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_failure: Option<OnFailure>,
}

impl Step {
    fn on_failure(&self) -> OnFailure {
        self.on_failure.unwrap_or_default()
    }
    fn max_attempts(&self) -> u32 {
        self.retry.as_ref().map(|r| r.attempts.max(1)).unwrap_or(1)
    }
}

/// The immutable definition (`workflow.yaml`).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Definition {
    pub name: String,
    pub version: u32,
    #[serde(default)]
    pub steps: Vec<Step>,
}

/// Which definition version a `state.json` belongs to (version binding).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WorkflowRef {
    pub name: String,
    pub version: u32,
}

/// Per-step runtime record in `state.json`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct StepState {
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<String>,
    /// Unix epoch seconds of the last heartbeat (lease liveness).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat: Option<i64>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub outputs: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The materialized snapshot (`state.json`).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RunState {
    pub workflow: WorkflowRef,
    #[serde(default)]
    pub steps: HashMap<String, StepState>,
}

/// One transition in the append-only `events.jsonl` audit log.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Event {
    pub time: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

// ---------------------------------------------------------------------------
// Public API (called by mcp/workflow.rs)
// ---------------------------------------------------------------------------

/// List every run under `<workspace>/workflows` with a status summary.
pub(crate) fn list_runs(settings: &SettingsState) -> Result<Value, String> {
    let now = persist::now_ts();
    let mut runs: Vec<Value> = Vec::new();
    for id in persist::list_run_ids(settings) {
        let Ok(dir) = persist::run_dir(settings, &id) else {
            continue;
        };
        let def = match persist::load_definition(&dir) {
            Ok(d) => d,
            Err(_) => {
                runs.push(json!({ "run_id": id, "error": "unreadable or invalid workflow.yaml" }));
                continue;
            }
        };
        let mut state = match persist::load_state(&dir) {
            Ok(s) => s,
            Err(_) => {
                runs.push(json!({ "run_id": id, "name": def.name, "error": "unreadable state.json" }));
                continue;
            }
        };
        engine::recompute(&def, &mut state, now);
        runs.push(json!({
            "run_id": id,
            "name": def.name,
            "version": def.version,
            "run_status": engine::classify_run(&def, &state),
            "counts": status_counts(&def, &state),
        }));
    }
    Ok(json!({ "runs": runs, "count": runs.len() }))
}

/// Instantiate a new run from a YAML definition: validate, write the three
/// files, and return the initial status.
pub(crate) fn create_run(
    settings: &SettingsState,
    run_id: Option<String>,
    definition_yaml: &str,
) -> Result<Value, String> {
    let def: Definition = serde_yaml::from_str(definition_yaml)
        .map_err(|e| format!("invalid workflow YAML: {e}"))?;
    validate::validate(&def)?;

    let id = match run_id {
        Some(rid) if !rid.trim().is_empty() => persist::sanitize_run_id(&rid)?,
        _ => generated_run_id(&def.name),
    };

    let dir = persist::create_run_dir(settings, &id)?;
    persist::write_definition_text(&dir, definition_yaml)?;

    let now = persist::now_ts();
    let mut state = RunState {
        workflow: WorkflowRef {
            name: def.name.clone(),
            version: def.version,
        },
        steps: HashMap::new(),
    };
    engine::recompute(&def, &mut state, now);

    let events = vec![event(None, "workflow_started", Some(json!({ "name": def.name, "version": def.version })))];
    commit(&dir, &events, &state)?;

    Ok(build_status(&id, &def, &state))
}

/// Definition + current state + what's ready/blocked/failed and why.
pub(crate) fn status(settings: &SettingsState, run_id: &str) -> Result<Value, String> {
    let (dir, def, mut state) = load_bundle(settings, run_id)?;
    engine::recompute(&def, &mut state, persist::now_ts());
    // Persist any recompute-derived changes (e.g. a reaped lease) so the on-disk
    // snapshot matches what we return.
    persist::write_state_atomic(&dir, &state)?;
    Ok(build_status(run_id, &def, &state))
}

/// Return the raw `workflow.yaml` text of a run, for loading back into the
/// page's editor (preserves UI-only fields like `title`).
pub(crate) fn definition_text(settings: &SettingsState, run_id: &str) -> Result<String, String> {
    let dir = persist::run_dir(settings, run_id)?;
    persist::read_definition_text(&dir)
}

/// The runnable (`ready`, unclaimed) steps with resolved action + inputs.
pub(crate) fn next_steps(settings: &SettingsState, run_id: &str) -> Result<Value, String> {
    let (_dir, def, mut state) = load_bundle(settings, run_id)?;
    let now = persist::now_ts();
    engine::recompute(&def, &mut state, now);

    let ready = engine::ready_step_ids(&def, &state);
    let mut steps: Vec<Value> = Vec::new();
    for id in &ready {
        let Some(step) = def.steps.iter().find(|s| &s.id == id) else {
            continue;
        };
        let inputs = engine::resolve_inputs(step, &state).unwrap_or_default();
        steps.push(json!({
            "id": step.id,
            "action": step.action,
            "description": step.description,
            "params": step.params,
            "inputs": inputs,
            "declared_outputs": step.outputs,
            "max_attempts": step.max_attempts(),
        }));
    }

    let run_status = engine::classify_run(&def, &state);
    let hint = if steps.is_empty() {
        format!("no runnable steps (run_status={run_status}); call workflow_status for blocked/failed reasons")
    } else {
        format!(
            "{} step(s) ready; call workflow_advance(run_id, step) to claim one, do the work, then report via workflow_record",
            steps.len()
        )
    };
    Ok(json!({ "run_id": run_id, "run_status": run_status, "steps": steps, "hint": hint }))
}

/// Claim a `ready` step (single-flight via lease) and hand its resolved action
/// back to the agent. There is no bound executor in this agent-in-the-loop build.
pub(crate) fn advance(
    settings: &SettingsState,
    run_id: &str,
    step_id: &str,
    agent_id: Option<String>,
) -> Result<Value, String> {
    let (dir, def, mut state) = load_bundle(settings, run_id)?;
    let now = persist::now_ts();
    engine::recompute(&def, &mut state, now);

    let step = find_step(&def, step_id)?;
    let ss = state.steps.get(step_id).cloned().unwrap_or_default();

    match ss.status {
        Status::Ready => {}
        Status::Running => {
            if engine::lease_live(ss.status, ss.heartbeat, now) {
                return Err(format!(
                    "step '{step_id}' is already claimed by '{}'; wait for it, or intervene (reset) if that lease is stale",
                    ss.lease.clone().unwrap_or_else(|| "another agent".into())
                ));
            }
            // Expired lease: fall through and re-claim (executors are idempotent).
        }
        other => {
            return Err(format!(
                "step '{step_id}' is '{:?}', not 'ready', so it cannot be advanced. Call workflow_status to see why.",
                other
            ))
        }
    }

    let lease = agent_id.filter(|a| !a.trim().is_empty()).unwrap_or_else(|| "agent".into());
    let attempt = ss.attempt.unwrap_or(0) + 1;
    let started = persist::now_string();

    let entry = state.steps.entry(step_id.to_string()).or_default();
    entry.status = Status::Running;
    entry.lease = Some(lease.clone());
    entry.heartbeat = Some(now);
    entry.started_at = Some(started);
    entry.attempt = Some(attempt);
    entry.reason = None;

    let inputs = engine::resolve_inputs(step, &state).unwrap_or_default();
    let events = vec![
        event(Some(step_id), "claimed", Some(json!({ "lease": lease, "attempt": attempt }))),
        event(Some(step_id), "started", None),
    ];
    commit(&dir, &events, &state)?;

    Ok(json!({
        "run_id": run_id,
        "step": step_id,
        "status": "running",
        "lease": lease,
        "attempt": attempt,
        "action": step.action,
        "description": step.description,
        "params": step.params,
        "inputs": inputs,
        "declared_outputs": step.outputs,
        "hint": "perform the action, then report the outcome with workflow_record(run_id, step, event=completed|failed|blocked, detail)",
    }))
}

/// Record an executor/agent outcome for a claimed step and re-evaluate.
pub(crate) fn record(
    settings: &SettingsState,
    run_id: &str,
    step_id: &str,
    event_kind: &str,
    detail: Option<Value>,
) -> Result<Value, String> {
    let (dir, def, mut state) = load_bundle(settings, run_id)?;
    let now = persist::now_ts();
    engine::recompute(&def, &mut state, now);

    let step = find_step(&def, step_id)?;
    let cur = state.steps.get(step_id).map(|s| s.status).unwrap_or_default();
    if cur != Status::Running && cur != Status::Ready {
        return Err(format!(
            "step '{step_id}' is '{cur_status}', so an outcome cannot be recorded. Advance it first, or use workflow_intervene.",
            cur_status = cur.as_str()
        ));
    }

    let outputs = detail
        .as_ref()
        .and_then(|d| d.get("outputs"))
        .and_then(|o| o.as_object())
        .cloned()
        .unwrap_or_default();
    let reason = detail
        .as_ref()
        .and_then(|d| d.get("reason").or_else(|| d.get("error")))
        .and_then(|r| r.as_str())
        .map(|s| s.to_string());

    let max_attempts = step.max_attempts();
    let attempt = state.steps.get(step_id).and_then(|s| s.attempt).unwrap_or(1).max(1);
    let now_str = persist::now_string();

    let mut events: Vec<Event> = Vec::new();
    let entry = state.steps.entry(step_id.to_string()).or_default();
    match event_kind {
        "completed" => {
            entry.status = Status::Completed;
            entry.finished_at = Some(now_str);
            entry.outputs = outputs;
            entry.reason = None;
            entry.lease = None;
            events.push(event(Some(step_id), "completed", detail.clone()));
        }
        "blocked" => {
            entry.status = Status::Blocked;
            entry.reason = reason.clone().or_else(|| Some("marked blocked by agent".into()));
            entry.lease = None;
            events.push(event(Some(step_id), "blocked", detail.clone()));
        }
        "failed" => {
            if attempt < max_attempts {
                entry.status = Status::Ready;
                entry.reason = None;
                entry.lease = None;
                events.push(event(
                    Some(step_id),
                    "retried",
                    Some(json!({ "attempt": attempt, "max_attempts": max_attempts, "error": reason })),
                ));
            } else {
                entry.status = Status::Failed;
                entry.finished_at = Some(now_str);
                entry.reason = reason.clone().or_else(|| Some("step failed".into()));
                entry.lease = None;
                events.push(event(Some(step_id), "failed", detail.clone()));
            }
        }
        other => {
            return Err(format!(
                "unknown event '{other}'; expected one of: completed, failed, blocked"
            ))
        }
    }

    engine::recompute(&def, &mut state, now);
    // A run-level event marks a terminal run.
    match engine::classify_run(&def, &state) {
        "completed" => events.push(event(None, "workflow_completed", None)),
        "failed" => events.push(event(None, "workflow_failed", None)),
        "blocked" => events.push(event(None, "workflow_blocked", None)),
        _ => {}
    }
    commit(&dir, &events, &state)?;

    let mut resp = build_status(run_id, &def, &state);
    if let Some(obj) = resp.as_object_mut() {
        obj.insert("recorded".into(), json!({ "step": step_id, "event": event_kind }));
    }
    Ok(resp)
}

/// Manual recovery: `retry` / `unblock` / `skip` / `reset` a step.
pub(crate) fn intervene(
    settings: &SettingsState,
    run_id: &str,
    step_id: &str,
    op: &str,
) -> Result<Value, String> {
    let (dir, def, mut state) = load_bundle(settings, run_id)?;
    let now = persist::now_ts();
    engine::recompute(&def, &mut state, now);
    find_step(&def, step_id)?;

    let now_str = persist::now_string();
    let entry = state.steps.entry(step_id.to_string()).or_default();
    let ev_name = match op {
        "retry" => {
            entry.status = Status::Ready;
            entry.attempt = Some(0);
            entry.reason = None;
            entry.lease = None;
            entry.heartbeat = None;
            entry.finished_at = None;
            "retried"
        }
        "unblock" => {
            entry.status = Status::Pending;
            entry.reason = None;
            entry.lease = None;
            "unblocked"
        }
        "skip" => {
            entry.status = Status::Skipped;
            entry.finished_at = Some(now_str);
            entry.reason = Some("manually skipped".into());
            entry.lease = None;
            "skipped"
        }
        "reset" => {
            *entry = StepState::default();
            "resumed"
        }
        other => {
            return Err(format!(
                "unknown op '{other}'; expected one of: retry, unblock, skip, reset"
            ))
        }
    };

    let events = vec![event(Some(step_id), ev_name, Some(json!({ "op": op })))];
    engine::recompute(&def, &mut state, now);
    commit(&dir, &events, &state)?;

    let mut resp = build_status(run_id, &def, &state);
    if let Some(obj) = resp.as_object_mut() {
        obj.insert("intervened".into(), json!({ "step": step_id, "op": op }));
    }
    Ok(resp)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Load definition + state for a run, enforcing version binding.
fn load_bundle(
    settings: &SettingsState,
    run_id: &str,
) -> Result<(PathBuf, Definition, RunState), String> {
    let dir = persist::run_dir(settings, run_id)?;
    let def = persist::load_definition(&dir)?;
    let state = persist::load_state(&dir)?;
    if state.workflow.name != def.name || state.workflow.version != def.version {
        return Err(format!(
            "state.json is stale: created for '{} v{}' but workflow.yaml is now '{} v{}'. \
             Reset the run (workflow_intervene op=reset per step, or delete its directory and re-create) \
             rather than applying old state to a new graph.",
            state.workflow.name, state.workflow.version, def.name, def.version
        ));
    }
    Ok((dir, def, state))
}

fn find_step<'a>(def: &'a Definition, step_id: &str) -> Result<&'a Step, String> {
    def.steps.iter().find(|s| s.id == step_id).ok_or_else(|| {
        let valid: Vec<&str> = def.steps.iter().map(|s| s.id.as_str()).collect();
        format!(
            "unknown step '{step_id}'; valid steps: {}",
            valid.join(", ")
        )
    })
}

fn event(step: Option<&str>, kind: &str, detail: Option<Value>) -> Event {
    Event {
        time: persist::now_string(),
        step: step.map(|s| s.to_string()),
        event: kind.to_string(),
        detail,
    }
}

fn commit(dir: &Path, events: &[Event], state: &RunState) -> Result<(), String> {
    // Events are the source of truth: append them first, then rewrite state.
    persist::append_events(dir, events)?;
    persist::write_state_atomic(dir, state)
}

fn generated_run_id(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-');
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    let short = &uuid[..8];
    if slug.is_empty() {
        format!("run-{short}")
    } else {
        format!("{slug}-{short}")
    }
}

fn status_counts(def: &Definition, state: &RunState) -> Value {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for s in ["pending", "ready", "running", "completed", "failed", "blocked", "skipped"] {
        counts.insert(s, 0);
    }
    for step in &def.steps {
        let st = state.steps.get(&step.id).map(|ss| ss.status.as_str()).unwrap_or("pending");
        if let Some(c) = counts.get_mut(st) {
            *c += 1;
        }
    }
    json!(counts)
}

/// Build the structured `status` payload with actionable hints.
fn build_status(run_id: &str, def: &Definition, state: &RunState) -> Value {
    let order = engine::topo_order(def);
    let run_status = engine::classify_run(def, state);

    let mut steps_view: Map<String, Value> = Map::new();
    let mut ready: Vec<String> = Vec::new();
    let mut blocked: Vec<Value> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();

    for id in &order {
        let Some(ss) = state.steps.get(id) else {
            continue;
        };
        let mut m = Map::new();
        m.insert("status".into(), json!(ss.status.as_str()));
        if let Some(r) = &ss.reason {
            m.insert("reason".into(), json!(r));
        }
        if let Some(a) = ss.attempt {
            m.insert("attempt".into(), json!(a));
        }
        if let Some(l) = &ss.lease {
            m.insert("lease".into(), json!(l));
        }
        if let Some(s) = &ss.started_at {
            m.insert("started_at".into(), json!(s));
        }
        if let Some(f) = &ss.finished_at {
            m.insert("finished_at".into(), json!(f));
        }
        if !ss.outputs.is_empty() {
            m.insert("outputs".into(), Value::Object(ss.outputs.clone()));
        }
        steps_view.insert(id.clone(), Value::Object(m));

        match ss.status {
            Status::Ready => ready.push(id.clone()),
            Status::Blocked => blocked.push(json!({ "id": id, "reason": ss.reason })),
            Status::Failed => failed.push(json!({ "id": id, "reason": ss.reason })),
            _ => {}
        }
    }

    let mut hints: Vec<String> = Vec::new();
    match run_status {
        "completed" => hints.push("workflow completed: every step is completed or skipped".into()),
        "failed" => hints.push(
            "workflow failed: a step failed under on_failure=stop; use workflow_intervene (retry/reset) on it to resume".into(),
        ),
        "blocked" => hints.push(
            "workflow blocked: nothing is runnable; resolve the blocked/failed steps listed below".into(),
        ),
        _ => {}
    }
    if !ready.is_empty() {
        hints.push(format!(
            "{} step(s) ready: {}; claim one with workflow_advance then report via workflow_record",
            ready.len(),
            ready.join(", ")
        ));
    }
    for b in &blocked {
        hints.push(format!(
            "step '{}' is blocked: {}",
            b.get("id").and_then(|v| v.as_str()).unwrap_or("?"),
            b.get("reason").and_then(|v| v.as_str()).unwrap_or("unknown reason")
        ));
    }
    for f in &failed {
        hints.push(format!(
            "step '{}' failed: {}; retry or skip it with workflow_intervene",
            f.get("id").and_then(|v| v.as_str()).unwrap_or("?"),
            f.get("reason").and_then(|v| v.as_str()).unwrap_or("unknown reason")
        ));
    }

    json!({
        "run_id": run_id,
        "workflow": { "name": def.name, "version": def.version },
        "run_status": run_status,
        "counts": status_counts(def, state),
        "ready": ready,
        "blocked": blocked,
        "failed": failed,
        "steps": steps_view,
        "hints": hints,
    })
}
