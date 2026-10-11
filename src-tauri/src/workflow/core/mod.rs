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

use std::collections::{HashMap, HashSet};
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

/// What put a step in `blocked` — decides whether a `recompute` may clear it.
///
/// A `Dependency` block is derived from the graph (an upstream failed or is
/// itself blocked), so the pure recompute owns it and lifts it the moment the
/// upstream is fixed. An `External` block was set by a caller (`record blocked`,
/// a human parked the step awaiting approval or an outside resource) and is a
/// judgement, not a derivation — the recompute must never touch it, or a spot
/// check would silently reopen what someone deliberately stopped.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum BlockedBy {
    Dependency,
    External,
}

/// How a dependent treats a dependency that ended `skipped`. The spec says a
/// skip satisfies "if the dependent's policy allows"; this is that policy.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum SkipPolicy {
    /// A skip clears the dependency (the default — e.g. an optional branch).
    #[default]
    Satisfy,
    /// A skip leaves the dependent waiting, as an unmet dependency would.
    Block,
    /// A skip propagates: the dependent is skipped too.
    Skip,
}

/// Why a step is `skipped` — decides whether a `recompute` may re-open it.
///
/// A `WhenFalse` skip is a live reading of a guard, so when an upstream reset or
/// cascade changes the inputs that guard reads, the step is re-derived (it may
/// become ready). A `Manual` or `Propagated` skip is a recorded decision and
/// stays terminal until a human changes it — nothing in the filesystem re-opens
/// a step someone deliberately skipped.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    WhenFalse,
    Manual,
    Propagated,
}

/// The vacuity class of a check id, tagged by the binding layer and treated here
/// as an opaque structural fact. The engine reasons only about the *composition*
/// of an `adopt:` list, never about what any check measures — so the words
/// describe collection shape, not any domain.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    /// Counts offenders over a collection; reads clean when the collection is
    /// empty, which proves nothing ("no media, so no media is missing subtitles").
    PerSubject,
    /// Reports an offender exactly when the subject itself is absent — the guard
    /// that turns a bundle of vacuous `PerSubject` zeros into a real claim.
    SubjectPresent,
    /// A single file's existence; cannot be vacuously true.
    FileExists,
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
    /// How long a claim on this step stays live — the lease timeout in seconds
    /// (see [`Step::lease_secs`] and [`engine::LEASE_TIMEOUT_SECS`]). Not a work
    /// budget and not an executor deadline: this build has no executor, so the
    /// only thing it governs is when an unreaped `running` step is assumed dead.
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
    /// How this step reacts when one of its dependencies ends `skipped`. Defaults
    /// to [`SkipPolicy::Satisfy`] — an optional branch that skipped still lets the
    /// dependent run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_skipped_dependency: Option<SkipPolicy>,
    /// How many times a claim on this step may be reaped as interrupted before it
    /// is given up on as `failed`. Separate from `retry.attempts` on purpose: a
    /// reaped lease is a *crash*, not an error the executor reported, and a user
    /// may want a different tolerance for each. Defaults to
    /// [`Step::DEFAULT_MAX_INTERRUPTS`] when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_interrupts: Option<u32>,
}

