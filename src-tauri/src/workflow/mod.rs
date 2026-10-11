//! Persistent workflow state machine — a generic, file-based framework for
//! describing and running multi-step processes that an AI agent can drive and
//! resume after any interruption without relying on conversation history.
//!
//! See `docs/design/workflow/core.md` for the full specification; a pipeline's
//! own document (e.g. `docs/design/workflow/dictation.md`) describes the steps,
//! their artifacts and how the world around them is verified.
//!
//! # Layout
//!
//! The domain-free framework lives in [`core`]: it knows about *steps*,
//! *dependencies* and *statuses*, never what a step actually does. Actions are
//! symbolic names; in this agent-in-the-loop build every step is handed back to
//! the agent (via the `workflow_*` MCP tools) rather than bound to an executor.
//!
//! This group root is the *binding* layer: it turns a dataset identity into a
//! run location and hands the domain-free [`core`] engine an explicit base
//! directory. A dataset-scoped run lives inside its own dataset at
//! `<dataset>/workflows/<run_id>` (self-contained, travels with sync/export);
//! dataset-less editor "jobs" live in the workspace `<workspace>/workflows`.
//! Binding concrete actions (download, transcribe, …) to the generic engine is
//! still future work; beyond the storage-location seam this root owns
//! [`verify`], the check vocabulary that a definition's `verify:` bindings are
//! scored against and that its `adopt:` list is allowed to inherit progress from.
//!
//! Desktop-only: the engine is reached through the desktop-gated MCP server
//! (`mcp/workflow.rs`) and, for the Workflow page, its `workflow_*` Tauri
//! command twins ([`commands`]) — both call the same [`core`] fns via these
//! wrappers.

pub(crate) mod core;
/// Tauri command twins of the `workflow_*` MCP tools, so the Workflow page can
/// drive the same engine in-app. Desktop-only (registered only in the desktop
/// handler list), matching the engine + MCP surface.
pub(crate) mod commands;
/// Built-in workflow templates (e.g. the Dataset Dictation pipeline) baked into
/// the binary via `include_str!` — the single source of truth for what the
/// Workflow page and the `workflow_builtin_templates` MCP tool seed a new run
/// with, replacing the former duplicate `docs/ai/workflow/` + `seed.ts` copies.
pub(crate) mod templates;
/// The check vocabulary behind a definition's `verify:` block: which ids exist,
/// who evaluates them and what they imply for a step's status.
pub(crate) mod verify;

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::datasets::DatasetType;
use crate::settings::SettingsState;

/// Sub-directory (inside a dataset, or under a workspace) that holds run dirs.
const RUNS_SUBDIR: &str = "workflows";
/// What makes a directory a run rather than a stray folder of the same name.
const DEFINITION_FILE: &str = "workflow.yaml";
/// The placeholder a template writes in a step's `params` to mean *"the dataset
/// this run is filed under"*. [`core`] copies `params` into the agent surface
/// verbatim — it knows steps, dependencies and statuses, never datasets — so
/// [`bind_subject`] in this layer is what resolves it.
const SUBJECT_REF: &str = "${run.dataset_uuid}";

/// The run id a dataset's own page opens and keeps progress in: `dictation-<uuid>`.
///
/// Nothing enforces it — an agent that called `workflow_create` with an id of its
/// own simply added another run, and runs are independent records. It is a
/// convention worth naming in the backend all the same, because a read-only report
/// that has to speak about *the* run of a dataset (see
/// [`crate::datasets::report`]) must know which one the UI is driving, and the
/// frontend writes the same string (`runIdFor` in
/// `src/components/tools/WorkflowPage.tsx`). Only dictation has one today.
pub(crate) fn canonical_run_id(dataset_uuid: &str) -> String {
    format!("{}-{}", DatasetType::Dictation.as_str(), dataset_uuid)
}

