//! Pre-run structural validation of a workflow definition.
//!
//! A definition that fails validation is rejected outright; the run never
//! starts. Checks are pure and side-effect free.
//!
//! Deviation from the design doc: the "every `action` resolves to a registered
//! executor" check is intentionally skipped. This build is agent-in-the-loop —
//! there is no executor registry — so actions are symbolic names the agent
//! resolves at run time. Only `action` non-emptiness is enforced here.
//!
//! The `verify:` id rule is enforced, but the vocabulary is *passed in*: this
//! module only knows the ids must name a check the caller can evaluate. A typo
//! there would silently disable a gate, so it fails like a bad `depends_on` does.
//! The same rule covers `adopt:`, where a typo would silently disable an
//! adoption — a quieter bug, but still a graph that lies about what was checked.
//! Each id is tagged with its vacuity [`CheckKind`], so the framework can also
//! refuse an `adopt:` list that is made only of collection scans (which read
//! clean on an empty subject) — see the F10 rule.
//!
//! Structural rules added for correctness (F9): a `${steps.X.outputs.Y}` read in
//! `inputs:` or `when:` may only reference a transitive ancestor `X` (a sibling
//! or a descendant would race on a value that may never exist), `timeout: 0` is
//! rejected (it would reap a claim the instant it is taken), and `retry.backoff`
//! must read as a lenient `<n><s|m|h>` duration or be empty.

use std::collections::{HashMap, HashSet};

use super::{CheckKind, Definition};