impl Step {
    fn on_failure(&self) -> OnFailure {
        self.on_failure.unwrap_or_default()
    }
    fn skip_policy(&self) -> SkipPolicy {
        self.on_skipped_dependency.unwrap_or_default()
    }
    fn max_attempts(&self) -> u32 {
        self.retry.as_ref().map(|r| r.attempts.max(1)).unwrap_or(1)
    }
    /// Reaps this many before the step is abandoned as `failed`. Kept small on
    /// purpose: an executor that crashes every time must not loop forever (a reap
    /// consumes no `retry.attempts`, so without this budget nothing stops it).
    fn max_interrupts(&self) -> u32 {
        self.max_interrupts.unwrap_or(Self::DEFAULT_MAX_INTERRUPTS)
    }
    const DEFAULT_MAX_INTERRUPTS: u32 = 3;
    /// Check ids this step adopts existing evidence for (never empty in practice:
    /// [`adopt`] skips a step whose list is absent or empty).
    fn adopt_ids(&self) -> &[String] {
        self.adopt.as_deref().unwrap_or(&[])
    }
    /// Seconds a claim on this step is honoured for before [`engine::recompute`]
    /// reaps it back to `pending`. Defaults to [`engine::LEASE_TIMEOUT_SECS`],
    /// which is tuned for a crash, not for a long batch: a step that legitimately
    /// outlives 5 minutes (transcribing a dataset, say) has to raise `timeout:`
    /// itself, or the engine declares it dead while it is still being worked on.
    fn lease_secs(&self) -> i64 {
        self.timeout
            .filter(|t| *t > 0)
            .map(|t| t.min(i64::MAX as u64) as i64)
            .unwrap_or(engine::LEASE_TIMEOUT_SECS)
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
    /// For a `blocked` step: whether the graph owns the block (`Dependency`, so
    /// a recompute lifts it when the upstream is fixed) or a caller does
    /// (`External`, preserved across every recompute — see [`BlockedBy`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<BlockedBy>,
    /// For a `skipped` step: why it was skipped, deciding whether a recompute may
    /// re-derive it — see [`SkipReason`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<SkipReason>,
    /// The token identifying the current `running` claim ([`advance`] mints it,
    /// [`record`] demands it once the step has been reaped at least once). A late
    /// report from an executor whose claim was already reaped and re-granted
    /// carries a stale id, so it is rejected instead of clobbering the live claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_id: Option<String>,
    /// How many times this step's lease has been reaped as interrupted, counted
    /// against [`Step::max_interrupts`]. Reset on a clean completion or an
    /// explicit human `retry`/`reset`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupts: Option<u32>,
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
/// `check_vocab` is the caller's vocabulary — every id a `verify:`/`adopt:` block
/// may name, each tagged with its vacuity [`CheckKind`]. Pass an empty slice when
/// the caller evaluates no checks, which turns the id-existence and vacuity rules
/// off rather than rejecting every binding.
pub(crate) fn create_run(
    base: &Path,
    run_id: Option<String>,
    definition_yaml: &str,
    check_vocab: &[(&str, CheckKind)],
) -> Result<Value, String> {
    let def: Definition = serde_yaml::from_str(definition_yaml)
        .map_err(|e| format!("invalid workflow YAML: {e}"))?;
    validate::validate(&def, check_vocab)?;

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
///
/// Read-only. It recomputes derived statuses in memory — so the view it returns is
/// accurate after a restart — but never writes: a status probe must not quietly
/// persist a reaped lease (which would move `state.json` ahead of `events.jsonl`
/// with no event behind it). An expired lease is *reported* in `expired_claims` and
/// the `lease_hint`; the reap is persisted only by a mutating call or [`reap`].
pub(crate) fn status(base: &Path, run_id: &str) -> Result<Value, String> {
    let (_dir, def, mut state) = load_bundle(base, run_id)?;
    let reaps = engine::recompute(&def, &mut state, persist::now_ts());
    let mut out = build_status(run_id, &def, &state);
    attach_expired_claims(&mut out, &reaps);
    Ok(out)
}

/// Explicitly persist the reaping of any expired lease, journalling one
/// `interrupted` (or, past the budget, `failed`) event per reaped claim. This is
/// the mutating home for a reap that [`status`] only reports: an agent that wants
/// to clear stalled claims without advancing or recording anything else calls it.
/// A second identical call finds nothing expired and writes nothing.
pub(crate) fn reap(base: &Path, run_id: &str) -> Result<Value, String> {
    let (dir, def, mut state) = load_bundle(base, run_id)?;
    let reaps = engine::recompute(&def, &mut state, persist::now_ts());
    if reaps.is_empty() {
        return Ok(build_status(run_id, &def, &state));
    }
    let events = reap_events(&reaps);
    commit(&dir, &events, &state)?;
    let mut out = build_status(run_id, &def, &state);
    attach_expired_claims(&mut out, &reaps);
    Ok(out)
}

/// Return the raw `workflow.yaml` text of a run, for loading back into the
/// page's editor (preserves UI-only fields like `title`).
pub(crate) fn definition_text(base: &Path, run_id: &str) -> Result<String, String> {
    let dir = persist::run_dir(base, run_id)?;
    persist::read_definition_text(&dir)
}

/// The runnable (`ready`, unclaimed) steps with resolved action + inputs.
///
/// Read-only like [`status`]: it recomputes in memory and reports any expired lease
/// in `expired_claims`/`lease_hint`, but persists nothing.
pub(crate) fn next_steps(base: &Path, run_id: &str) -> Result<Value, String> {
    let (_dir, def, mut state) = load_bundle(base, run_id)?;
    let now = persist::now_ts();
    let reaps = engine::recompute(&def, &mut state, now);

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
            "lease_secs": step.lease_secs(),
        }));
    }

    // A step claimed and never reported is the one state in which nothing is
    // runnable *and* nothing will ever become runnable by itself. Name it:
    // "no runnable steps" reads to a waiting agent like "the app is busy",
    // which is how a session ends with a claimed step and no result.
    let claimed: Vec<Value> = def
        .steps
        .iter()
        .filter_map(|s| {
            let ss = state.steps.get(&s.id)?;
            (ss.status == Status::Running).then(|| {
                json!({
                    "id": s.id,
                    "lease": ss.lease,
                    "started_at": ss.started_at,
                    "attempt": ss.attempt,
                    "claim_id": ss.claim_id,
                    "interrupts": ss.interrupts,
                    "lease_secs": s.lease_secs(),
                })
            })
        })
        .collect();

    let run_status = engine::classify_run(&def, &state);
    let hint = if !steps.is_empty() {
        format!(
            "{} step(s) ready; call workflow_advance(run_id, step) to claim one, do the work, then report via workflow_record",
            steps.len()
        )
    } else if let Some(held) = claimed.first().and_then(|c| c["id"].as_str()) {
        format!(
            "no runnable steps: '{}' is claimed{} and its outcome was never reported, so nothing \
             else can start. Report it with workflow_record(run_id, step, \
             event=completed|failed) — a reaped lease makes the step ready again and \
             workflow_record accepts a ready step, so recovery needs no intervene. \
             workflow_intervene(op=reset) only hands the claim to someone else while the work \
             may still be running.",
            held,
            if claimed.len() > 1 { format!(" (with {} other(s))", claimed.len() - 1) } else { String::new() },
        )
    } else {
        format!("no runnable steps (run_status={run_status}); call workflow_status for blocked/failed reasons")
    };
    let mut out = json!({ "run_id": run_id, "run_status": run_status, "steps": steps, "claimed": claimed, "hint": hint });
    attach_expired_claims(&mut out, &reaps);
    Ok(out)
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
    // A mutating call is one of the homes for a reap: any expired claim this
    // advance touches is journalled (`interrupted`/`failed`) in the same write.
    let reaps = engine::recompute(&def, &mut state, now);

    let step = find_step(&def, step_id)?;
    let ss = state.steps.get(step_id).cloned().unwrap_or_default();

    match ss.status {
        Status::Ready => {}
        Status::Running => {
            if engine::lease_live(step, ss.heartbeat, now) {
                return Err(format!(
                    "step '{step_id}' is already claimed by '{}' (lease held for {}s of its {}s); \
                     wait for it, or workflow_intervene(op=reset) if that claim is stale",
                    ss.lease.clone().unwrap_or_else(|| "another agent".into()),
                    now - ss.heartbeat.unwrap_or(now),
                    step.lease_secs(),
                ));
            }
            // Expired lease: fall through and re-claim (executors are idempotent).
        }
        other => {
            // Name the obstacle instead of forwarding to another call: the reason is
            // already in hand, and an agent that has to make one more round trip to
            // learn why a step is not ready usually does not make it.
            let unmet: Vec<String> = step
                .depends_on
                .iter()
                .filter(|dep| {
                    !matches!(
                        state.steps.get(dep.as_str()).map(|s| s.status),
                        Some(Status::Completed) | Some(Status::Skipped)
                    )
                })
                .map(|dep| {
                    let st = state
                        .steps
                        .get(dep.as_str())
                        .map(|s| s.status.as_str())
                        .unwrap_or("pending");
                    format!("'{dep}' is '{st}'")
                })
                .collect();
            let why = if unmet.is_empty() {
                ss.reason
                    .clone()
                    .unwrap_or_else(|| "no dependency is unmet, so this status was recorded".into())
            } else {
                format!("unfinished dependencies: {}", unmet.join(", "))
            };
            return Err(format!(
                "step '{step_id}' is '{:?}', not 'ready', so it cannot be advanced: {why}. Advance \
                 what it waits on first, or workflow_intervene to change this step's state.",
                other
            ))
        }
    }

    let lease = agent_id.filter(|a| !a.trim().is_empty()).unwrap_or_else(|| "agent".into());
    let attempt = ss.attempt.unwrap_or(0) + 1;
    let started = persist::now_string();
    // A fresh, unique token per claim: the only thing that lets `record` tell a
    // live holder from a reaped executor reporting late (see [`record`]'s token gate).
    let claim_id = new_claim_id(attempt);

    let entry = state.steps.entry(step_id.to_string()).or_default();
    entry.status = Status::Running;
    entry.lease = Some(lease.clone());
    entry.heartbeat = Some(now);
    entry.started_at = Some(started);
    entry.attempt = Some(attempt);
    entry.claim_id = Some(claim_id.clone());
    entry.reason = None;

    let inputs = engine::resolve_inputs(step, &state).unwrap_or_default();
    let mut events = reap_events(&reaps);
    events.push(event(
        Some(step_id),
        "claimed",
        Some(json!({ "lease": lease, "attempt": attempt, "claim_id": claim_id })),
    ));
    events.push(event(
        Some(step_id),
        "started",
        Some(json!({ "claim_id": claim_id })),
    ));
    commit(&dir, &events, &state)?;

    Ok(json!({
        "run_id": run_id,
        "step": step_id,
        "status": "running",
        "lease": lease,
        "attempt": attempt,
        "claim_id": claim_id,
        "action": step.action,
        "description": step.description,
        "params": step.params,
        "inputs": inputs,
        "declared_outputs": step.outputs,
        "lease_secs": step.lease_secs(),
        "hint": "perform the action, then report the outcome with workflow_record(run_id, step, event=completed|failed|blocked, detail) before this step's lease expires; pass claim_id back on the report if this step was ever reaped",
    }))
}