/// Resolve the run *base directory* for a call that only needs a **scope** —
/// creating a run or listing runs, where no existing run is being named.
///
/// - `dataset_uuid` present → the dataset's own `<dataset>/workflows` dir, so a
///   dataset-scoped run travels with the dataset (self-contained, sync-friendly).
/// - absent → the workspace jobs dir `<workspace>/workflows`, for dataset-less
///   editor "jobs" (reusable pipeline templates).
///
/// The domain-free [`core`] engine only ever sees this base path; it never
/// resolves dataset identity itself.
fn run_base(settings: &SettingsState, dataset_uuid: Option<&str>) -> Result<PathBuf, String> {
    match dataset_uuid.map(str::trim).filter(|s| !s.is_empty()) {
        Some(uuid) => {
            let dir = crate::datasets::find_dataset_dir(settings, uuid)?;
            Ok(dir.join(RUNS_SUBDIR))
        }
        None => Ok(settings.workspace_subdir(RUNS_SUBDIR, "")),
    }
}

/// Every dataset's `workflows` dir that holds a run named `run_id`.
///
/// Cheap in practice: a handful of roots times a handful of dataset directories,
/// each one `stat`. The caller has already established `run_id` is a single
/// directory name ([`run_base_for`]), so this only joins it.
fn dataset_run_bases(settings: &SettingsState, run_id: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for ty in DatasetType::ALL {
        for root in crate::datasets::dataset_roots(settings, ty) {
            let Ok(entries) = fs::read_dir(&root) else {
                continue;
            };
            for entry in entries.flatten() {
                let base = entry.path().join(RUNS_SUBDIR);
                if base.join(run_id).join(DEFINITION_FILE).is_file() {
                    found.push(base);
                }
            }
        }
    }
    found
}

/// Resolve the base directory of an **existing** run, which may be named with or
/// without `dataset_uuid`.
///
/// The scope is not optional information — a dataset-scoped run lives inside its
/// dataset and a job run inside the workspace — but requiring an agent to repeat
/// the uuid on every one of the dozen calls in a pipeline was a real failure
/// mode: a `workflow_record` that forgot it looked for the run in the workspace
/// jobs dir, reported "not found", and left the step `running` forever. So the
/// run id is allowed to identify the run: search the workspace jobs dir first
/// (that is where a job is), then every dataset's `workflows/`.
///
/// An unambiguous hit wins. Several datasets holding the same run id are refused
/// rather than guessed at, since writing progress to the wrong run is exactly the
/// silent corruption the lease mechanism exists to prevent. Nothing found falls
/// through to the scope of [`run_base`], where the engine's own "not found"
/// reports the path it looked at.
fn run_base_for(
    settings: &SettingsState,
    dataset_uuid: Option<&str>,
    run_id: &str,
) -> Result<PathBuf, String> {
    let run_id = run_id.trim();
    // An explicit scope is the caller's answer, and an id that could not name a
    // single directory is left to the engine's own validation rather than joined
    // onto a path here.
    let explicit = dataset_uuid.map(str::trim).filter(|s| !s.is_empty()).is_some();
    let searchable = !run_id.is_empty()
        && !run_id.contains('/')
        && !run_id.contains('\\')
        && !run_id.contains("..");
    if explicit || !searchable {
        return run_base(settings, dataset_uuid);
    }
    let jobs = settings.workspace_subdir(RUNS_SUBDIR, "");
    if jobs.join(run_id).join(DEFINITION_FILE).is_file() {
        return Ok(jobs);
    }
    match dataset_run_bases(settings, run_id).as_slice() {
        [] => Ok(jobs),
        [only] => Ok(only.clone()),
        many => {
            let owners: Vec<String> = many
                .iter()
                .map(|base| {
                    base.parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| base.display().to_string())
                })
                .collect();
            Err(format!(
                "run '{run_id}' exists in {} datasets ({}); pass dataset_uuid to say which one this \
                 call is about, since writing progress to the wrong run would corrupt both",
                many.len(),
                owners.join(", "),
            ))
        }
    }
}

/// Where an existing run lives, and what it is about.
struct RunScope {
    base: PathBuf,
    /// The dataset a dataset-scoped run is filed under; `None` for a workspace
    /// job, which has no subject.
    dataset_uuid: Option<String>,
}