/// Validate a definition. Returns an actionable error message on the first
/// problem found.
///
/// `check_vocab` is the caller's evaluation vocabulary — every id a `verify:` or
/// `adopt:` block may name, tagged with its [`CheckKind`]. An empty slice means
/// the caller evaluates no checks at all, which leaves the id-existence and
/// vacuity rules off rather than rejecting ids the caller simply does not know.
pub(super) fn validate(def: &Definition, check_vocab: &[(&str, CheckKind)]) -> Result<(), String> {
    let checks: HashMap<&str, CheckKind> = check_vocab.iter().map(|(id, k)| (*id, *k)).collect();
    if def.name.trim().is_empty() {
        return Err("definition is missing a non-empty 'name'".into());
    }
    if def.steps.is_empty() {
        return Err("definition has no steps".into());
    }

    // Unique ids + non-empty actions.
    let mut ids: HashSet<&str> = HashSet::new();
    for step in &def.steps {
        if step.id.trim().is_empty() {
            return Err("a step has an empty 'id'".into());
        }
        if !ids.insert(step.id.as_str()) {
            return Err(format!("duplicate step id '{}'", step.id));
        }
        if step.action.trim().is_empty() {
            return Err(format!("step '{}' has an empty 'action'", step.id));
        }
    }

    // Every depends_on reference points at an existing step.
    for step in &def.steps {
        for dep in &step.depends_on {
            if !ids.contains(dep.as_str()) {
                return Err(format!(
                    "step '{}' depends_on unknown step '{dep}'",
                    step.id
                ));
            }
            if dep == &step.id {
                return Err(format!("step '{}' depends on itself", step.id));
            }
        }
    }

    // Every ${steps.X.outputs.Y} input reference names an existing step that
    // declares Y among its outputs.
    let by_id: HashMap<&str, &super::Step> =
        def.steps.iter().map(|s| (s.id.as_str(), s)).collect();
    for step in &def.steps {
        for (key, expr) in &step.inputs {
            if let Some((src_id, out_key)) = super::engine::parse_output_ref(expr) {
                let Some(src) = by_id.get(src_id.as_str()) else {
                    return Err(format!(
                        "step '{}' input '{key}' references unknown step '{src_id}'",
                        step.id
                    ));
                };
                if !src.outputs.iter().any(|o| o == &out_key) {
                    return Err(format!(
                        "step '{}' input '{key}' references output '{out_key}' not declared by step '{src_id}'",
                        step.id
                    ));
                }
            }
        }
    }

    // Acyclic via iterative DFS colouring.
    detect_cycle(def)?;

    // F9 — a step may only read the outputs of its transitive ancestors. A
    // reference to a step that does not sit upstream (a sibling, or one reachable
    // only through this step) races on a value that may never be produced, so it
    // is refused here rather than left to strand a `when:` guard at run time.
    // This is the only check that reads `when:` at all — its references were
    // otherwise validated nowhere.
    let ancestors = transitive_ancestors(def);
    for step in &def.steps {
        let anc = ancestors.get(step.id.as_str()).cloned().unwrap_or_default();
        let mut exprs: Vec<(&str, &str)> = step
            .inputs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        if let Some(w) = &step.when {
            exprs.push(("when", w.as_str()));
        }
        for (field, expr) in exprs {
            for (src_id, out_key) in super::engine::scan_output_refs(expr) {
                if src_id == step.id {
                    return Err(format!(
                        "step '{}' {field} references its own output '{out_key}'",
                        step.id
                    ));
                }
                if !anc.contains(src_id.as_str()) {
                    return Err(format!(
                        "step '{}' {field} references output '{out_key}' of step '{src_id}', which is \
                         not a transitive ancestor of '{}' — only upstream steps may be read",
                        step.id, step.id
                    ));
                }
            }
        }
    }

    // F9 — `timeout: 0` would reap a claim the instant it is taken; a retry
    // backoff must parse as a lenient duration (see [`valid_backoff`]).
    for step in &def.steps {
        if step.timeout == Some(0) {
            return Err(format!(
                "step '{}' has timeout: 0; use a positive number of seconds (or omit it for the default lease)",
                step.id
            ));
        }
        if let Some(b) = step.retry.as_ref().and_then(|r| r.backoff.as_deref()) {
            if !valid_backoff(b) {
                return Err(format!(
                    "step '{}' retry.backoff '{b}' is not a valid duration; use e.g. \"30s\", \"5m\", \"1h\", or leave it empty",
                    step.id
                ));
            }
        }
    }

    // Every check id named under `verify:` or `adopt:` must be one the caller can
    // evaluate.
    if !checks.is_empty() {
        for step in &def.steps {
            let bound: [(&str, &[String]); 3] = [
                ("verify.blocks", step.verify.as_ref().map(|v| v.blocks.as_slice()).unwrap_or(&[])),
                ("verify.proves", step.verify.as_ref().map(|v| v.proves.as_slice()).unwrap_or(&[])),
                ("adopt", step.adopt.as_deref().unwrap_or(&[])),
            ];
            for (field, ids) in bound {
                for id in ids {
                    if !checks.contains_key(id.as_str()) {
                        return Err(format!(
                            "step '{}' {field} names unknown check '{id}'; this dataset kind evaluates: {}",
                            step.id,
                            {
                                let mut v: Vec<&str> = checks.keys().copied().collect();
                                v.sort();
                                v.join(", ")
                            },
                        ));
                    }
                }
            }
        }

        // F10 — an `adopt:` list made only of `per_subject` collection scans reads
        // clean when its subject is empty, which proves nothing ("no media, so no
        // media is missing subtitles"). It must be paired with at least one check
        // that reports an offender on an empty subject (`subject_present` or
        // `file_exists`), or adopting on it would certify work nobody did. Note the
        // guard may be `file_exists` too: a `sync_media`-style list guarded by a
        // `database_missing` (`file_exists`) check is faithful, not vacuous.
        for step in &def.steps {
            let adopt_ids = step.adopt.as_deref().unwrap_or(&[]);
            if adopt_ids.is_empty() {
                continue;
            }
            let kind = |id: &str| checks.get(id).copied();
            let any_per_subject = adopt_ids.iter().any(|id| kind(id) == Some(CheckKind::PerSubject));
            let any_guard = adopt_ids.iter().any(|id| {
                matches!(kind(id), Some(CheckKind::SubjectPresent) | Some(CheckKind::FileExists))
            });
            if any_per_subject && !any_guard {
                return Err(format!(
                    "step '{}' adopt: [{}] counts offenders over a subject that may be empty; pair it with a \
                     check that reports an offender when the subject is absent (subject_present or file_exists), \
                     or a clean result would certify work nobody did",
                    step.id,
                    adopt_ids.iter().map(String::as_str).collect::<Vec<_>>().join(", "),
                ));
            }
        }
    }

    Ok(())
}

