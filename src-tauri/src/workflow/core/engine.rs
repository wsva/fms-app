//! Engine core: readiness derivation, run classification, data-flow resolution
//! and lease/interruption recovery. All logic here is a pure function of the
//! definition + current statuses + a `now` timestamp, so re-running it after a
//! restart always yields the same "what to do next" — the property that makes
//! resume safe.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};

use super::{BlockedBy, Definition, OnFailure, RunState, SkipPolicy, SkipReason, Status, Step};

/// A `running` step whose heartbeat is older than its own [`Step::lease_secs`]
/// (this being the default) is treated as interrupted (its executor died
/// mid-flight) and returned to `pending`.
///
/// The default is a *crash* timeout, not a work budget, and the two are easy to
/// confuse: it is deliberately short so a dead claim frees the pipeline quickly.
/// A step that legitimately runs longer than this must say so with `timeout:`,
/// or the engine reaps a claim that is still being worked on and the work is
/// performed twice — once by the live executor and once by whoever re-claims it.
pub(super) const LEASE_TIMEOUT_SECS: i64 = 300;

/// Parse an exact `${steps.<id>.outputs.<key>}` reference.
pub(super) fn parse_output_ref(expr: &str) -> Option<(String, String)> {
    let s = expr.trim();
    let inner = s.strip_prefix("${")?.strip_suffix('}')?;
    let mut parts = inner.split('.');
    if parts.next()? != "steps" {
        return None;
    }
    let id = parts.next()?.to_string();
    if parts.next()? != "outputs" {
        return None;
    }
    let key = parts.next()?.to_string();
    if parts.next().is_some() || id.is_empty() || key.is_empty() {
        return None;
    }
    Some((id, key))
}

