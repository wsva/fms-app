//! Workflow core: typed schema (`Definition`/`RunState`/`Event`), the pure
//! engine (readiness + transitions, in [`engine`]), file IO ([`persist`]),
//! validation ([`validate`]), and the run-scoped public API the MCP tools call.
//!
//! Domain-free: nothing here knows what a step's `action` actually does, where a
//! run lives on disk, or what any check id *means*. Every public fn takes an
//! explicit *run base directory* (the directory holding that group's run
//! sub-directories); the caller resolves it (workspace jobs dir vs. a dataset's
//! own `workflows/`).
//!
//! [`verify`] is the one exception to "the caller decides everything": a
//! definition may bind check ids to a step under `verify:`, but the framework
//! treats them as opaque strings — the *binding layer* (`workflow::verify`) owns
//! the vocabulary, evaluates the checks and hands the outcome over as counts.
//! What lives here is the semantics that belongs to no domain: what a binding
//! implies, and the one-way demotion it can trigger.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

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

/// What a step's completion is *evidence* about, in the domain's check ids.
///
/// The framework never evaluates these: it only knows that a non-empty finding
/// count for a bound id is bad news, and which of the two kinds of bad news it
/// is. Ids are validated against the caller-supplied vocabulary
/// ([`create_run`]'s `check_ids`) so a typo fails the definition instead of
/// silently disabling a gate.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct StepVerify {
    /// Findings here mean "not yet": the step should not be attempted.
    #[serde(default)]
    pub blocks: Vec<String>,
    /// Findings here mean the step's output is missing, partial or drifted — a
    /// `completed` claim these contradict can be invalidated.
    #[serde(default)]
    pub proves: Vec<String>,
}

impl StepVerify {
    fn ids(&self) -> impl Iterator<Item = &str> {
        self.blocks.iter().map(String::as_str).chain(self.proves.iter().map(String::as_str))
    }
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
    /// Evidence this step claims to produce. Opaque ids, evaluated by the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<StepVerify>,
    /// Check ids that must come back **measured and clean** for this step to be
    /// recorded as completed without running it (see [`adopt`]).
    ///
    /// Opt-in per step and deliberately rare: a clean result has to *prove* the
    /// artifact exists, which an id counting offenders over an empty subject does
    /// not. "No media, so no media is missing subtitles" is vacuously clean, and
    /// adopting on that would put a green tick on work nobody did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopt: Option<Vec<String>>,
}