/// For each step, the set of ids reachable by following `depends_on` transitively
/// (its ancestors — the steps whose outputs it may legitimately read). Called
/// only after [`detect_cycle`] has cleared the graph, so the walk terminates.
fn transitive_ancestors(def: &Definition) -> HashMap<String, HashSet<String>> {
    let deps: HashMap<&str, Vec<&str>> = def
        .steps
        .iter()
        .map(|s| (s.id.as_str(), s.depends_on.iter().map(|d| d.as_str()).collect()))
        .collect();
    let mut out: HashMap<String, HashSet<String>> = HashMap::new();
    for s in &def.steps {
        let mut seen: HashSet<String> = HashSet::new();
        let mut stack: Vec<&str> = deps.get(s.id.as_str()).cloned().unwrap_or_default();
        while let Some(cur) = stack.pop() {
            if seen.insert(cur.to_string()) {
                if let Some(next) = deps.get(cur) {
                    stack.extend(next.iter().copied());
                }
            }
        }
        out.insert(s.id.clone(), seen);
    }
    out
}

/// A lenient retry-backoff grammar: empty, or a positive integer run of digits
/// followed by exactly one unit of `s`/`m`/`h` (`"30s"`, `"5m"`, `"1h"`). A stray
/// typo (`"30x"`) would otherwise read to an agent as "no delay".
fn valid_backoff(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return true;
    }
    let mut it = t.chars().peekable();
    let mut digits = 0usize;
    while it.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        it.next();
        digits += 1;
    }
    if digits == 0 {
        return false;
    }
    matches!((it.next(), it.next()), (Some(u), None) if matches!(u, 's' | 'm' | 'h'))
}

/// Depth-first cycle detection over the `depends_on` graph.
fn detect_cycle(def: &Definition) -> Result<(), String> {
    // colour: 0 = unvisited, 1 = on stack, 2 = done
    let mut colour: HashMap<&str, u8> = def.steps.iter().map(|s| (s.id.as_str(), 0u8)).collect();
    let deps: HashMap<&str, Vec<&str>> = def
        .steps
        .iter()
        .map(|s| (s.id.as_str(), s.depends_on.iter().map(|d| d.as_str()).collect()))
        .collect();

    for start in deps.keys() {
        if colour.get(*start).copied().unwrap_or(0) != 0 {
            continue;
        }
        // Stack of (node, next-dependency-index). Tuples are Copy, so we index
        // and copy out rather than holding a `last_mut()` borrow across the
        // push/pop below.
        let mut stack: Vec<(&str, usize)> = vec![(*start, 0)];
        colour.insert(*start, 1);
        while !stack.is_empty() {
            let top = stack.len() - 1;
            let (node, idx) = stack[top];
            let node_deps = deps.get(node).cloned().unwrap_or_default();
            if idx < node_deps.len() {
                let dep = node_deps[idx];
                stack[top].1 += 1;
                match colour.get(dep).copied().unwrap_or(0) {
                    1 => {
                        return Err(format!(
                            "dependency cycle detected involving step '{dep}'"
                        ))
                    }
                    0 => {
                        colour.insert(dep, 1);
                        stack.push((dep, 0));
                    }
                    _ => {}
                }
            } else {
                colour.insert(node, 2);
                stack.pop();
            }
        }
    }
    Ok(())
}