/// Extract every `${steps.<id>.outputs.<key>}` reference embedded in `expr`,
/// whether it is the whole value or interpolated inside a larger string. Shared
/// by validation (which reads `inputs:`/`when:`) and adoption (which needs to
/// know which declared outputs a dependent will read back).
pub(super) fn scan_output_refs(expr: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = expr.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if expr[i..].starts_with("${") {
            if let Some(end) = expr[i..].find('}') {
                let candidate = &expr[i..i + end + 1];
                if let Some((id, key)) = parse_output_ref(candidate) {
                    out.push((id, key));
                }
                i += end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Replace every embedded `${steps.id.outputs.key}` in `expr` with the string
/// form of the referenced output. Errors if any reference is unavailable.
fn interpolate_refs(expr: &str, state: &RunState) -> Result<String, String> {
    let mut out = String::new();
    let bytes = expr.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if expr[i..].starts_with("${") {
            if let Some(end) = expr[i..].find('}') {
                let candidate = &expr[i..i + end + 1];
                if let Some((id, key)) = parse_output_ref(candidate) {
                    let val = state
                        .steps
                        .get(&id)
                        .and_then(|ss| ss.outputs.get(&key).cloned())
                        .ok_or_else(|| {
                            format!("reference '{candidate}' is not available yet")
                        })?;
                    out.push_str(&value_to_string(&val));
                    i += end + 1;
                    continue;
                }
            }
        }
        // Copy one char (byte-safe: push the char boundary).
        let ch_len = utf8_char_len(bytes[i]);
        out.push_str(&expr[i..i + ch_len]);
        i += ch_len;
    }
    Ok(out)
}

fn utf8_char_len(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xFF => 4,
        _ => 1,
    }
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        // A deliberate "no value" (see F7): rendered as the empty string so an
        // embedded `when:` reference resolves to false, never to the literal
        // "null" (which a case-insensitive truthiness read would accept).
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Resolve a step's `inputs` map against recorded outputs. A whole-value
/// reference keeps the output's JSON type; embedded references interpolate into
/// a string. Errors (actionably) if any reference is unavailable.
pub(super) fn resolve_inputs(step: &Step, state: &RunState) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    for (key, expr) in &step.inputs {
        if let Some((id, out_key)) = parse_output_ref(expr) {
            let val = state
                .steps
                .get(&id)
                .and_then(|ss| ss.outputs.get(&out_key).cloned())
                .ok_or_else(|| {
                    format!(
                        "input '{key}' references ${{steps.{id}.outputs.{out_key}}} which is not available yet"
                    )
                })?;
            out.insert(key.clone(), val);
        } else if expr.contains("${steps.") {
            let s = interpolate_refs(expr, state)
                .map_err(|e| format!("input '{key}': {e}"))?;
            out.insert(key.clone(), Value::String(s));
        } else {
            out.insert(key.clone(), Value::String(expr.clone()));
        }
    }
    Ok(out)
}

/// Evaluate a step's `when` guard. Absent ⇒ true. A guard resolves its
/// references then is false when it reads empty/`false`/`0`/`no`/`unknown`
/// (case-insensitive). `null` resolves to the empty string (see
/// [`value_to_string`]); the legacy `"unknown"`/`"no"` strings are still read as
/// no-value so runs written before the typed marker are not stranded.
fn eval_when(step: &Step, state: &RunState) -> Result<bool, String> {
    let Some(expr) = &step.when else {
        return Ok(true);
    };
    let resolved = interpolate_refs(expr, state)?;
    let t = resolved.trim().to_lowercase();
    Ok(!(t.is_empty() || t == "false" || t == "0" || t == "no" || t == "unknown"))
}

/// Topological order of step ids (dependencies before dependents) via Kahn's
/// algorithm, seeded in definition order for determinism. The definition is
/// validated acyclic before this is reached; a defensive fallback appends any
/// stragglers in definition order.
pub(super) fn topo_order(def: &Definition) -> Vec<String> {
    let idset: HashSet<&str> = def.steps.iter().map(|s| s.id.as_str()).collect();
    let mut indeg: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for s in &def.steps {
        let mut d = 0usize;
        for dep in &s.depends_on {
            if dep != &s.id && idset.contains(dep.as_str()) {
                d += 1;
                dependents.entry(dep.as_str()).or_default().push(s.id.as_str());
            }
        }
        indeg.insert(s.id.as_str(), d);
    }

    let mut queue: Vec<&str> = def
        .steps
        .iter()
        .filter(|s| indeg.get(s.id.as_str()).copied().unwrap_or(0) == 0)
        .map(|s| s.id.as_str())
        .collect();
    let mut order: Vec<String> = Vec::with_capacity(def.steps.len());
    while !queue.is_empty() {
        let node = queue.remove(0);
        order.push(node.to_string());
        if let Some(children) = dependents.get(node) {
            for &ch in children {
                if let Some(c) = indeg.get_mut(ch) {
                    *c -= 1;
                    if *c == 0 {
                        queue.push(ch);
                    }
                }
            }
        }
    }
    // Defensive: include any id somehow missed (a cycle), preserving definition order.
    for s in &def.steps {
        if !order.iter().any(|o| o == &s.id) {
            order.push(s.id.clone());
        }
    }
    order
}

/// Whether a `running` step's lease is still live (heartbeat within its own
/// timeout). Callers gate on the status themselves, since a lease only means
/// anything for a step that is actually claimed.
pub(super) fn lease_live(step: &Step, heartbeat: Option<i64>, now: i64) -> bool {
    heartbeat
        .map(|hb| now - hb < step.lease_secs())
        .unwrap_or(false)
}

/// What happened to a running step whose lease expired: enough to journal one
/// `interrupted` (or, past the budget, `failed`) event in the operation that
/// persists the reap. The caller owns the write; this only says *which* steps
/// died, under whose claim, and whether the interrupt budget ran out.
#[derive(Debug)]
pub(super) struct ReapedClaim {
    pub id: String,
    pub attempt: Option<u32>,
    pub claim_id: Option<String>,
    pub holder: Option<String>,
    /// Epoch seconds of the last heartbeat that kept the claim alive.
    pub expired_at: Option<i64>,
    pub timeout_secs: i64,
    /// The interrupt budget ran out, so the step went to `failed`, not `pending`.
    pub gave_up: bool,
}

/// Reap every `running` step whose lease has expired: one executor died
/// mid-flight (or simply never reported), so its claim is given up. Each reap
/// counts against [`Step::max_interrupts`] — a crash consumes no `retry.attempts`,
/// so without this budget an executor that fails every time would loop forever.
///
/// Under budget the step returns to `pending` with a reason quoting the timeout
/// (so a reader can tell "this claim was too old" from "this step was never
/// finished"); past it the step is abandoned as `failed`, naming the last holder.
/// The claim id is kept (a re-claim overwrites it, a late report carrying the dead
/// token is then recognisable as stale); the lease and heartbeat are cleared.
pub(super) fn reap_expired(def: &Definition, state: &mut RunState, now: i64) -> Vec<ReapedClaim> {
    let mut reaped = Vec::new();
    for step in &def.steps {
        let Some(ss) = state.steps.get_mut(&step.id) else { continue };
        if ss.status != Status::Running || lease_live(step, ss.heartbeat, now) {
            continue;
        }
        let interrupts = ss.interrupts.unwrap_or(0) + 1;
        ss.interrupts = Some(interrupts);
        let timeout_secs = step.lease_secs();
        let gave_up = interrupts > step.max_interrupts();
        reaped.push(ReapedClaim {
            id: step.id.clone(),
            attempt: ss.attempt,
            claim_id: ss.claim_id.clone(),
            holder: ss.lease.clone(),
            expired_at: ss.heartbeat,
            timeout_secs,
            gave_up,
        });
        if gave_up {
            ss.status = Status::Failed;
            ss.reason = Some(format!(
                "interrupted: {} consecutive claims reaped without a report (max_interrupts={}); \
                 the last holder was '{}'. Raise this step's `timeout:` if the work legitimately \
                 takes longer, or workflow_intervene(op=retry) to try again",
                interrupts,
                step.max_interrupts(),
                ss.lease.as_deref().unwrap_or("unknown"),
            ));
        } else {
            ss.status = Status::Pending;
            ss.reason = Some(format!(
                "interrupted: lease expired after {}s without a heartbeat (raise this step's \
                 `timeout:` if the work legitimately takes longer)",
                timeout_secs,
            ));
        }
        ss.lease = None;
        ss.heartbeat = None;
    }
    reaped
}

/// Recompute all derived statuses in place. Recorded decisions and live claims
/// are preserved; everything else is re-derived from the dependency graph:
/// - `completed` and `failed` are always preserved (a terminal judgement);
/// - a `running` claim with a live lease is preserved (and an expired one reaped
///   by [`reap_expired`] first, above);
/// - an **external** `blocked` (`blocked_by = External`) is preserved — the graph
///   must never reopen a step a caller deliberately parked;
/// - a `skipped` step is preserved only when it was a recorded decision
///   (`skip_reason` absent or `Manual`); a `when_false`/`propagated` skip is
///   re-derived, so an upstream change can bring it back.
///
/// A graph-derived block sets `blocked_by = Dependency`; when such a step becomes
/// ready the flag is cleared. Reaps expired leases and returns what it reaped so
/// the *mutating* caller can journal one `interrupted`/`failed` event each; a
/// read-only caller (status/next) reports them without persisting. Pure given
/// `now` — time is an input, never read from the clock here.
pub(super) fn recompute(def: &Definition, state: &mut RunState, now: i64) -> Vec<ReapedClaim> {
    // Ensure every defined step has a state entry.
    for step in &def.steps {
        state.steps.entry(step.id.clone()).or_default();
    }

    // Reap interrupted claims before re-deriving, so a dead claim neither blocks
    // its dependents as `running` nor is counted live.
    let reaped = reap_expired(def, state, now);

    let by_id: HashMap<&str, &Step> = def.steps.iter().map(|s| (s.id.as_str(), s)).collect();

    // Is the run halted by a stop-policy failure?
    let halted_by = def.steps.iter().find(|s| {
        state.steps.get(&s.id).map(|ss| ss.status == Status::Failed).unwrap_or(false)
            && s.on_failure() == OnFailure::Stop
    }).map(|s| s.id.clone());

    for id in topo_order(def) {
        let Some(step) = by_id.get(id.as_str()) else {
            continue;
        };
        let cur = state.steps.get(&id).cloned().unwrap_or_default();
        // Preserve terminal statuses and live running leases.
        if matches!(cur.status, Status::Completed | Status::Failed | Status::Running) {
            continue;
        }
        // A caller-blocked step is a judgement, not a derivation — leave it.
        if cur.status == Status::Blocked && cur.blocked_by == Some(BlockedBy::External) {
            continue;
        }
        // A recorded skip (manual, or a legacy one with no reason) is terminal.
        if cur.status == Status::Skipped
            && !matches!(cur.skip_reason, Some(SkipReason::WhenFalse) | Some(SkipReason::Propagated))
        {
            continue;
        }
        let d = derive_status(step, state, &by_id, halted_by.as_deref());
        if let Some(ss) = state.steps.get_mut(&id) {
            ss.status = d.status;
            ss.reason = d.reason;
            ss.blocked_by = d.blocked_by;
            ss.skip_reason = d.skip_reason;
        }
    }

    reaped
}

/// What [`derive_status`] concluded about one step, including the two
/// provenance tags the graph owns (`blocked_by`, `skip_reason`).
pub(super) struct Derived {
    pub status: Status,
    pub reason: Option<String>,
    pub blocked_by: Option<BlockedBy>,
    pub skip_reason: Option<SkipReason>,
}

impl Derived {
    fn plain(status: Status, reason: Option<String>) -> Self {
        Self { status, reason, blocked_by: None, skip_reason: None }
    }
    fn blocked(reason: String) -> Self {
        Self {
            status: Status::Blocked,
            reason: Some(reason),
            blocked_by: Some(BlockedBy::Dependency),
            skip_reason: None,
        }
    }
    fn skipped(reason: &str, kind: SkipReason) -> Self {
        Self {
            status: Status::Skipped,
            reason: Some(reason.to_string()),
            blocked_by: None,
            skip_reason: Some(kind),
        }
    }
}

/// Derive the status of a single non-terminal step from its dependencies.
fn derive_status(
    step: &Step,
    state: &RunState,
    by_id: &HashMap<&str, &Step>,
    halted_by: Option<&str>,
) -> Derived {
    if let Some(h) = halted_by {
        return Derived::blocked(format!(
            "run halted: step '{h}' failed with on_failure=stop; retry or reset '{h}' to resume"
        ));
    }

    let mut block_reason: Option<String> = None;
    let mut propagate_skip = false;
    let mut unsat = false;
    let skip_policy = step.skip_policy();

    for dep in &step.depends_on {
        let dep_status = state.steps.get(dep).map(|ss| ss.status).unwrap_or(Status::Pending);
        match dep_status {
            Status::Completed => {}
            // A skip satisfies, blocks, or propagates per the dependent's policy.
            Status::Skipped => match skip_policy {
                SkipPolicy::Satisfy => {}
                SkipPolicy::Block => {
                    if block_reason.is_none() {
                        block_reason = Some(format!("dependency '{dep}' was skipped"));
                    }
                }
                SkipPolicy::Skip => propagate_skip = true,
            },
            Status::Failed => {
                let dep_policy = by_id
                    .get(dep.as_str())
                    .map(|s| s.on_failure())
                    .unwrap_or(OnFailure::Stop);
                if dep_policy == OnFailure::SkipDependents {
                    propagate_skip = true;
                } else if block_reason.is_none() {
                    block_reason = Some(format!("dependency '{dep}' failed"));
                }
            }
            Status::Blocked => {
                if block_reason.is_none() {
                    block_reason = Some(format!("dependency '{dep}' is blocked"));
                }
            }
            _ => unsat = true, // Pending | Ready | Running
        }
    }

    if propagate_skip {
        return Derived::skipped("an upstream skip propagated", SkipReason::Propagated);
    }
    if let Some(reason) = block_reason {
        return Derived::blocked(reason);
    }
    if unsat {
        return Derived::plain(Status::Pending, None);
    }

    // All dependencies satisfied — evaluate the guard and resolve inputs.
    match eval_when(step, state) {
        Err(e) => return Derived::blocked(format!("cannot evaluate 'when': {e}")),
        Ok(false) => {
            return Derived::skipped("'when' condition is false", SkipReason::WhenFalse)
        }
        Ok(true) => {}
    }
    if let Err(e) = resolve_inputs(step, state) {
        return Derived::blocked(e);
    }
    Derived::plain(Status::Ready, None)
}

/// Enforce the invariant "a `completed` step's dependencies are satisfied": any
/// `completed` step whose dependency is no longer satisfied is reopened to
/// `ready` (keeping its `outputs`, so a `when:` guard still resolves), and the
/// cascade continues to its transitive dependents in one topological pass.
///
/// This is F3: demotion (`verify apply`, a failed `record`, or `intervene
/// reset/retry`) may not leave a green node sitting on top of a stale upstream —
/// that reproduces exactly the false-`completed` the verification seam exists to
/// catch. Returns the reopened `(step id, unsatisfied dependency id)` pairs so
/// the caller can journal one `invalidated` event each with `cascaded_from`.
pub(super) fn reopen_stale_completed(def: &Definition, state: &mut RunState) -> Vec<(String, String)> {
    let by_id: HashMap<&str, &Step> = def.steps.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut reopened: Vec<(String, String)> = Vec::new();
    for id in topo_order(def) {
        let Some(step) = by_id.get(id.as_str()) else { continue };
        let cur = state.steps.get(&id).map(|ss| ss.status).unwrap_or_default();
        if cur != Status::Completed {
            continue;
        }
        // Find the first dependency that is no longer satisfied under this step's
        // skip policy. Because we walk in topo order, an unsatisfied dep has
        // already been reopened this pass (or was never completed), so its new
        // status is what we read.
        let unsatisfied = step.depends_on.iter().find(|dep| {
            let ds = state.steps.get(dep.as_str()).map(|ss| ss.status).unwrap_or(Status::Pending);
            match ds {
                Status::Completed => false,
                Status::Skipped => step.skip_policy() != SkipPolicy::Satisfy,
                _ => true,
            }
        });
        if let Some(dep) = unsatisfied {
            if let Some(ss) = state.steps.get_mut(&id) {
                ss.status = Status::Ready;
                ss.finished_at = None;
                ss.lease = None;
                ss.heartbeat = None;
                ss.blocked_by = None;
                ss.skip_reason = None;
                ss.reason = Some(format!("reopened: dependency '{dep}' is no longer satisfied"));
            }
            reopened.push((id.clone(), dep.clone()));
        }
    }
    reopened
}

/// Ids of steps that are `ready` and unclaimed, in topological order.
pub(super) fn ready_step_ids(def: &Definition, state: &RunState) -> Vec<String> {
    topo_order(def)
        .into_iter()
        .filter(|id| state.steps.get(id).map(|ss| ss.status == Status::Ready).unwrap_or(false))
        .collect()
}

/// Classify the overall run: `in_progress`, `completed`, `completed_with_failures`,
/// `failed` or `blocked`.
///
/// The distinction that matters (N3) is a run that finished with a `continue`-policy
/// step failed: it has no work left it can still do, yet is not `completed` either —
/// reporting it `blocked` (the old fallthrough) reads to a waiting agent like "somebody
/// must intervene to unstick it", when in fact the graph is done and just has a red
/// node. Precedence: a `stop` failure halts everything (`failed`); live work anywhere
/// (`in_progress`); genuinely all-done (`completed`); finished but with a non-stop
/// failure and nothing still waiting (`completed_with_failures`); otherwise stuck
/// behind a failed/blocked/pending step (`blocked`).
pub(super) fn classify_run(def: &Definition, state: &RunState) -> &'static str {
    let mut any_running = false;
    let mut any_ready = false;
    let mut any_blocked = false;
    let mut any_failed = false;
    let mut any_pending = false;
    let mut all_done = true;

    for step in &def.steps {
        let status = state.steps.get(&step.id).map(|ss| ss.status).unwrap_or_default();
        match status {
            Status::Running => {
                any_running = true;
                all_done = false;
            }
            Status::Ready => {
                any_ready = true;
                all_done = false;
            }
            Status::Pending => {
                any_pending = true;
                all_done = false;
            }
            Status::Blocked => {
                any_blocked = true;
                all_done = false;
            }
            Status::Failed => {
                any_failed = true;
                all_done = false;
            }
            Status::Completed | Status::Skipped => {}
        }
    }

    let stop_failure = def.steps.iter().any(|s| {
        state.steps.get(&s.id).map(|ss| ss.status == Status::Failed).unwrap_or(false)
            && s.on_failure() == OnFailure::Stop
    });

    if stop_failure {
        "failed"
    } else if any_running || any_ready {
        "in_progress"
    } else if all_done {
        "completed"
    } else if any_failed && !any_pending && !any_blocked {
        // Every non-failed step is done; the failures were `continue`-policy, so
        // the run reached its end with a red node rather than stalling.
        "completed_with_failures"
    } else {
        "blocked"
    }
}