/// Locate an existing run and recover the dataset it belongs to.
///
/// The uuid is not decoration. A step whose action is `dataset_generate_subtitles`
/// and whose `params` are `{}` is an instruction the agent cannot carry out: the
/// subject of the work appears nowhere in the call, and only in the path the run
/// happens to be filed under. So the binding layer reads it back out of that
/// dataset's `info.json` and hands it to the agent surface ([`bind_subject`]).
///
/// An explicit `dataset_uuid` is believed as given — resolving it to a directory
/// already proved the dataset exists, and re-reading its `info.json` could only
/// disagree with the caller.
fn run_scope(
    settings: &SettingsState,
    dataset_uuid: Option<&str>,
    run_id: &str,
) -> Result<RunScope, String> {
    let explicit = dataset_uuid.map(str::trim).filter(|s| !s.is_empty());
    let base = run_base_for(settings, explicit, run_id)?;
    // The dataset directory is the parent of its `workflows/`; a workspace jobs
    // dir has the workspace there instead, and a workspace holds no `info.json`.
    let dataset_uuid = match explicit {
        Some(uuid) => Some(uuid.to_string()),
        None => base
            .parent()
            .and_then(crate::datasets::info::read_info_opt)
            .map(|info| info.uuid),
    };
    Ok(RunScope { base, dataset_uuid })
}

/// Resolve [`SUBJECT_REF`] inside a response, and name the subject at its root
/// as well — a run opened from a template that never wrote the placeholder still
/// gets told what its actions operate on.
fn bind_subject(mut value: Value, dataset_uuid: Option<&str>) -> Value {
    let Some(uuid) = dataset_uuid else {
        return value;
    };
    substitute_subject(&mut value, &uuid);
    if let Value::Object(map) = &mut value {
        if !map.contains_key("dataset_uuid") {
            map.insert("dataset_uuid".into(), json!(uuid));
        }
    }
    value
}

/// Depth-first, exact-match only: a placeholder that is part of a longer string
/// is the template author's own text, not ours to rewrite.
fn substitute_subject(node: &mut Value, uuid: &str) {
    match node {
        Value::String(text) if text == SUBJECT_REF => *text = uuid.to_string(),
        Value::Array(items) => items.iter_mut().for_each(|item| substitute_subject(item, uuid)),
        Value::Object(map) => map.values_mut().for_each(|item| substitute_subject(item, uuid)),
        _ => {}
    }
}

// Run-scoped public API. These thin wrappers resolve the run base directory from
// `(settings, dataset_uuid, run_id)` — an existing run is found by its id even
// when the caller omitted the scope (see [`run_base_for`]) — and hand the
// explicit path to the domain-free [`core`] engine, so callers reference
// `crate::workflow::*` and never name the `core` child module (which would
// otherwise shadow Rust's `core` crate).

pub(crate) fn list_runs(
    settings: &SettingsState,
    dataset_uuid: Option<&str>,
) -> Result<Value, String> {
    core::list_runs(&run_base(settings, dataset_uuid)?)
}

pub(crate) fn create_run(
    settings: &SettingsState,
    dataset_uuid: Option<&str>,
    run_id: Option<String>,
    definition_yaml: &str,
) -> Result<Value, String> {
    // Creating is the one operation that must not search: it has no run to find
    // yet, so the scope is the caller's to state. It does refuse to *shadow* one
    // — an id already taken by a dataset-scoped run would otherwise quietly get
    // a second, empty copy in the workspace jobs dir.
    if dataset_uuid.map(str::trim).filter(|s| !s.is_empty()).is_none() {
        if let Some(id) = run_id.as_deref().map(str::trim) {
            if !id.is_empty() && !dataset_run_bases(settings, id).is_empty() {
                return Err(format!(
                    "run '{id}' already exists inside a dataset, so it cannot also be created as a \
                     workspace job. Drive the existing one (the run-scoped tools find it by run_id \
                     alone), or pass dataset_uuid to scope a new run to a dataset, or choose \
                     another run_id."
                ));
            }
        }
    }
    // The check vocabulary comes from the binding layer, so `verify:` ids are
    // validated against what the app can actually evaluate.
    core::create_run(
        &run_base(settings, dataset_uuid)?,
        run_id,
        definition_yaml,
        &verify::check_ids(),
    )
}

pub(crate) fn status(
    settings: &SettingsState,
    dataset_uuid: Option<&str>,
    run_id: &str,
) -> Result<Value, String> {
    core::status(&run_base_for(settings, dataset_uuid, run_id)?, run_id)
}

pub(crate) fn definition_text(
    settings: &SettingsState,
    dataset_uuid: Option<&str>,
    run_id: &str,
) -> Result<String, String> {
    core::definition_text(&run_base_for(settings, dataset_uuid, run_id)?, run_id)
}

