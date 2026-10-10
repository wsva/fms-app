//! The verification seam: measure the world, then let [`core::verify`] score a
//! run against that measurement.
//!
//! The split is deliberate. [`core`] owns what a `verify:` binding *means* —
//! verdicts, gates, the one-way demotion of a contradicted `completed` — while
//! treating check ids as opaque strings. This module owns the *vocabulary*: which
//! ids exist and how each one is evaluated, because a check needs SQL and the
//! filesystem and so can live neither in YAML nor in the domain-free engine.
//!
//! Today exactly one provider answers, the dictation audit
//! ([`crate::datasets::dictation::audit`]). Another dataset kind taking part
//! means adding a branch to [`check_ids`] and [`evidence`] — nothing changes in
//! [`core`], the command twin or the MCP tool, which is the proof that the
//! mechanism was never dictation-specific.

use std::collections::HashMap;

use serde_json::Value;

use crate::datasets::DatasetType;
use crate::settings::SettingsState;

use super::core;

/// Every check id a `verify:` block may name, across all providers. Handed to
/// [`core::create_run`] for validation, so the vocabulary a definition can
/// reference and the vocabulary the app can evaluate are one list rather than two
/// that drift apart.
pub(crate) fn check_ids() -> Vec<&'static str> {
    crate::datasets::dictation::audit::check_ids().collect()
}

/// Measure one dataset as `check id → number of offenders`, where `0` means
/// "evaluated and clean" and an absent key means "not evaluated".
///
/// A dataset of a kind with no check provider yet is refused *actionably* rather
/// than reported as vacuously verified: an empty evidence map would make every
/// bound step `unknown`, which reads like a bug instead of like the truth.
pub(crate) fn evidence(
    settings: &SettingsState,
    dataset_uuid: &str,
) -> Result<HashMap<String, usize>, String> {
    let (_dir, ty) = crate::datasets::find_dataset_dir_typed(settings, dataset_uuid)?;
    if ty != DatasetType::Dictation {
        return Err(format!(
            "no check provider for '{}' datasets yet — verification is implemented for dictation \
             only; inspect this pipeline with workflow_status instead",
            ty.as_str()
        ));
    }
    let report = crate::datasets::dictation::audit::audit(settings, dataset_uuid)?;
    Ok(report
        .checks
        .into_iter()
        .map(|c| (c.id.to_string(), c.count))
        .collect())
}

/// Verify a dataset-scoped run against the current state of its dataset.
///
/// The run is always sought inside the dataset's own `workflows/` dir, since a
/// workspace-level job has no subject to probe. `apply` allows the one-way
/// demotion of `completed` steps the evidence contradicts; without it this is a
/// pure read that writes nothing.
pub(crate) fn verify(
    settings: &SettingsState,
    dataset_uuid: &str,
    run_id: &str,
    apply: bool,
) -> Result<Value, String> {
    let base = super::run_base(settings, Some(dataset_uuid))?;
    let evidence = evidence(settings, dataset_uuid)?;
    core::verify(&base, run_id, &evidence, apply)
}

/// Adopt the evidence into a dataset-scoped run: a step whose `adopt:` checks were
/// all measured and came back clean becomes `completed` without being run.
///
/// The run is opened from `definition_yaml` when this dataset has none yet. That
/// is on purpose: the whole point of adoption is that a dataset prepared
/// elsewhere should show its real state the first time anyone looks at it, and
/// "there is no run" is not a state of the pipeline, it is the absence of a
/// record of one. Nothing is torn down here — the mirror operation,
/// [`verify`] with `apply`, is the one that needs to be asked — and every
/// adoption is journalled as `adopted` rather than as work performed, so
/// `events.jsonl` still distinguishes a tick earned from a tick inherited.
///
/// When the run already exists and the passed definition differs semantically from
/// the stored one, the run is rebound to it *only* if its whole history is
/// inherited — see [`core::history_is_inherited_only`] — because a template edit
/// that adds a binding must not silently stop applying to every dataset scanned
/// since. `adoption.refreshed` reports whether that happened; a definition that
/// fails validation is reported in `adoption.refresh_error` and the scan proceeds
/// against the run's own, older text.
pub(crate) fn adopt(
    settings: &SettingsState,
    dataset_uuid: &str,
    run_id: &str,
    definition_yaml: Option<&str>,
) -> Result<Value, String> {
    let base = super::run_base(settings, Some(dataset_uuid))?;
    let evidence = evidence(settings, dataset_uuid)?;
    let mut refreshed = false;
    let mut refresh_error: Option<String> = None;
    if !core::run_exists(&base, run_id) {
        let yaml = definition_yaml.ok_or_else(|| {
            format!(
                "no run '{run_id}' for dataset '{dataset_uuid}'; pass its workflow.yaml text as \
                 yaml_text to open one, or call workflow_create_run first"
            )
        })?;
        core::create_run(&base, Some(run_id.to_string()), yaml, &check_ids())?;
    } else if let Some(yaml) = definition_yaml {
        // A run this page opened for itself is a projection of the app's current
        // template, so when the template moves and the run holds nothing but
        // adoptions, the projection moves with it — otherwise a definition edit
        // would silently strand every existing dataset on the checks it predates.
        // A run an agent or a human put progress into keeps the definition it was
        // created from; that is why the text is stored beside the state at all.
        let stored = core::definition_text(&base, run_id)?;
        if documents_differ(&stored, yaml) && core::history_is_inherited_only(&base, run_id)? {
            if let Err(e) = core::rebind_definition(&base, run_id, yaml, &check_ids()) {
                // An editor buffer that will not validate must not sink the scan:
                // the run keeps its own definition and adoption proceeds on it.
                refresh_error = Some(e);
            } else {
                refreshed = true;
            }
        }
    }
    let mut resp = core::adopt(&base, run_id, &evidence)?;
    if let Some(adoption) = resp
        .as_object_mut()
        .and_then(|obj| obj.get_mut("adoption"))
        .and_then(|a| a.as_object_mut())
    {
        adoption.insert("refreshed".into(), Value::Bool(refreshed));
        if let Some(e) = refresh_error {
            adoption.insert("refresh_error".into(), Value::String(e));
        }
    }
    Ok(resp)
}