/// Record an executor/agent outcome for a claimed step and re-evaluate.
///
/// `claim_id` is the token [`advance`] minted for this claim. It is only *demanded*
/// for a step that has already been reaped once (see the gate below): a first-time
/// executor may report without it, so existing agents keep working, but a step that
/// has lost a claim to a crash must not be completed by a late report carrying a
/// stale token that would clobber the live claim.
pub(crate) fn record(
    base: &Path,
    run_id: &str,
    step_id: &str,
    event_kind: &str,
    detail: Option<Value>,
    claim_id: Option<&str>,
) -> Result<Value, String> {
    let (dir, def, mut state) = load_bundle(base, run_id)?;
    let now = persist::now_ts();
    // Mutating call: any expired lease reaped here is journalled in this same write.
    let reaps = engine::recompute(&def, &mut state, now);

    let step = find_step(&def, step_id)?;
    let cur = state.steps.get(step_id).map(|s| s.status).unwrap_or_default();
    if cur != Status::Running && cur != Status::Ready {
        return Err(format!(
            "step '{step_id}' is '{cur_status}', so an outcome cannot be recorded. Advance it first, or use workflow_intervene.",
            cur_status = cur.as_str()
        ));
    }

    // Claim-token gate (3.2). Only a *live* claim can be clobbered, so this bites
    // only while the step is `Running`; a reaped-then-ready step (no live claim)
    // still accepts the late original executor's report, which is the documented
    // recovery path. Strict once the step has been reaped at least once, lenient
    // otherwise (a first claim reports with no token at all).
    if cur == Status::Running {
        let live = state.steps.get(step_id).and_then(|s| s.claim_id.clone());
        let interrupts = state.steps.get(step_id).and_then(|s| s.interrupts).unwrap_or(0);
        match (claim_id, &live) {
            (Some(tok), Some(live)) if tok != live => {
                return Err(format!(
                    "step '{step_id}' is now claimed under token '{live}', not '{tok}': this report \
                     is from an executor whose claim was reaped and re-granted, so accepting it would \
                     clobber the live claim. Nothing was written. The current holder reports with \
                     claim_id='{live}'."
                ));
            }
            (None, _) if interrupts > 0 => {
                return Err(format!(
                    "step '{step_id}' has been reaped {interrupts} time(s) and is claimed under token \
                     '{}'; a report must present that claim_id (returned by workflow_advance), because \
                     a step that already lost a claim to a crash must not be completed by a token we \
                     cannot match to the live holder. Nothing was written.",
                    live.clone().unwrap_or_else(|| "(none recorded)".into())
                ));
            }
            _ => {}
        }
    }

    let mut outputs = detail
        .as_ref()
        .and_then(|d| d.get("outputs"))
        .and_then(|o| o.as_object())
        .cloned()
        .unwrap_or_default();
    // An agent's first instinct is to put the values straight on `detail`
    // (`{"model_version": "parakeet-v3"}`) instead of nested under `outputs`.
    // Accept that shape: the alternative is recording a completion with an empty
    // output map, which silently strands every dependent that reads one.
    if outputs.is_empty() {
        if let Some(d) = detail.as_ref().and_then(|d| d.as_object()) {
            outputs = d
                .iter()
                .filter(|(k, _)| !matches!(k.as_str(), "outputs" | "reason" | "error"))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
        }
    }
    let reason = detail
        .as_ref()
        .and_then(|d| d.get("reason").or_else(|| d.get("error")))
        .and_then(|r| r.as_str())
        .map(|s| s.to_string());

    // A completion missing a declared output is the one recording mistake whose
    // cost only shows up three steps later: a dependent's `when:` guard or
    // `inputs:` reference reads it, finds nothing, and sits `blocked` with no
    // event ever naming the cause. Refuse the write instead — the step is still
    // `running`, so the corrected call is accepted and nothing is lost.
    if event_kind == "completed" {
        let missing: Vec<&str> = step
            .outputs
            .iter()
            .map(String::as_str)
            .filter(|key| !outputs.contains_key(*key))
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "step '{step_id}' declares outputs [{}] but this completion is missing [{}]. \
                 Re-call workflow_record carrying those names under detail.outputs (or directly \
                 on detail), using JSON `null` when a value genuinely does not exist so a \
                 dependent's 'when:' guard resolves false instead of the step sitting blocked. \
                 Nothing was written; '{step_id}' is still '{cur_status}'.",
                step.outputs.join(", "),
                missing.join(", "),
                cur_status = cur.as_str(),
            ));
        }
    }

    let max_attempts = step.max_attempts();
    let attempt = state.steps.get(step_id).and_then(|s| s.attempt).unwrap_or(1).max(1);
    let now_str = persist::now_string();

    let mut events: Vec<Event> = reap_events(&reaps);
    // The token this operation closes, captured before the entry clears it:
    // every outcome event names the claim it reports on (spec 3.2).
    let closing_claim = state.steps.get(step_id).and_then(|s| s.claim_id.clone());
    let entry = state.steps.entry(step_id.to_string()).or_default();
    match event_kind {
        "completed" => {
            entry.status = Status::Completed;
            entry.finished_at = Some(now_str);
            entry.outputs = outputs;
            entry.reason = None;
            entry.lease = None;
            entry.blocked_by = None;
            entry.skip_reason = None;
            // A clean completion ends the crash history and the live claim.
            entry.claim_id = None;
            entry.interrupts = None;
            events.push(event(
                Some(step_id),
                "completed",
                detail_with_claim(detail.clone(), closing_claim.as_deref()),
            ));
        }
        "blocked" => {
            entry.status = Status::Blocked;
            entry.reason = reason.clone().or_else(|| Some("marked blocked by agent".into()));
            entry.lease = None;
            // A caller-blocked step survives every later recompute until an
            // explicit `unblock` — the graph may not silently reopen it (F5).
            entry.blocked_by = Some(BlockedBy::External);
            entry.skip_reason = None;
            entry.claim_id = None;
            events.push(event(
                Some(step_id),
                "blocked",
                detail_with_claim(detail.clone(), closing_claim.as_deref()),
            ));
        }
        "failed" => {
            if attempt < max_attempts {
                entry.status = Status::Ready;
                entry.reason = None;
                entry.lease = None;
                entry.blocked_by = None;
                entry.skip_reason = None;
                entry.claim_id = None;
                events.push(event(
                    Some(step_id),
                    "retried",
                    Some(json!({
                        "attempt": attempt,
                        "max_attempts": max_attempts,
                        "error": reason,
                        "claim_id": closing_claim,
                    })),
                ));
            } else {
                entry.status = Status::Failed;
                entry.finished_at = Some(now_str);
                entry.reason = reason.clone().or_else(|| Some("step failed".into()));
                entry.lease = None;
                entry.blocked_by = None;
                entry.skip_reason = None;
                entry.claim_id = None;
                events.push(event(
                    Some(step_id),
                    "failed",
                    detail_with_claim(detail.clone(), closing_claim.as_deref()),
                ));
            }
        }
        other => {
            return Err(format!(
                "unknown event '{other}'; expected one of: completed, failed, blocked"
            ))
        }
    }

    // A step that fell back to `ready`/`failed`/`blocked` may leave a
    // already-`completed` dependent stranded on a now-unsatisfied upstream: reopen
    // it (F3) before re-deriving readiness, so the invariant never breaks. The
    // `invalidated` event records where the dependent actually *landed* after
    // recompute (normally `pending`), not the transient `ready` reopen set (N1/3.5).
    let reopened = engine::reopen_stale_completed(&def, &mut state);
    engine::recompute(&def, &mut state, now);
    events.extend(invalidated_events(&reopened, &state));
    // A run-level event marks a terminal run.
    match engine::classify_run(&def, &state) {
        "completed" => events.push(event(None, "workflow_completed", None)),
        "completed_with_failures" => {
            let failed: Vec<&str> = def
                .steps
                .iter()
                .filter(|s| state.steps.get(&s.id).map(|ss| ss.status == Status::Failed).unwrap_or(false))
                .map(|s| s.id.as_str())
                .collect();
            events.push(event(None, "workflow_completed_with_failures", Some(json!({ "failed": failed }))));
        }
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
    // A mutating call is a home for a reap: an expired claim this intervene
    // touches is journalled (`interrupted`/`failed`) in the same write.
    let reaps = engine::recompute(&def, &mut state, now);
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
            entry.blocked_by = None;
            entry.skip_reason = None;
            // A human retry restarts the crash history and the live claim.
            entry.claim_id = None;
            entry.interrupts = None;
            "retried"
        }
        "unblock" => {
            entry.status = Status::Pending;
            entry.reason = None;
            entry.lease = None;
            // The caller's parked block is lifted — the graph re-derives from here.
            entry.blocked_by = None;
            entry.claim_id = None;
            "unblocked"
        }
        "skip" => {
            entry.status = Status::Skipped;
            entry.finished_at = Some(now_str);
            entry.reason = Some("manually skipped".into());
            entry.lease = None;
            // A manual skip is a recorded decision: `recompute` preserves it and
            // never re-derives it from a `when:` guard (F5c).
            entry.skip_reason = Some(SkipReason::Manual);
            entry.blocked_by = None;
            entry.claim_id = None;
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

    let mut events = reap_events(&reaps);
    events.push(event(Some(step_id), ev_name, Some(json!({ "op": op }))));
    // Resetting or retrying a step that had `completed` dependents strands them on
    // a now-unsatisfied upstream: reopen them (F3) before re-deriving readiness.
    // The `invalidated` event records where each dependent actually landed.
    let reopened = if matches!(op, "reset" | "retry") {
        engine::reopen_stale_completed(&def, &mut state)
    } else {
        Vec::new()
    };
    engine::recompute(&def, &mut state, now);
    events.extend(invalidated_events(&reopened, &state));
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
    let reaps = engine::recompute(&def, &mut state, now);

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
    // A reap this verify observed is a status change and must cost an event, so it
    // is journalled whenever this call writes at all (its own demotions, or a reap).
    if changed || !reaps.is_empty() {
        let mut all_events = reap_events(&reaps);
        all_events.append(&mut events);
        // A demotion may strand an already-`completed` dependent on a now-
        // unsatisfied upstream: reopen it too (F3) before re-deriving readiness,
        // recording where each reopened dependent actually landed.
        let reopened = engine::reopen_stale_completed(&def, &mut state);
        // Re-derive what the demotion un-satisfied, then append-then-write.
        engine::recompute(&def, &mut state, now);
        all_events.extend(invalidated_events(&reopened, &state));
        commit(&dir, &all_events, &state)?;
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
    outputs: &HashMap<String, Map<String, Value>>,
) -> Result<Value, String> {
    let (dir, def, mut state) = load_bundle(base, run_id)?;
    let now = persist::now_ts();
    let reaps = engine::recompute(&def, &mut state, now);

    // Which outputs of each step a dependent reads back (through `inputs:` or a
    // `when:` guard). Adopting a step whose consumed output the call did not supply
    // would strand that dependent — the silent block F4 closes — so such a step is
    // withheld unless the adopt call hands the value over.
    let mut consumed: HashMap<String, HashSet<String>> = HashMap::new();
    for s in &def.steps {
        let mut exprs: Vec<&String> = s.inputs.values().collect();
        if let Some(w) = &s.when {
            exprs.push(w);
        }
        for e in exprs {
            for (src_id, out_key) in engine::scan_output_refs(e) {
                consumed.entry(src_id).or_default().insert(out_key);
            }
        }
    }

    let mut adopted: Vec<String> = Vec::new();
    let mut withheld: Vec<Value> = Vec::new();
    let mut events: Vec<Event> = Vec::new();

    // Adopt in topological order (F4), so a dependent is only ever considered once
    // its upstreams have had their chance to be adopted first — inheriting progress
    // out of definition order would mark a step green on top of an unadopted parent.
    for step_id in engine::topo_order(&def) {
        let Some(step) = def.steps.iter().find(|s| s.id == step_id) else {
            continue;
        };
        let wanted = step.adopt_ids();
        if wanted.is_empty() {
            continue;
        }
        let status = state.steps.get(&step_id).map(|ss| ss.status).unwrap_or_default();
        if !matches!(status, Status::Pending | Status::Ready) {
            continue;
        }

        // Dependency gating: every dependency must already be satisfied — completed,
        // skipped under this step's policy, or adopted earlier in this pass (its
        // status was just set to `completed` below). Otherwise the green tick would
        // sit on an upstream that has not earned it.
        let unmet = step.depends_on.iter().find(|dep| {
            let ds = state.steps.get(dep.as_str()).map(|ss| ss.status).unwrap_or(Status::Pending);
            match ds {
                Status::Completed => false,
                Status::Skipped => step.skip_policy() != SkipPolicy::Satisfy,
                _ => true,
            }
        });
        if let Some(dep) = unmet {
            withheld.push(json!({
                "step": step_id,
                "why": "dependency not satisfied",
                "checks": [dep.as_str()],
            }));
            continue;
        }

        let (unmeasured, findings) = (
            wanted.iter().map(String::as_str).filter(|id| !evidence.contains_key(*id)).collect::<Vec<_>>(),
            wanted.iter()
                .filter_map(|id| evidence.get(id.as_str()).filter(|n| **n > 0).map(|n| json!({ "check": id, "count": n })))
                .collect::<Vec<Value>>(),
        );
        if !unmeasured.is_empty() {
            withheld.push(json!({ "step": step_id, "why": "not measured", "checks": unmeasured }));
            continue;
        }
        if !findings.is_empty() {
            withheld.push(json!({ "step": step_id, "why": "the evidence disagrees", "checks": findings }));
            continue;
        }

        // Outputs gating: a declared output some dependent reads must be supplied by
        // the adopt call, or the dependent would resolve to nothing and sit blocked.
        let provided = outputs.get(&step_id);
        let missing: Vec<&String> = step
            .outputs
            .iter()
            .filter(|o| consumed.get(&step_id).map(|c| c.contains(o.as_str())).unwrap_or(false))
            .filter(|o| !provided.map(|p| p.contains_key(o.as_str())).unwrap_or(false))
            .collect();
        if !missing.is_empty() {
            withheld.push(json!({
                "step": step_id,
                "why": "outputs not derivable",
                "outputs": missing,
            }));
            continue;
        }

        let entry = state.steps.entry(step_id.clone()).or_default();
        entry.status = Status::Completed;
        entry.finished_at = Some(persist::now_string());
        entry.lease = None;
        entry.heartbeat = None;
        entry.blocked_by = None;
        entry.skip_reason = None;
        if let Some(provided) = provided {
            for (k, v) in provided {
                entry.outputs.insert(k.clone(), v.clone());
            }
        }
        events.push(event(
            Some(step_id.as_str()),
            "adopted",
            Some(json!({
                "from": status.as_str(),
                "to": "completed",
                "checks": wanted,
                "by": "workflow_adopt",
            })),
        ));
        adopted.push(step_id.clone());
    }

    let changed = !events.is_empty();
    // A reap adoption observed is a status change and must cost an event, so it is
    // journalled whenever this call writes at all (its own adoptions, or a reap).
    if changed || !reaps.is_empty() {
        let mut all_events = reap_events(&reaps);
        all_events.append(&mut events);
        // What the adopted claim satisfies has to be re-derived, exactly as a
        // reported completion would.
        engine::recompute(&def, &mut state, now);
        commit(&dir, &all_events, &state)?;
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
    known_checks: &[(&str, CheckKind)],
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
    let reaps = engine::recompute(&def, &mut state, persist::now_ts());
    let mut events = reap_events(&reaps);
    events.push(event(
        None,
        "definition_refreshed",
        Some(json!({
            "steps": def.steps.len(),
            "dropped": dropped,
            "by": "workflow_adopt",
        })),
    ));
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

/// A fresh, unique token per claim (spec 3.2): the attempt number plus a random
/// suffix, so a late report from an executor whose claim was already reaped and
/// re-granted carries a stale id [`record`] can reject instead of honouring.
fn new_claim_id(attempt: u32) -> String {
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    format!("{attempt}-{}", &uuid[..8])
}

/// Attach the claim a closing event refers to (spec 3.2): `claimed` always names
/// its token, and so do the events that end a claim (`started`, `completed`,
/// `failed`, `retried`, `blocked`), so a group-3 replay can rebuild the token
/// without guessing. A caller-supplied `claim_id` in `detail` wins; a non-object
/// detail is left exactly as written.
fn detail_with_claim(detail: Option<Value>, claim_id: Option<&str>) -> Option<Value> {
    let Some(claim) = claim_id else { return detail };
    let mut obj = match detail {
        Some(Value::Object(o)) => o,
        Some(other) => return Some(other),
        None => serde_json::Map::new(),
    };
    obj.entry("claim_id").or_insert(json!(claim));
    Some(Value::Object(obj))
}

/// One event per claim this operation reaped (spec 3.1): always an `interrupted`
/// event, plus a terminal `failed` event when the step exhausted its interrupt
/// budget ([`Step::max_interrupts`]). `detail` carries the whole claim — attempt,
/// claim id, holder, when it expired, the timeout — so a later replay can rebuild
/// the interruption without guessing. These belong to the reap, not to whatever
/// else the calling operation does, so they are prepended before its own events.
fn reap_events(reaps: &[engine::ReapedClaim]) -> Vec<Event> {
    let mut events = Vec::new();
    for r in reaps {
        events.push(event(
            Some(&r.id),
            "interrupted",
            Some(json!({
                "attempt": r.attempt,
                "claim_id": r.claim_id,
                "holder": r.holder,
                "expired_at": r.expired_at,
                "timeout_secs": r.timeout_secs,
                "gave_up": r.gave_up,
            })),
        ));
        if r.gave_up {
            events.push(event(
                Some(&r.id),
                "failed",
                Some(json!({
                    "reason": "interrupt budget exhausted (max_interrupts reached)",
                    "claim_id": r.claim_id,
                    "holder": r.holder,
                    "by": "reap",
                })),
            ));
        }
    }
    events
}

/// Report — never persist — the claims this read-only call found expired (spec
/// 3.1), so a probe of a stalled run says so out loud. [`status`] and
/// [`next_steps`] carry it; the reap itself is left to a mutating call or
/// [`reap`], which is the only way it costs an event.
fn attach_expired_claims(out: &mut Value, reaps: &[engine::ReapedClaim]) {
    if reaps.is_empty() {
        return;
    }
    let Some(obj) = out.as_object_mut() else { return };
    let claims: Vec<Value> = reaps
        .iter()
        .map(|r| json!({
            "step": r.id,
            "holder": r.holder,
            "claim_id": r.claim_id,
            "expired_at": r.expired_at,
            "timeout_secs": r.timeout_secs,
            "gave_up": r.gave_up,
        }))
        .collect();
    obj.insert("expired_claims".into(), Value::Array(claims));
    obj.insert(
        "lease_hint".into(),
        json!("these claims have expired, but this is a read-only call so nothing \
              was persisted; the reap lands on the next advance/record/intervene, \
              or call workflow_reap to clear them now"),
    );
}

/// One `invalidated` event per step the F3 cascade reopened, recording the status
/// it *actually* landed on after the following recompute (normally `pending`, not
/// the transient `ready` the reopen sets) — spec 3.5/N1. The caller runs
/// [`engine::reopen_stale_completed`] then [`engine::recompute`] and hands the
/// reopened pairs plus the settled state here, so the event never lies about where
/// a demoted dependent ended up.
fn invalidated_events(reopened: &[(String, String)], state: &RunState) -> Vec<Event> {
    reopened
        .iter()
        .map(|(id, dep)| {
            let to = state.steps.get(id).map(|ss| ss.status.as_str()).unwrap_or("pending");
            event(
                Some(id.as_str()),
                "invalidated",
                Some(json!({
                    "from": "completed",
                    "to": to,
                    "cascaded_from": dep,
                    "by": "cascade",
                })),
            )
        })
        .collect()
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
        "completed_with_failures" => hints.push(
            "workflow finished with failures: no step is runnable or waiting, but at least one failed \
             under on_failure=continue; the run reached its end with a red node — retry or skip the \
             failed steps below with workflow_intervene if they matter".into(),
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

    /// 3-arg `adopt` for the tests that supply no outputs (shadows the 4-arg core
    /// fn inside this module; tests that exercise the outputs gate call
    /// `super::adopt` directly with a real map).
    fn adopt(base: &Path, run_id: &str, evidence: &HashMap<String, usize>) -> Result<Value, String> {
        super::adopt(base, run_id, evidence, &HashMap::new())
    }

    /// 5-arg `record` for the tests that never exercise the claim-token gate
    /// (shadows the 6-arg core fn inside this module; the token tests call
    /// `super::record` directly with a real or stale `claim_id`).
    fn record(
        base: &Path,
        run_id: &str,
        step_id: &str,
        event_kind: &str,
        detail: Option<Value>,
    ) -> Result<Value, String> {
        super::record(base, run_id, step_id, event_kind, detail, None)
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
        create_run(&base, Some("probe".into()), YAML, &[("made", CheckKind::PerSubject), ("needing", CheckKind::PerSubject)]).unwrap();
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
        create_run(&base, Some("probe".into()), YAML, &[("made", CheckKind::PerSubject), ("needing", CheckKind::PerSubject)]).unwrap();

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
        create_run(&base, Some("probe".into()), YAML, &[("made", CheckKind::PerSubject), ("needing", CheckKind::PerSubject)]).unwrap();

        let gated = verify(&base, "probe", &evidence(&[("made", 0), ("needing", 2)]), true).unwrap();
        assert_eq!(gated["verdicts"]["follower"]["status"], json!("pending"));
        assert_eq!(gated["verdicts"]["follower"]["blocks"][0]["check"], json!("needing"));
        assert!(gated["verification"]["gated"].as_array().unwrap().iter().any(|v| v == "follower"));
        // A gate is advice, not an action: nothing was applied, nothing was written.
        assert_eq!(gated["verification"]["changed"], json!(false));

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A step a dependent reads through a `when:` guard. A completion that records
    /// no value for that guard is not a partially-done step — it is a graph whose
    /// next node can never be scheduled, with nothing in the log saying so.
    const OUTPUT_YAML: &str = r#"name: probe
version: 1
steps:
  - id: decider
    action: dataset_get
    outputs: [flag]
  - id: follower
    action: dataset_get
    depends_on: [decider]
    when: "${steps.decider.outputs.flag}"
"#;

    #[test]
    fn a_completion_is_refused_when_it_omits_an_output_the_graph_reads() {
        let base = temp_base("outputs");
        create_run(&base, Some("probe".into()), OUTPUT_YAML, &[]).unwrap();
        advance(&base, "probe", "decider", None).unwrap();

        // Detail that says something about the work but carries none of what the
        // step declares: refused, and the refusal is the fix instructions.
        let err = record(&base, "probe", "decider", "completed", Some(json!({ "note": "done" })))
            .expect_err("a declared output may not be silently omitted");
        assert!(err.contains("flag"), "the error must name the missing output: {err}");
        // Nothing was written, so the step is still `running` and the corrected
        // call lands on it — no re-claim, no lost attempt.
        assert_eq!(status(&base, "probe").unwrap()["steps"]["decider"]["status"], json!("running"));
        assert_eq!(events_text(&base).matches("\"completed\"").count(), 0);

        // The shape an agent reaches for naturally — the values straight on
        // `detail` — is accepted, not just the nested one, and the guard resolves.
        let done = record(&base, "probe", "decider", "completed", Some(json!({ "flag": "false" })))
            .unwrap();
        assert_eq!(done["steps"]["decider"]["outputs"]["flag"], json!("false"));
        assert_eq!(done["steps"]["follower"]["status"], json!("skipped"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn adoption_completes_a_step_only_when_its_evidence_is_measured_and_clean() {
        let base = temp_base("adopt");
        create_run(&base, Some("probe".into()), ADOPT_YAML, &[("present", CheckKind::FileExists), ("needing", CheckKind::PerSubject)]).unwrap();
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
        create_run(&base, Some("probe".into()), ADOPT_YAML, &[("present", CheckKind::FileExists), ("needing", CheckKind::PerSubject)]).unwrap();

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
        create_run(&base, Some("probe".into()), ADOPT_YAML, &[("present", CheckKind::FileExists), ("needing", CheckKind::PerSubject)]).unwrap();
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
        create_run(&base, Some("probe".into()), ADOPT_YAML, &[("present", CheckKind::FileExists), ("needing", CheckKind::PerSubject)]).unwrap();
        adopt(&base, "probe", &evidence(&[("present", 0)])).unwrap();
        assert!(
            history_is_inherited_only(&base, "probe").unwrap(),
            "an adoption is inherited progress, not a reason to freeze the definition"
        );

        rebind_definition(&base, "probe", ADOPT_YAML_V2, &[("present", CheckKind::FileExists), ("needing", CheckKind::PerSubject)]).unwrap();
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
        create_run(&base, Some("probe".into()), ADOPT_YAML, &[("present", CheckKind::FileExists), ("needing", CheckKind::PerSubject)]).unwrap();

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
        create_run(&base, Some("probe".into()), ADOPT_YAML, &[("present", CheckKind::FileExists), ("needing", CheckKind::PerSubject)]).unwrap();
        adopt(&base, "probe", &evidence(&[("present", 0)])).unwrap();
        let before = state_text(&base);

        // The id does not exist in the evaluator's vocabulary, so the refresh is
        // refused outright — the run must still be readable and unchanged.
        let bad = ADOPT_YAML.replace("adopt: [present]", "adopt: [no_such_check]");
        assert!(rebind_definition(&base, "probe", &bad, &[("present", CheckKind::FileExists), ("needing", CheckKind::PerSubject)]).is_err());
        assert_eq!(state_text(&base), before);
        assert_eq!(status(&base, "probe").unwrap()["steps"]["preparer"]["status"], json!("completed"));

        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- F3 / F5 / F7 / F9 / F10 / F4 invariant coverage -----------------

    /// `a → b → c` plus an independent `x`, for the F3 cascade and the F5
    /// external-block preservation.
    const CHAIN_YAML: &str = r#"name: probe
version: 1
steps:
  - id: a
    action: dataset_get
  - id: b
    action: dataset_get
    depends_on: [a]
  - id: c
    action: dataset_get
    depends_on: [b]
  - id: x
    action: dataset_get
"#;

    /// Claim and report a step as an agent would (these steps declare no outputs).
    fn complete_step(base: &Path, id: &str) {
        advance(base, "probe", id, None).unwrap();
        record(base, "probe", id, "completed", Some(json!({ "note": "ok" }))).unwrap();
    }

    #[test]
    fn demoting_a_step_reopens_its_completed_dependents() {
        let base = temp_base("cascade");
        create_run(&base, Some("probe".into()), CHAIN_YAML, &[]).unwrap();
        for id in ["a", "b", "c", "x"] {
            complete_step(&base, id);
        }
        assert_eq!(status(&base, "probe").unwrap()["steps"]["c"]["status"], json!("completed"));

        // Knock `a` out from under the chain: the invariant "completed ⇒ deps
        // satisfied" forces `b` and `c` open again, while the independent `x` stands.
        intervene(&base, "probe", "a", "reset").unwrap();
        let after = status(&base, "probe").unwrap();
        assert_ne!(after["steps"]["b"]["status"], json!("completed"), "b must reopen");
        assert_ne!(after["steps"]["c"]["status"], json!("completed"), "c must reopen");
        assert_eq!(after["steps"]["x"]["status"], json!("completed"), "an independent branch is untouched");
        let ev = events_text(&base);
        assert!(ev.contains("cascaded_from"), "the cascade must be journalled with its cause");
        assert_eq!(ev.matches("\"invalidated\"").count(), 2, "b and c reopen, one event each");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn an_external_block_survives_every_recompute_until_unblocked() {
        let base = temp_base("ext-block");
        create_run(&base, Some("probe".into()), CHAIN_YAML, &[]).unwrap();
        // A source step sits `ready`; a caller parks it.
        record(&base, "probe", "a", "blocked", Some(json!({ "reason": "awaiting approval" }))).unwrap();
        // `status` recomputes every time: a graph pass must not reopen a caller's park.
        assert_eq!(status(&base, "probe").unwrap()["steps"]["a"]["status"], json!("blocked"));
        assert_eq!(status(&base, "probe").unwrap()["steps"]["a"]["status"], json!("blocked"));

        intervene(&base, "probe", "a", "unblock").unwrap();
        assert_eq!(status(&base, "probe").unwrap()["steps"]["a"]["status"], json!("ready"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_when_false_skip_reopens_when_its_guard_input_changes() {
        let base = temp_base("whenfalse");
        create_run(&base, Some("probe".into()), OUTPUT_YAML, &[]).unwrap();
        advance(&base, "probe", "decider", None).unwrap();
        record(&base, "probe", "decider", "completed", Some(json!({ "flag": "false" }))).unwrap();
        assert_eq!(status(&base, "probe").unwrap()["steps"]["follower"]["status"], json!("skipped"));

        // The skip was a live reading of the guard, not a decision: reset the input
        // and the step is re-derived.
        intervene(&base, "probe", "decider", "reset").unwrap();
        assert_eq!(status(&base, "probe").unwrap()["steps"]["follower"]["status"], json!("pending"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_manual_skip_survives_an_upstream_reset() {
        let base = temp_base("manualskip");
        create_run(&base, Some("probe".into()), OUTPUT_YAML, &[]).unwrap();
        advance(&base, "probe", "decider", None).unwrap();
        record(&base, "probe", "decider", "completed", Some(json!({ "flag": "false" }))).unwrap();
        // A human overwrites the derived skip with their own judgement.
        intervene(&base, "probe", "follower", "skip").unwrap();

        // Resetting the guard's input must NOT reopen a recorded manual skip.
        intervene(&base, "probe", "decider", "reset").unwrap();
        assert_eq!(status(&base, "probe").unwrap()["steps"]["follower"]["status"], json!("skipped"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_typed_null_output_satisfies_the_contract_and_reads_false() {
        let base = temp_base("null-out");
        create_run(&base, Some("probe".into()), OUTPUT_YAML, &[]).unwrap();
        advance(&base, "probe", "decider", None).unwrap();
        // `flag` present but null: the name is carried (completion accepted) and a
        // dependent's `when:` guard reads it as false, not the literal "null".
        let done = record(&base, "probe", "decider", "completed", Some(json!({ "flag": null }))).unwrap();
        assert_eq!(done["steps"]["decider"]["outputs"]["flag"], json!(null));
        assert_eq!(done["steps"]["follower"]["status"], json!("skipped"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn an_output_reference_to_a_non_ancestor_is_rejected() {
        let base = temp_base("bad-ref");
        // `b` reads a sibling `s` that is not upstream of it.
        let yaml = r#"name: probe
version: 1
steps:
  - id: s
    action: dataset_get
    outputs: [v]
  - id: b
    action: dataset_get
    inputs:
      w: "${steps.s.outputs.v}"
"#;
        let err = create_run(&base, Some("r1".into()), yaml, &[]).expect_err("a non-ancestor read must fail");
        assert!(err.contains("transitive ancestor"), "unexpected error: {err}");

        // A `when:` guard racing a value that may never exist is caught too.
        let when_yaml = r#"name: probe
version: 1
steps:
  - id: s
    action: dataset_get
    outputs: [v]
  - id: b
    action: dataset_get
    when: "${steps.s.outputs.v}"
"#;
        assert!(create_run(&base, Some("r2".into()), when_yaml, &[]).is_err());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_zero_timeout_or_malformed_backoff_is_rejected() {
        let base = temp_base("bad-grammar");
        let zero = "name: probe\nversion: 1\nsteps:\n  - id: a\n    action: dataset_get\n    timeout: 0\n";
        assert!(create_run(&base, Some("r1".into()), zero, &[]).is_err(), "timeout: 0 must be rejected");
        let bad = "name: probe\nversion: 1\nsteps:\n  - id: a\n    action: dataset_get\n    retry:\n      attempts: 2\n      backoff: 30x\n";
        let err = create_run(&base, Some("r2".into()), bad, &[]).expect_err("a bad backoff must be rejected");
        assert!(err.contains("backoff"), "unexpected error: {err}");
        // The accepted grammar.
        let good = "name: probe\nversion: 1\nsteps:\n  - id: a\n    action: dataset_get\n    timeout: 600\n    retry:\n      attempts: 2\n      backoff: 30s\n";
        assert!(create_run(&base, Some("r3".into()), good, &[]).is_ok());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn an_adopt_list_of_only_per_subject_checks_is_rejected() {
        let base = temp_base("vacuous");
        let yaml = "name: probe\nversion: 1\nsteps:\n  - id: a\n    action: dataset_init_dir\n    adopt: [per_only]\n";
        let vocab = [("per_only", CheckKind::PerSubject), ("file_check", CheckKind::FileExists)];
        let err = create_run(&base, Some("r1".into()), yaml, &vocab).expect_err("a vacuous adopt list must be rejected");
        assert!(err.contains("empty"), "unexpected error: {err}");

        // Pairing it with a subject-present/file-exists guard makes the clean result
        // non-vacuous — the shape a literal "per_subject needs subject_absent" rule
        // would have wrongly rejected (a `sync_media`-style list guarded by a file).
        let paired = yaml.replace("adopt: [per_only]", "adopt: [per_only, file_check]");
        assert!(create_run(&base, Some("r2".into()), &paired, &vocab).is_ok());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn adoption_waits_for_an_unsatisfied_dependency_even_with_clean_evidence() {
        let base = temp_base("adopt-dep");
        let yaml = r#"name: probe
version: 1
steps:
  - id: a
    action: dataset_get
  - id: b
    action: dataset_init_dir
    depends_on: [a]
    adopt: [present]
"#;
        create_run(&base, Some("probe".into()), yaml, &[("present", CheckKind::FileExists)]).unwrap();
        // `b`'s evidence is clean, but its dependency `a` has not run: adopt withholds
        // rather than mark a green node on top of an unstarted parent.
        let res = adopt(&base, "probe", &evidence(&[("present", 0)])).unwrap();
        assert_eq!(res["steps"]["b"]["status"], json!("pending"));
        assert_eq!(res["adoption"]["withheld"][0]["step"], json!("b"));
        assert_eq!(res["adoption"]["withheld"][0]["why"], json!("dependency not satisfied"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn adoption_withholds_a_step_that_reads_an_unsupplied_output() {
        let base = temp_base("adopt-outputs");
        let yaml = r#"name: probe
version: 1
steps:
  - id: a
    action: dataset_init_dir
    adopt: [present]
    outputs: [v]
  - id: b
    action: dataset_get
    depends_on: [a]
    inputs:
      w: "${steps.a.outputs.v}"
"#;
        create_run(&base, Some("probe".into()), yaml, &[("present", CheckKind::FileExists)]).unwrap();
        // Clean evidence, but a's `v` is read by b and the call supplies nothing:
        // adopting a would strand b in a silent block, so it is withheld.
        let withheld = super::adopt(&base, "probe", &evidence(&[("present", 0)]), &HashMap::new()).unwrap();
        assert_eq!(withheld["steps"]["a"]["status"], json!("ready"));
        assert_eq!(withheld["adoption"]["withheld"][0]["why"], json!("outputs not derivable"));

        // Supplying the value lets the adoption proceed and opens the dependent.
        let mut provided = HashMap::new();
        let mut vals = Map::new();
        vals.insert("v".into(), json!(7));
        provided.insert("a".to_string(), vals);
        let done = super::adopt(&base, "probe", &evidence(&[("present", 0)]), &provided).unwrap();
        assert_eq!(done["steps"]["a"]["status"], json!("completed"));
        assert_eq!(done["steps"]["b"]["status"], json!("ready"));
        assert_eq!(done["steps"]["a"]["outputs"]["v"], json!(7));

        let _ = std::fs::remove_dir_all(&base);
    }

    // ---------------------------------------------------------------------
    // Group 2 (review round 3): read-only status/next + a real home for the
    // reap, the claim token, the interrupt budget, the `completed_with_failures`
    // outcome, and the cascaded `invalidated` recording where a dependent landed.
    // ---------------------------------------------------------------------

    /// Age a step's lease so the next recompute treats it as expired without
    /// waiting out the real `LEASE_TIMEOUT_SECS`: rewrite its `heartbeat` in
    /// `state.json` to the far past. The engine reads the persisted heartbeat on
    /// the next load, so this drives every reap test deterministically.
    fn expire_lease(base: &Path, step: &str) {
        let path = persist::run_dir(base, "probe").unwrap().join("state.json");
        let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        v["steps"][step]["heartbeat"] = json!(persist::now_ts() - 10_000_000);
        std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    }

    #[test]
    fn status_and_next_are_read_only_while_reap_persists_the_interrupted_event() {
        let base = temp_base("readonly");
        create_run(&base, Some("probe".into()), YAML, &[("made", CheckKind::PerSubject), ("needing", CheckKind::PerSubject)]).unwrap();
        advance(&base, "probe", "maker", None).unwrap();
        expire_lease(&base, "maker");

        let before_state = state_text(&base);
        let before_events = events_text(&base);

        // A read-only probe reports the expired claim but writes nothing at all.
        let st = status(&base, "probe").unwrap();
        assert_eq!(st["expired_claims"][0]["step"], json!("maker"));
        assert!(st["lease_hint"].is_string(), "status must hint the reap is pending");
        assert_eq!(state_text(&base), before_state, "status must not rewrite state.json");
        assert_eq!(events_text(&base), before_events, "status must not append events");

        // …and the same is true of `next`.
        let nx = next_steps(&base, "probe").unwrap();
        assert_eq!(nx["expired_claims"][0]["step"], json!("maker"));
        assert_eq!(state_text(&base), before_state, "next must not rewrite state.json");
        assert_eq!(events_text(&base), before_events, "next must not append events");

        // The explicit reap is the mutating home: exactly one `interrupted` event.
        let reaped = reap(&base, "probe").unwrap();
        assert_eq!(reaped["steps"]["maker"]["status"], json!("ready"), "reaped, then re-derived to ready");
        assert_eq!(events_text(&base).matches("\"interrupted\"").count(), 1);
        assert!(events_text(&base).contains("\"claim_id\""), "the interrupted event carries the claim");
        assert_ne!(state_text(&base), before_state, "a reap that acts must rewrite state");

        // A second reap finds nothing expired and writes nothing further.
        let after_state = state_text(&base);
        let after_events = events_text(&base);
        let again = reap(&base, "probe").unwrap();
        assert!(again.get("expired_claims").is_none(), "nothing left to report");
        assert_eq!(state_text(&base), after_state);
        assert_eq!(events_text(&base), after_events);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_stale_claim_token_is_rejected_and_a_live_one_is_honoured() {
        let base = temp_base("token");
        create_run(&base, Some("probe".into()), YAML, &[("made", CheckKind::PerSubject), ("needing", CheckKind::PerSubject)]).unwrap();

        let first = advance(&base, "probe", "maker", None).unwrap();
        let cid1 = first["claim_id"].as_str().unwrap().to_string();
        // The lease dies and the step is re-claimed by a fresh executor.
        expire_lease(&base, "maker");
        let second = advance(&base, "probe", "maker", None).unwrap();
        let cid2 = second["claim_id"].as_str().unwrap().to_string();
        assert_ne!(cid1, cid2, "each claim mints a fresh token");
        assert_eq!(second["status"], json!("running"));

        // The dead executor finally reports — with its now-stale token. Rejected,
        // and the state is byte-identical afterwards, so the live claim survives.
        let before_state = state_text(&base);
        let before_events = events_text(&base);
        let err = super::record(&base, "probe", "maker", "completed", None, Some(&cid1))
            .expect_err("a stale claim token must be refused");
        assert!(err.contains(&cid2), "the error must name the live claim: {err}");
        assert_eq!(state_text(&base), before_state, "a refused report writes nothing");
        assert_eq!(events_text(&base), before_events);

        // The current holder reports with its own token and succeeds.
        let done = super::record(&base, "probe", "maker", "completed", None, Some(&cid2)).unwrap();
        assert_eq!(done["steps"]["maker"]["status"], json!("completed"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn repeated_interruption_exhausts_the_budget_and_fails_the_step() {
        let base = temp_base("budget");
        create_run(&base, Some("probe".into()), YAML, &[("made", CheckKind::PerSubject), ("needing", CheckKind::PerSubject)]).unwrap();
        // Default `max_interrupts` is 3, so the 4th reap gives up on the step.
        for i in 1..=4 {
            advance(&base, "probe", "maker", None).unwrap();
            expire_lease(&base, "maker");
            let s = reap(&base, "probe").unwrap();
            let st = s["steps"]["maker"]["status"].as_str().unwrap();
            if i < 4 {
                assert_eq!(s["expired_claims"][0]["gave_up"], json!(false), "reap #{i} is inside the budget");
                assert_ne!(st, "failed", "a step within its interrupt budget stays runnable");
            } else {
                assert_eq!(s["expired_claims"][0]["gave_up"], json!(true), "the {i}th reap exhausts the budget");
                assert_eq!(st, "failed", "an exhausted step is abandoned as failed");
            }
        }
        let ev = events_text(&base);
        assert_eq!(ev.matches("\"interrupted\"").count(), 4, "one interrupted event per reap");
        assert!(ev.contains("max_interrupts"), "the giving-up reap must journal a failed reason");
        assert!(advance(&base, "probe", "maker", None).is_err(), "a failed step is not advanceable");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_run_finished_around_a_continue_failure_is_completed_with_failures() {
        let base = temp_base("terminal");
        let yaml = r#"name: probe
version: 1
steps:
  - id: soft
    action: dataset_get
    on_failure: continue
  - id: hard
    action: dataset_get
    on_failure: stop
"#;
        create_run(&base, Some("probe".into()), yaml, &[]).unwrap();
        // Write the step states straight into `state.json`, then read the honest
        // (recomputed) `run_status` a read-only probe returns. Each entry is a full
        // `StepState` object (`status` plus any provenance tags), not a bare string.
        let run_status_of = |states: &[(&str, Value)]| -> String {
            let path = persist::run_dir(&base, "probe").unwrap().join("state.json");
            let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            for (id, st) in states {
                v["steps"][*id] = st.clone();
            }
            std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
            status(&base, "probe").unwrap()["run_status"].as_str().unwrap().to_string()
        };

        assert_eq!(run_status_of(&[("soft", json!({ "status": "completed" })), ("hard", json!({ "status": "completed" }))]), "completed");
        // A continue-failure with everything else done: never the old `blocked`
        // fallthrough, never `completed` — it finished with a red node.
        assert_eq!(run_status_of(&[("soft", json!({ "status": "failed" })), ("hard", json!({ "status": "completed" }))]), "completed_with_failures");
        // A stop-failure halts the whole run, whatever else is done.
        assert_eq!(run_status_of(&[("soft", json!({ "status": "completed" })), ("hard", json!({ "status": "failed" }))]), "failed");
        // Nothing runnable but an external block still waiting: genuinely blocked.
        assert_eq!(
            run_status_of(&[("soft", json!({ "status": "blocked", "blocked_by": "external" })), ("hard", json!({ "status": "completed" }))]),
            "blocked",
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_cascaded_invalidation_records_where_the_dependent_landed() {
        let base = temp_base("cascade-to");
        // `follower` depends on `maker`; both are completed, then a contradicting
        // probe demotes `maker`. `follower` must reopen too, and its `invalidated`
        // event must record where it actually landed (recompute moves it to
        // `pending` behind a `ready` parent), not the transient `ready` the reopen
        // set — the exact N1/3.5 fix.
        let yaml = r#"name: probe
version: 1
steps:
  - id: maker
    action: dataset_get
    verify:
      proves: [made]
  - id: follower
    action: dataset_get
    depends_on: [maker]
"#;
        create_run(&base, Some("probe".into()), yaml, &[("made", CheckKind::PerSubject)]).unwrap();
        complete_maker(&base);
        advance(&base, "probe", "follower", None).unwrap();
        super::record(&base, "probe", "follower", "completed", None, None).unwrap();
        assert_eq!(status(&base, "probe").unwrap()["steps"]["follower"]["status"], json!("completed"));

        let dirty = evidence(&[("made", 3)]);
        let resp = verify(&base, "probe", &dirty, true).unwrap();
        let landed = resp["steps"]["follower"]["status"].clone();
        assert_eq!(landed, json!("pending"), "the reopened dependent re-derives to pending behind a ready parent");

        let ev = events_text(&base);
        let line = ev
            .lines()
            .find(|l| {
                serde_json::from_str::<Value>(l)
                    .map(|v| {
                        v["step"] == json!("follower")
                            && v["event"] == json!("invalidated")
                            && v["detail"]["cascaded_from"] == json!("maker")
                    })
                    .unwrap_or(false)
            })
            .expect("a cascaded invalidated event for follower must exist");
        let parsed: Value = serde_json::from_str(line).unwrap();
        assert_eq!(parsed["detail"]["to"], landed, "the invalidated `to` must equal the resulting status");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A deterministic LCG, so the random-DAG property test is reproducible
    /// without pulling in a `rand` dependency.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 33
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() as usize) % n
        }
    }

    /// Assert the engine invariant over the honest (recomputed) view: no step is
    /// `ready`, `running` or `completed` while a non-skipped dependency is
    /// unsatisfied (neither `completed` nor `skipped`).
    fn invariant_ok(base: &Path, steps: &[String], deps: &[Vec<String>]) -> Result<(), String> {
        let st = status(base, "probe")?;
        for (i, id) in steps.iter().enumerate() {
            let s = st["steps"][id].as_str().unwrap_or("pending");
            if matches!(s, "ready" | "running" | "completed") {
                for dep in &deps[i] {
                    let ds = st["steps"][dep].as_str().unwrap_or("pending");
                    if ds != "completed" && ds != "skipped" {
                        return Err(format!("step '{id}' is '{s}' while dependency '{dep}' is '{ds}'"));
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn no_step_is_active_over_an_unsatisfied_dependency_across_random_operations() {
        // Several seeds build several acyclic-by-construction DAGs; each is driven
        // through a random sequence of advance/record/intervene and the invariant is
        // re-checked after every single operation.
        for seed in 1..=8u64 {
            let mut rng = Lcg(seed * 2654435761);
            let n = 3 + rng.below(5); // 3..=7 steps
            let steps: Vec<String> = (0..n).map(|i| format!("s{i}")).collect();
            let mut lines = String::from("name: probe\nversion: 1\nsteps:\n");
            let mut deps: Vec<Vec<String>> = Vec::new();
            for i in 0..n {
                let mut d: Vec<String> = Vec::new();
                for j in 0..i {
                    if rng.below(3) == 0 {
                        d.push(format!("s{j}"));
                    }
                }
                deps.push(d.clone());
                lines.push_str(&format!("  - id: s{i}\n    action: dataset_get\n"));
                if !d.is_empty() {
                    lines.push_str(&format!("    depends_on: [{}]\n", d.join(", ")));
                }
            }

            let base = temp_base(&format!("prop-{seed}"));
            create_run(&base, Some("probe".into()), &lines, &[]).unwrap();
            invariant_ok(&base, &steps, &deps).expect("a fresh run already violates the invariant");

            for step_no in 0..40 {
                let pick = rng.below(n);
                match rng.below(4) {
                    0 => {
                        if advance(&base, "probe", &steps[pick], None).is_ok() {
                            super::record(&base, "probe", &steps[pick], "completed", None, None).ok();
                        }
                    }
                    1 => {
                        intervene(&base, "probe", &steps[pick], "reset").ok();
                    }
                    2 => {
                        intervene(&base, "probe", &steps[pick], "retry").ok();
                    }
                    _ => {
                        next_steps(&base, "probe").ok();
                    }
                }
                if let Err(e) = invariant_ok(&base, &steps, &deps) {
                    let _ = std::fs::remove_dir_all(&base);
                    panic!("seed {seed}, op {step_no} on '{}': {e}", steps[pick]);
                }
            }
            let _ = std::fs::remove_dir_all(&base);
        }
    }
}
