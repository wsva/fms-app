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

use std::collections::{HashMap, HashSet};

use super::Definition;

/// Validate a definition. Returns an actionable error message on the first
/// problem found.
///
/// `check_ids` is the caller's evaluation vocabulary. An empty slice means the
/// caller evaluates no checks at all, which leaves `verify:` blocks unchecked
/// rather than rejecting ids the caller simply does not know.
pub(super) fn validate(def: &Definition, check_ids: &[&str]) -> Result<(), String> {
    let checks: HashSet<&str> = check_ids.iter().copied().collect();
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
                    if !checks.contains(id.as_str()) {
                        return Err(format!(
                            "step '{}' {field} names unknown check '{id}'; this dataset kind evaluates: {}",
                            step.id,
                            {
                                let mut v: Vec<&str> = checks.iter().copied().collect();
                                v.sort();
                                v.join(", ")
                            },
                        ));
                    }
                }
            }
        }
    }

    Ok(())
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
