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

#[cfg(test)]
mod tests {
    //! The binding is only trustworthy while both sides agree on the vocabulary, and
    //! nothing but these tests stops a template from naming a check no provider
    //! registers — a typo there silently disables a gate in front of a user.
    use super::check_ids;

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
}