pub(crate) fn next_steps(
    settings: &SettingsState,
    dataset_uuid: Option<&str>,
    run_id: &str,
) -> Result<Value, String> {
    let scope = run_scope(settings, dataset_uuid, run_id)?;
    core::next_steps(&scope.base, run_id)
        .map(|value| bind_subject(value, scope.dataset_uuid.as_deref()))
}

pub(crate) fn advance(
    settings: &SettingsState,
    dataset_uuid: Option<&str>,
    run_id: &str,
    step_id: &str,
    agent_id: Option<String>,
) -> Result<Value, String> {
    let scope = run_scope(settings, dataset_uuid, run_id)?;
    core::advance(&scope.base, run_id, step_id, agent_id)
        .map(|value| bind_subject(value, scope.dataset_uuid.as_deref()))
}

pub(crate) fn record(
    settings: &SettingsState,
    dataset_uuid: Option<&str>,
    run_id: &str,
    step_id: &str,
    event_kind: &str,
    detail: Option<Value>,
) -> Result<Value, String> {
    core::record(
        &run_base_for(settings, dataset_uuid, run_id)?,
        run_id,
        step_id,
        event_kind,
        detail,
    )
}

pub(crate) fn intervene(
    settings: &SettingsState,
    dataset_uuid: Option<&str>,
    run_id: &str,
    step_id: &str,
    op: &str,
) -> Result<Value, String> {
    core::intervene(&run_base_for(settings, dataset_uuid, run_id)?, run_id, step_id, op)
}

/// Score a dataset-scoped run against freshly measured evidence. Not a thin
/// wrapper on purpose: unlike the other run fns it needs the *dataset* (that is
/// the subject being probed), and it lives in [`verify`] where the check
/// vocabulary sits.
pub(crate) fn verify(
    settings: &SettingsState,
    dataset_uuid: &str,
    run_id: &str,
    apply: bool,
) -> Result<Value, String> {
    verify::verify(settings, dataset_uuid, run_id, apply)
}

/// Adopt on-disk evidence into a dataset-scoped run, opening the run first when
/// the dataset has none. Same reason as [`verify`]: it needs the dataset as the
/// subject to probe, so it is not a thin wrapper over a [`core`] fn.
pub(crate) fn adopt(
    settings: &SettingsState,
    dataset_uuid: &str,
    run_id: &str,
    definition_yaml: Option<&str>,
) -> Result<Value, String> {
    verify::adopt(settings, dataset_uuid, run_id, definition_yaml)
}

// ---------------------------------------------------------------------------
// Built-in templates
// ---------------------------------------------------------------------------

/// List every built-in template shipped in the binary, each parsed once for its
/// `name`/`version` metadata alongside its raw YAML text (so the frontend and
/// an agent get the exact same definition, no second source of truth).
pub(crate) fn builtin_templates() -> Result<Value, String> {
    let mut out = Vec::new();
    for (category, id, yaml) in templates::all() {
        let def: core::Definition = serde_yaml::from_str(yaml)
            .map_err(|e| format!("built-in template '{id}' failed to parse: {e}"))?;
        out.push(json!({
            "category": category,
            "id": id,
            "name": def.name,
            "version": def.version,
            "step_count": def.steps.len(),
            "yaml": yaml,
        }));
    }
    Ok(json!({ "templates": out, "count": out.len() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dataset_scoped_response_hands_back_an_executable_call() {
        let response = json!({
            "run_id": "dictation-abc",
            "steps": [{ "id": "sync_media", "action": "dataset_sync_media", "params": { "uuid": SUBJECT_REF } }],
        });
        let bound = bind_subject(response, Some("abc"));
        assert_eq!(bound["dataset_uuid"], json!("abc"));
        assert_eq!(bound["steps"][0]["params"]["uuid"], json!("abc"));
    }

    #[test]
    fn a_job_with_no_subject_keeps_the_placeholder_instead_of_inventing_one() {
        let response = json!({ "params": { "uuid": SUBJECT_REF, "mode": "new" } });
        let bound = bind_subject(response, None);
        assert_eq!(bound["params"]["uuid"], json!(SUBJECT_REF));
        assert_eq!(bound["params"]["mode"], json!("new"));
        assert!(bound.get("dataset_uuid").is_none());
    }
}
