//! Engine core: readiness derivation, run classification, data-flow resolution
//! and lease/interruption recovery. All logic here is a pure function of the
//! definition + current statuses + a `now` timestamp, so re-running it after a
//! restart always yields the same "what to do next" — the property that makes
//! resume safe.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};

use super::{Definition, OnFailure, RunState, Status, Step};

/// A `running` step whose heartbeat is older than this is treated as
/// interrupted (its executor died mid-flight) and returned to `pending`.
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
/// references then is false when empty/`false`/`0`/`no` (case-insensitive).
fn eval_when(step: &Step, state: &RunState) -> Result<bool, String> {
    let Some(expr) = &step.when else {
        return Ok(true);
    };
    let resolved = interpolate_refs(expr, state)?;
    let t = resolved.trim().to_lowercase();
    Ok(!(t.is_empty() || t == "false" || t == "0" || t == "no"))
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

/// Whether a `running` step's lease is still live (heartbeat within timeout).
pub(super) fn lease_live(status: Status, heartbeat: Option<i64>, now: i64) -> bool {
    status == Status::Running
        && heartbeat.map(|hb| now - hb < LEASE_TIMEOUT_SECS).unwrap_or(false)
}

/// Recompute all derived statuses in place. Recorded statuses (`completed`,
/// `failed`, `skipped`) and live `running` leases are preserved; everything else
/// is re-derived from the dependency graph. Also reaps expired leases.
pub(super) fn recompute(def: &Definition, state: &mut RunState, now: i64) {
    // Ensure every defined step has a state entry.
    for step in &def.steps {
        state.steps.entry(step.id.clone()).or_default();
    }

    // Reap interrupted steps: running with an expired lease → pending.
    for step in &def.steps {
        if let Some(ss) = state.steps.get_mut(&step.id) {
            if ss.status == Status::Running && !lease_live(ss.status, ss.heartbeat, now) {
                ss.status = Status::Pending;
                ss.lease = None;
                ss.reason = Some("interrupted: lease expired".into());
            }
        }
    }

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
        let cur = state.steps.get(&id).map(|ss| ss.status).unwrap_or_default();
        // Preserve terminal statuses and live running leases.
        if matches!(cur, Status::Completed | Status::Failed | Status::Skipped) {
            continue;
        }
        if cur == Status::Running {
            continue;
        }
        let (status, reason) = derive_status(step, state, &by_id, halted_by.as_deref());
        if let Some(ss) = state.steps.get_mut(&id) {
            ss.status = status;
            ss.reason = reason;
        }
    }
}

/// Derive the status of a single non-terminal step from its dependencies.
fn derive_status(
    step: &Step,
    state: &RunState,
    by_id: &HashMap<&str, &Step>,
    halted_by: Option<&str>,
) -> (Status, Option<String>) {
    if let Some(h) = halted_by {
        return (
            Status::Blocked,
            Some(format!(
                "run halted: step '{h}' failed with on_failure=stop; retry or reset '{h}' to resume"
            )),
        );
    }

    let mut block_reason: Option<String> = None;
    let mut skip = false;
    let mut unsat = false;

    for dep in &step.depends_on {
        let dep_status = state.steps.get(dep).map(|ss| ss.status).unwrap_or(Status::Pending);
        match dep_status {
            // A skip satisfies a dependency by default (design: "skipped and the
            // dependent's policy allows a skip to satisfy it").
            Status::Completed | Status::Skipped => {}
            Status::Failed => {
                let dep_policy = by_id
                    .get(dep.as_str())
                    .map(|s| s.on_failure())
                    .unwrap_or(OnFailure::Stop);
                if dep_policy == OnFailure::SkipDependents {
                    skip = true;
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

    if skip {
        return (
            Status::Skipped,
            Some("an upstream failure used on_failure=skip_dependents".into()),
        );
    }
    if let Some(reason) = block_reason {
        return (Status::Blocked, Some(reason));
    }
    if unsat {
        return (Status::Pending, None);
    }

    // All dependencies satisfied — evaluate the guard and resolve inputs.
    match eval_when(step, state) {
        Err(e) => return (Status::Blocked, Some(format!("cannot evaluate 'when': {e}"))),
        Ok(false) => return (Status::Skipped, Some("'when' condition is false".into())),
        Ok(true) => {}
    }
    if let Err(e) = resolve_inputs(step, state) {
        return (Status::Blocked, Some(e));
    }
    (Status::Ready, None)
}

/// Ids of steps that are `ready` and unclaimed, in topological order.
pub(super) fn ready_step_ids(def: &Definition, state: &RunState) -> Vec<String> {
    topo_order(def)
        .into_iter()
        .filter(|id| state.steps.get(id).map(|ss| ss.status == Status::Ready).unwrap_or(false))
        .collect()
}

/// Classify the overall run: `in_progress`, `completed`, `failed` or `blocked`.
pub(super) fn classify_run(def: &Definition, state: &RunState) -> &'static str {
    let mut any_running = false;
    let mut any_ready = false;
    let mut any_blocked = false;
    let mut any_failed = false;
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
            Status::Pending => all_done = false,
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
    } else if any_failed || any_blocked {
        "blocked"
    } else {
        "completed"
    }
}