impl Step {
    fn on_failure(&self) -> OnFailure {
        self.on_failure.unwrap_or_default()
    }
    fn max_attempts(&self) -> u32 {
        self.retry.as_ref().map(|r| r.attempts.max(1)).unwrap_or(1)
    }
    /// Check ids this step adopts existing evidence for (never empty in practice:
    /// [`adopt`] skips a step whose list is absent or empty).
    fn adopt_ids(&self) -> &[String] {
        self.adopt.as_deref().unwrap_or(&[])
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

/// List every run in `base` (a run root directory) with a status summary.
pub(crate) fn list_runs(base: &Path) -> Result<Value, String> {
    let now = persist::now_ts();
    let mut runs: Vec<Value> = Vec::new();
    for id in persist::list_run_ids(base) {
        let Ok(dir) = persist::run_dir(base, &id) else {
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
///
/// `check_ids` is the caller's vocabulary — every id a `verify:` block may name.
/// Pass an empty slice when the caller evaluates no checks, which turns the
/// id-existence rule off rather than rejecting every binding.
pub(crate) fn create_run(
    base: &Path,
    run_id: Option<String>,
    definition_yaml: &str,
    check_ids: &[&str],
) -> Result<Value, String> {
    let def: Definition = serde_yaml::from_str(definition_yaml)
        .map_err(|e| format!("invalid workflow YAML: {e}"))?;
    validate::validate(&def, check_ids)?;

    let id = match run_id {
        Some(rid) if !rid.trim().is_empty() => persist::sanitize_run_id(&rid)?,
        _ => generated_run_id(&def.name),
    };

    let dir = persist::create_run_dir(base, &id)?;
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

/// Whether a run of this name already exists under `base`. Handed to the binding
/// layer, which needs to tell "create it, then write into it" from "load it".
pub(crate) fn run_exists(base: &Path, run_id: &str) -> bool {
    persist::run_exists(base, run_id)
}

/// Definition + current state + what's ready/blocked/failed and why.
pub(crate) fn status(base: &Path, run_id: &str) -> Result<Value, String> {
    let (dir, def, mut state) = load_bundle(base, run_id)?;
    engine::recompute(&def, &mut state, persist::now_ts());
    // Persist any recompute-derived changes (e.g. a reaped lease) so the on-disk
    // snapshot matches what we return.
    persist::write_state_atomic(&dir, &state)?;
    Ok(build_status(run_id, &def, &state))
}

/// Return the raw `workflow.yaml` text of a run, for loading back into the
/// page's editor (preserves UI-only fields like `title`).
pub(crate) fn definition_text(base: &Path, run_id: &str) -> Result<String, String> {
    let dir = persist::run_dir(base, run_id)?;
    persist::read_definition_text(&dir)
}

/// The runnable (`ready`, unclaimed) steps with resolved action + inputs.
pub(crate) fn next_steps(base: &Path, run_id: &str) -> Result<Value, String> {
    let (_dir, def, mut state) = load_bundle(base, run_id)?;
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
    base: &Path,
    run_id: &str,
    step_id: &str,
    agent_id: Option<String>,
) -> Result<Value, String> {
    let (dir, def, mut state) = load_bundle(base, run_id)?;
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
    base: &Path,
    run_id: &str,
    step_id: &str,
    event_kind: &str,
    detail: Option<Value>,
) -> Result<Value, String> {
    let (dir, def, mut state) = load_bundle(base, run_id)?;
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
    base: &Path,
    run_id: &str,
    step_id: &str,
    op: &str,
) -> Result<Value, String> {
    let (dir, def, mut state) = load_bundle(base, run_id)?;
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
// Verification
// ---------------------------------------------------------------------------

/// Score a run's steps against measured evidence, and optionally act on it.
///
/// `evidence` maps a check id to its offender count; an id the probe could not
/// evaluate is simply absent (which makes the affected step `undecidable`, not
/// clean). Verdicts are a **projection**: they are recomputed on every call and
/// never stored in `state.json`, so a verdict means "as of this probe" instead of
/// being a fact with no provenance.
///
/// The one way a probe may touch progress is `apply`: a `completed` step whose
/// `proves` evidence contradicts it goes back to `ready`. Verification only ever
/// moves a step *down* — a clean probe proves an artifact exists, never that this
/// run made it — and every demotion costs an `invalidated` event, so
/// `events.jsonl` stays the audit trail of progress decisions. A probe that
/// changes nothing writes nothing.
pub(crate) fn verify(
    base: &Path,
    run_id: &str,
    evidence: &HashMap<String, usize>,
    apply: bool,
) -> Result<Value, String> {
    let (dir, def, mut state) = load_bundle(base, run_id)?;
    let now = persist::now_ts();
    engine::recompute(&def, &mut state, now);

    let mut verdicts: Map<String, Value> = Map::new();
    let mut drifted: Vec<String> = Vec::new();
    let mut gated: Vec<String> = Vec::new();
    let mut unbound: Vec<String> = Vec::new();
    let mut undecidable: Vec<String> = Vec::new();
    let mut events: Vec<Event> = Vec::new();
    let mut applied: Vec<Value> = Vec::new();

    for step in &def.steps {
        let status = state.steps.get(&step.id).map(|ss| ss.status).unwrap_or_default();
        let Some(binding) = step.verify.as_ref() else {
            unbound.push(step.id.clone());
            verdicts.insert(step.id.clone(), json!({
                "status": status.as_str(),
                "verdict": "unknown",
                "reason": "this step declares no verify: binding",
            }));
            continue;
        };

        // Findings per bound side, and the ids this probe could not answer for.
        let hits = |ids: &[String]| -> Vec<Value> {
            ids.iter()
                .filter_map(|id| evidence.get(id.as_str()).filter(|n| **n > 0).map(|n| json!({ "check": id, "count": n })))
                .collect()
        };
        let missing: Vec<&str> =
            binding.ids().filter(|id| !evidence.contains_key(*id)).collect();
        let (blocks, proves) = (hits(&binding.blocks), hits(&binding.proves));

        let verdict = if binding.proves.is_empty() {
            // Nothing is claimed about the output, so nothing can contradict it.
            "unknown"
        } else if !proves.is_empty() {
            "drifted"
        } else if binding.proves.iter().all(|id| !evidence.contains_key(id.as_str())) {
            "unknown"
        } else {
            "verified"
        };
        if verdict == "unknown" && !missing.is_empty() {
            undecidable.push(step.id.clone());
        }
        if !missing.is_empty() && verdict == "verified" {
            // A clean bill of health over ids we never measured would be a lie.
            verdicts.insert(step.id.clone(), json!({
                "status": status.as_str(), "verdict": "unknown",
                "blocks": blocks, "proves": proves, "undecidable": missing,
            }));
            continue;
        }

        verdicts.insert(step.id.clone(), json!({
            "status": status.as_str(),
            "verdict": verdict,
            "blocks": blocks,
            "proves": proves,
            "bound": { "blocks": binding.blocks, "proves": binding.proves },
            "undecidable": missing,
        }));

        if verdict == "drifted" {
            if status == Status::Completed {
                drifted.push(step.id.clone());
                if apply {
                    let entry = state.steps.entry(step.id.clone()).or_default();
                    entry.status = Status::Ready;
                    entry.finished_at = None;
                    entry.lease = None;
                    entry.heartbeat = None;
                    entry.reason = None;
                    events.push(event(
                        Some(step.id.as_str()),
                        "invalidated",
                        Some(json!({
                            "from": "completed",
                            "to": "ready",
                            "checks": proves,
                            "by": "workflow_verify",
                        })),
                    ));
                    applied.push(json!({ "step": step.id, "from": "completed", "to": "ready" }));
                }
            }
        } else if !blocks.is_empty() && matches!(status, Status::Pending | Status::Ready) {
            gated.push(step.id.clone());
        }
    }

    let changed = !events.is_empty();
    if changed {
        // Re-derive what the demotion un-satisfied, then append-then-write.
        engine::recompute(&def, &mut state, now);
        commit(&dir, &events, &state)?;
        for (id, v) in verdicts.iter_mut() {
            if let Some(ss) = state.steps.get(id) {
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("status".into(), json!(ss.status.as_str()));
                }
            }
        }
    }

    let hint = if !applied.is_empty() {
        format!(
            "{} completed step(s) invalidated by the evidence and returned to ready: {}; perform their actions again and report with workflow_record",
            applied.len(),
            applied.iter().filter_map(|a| a.get("step").and_then(|s| s.as_str())).collect::<Vec<_>>().join(", "),
        )
    } else if !drifted.is_empty() {
        format!(
            "{} completed step(s) are contradicted by the evidence: {}; re-verify with apply=true to send them back to ready",
            drifted.len(),
            drifted.join(", "),
        )
    } else if unbound.len() == def.steps.len() {
        "no step of this definition declares a verify: binding, so nothing could be checked".into()
    } else {
        format!(
            "no completed step is contradicted; {} step(s) gated, {} step(s) bind no checks",
            gated.len(),
            unbound.len(),
        )
    };

    let mut resp = build_status(run_id, &def, &state);
    if let Some(obj) = resp.as_object_mut() {
        obj.insert("verdicts".into(), Value::Object(verdicts));
        obj.insert(
            "verification".into(),
            json!({
                "apply": apply,
                "changed": changed,
                "drifted": drifted,
                "gated": gated,
                "unbound": unbound,
                "undecidable": undecidable,
                "applied": applied,
            }),
        );
        obj.insert("verify_hint".into(), json!(hint));
    }
    Ok(resp)
}

// ---------------------------------------------------------------------------
// Adoption
// ---------------------------------------------------------------------------

/// Record a step as completed because its outcome is already on disk.
///
/// This is the mirror image of [`verify`]'s demotion, and the only act in the
/// engine that moves a step *up* without running it — which is why it is opt-in
/// per step (`adopt:`) rather than a rule over `verify:` bindings. The condition
/// is strict: every listed id must have been **measured** (present in
/// `evidence`) and come back with no offenders. An unmeasured id withholds, in
/// the same spirit as a verdict that refuses to certify what was never probed.
///
/// A step that is `completed`, `failed`, `skipped` or mid-run is left alone: a
/// human's intervention outranks the filesystem. And like every other transition
/// this costs an event — `adopted`, carrying which checks justified it — so a
/// green node is never indistinguishable from a tick that was earned by work.
/// Calling it again with the same evidence writes nothing.
pub(crate) fn adopt(
    base: &Path,
    run_id: &str,
    evidence: &HashMap<String, usize>,
) -> Result<Value, String> {
    let (dir, def, mut state) = load_bundle(base, run_id)?;
    let now = persist::now_ts();
    engine::recompute(&def, &mut state, now);

    let mut adopted: Vec<String> = Vec::new();
    let mut withheld: Vec<Value> = Vec::new();
    let mut events: Vec<Event> = Vec::new();

    for step in &def.steps {
        let wanted = step.adopt_ids();
        if wanted.is_empty() {
            continue;
        }
        let status = state.steps.get(&step.id).map(|ss| ss.status).unwrap_or_default();
        if !matches!(status, Status::Pending | Status::Ready) {
            continue;
        }

        let (unmeasured, findings) = (
            wanted.iter().map(String::as_str).filter(|id| !evidence.contains_key(*id)).collect::<Vec<_>>(),
            wanted.iter()
                .filter_map(|id| evidence.get(id.as_str()).filter(|n| **n > 0).map(|n| json!({ "check": id, "count": n })))
                .collect::<Vec<Value>>(),
        );
        if !unmeasured.is_empty() {
            withheld.push(json!({ "step": step.id, "why": "not measured", "checks": unmeasured }));
            continue;
        }
        if !findings.is_empty() {
            withheld.push(json!({ "step": step.id, "why": "the evidence disagrees", "checks": findings }));
            continue;
        }

        let entry = state.steps.entry(step.id.clone()).or_default();
        entry.status = Status::Completed;
        entry.finished_at = Some(persist::now_string());
        entry.lease = None;
        entry.heartbeat = None;
        events.push(event(
            Some(step.id.as_str()),
            "adopted",
            Some(json!({
                "from": status.as_str(),
                "to": "completed",
                "checks": wanted,
                "by": "workflow_adopt",
            })),
        ));
        adopted.push(step.id.clone());
    }

    let changed = !events.is_empty();
    if changed {
        // What the adopted claim satisfies has to be re-derived, exactly as a
        // reported completion would.
        engine::recompute(&def, &mut state, now);
        commit(&dir, &events, &state)?;
    }

    let hint = if changed {
        format!(
            "{} step(s) adopted as completed from evidence already on disk: {}; they need not be run, \
             and re-scan after any later change to their adopt: checks",
            adopted.len(),
            adopted.join(", "),
        )
    } else if withheld.is_empty() {
        "nothing adopted: no step of this definition declares an adopt: list that is waiting for \
         evidence".into()
    } else {
        format!(
            "nothing adopted; {} step(s) withheld by the evidence (see 'withheld')",
            withheld.len()
        )
    };

    let mut resp = build_status(run_id, &def, &state);
    if let Some(obj) = resp.as_object_mut() {
        obj.insert(
            "adoption".into(),
            json!({ "changed": changed, "adopted": adopted, "withheld": withheld }),
        );
        obj.insert("adopt_hint".into(), json!(hint));
    }
    Ok(resp)
}

// ---------------------------------------------------------------------------
// Definition refresh
// ---------------------------------------------------------------------------

/// Whether every status in this run was inherited rather than earned.
///
/// True when `events.jsonl` holds nothing but the run opening, adoptions,
/// invalidations and definition refreshes — no step was ever advanced, recorded,
/// skipped, retried or reset by an agent or a human. The binding layer uses it to
/// decide whether a run a page opened for itself may follow the app's current
/// template. A run holding real progress keeps the definition it was created
/// from, which is the whole reason the text is stored beside the state.
pub(crate) fn history_is_inherited_only(base: &Path, run_id: &str) -> Result<bool, String> {
    let dir = persist::run_dir(base, run_id)?;
    Ok(persist::read_events(&dir).iter().all(|e| {
        matches!(
            e.event.as_str(),
            "workflow_started" | "adopted" | "invalidated" | "definition_refreshed"
        )
    }))
}

/// Point a run at a new definition text, keeping the status of every step that
/// still exists under the same id.
///
/// Not a general operation — see [`history_is_inherited_only`] for who may call it.
/// Steps that vanished from the definition lose their record; steps the new text
/// adds arrive through the ordinary recompute (`pending`, or `ready`/`skipped` as
/// their dependencies and `when:` guards say). The refresh is journalled, so the
/// log still explains why a run's history stops matching its earlier graph.
pub(crate) fn rebind_definition(
    base: &Path,
    run_id: &str,
    definition_yaml: &str,
    known_checks: &[&str],
) -> Result<(), String> {
    let dir = persist::run_dir(base, run_id)?;
    let def: Definition = serde_yaml::from_str(definition_yaml)
        .map_err(|e| format!("invalid workflow YAML: {e}"))?;
    validate::validate(&def, known_checks)?;
    let mut state = persist::load_state(&dir)?;

    let carried: HashMap<String, StepState> = def
        .steps
        .iter()
        .filter_map(|s| state.steps.remove(s.id.as_str()).map(|ss| (s.id.clone(), ss)))
        .collect();
    let dropped: Vec<String> = state.steps.keys().cloned().collect();
    state.steps = carried;
    state.workflow = WorkflowRef {
        name: def.name.clone(),
        version: def.version,
    };

    persist::write_definition_text(&dir, definition_yaml)?;
    let events = vec![event(
        None,
        "definition_refreshed",
        Some(json!({
            "steps": def.steps.len(),
            "dropped": dropped,
            "by": "workflow_adopt",
        })),
    )];
    engine::recompute(&def, &mut state, persist::now_ts());
    commit(&dir, &events, &state)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Load definition + state for a run, enforcing version binding.
fn load_bundle(
    base: &Path,
    run_id: &str,
) -> Result<(PathBuf, Definition, RunState), String> {
    let dir = persist::run_dir(base, run_id)?;
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

#[cfg(test)]
mod tests {
    //! The verification honesty rules are the entire point of the seam, and they are
    //! behaviour, not prose: a verdict is a projection that never lands in
    //! `state.json`, evidence may only ever move a step *down*, and a probe that
    //! could not measure an id cannot certify over it. Tested against a real run
    //! directory, because `verify` writes through `persist` and that is half the risk.
    use super::*;

    const YAML: &str = r#"name: probe
version: 1
steps:
  - id: maker
    action: dataset_get
    verify:
      proves: [made]
      blocks: [needing]
  - id: follower
    action: dataset_get
    depends_on: [maker]
    verify:
      blocks: [needing]
  - id: unbound
    action: dataset_get
"#;

    /// The mirror case: a step that opts into inheriting progress from evidence,
    /// plus the dependent that only opens up once it does.
    const ADOPT_YAML: &str = r#"name: probe
version: 1
steps:
  - id: preparer
    action: dataset_init_dir
    adopt: [present]
  - id: consumer
    action: dataset_get
    depends_on: [preparer]
"#;

    fn temp_base(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fms-core-verify-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn evidence(pairs: &[(&str, usize)]) -> HashMap<String, usize> {
        pairs.iter().map(|(id, n)| (id.to_string(), *n)).collect()
    }

    fn state_text(base: &Path) -> String {
        std::fs::read_to_string(persist::run_dir(base, "probe").unwrap().join("state.json")).unwrap()
    }

    fn events_text(base: &Path) -> String {
        std::fs::read_to_string(persist::run_dir(base, "probe").unwrap().join("events.jsonl")).unwrap()
    }

    /// Complete `maker` the way an agent would: claim it, then report the outcome.
    fn complete_maker(base: &Path) {
        advance(base, "probe", "maker", None).unwrap();
        record(base, "probe", "maker", "completed", Some(json!({ "outputs": { "n": 1 } }))).unwrap();
    }

    #[test]
    fn contradicted_evidence_demotes_a_completed_step_and_writes_an_event() {
        let base = temp_base("demote");
        create_run(&base, Some("probe".into()), YAML, &["made", "needing"]).unwrap();
        complete_maker(&base);

        let dirty = evidence(&[("made", 3), ("needing", 0)]);

        // Reporting the drift changes nothing: `changed` is false and state.json is
        // untouched, so a probe cannot quietly rewrite progress.
        let report = verify(&base, "probe", &dirty, false).unwrap();
        assert_eq!(report["verdicts"]["maker"]["verdict"], json!("drifted"));
        assert_eq!(report["verdicts"]["maker"]["status"], json!("completed"));
        assert_eq!(report["verification"]["changed"], json!(false));
        assert_eq!(report["verification"]["applied"].as_array().unwrap().len(), 0);
        assert!(!state_text(&base).contains("verdict"), "verdicts must never be persisted");

        // Applied, it is a demotion — down to `ready`, one `invalidated` event, and the
        // step's recorded outputs kept so `when:` guards still resolve.
        let applied = verify(&base, "probe", &dirty, true).unwrap();
        assert_eq!(applied["verdicts"]["maker"]["status"], json!("ready"));
        assert_eq!(applied["steps"]["maker"]["outputs"]["n"], json!(1));
        assert_eq!(events_text(&base).matches("\"invalidated\"").count(), 1);
        assert!(events_text(&base).contains("workflow_verify"));

        // Re-applying an already-demoted step adds no second event: the probe is
        // idempotent while nothing changes.
        verify(&base, "probe", &dirty, true).unwrap();
        assert_eq!(events_text(&base).matches("\"invalidated\"").count(), 1);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_clean_probe_never_promotes_and_never_certifies_an_unmeasured_id() {
        let base = temp_base("unknown");
        create_run(&base, Some("probe".into()), YAML, &["made", "needing"]).unwrap();

        // Nothing has run, so `maker` sits at `ready` (no dependencies). A clean probe
        // over the artifacts it *would* produce says `verified` — and changes nothing:
        // there is no upward path from evidence to `completed`.
        let clean = verify(&base, "probe", &evidence(&[("made", 0), ("needing", 0)]), true).unwrap();
        assert_eq!(clean["steps"]["maker"]["status"], json!("ready"));
        assert_eq!(clean["verdicts"]["maker"]["verdict"], json!("verified"));
        assert_eq!(clean["verification"]["changed"], json!(false));
        // …and an unbound step is honestly `unknown`, listed as such.
        assert_eq!(clean["verdicts"]["unbound"]["verdict"], json!("unknown"));
        assert!(clean["verification"]["unbound"].as_array().unwrap().iter().any(|v| v == "unbound"));

        // `made` absent from the map means the provider never evaluated it, so the
        // clean bill of health above would have been a lie.
        let partial = verify(&base, "probe", &evidence(&[("needing", 0)]), false).unwrap();
        assert_eq!(partial["verdicts"]["maker"]["verdict"], json!("unknown"));
        assert!(partial["verdicts"]["maker"]["undecidable"].as_array().unwrap().iter().any(|v| v == "made"));
        assert!(partial["verification"]["undecidable"].as_array().unwrap().iter().any(|v| v == "maker"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn outstanding_blocks_evidence_gates_a_step_without_touching_it() {
        let base = temp_base("gate");
        create_run(&base, Some("probe".into()), YAML, &["made", "needing"]).unwrap();

        let gated = verify(&base, "probe", &evidence(&[("made", 0), ("needing", 2)]), true).unwrap();
        assert_eq!(gated["verdicts"]["follower"]["status"], json!("pending"));
        assert_eq!(gated["verdicts"]["follower"]["blocks"][0]["check"], json!("needing"));
        assert!(gated["verification"]["gated"].as_array().unwrap().iter().any(|v| v == "follower"));
        // A gate is advice, not an action: nothing was applied, nothing was written.
        assert_eq!(gated["verification"]["changed"], json!(false));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn adoption_completes_a_step_only_when_its_evidence_is_measured_and_clean() {
        let base = temp_base("adopt");
        create_run(&base, Some("probe".into()), ADOPT_YAML, &["present", "needing"]).unwrap();
        assert_eq!(status(&base, "probe").unwrap()["steps"]["preparer"]["status"], json!("ready"));

        // Nothing has run, but the artifact `preparer` would produce is there and the
        // step asked to be allowed to inherit it: record it, open the dependent, and
        // say in the log who moved the step.
        let done = adopt(&base, "probe", &evidence(&[("present", 0)])).unwrap();
        assert_eq!(done["adoption"]["changed"], json!(true));
        assert_eq!(done["steps"]["preparer"]["status"], json!("completed"));
        assert_eq!(done["steps"]["consumer"]["status"], json!("ready"));
        let events = events_text(&base);
        assert_eq!(events.matches("\"adopted\"").count(), 1);
        assert!(events.contains("workflow_adopt"), "the event must name its author");

        // The same evidence again writes nothing.
        let again = adopt(&base, "probe", &evidence(&[("present", 0)])).unwrap();
        assert_eq!(again["adoption"]["changed"], json!(false));
        assert_eq!(events_text(&base).matches("\"adopted\"").count(), 1);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn adoption_withholds_when_the_check_disagrees_or_was_never_measured() {
        let base = temp_base("withhold");
        create_run(&base, Some("probe".into()), ADOPT_YAML, &["present", "needing"]).unwrap();

        // Findings: the artifact is not there after all.
        let dirty = adopt(&base, "probe", &evidence(&[("present", 1)])).unwrap();
        assert_eq!(dirty["adoption"]["changed"], json!(false));
        assert_eq!(dirty["steps"]["preparer"]["status"], json!("ready"));
        assert_eq!(dirty["adoption"]["withheld"][0]["step"], json!("preparer"));
        assert_eq!(dirty["adoption"]["withheld"][0]["why"], json!("the evidence disagrees"));

        // An absent id is not a clean one: nothing was probed, so nothing is proved.
        let blind = adopt(&base, "probe", &evidence(&[])).unwrap();
        assert_eq!(blind["adoption"]["changed"], json!(false));
        assert_eq!(blind["adoption"]["withheld"][0]["why"], json!("not measured"));

        // Both attempts wrote nothing at all — only `workflow_started` is on disk.
        assert_eq!(events_text(&base).lines().count(), 1);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// `ADOPT_YAML` grown the way a template grows between releases: one step
    /// renamed away, one added, `preparer` untouched. Used to prove a refresh
    /// carries inherited status instead of restarting the run.
    const ADOPT_YAML_V2: &str = r#"name: probe
version: 2
steps:
  - id: preparer
    action: dataset_init_dir
    adopt: [present]
  - id: newcomer
    action: dataset_get
    depends_on: [preparer]
"#;

    #[test]
    fn adoption_never_overrides_a_step_a_human_already_judged() {
        let base = temp_base("adopt-skip");
        create_run(&base, Some("probe".into()), ADOPT_YAML, &["present", "needing"]).unwrap();
        intervene(&base, "probe", "preparer", "skip").unwrap();

        // Clean evidence, and still no move: `skipped` is a decision, and re-deriving
        // it from the filesystem every scan would erase the human's.
        let res = adopt(&base, "probe", &evidence(&[("present", 0)])).unwrap();
        assert_eq!(res["adoption"]["changed"], json!(false));
        assert_eq!(res["steps"]["preparer"]["status"], json!("skipped"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_run_that_only_inherited_progress_may_follow_a_newer_definition() {
        let base = temp_base("refresh");
        create_run(&base, Some("probe".into()), ADOPT_YAML, &["present", "needing"]).unwrap();
        adopt(&base, "probe", &evidence(&[("present", 0)])).unwrap();
        assert!(
            history_is_inherited_only(&base, "probe").unwrap(),
            "an adoption is inherited progress, not a reason to freeze the definition"
        );

        rebind_definition(&base, "probe", ADOPT_YAML_V2, &["present", "needing"]).unwrap();
        let after = status(&base, "probe").unwrap();
        assert_eq!(after["workflow"]["version"], json!(2));
        assert_eq!(
            after["steps"]["preparer"]["status"],
            json!("completed"),
            "a step that still exists keeps the status the evidence gave it"
        );
        assert_eq!(after["steps"]["newcomer"]["status"], json!("ready"), "added steps arrive through the ordinary recompute");
        assert!(after["steps"].get("consumer").is_none(), "a vanished step loses its record");
        assert!(events_text(&base).contains("\"definition_refreshed\""));
        assert!(definition_text(&base, "probe").unwrap().contains("newcomer"), "state.json and workflow.yaml must not drift apart");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn earned_progress_freezes_a_run_against_the_current_template() {
        let base = temp_base("refresh-earned");
        create_run(&base, Some("probe".into()), ADOPT_YAML, &["present", "needing"]).unwrap();

        // Nothing adopted, just a claimed step: somebody is working on this run.
        assert!(history_is_inherited_only(&base, "probe").unwrap(), "a bare run opening is not progress");
        advance(&base, "probe", "preparer", None).unwrap();
        assert!(
            !history_is_inherited_only(&base, "probe").unwrap(),
            "a run an agent touched keeps the definition it was created from"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_refresh_validates_before_it_rewrites_anything() {
        let base = temp_base("refresh-bad");
        create_run(&base, Some("probe".into()), ADOPT_YAML, &["present", "needing"]).unwrap();
        adopt(&base, "probe", &evidence(&[("present", 0)])).unwrap();
        let before = state_text(&base);

        // The id does not exist in the evaluator's vocabulary, so the refresh is
        // refused outright — the run must still be readable and unchanged.
        let bad = ADOPT_YAML.replace("adopt: [present]", "adopt: [no_such_check]");
        assert!(rebind_definition(&base, "probe", &bad, &["present", "needing"]).is_err());
        assert_eq!(state_text(&base), before);
        assert_eq!(status(&base, "probe").unwrap()["steps"]["preparer"]["status"], json!("completed"));

        let _ = std::fs::remove_dir_all(&base);
    }
}