/// Whether two YAML texts mean *different* definitions. Comment-only and
/// formatting-only edits must not count, or a prose tweak to a template would
/// rewrite a run's file and journal a refresh that changed no behaviour.
fn documents_differ(stored: &str, current: &str) -> bool {
    match (
        serde_yaml::from_str::<serde_yaml::Value>(stored),
        serde_yaml::from_str::<serde_yaml::Value>(current),
    ) {
        (Ok(a), Ok(b)) => a != b,
        // Unreadable on either side: call it different and let validation decide.
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    //! The binding is only trustworthy while both sides agree on the vocabulary, and
    //! nothing but these tests stops a template from naming a check no provider
    //! registers — a typo there silently disables a gate in front of a user.
    use super::{check_ids, documents_differ};

    fn temp_base(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fms-verify-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn built_in_templates_bind_only_registered_checks() {
        let ids = check_ids();
        assert!(!ids.is_empty(), "the check vocabulary must not be empty");
        let base = temp_base("templates");
        for (category, _, yaml) in crate::workflow::templates::all() {
            let result = crate::workflow::core::create_run(&base, Some(format!("probe-{category}")), yaml, &ids);
            assert!(result.is_ok(), "'{category}' template rejected: {}", result.unwrap_err());
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_dangling_verify_id_is_rejected() {
        let base = temp_base("dangling");
        let yaml = "name: probe\nversion: 1\nsteps:\n  - id: a\n    action: dataset_get\n    verify:\n      proves: [no_such_check]\n";
        let err = crate::workflow::core::create_run(&base, Some("dangling".into()), yaml, &check_ids())
            .expect_err("an unregistered check id must not create a run");
        assert!(err.contains("no_such_check"), "unexpected error text: {err}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A dangling `adopt:` id is worse than a dangling `verify:` one: instead of
    /// silently disabling a gate it silently disables a promotion, so the step
    /// never turns green no matter what is on disk.
    #[test]
    fn a_dangling_adopt_id_is_rejected() {
        let base = temp_base("dangling-adopt");
        let yaml = "name: probe\nversion: 1\nsteps:\n  - id: a\n    action: dataset_init_dir\n    adopt: [no_such_check]\n";
        let err = crate::workflow::core::create_run(&base, Some("dangling-adopt".into()), yaml, &check_ids())
            .expect_err("an unregistered adopt check id must not create a run");
        assert!(err.contains("no_such_check"), "unexpected error text: {err}");
        assert!(err.contains("adopt"), "the error should name the field: {err}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The refresh decision hinges on this comparison: prose-only edits must not
    /// rewrite a run's definition and journal a change that altered no behaviour,
    /// while a real binding edit must not be mistaken for formatting.
    #[test]
    fn only_a_semantic_edit_counts_as_a_new_definition() {
        let yaml = "name: probe\nversion: 1\nsteps:\n  - id: a\n    action: dataset_get\n";
        assert!(!documents_differ(yaml, yaml), "identical text is not a change");
        assert!(
            !documents_differ(yaml, &format!("{yaml}# a note for the next reader\n")),
            "a comment must not count as a definition change"
        );
        assert!(
            documents_differ(yaml, &yaml.replace("action: dataset_get", "action: dataset_get\n    adopt: [database_missing]")),
            "a new binding must count"
        );
        assert!(
            documents_differ(yaml, "name: [unclosed"),
            "unreadable text is left to validation rather than silently passed"
        );
    }
}
