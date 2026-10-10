//! The whole read-only picture of one dataset, in a single call.
//!
//! An agent deciding what to do next needs three answers that live in three
//! modules: what the dataset *is* (`info.json` and its inventory), what is
//! missing or disagreeing on disk ([`super::dictation::audit`]), and what the
//! pipeline's own *record* says ([`crate::workflow`], scored against that same
//! audit). Asking for them separately costs three round trips plus an alignment
//! job the caller has to do itself — and the caller is usually a model with a
//! context budget, which is why the model state that `generate_subtitles` depends
//! on is folded in here too.
//!
//! Nothing in this module writes. Runs are scored with `apply: false`, so a report
//! can describe drift and name what is runnable, but repairing either stays with
//! `workflow_verify` / `workflow_adopt`, which own the event and therefore the
//! audit trail. Same reason [`super::dictation::audit`] never fixes anything.
//!
//! Desktop-only, like the audit and the engine it reads.

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::datasets::{find_dataset_dir_typed, read_info, DatasetType};
use crate::settings::SettingsState;

/// Assemble the report for one dataset: identity, audit, workflow record, model
/// state, and one actionable `hint` naming the next call to make.
pub(crate) async fn full_report(app: &AppHandle, uuid: &str) -> Result<Value, String> {
    let settings = app.state::<SettingsState>();
    let (dir, ty) = find_dataset_dir_typed(&settings, uuid)?;
    let info = read_info(&dir)?;
    // Only dictation has a provider today. Saying so beats returning an empty
    // `checks` list, which reads as "all clean" instead of "not measured".
    let dictation = ty == DatasetType::Dictation;

    let mut report = json!({
        "dataset": {
            "uuid": uuid,
            "kind": ty.as_str(),
            "name": info.name,
            "language": info.language,
            "path": dir.to_string_lossy(),
            "info": serde_json::to_value(&info).unwrap_or(Value::Null),
        },
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "audit": audit_section(&settings, uuid, dictation, ty.as_str())?,
        "workflow": workflow_section(&settings, uuid, dictation)?,
        "stt_model": stt_section(app).await,
    });
    // Last, because it reads the four sections above: the hint is a projection of
    // the report, never a second opinion about the dataset.
    report["hint"] = json!(next_action(&report));
    Ok(report)
}

/// The subject, measured: facts, inventory counts and every named check.
fn audit_section(
    settings: &SettingsState,
    uuid: &str,
    dictation: bool,
    kind: &str,
) -> Result<Value, String> {
    if !dictation {
        return Ok(json!({
            "available": false,
            "reason": format!(
                "no audit provider for '{kind}' datasets yet — read its inventory with dataset_get"
            ),
        }));
    }
    let audit = super::dictation::audit::audit(settings, uuid)?;
    let mut value = serde_json::to_value(&audit).unwrap_or_else(|_| json!({}));
    if let Some(obj) = value.as_object_mut() {
        // Identity already sits under `dataset`; a second copy can only disagree.
        for key in ["dataset_uuid", "path", "info"] {
            obj.remove(key);
        }
    }
    Ok(value)
}

/// Every run this dataset carries, scored against the audit, plus what is
/// runnable next with its action and params already resolved.
fn workflow_section(
    settings: &SettingsState,
    uuid: &str,
    dictation: bool,
) -> Result<Value, String> {
    let listed = crate::workflow::list_runs(settings, Some(uuid))?;
    let none: &[Value] = &[];
    let found = listed
        .get("runs")
        .and_then(Value::as_array)
        .map(|runs| runs.as_slice())
        .unwrap_or(none);

    let mut runs = Vec::with_capacity(found.len());
    for run in found {
        let Some(run_id) = run.get("run_id").and_then(Value::as_str) else {
            continue;
        };
        // A run the engine cannot read is still worth reporting: "this dataset
        // carries a broken run" is actionable, an error that sinks the whole
        // report is not.
        let mut scored = if dictation {
            crate::workflow::verify(settings, uuid, run_id, false)
        } else {
            crate::workflow::status(settings, Some(uuid), run_id)
        }
        .unwrap_or_else(|e| json!({ "run_id": run_id, "error": e }));

        if let Some(obj) = scored.as_object_mut() {
            if let Ok(next) = crate::workflow::next_steps(settings, Some(uuid), run_id) {
                obj.insert("next".into(), next);
            }
        }
        runs.push(scored);
    }
    Ok(json!({ "run_count": runs.len(), "runs": runs }))
}

/// The STT model state, minus the parts that are not about this dataset.
async fn stt_section(app: &AppHandle) -> Value {
    let state = app.state::<crate::models::ModelState>();
    match crate::models::model_get_status(state).await {
        Ok(mut status) => {
            // The catalogue of models one could download would bury the single
            // field a recipe asks about: is one loaded right now.
            status.models.clear();
            status.download_progress = None;
            serde_json::to_value(&status).unwrap_or_else(|_| json!({}))
        }
        Err(e) => json!({ "error": e }),
    }
}

/// One actionable sentence for the whole report.
///
/// Derived here rather than lifted from the engine's own `hints` because those are
/// per-run and per-step, and a recipe reads one line first. The order is the order
/// of what blocks the most work: an unusable subject, no record at all, a record
/// the audit contradicts, a failure, then ordinary readiness.
fn next_action(report: &Value) -> String {
    let audit = &report["audit"];
    if audit.get("available").and_then(Value::as_bool) == Some(false) {
        return audit["reason"]
            .as_str()
            .unwrap_or("no audit provider for this dataset kind")
            .to_string();
    }

    let workflow = &report["workflow"];
    if workflow["run_count"].as_u64().unwrap_or(0) == 0 {
        return "no workflow run recorded for this dataset — call workflow_builtin_templates, \
                then workflow_adopt with that YAML as definition_yaml to open a run whose \
                statuses come from the audit above (every step whose adopt: checks were \
                measured clean is marked completed without being run)"
            .into();
    }

    let run = &workflow["runs"][0];
    let run_id = run["run_id"].as_str().unwrap_or("?");
    let mut hint = if let Some(e) = run.get("error").and_then(Value::as_str) {
        format!(
            "run '{run_id}' cannot be read: {e} — recreate it with workflow_create, or remove \
             <dataset>/workflows/{run_id} and adopt again"
        )
    } else {
        let drifted = id_list(&run["verification"]["drifted"]);
        let failed = id_list(&run["failed"]);
        let ready = id_list(&run["ready"]);
        if !drifted.is_empty() {
            format!(
                "{} completed step(s) are contradicted by the audit — {}; call workflow_verify \
                 with apply=true to return them to ready, then perform their actions again",
                drifted.len(),
                drifted.join(", ")
            )
        } else if !failed.is_empty() {
            format!(
                "{} step(s) failed — {}; workflow_intervene with op=retry on run '{run_id}' (op=skip \
                 only with the user's ok), reasons are in its hints",
                failed.len(),
                failed.join(", ")
            )
        } else if !ready.is_empty() {
            format!(
                "run '{run_id}': {} — call workflow_advance for one, perform the action and params \
                 it hands back, then workflow_record with event=completed and its declared outputs",
                ready.join(", ")
            )
        } else {
            match run["run_status"].as_str().unwrap_or("") {
                "completed" => format!(
                    "run '{run_id}' is complete: every step is done and the audit contradicts none \
                     of them"
                ),
                "blocked" => format!(
                    "run '{run_id}' is blocked and nothing is runnable ({}); a blocked step waits \
                     on a dependency's output, so record that dependency first",
                    id_list(&run["blocked"]).join(", ")
                ),
                other => format!(
                    "run '{run_id}' is '{other}' with nothing ready — call workflow_status for the \
                     per-step reasons"
                ),
            }
        }
    };

    // The one prerequisite no run knows about: without a loaded model every
    // transcription step fails, and that reads like a pipeline bug.
    if report["stt_model"]["active_version"].as_str().is_none() {
        hint.push_str(" Note: no STT model is loaded — see stt_model.hint; transcription fails until model_load succeeds.");
    }
    hint
}

/// Read step ids from either shape the engine emits: `["a", "b"]` for `ready`,
/// `[{"id": "a", "reason": …}]` for `blocked` / `failed` / `drifted` lists.
fn id_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .or_else(|| item.get("id").and_then(Value::as_str).map(str::to_string))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    //! The hint is the part a recipe actually acts on, so its precedence is
    //! behaviour worth pinning: drift must outrank readiness, an unreadable run
    //! must be reported rather than fatal, and a missing model must be mentioned.
    use super::*;

    fn report(workflow: Value, stt: Value) -> Value {
        json!({
            "dataset": { "uuid": "u", "kind": "dictation" },
            "audit": { "available": true, "checks": [] },
            "workflow": workflow,
            "stt_model": stt,
        })
    }

    fn loaded() -> Value {
        json!({ "active_version": "parakeet-tdt-v0.1.0" })
    }

    #[test]
    fn no_run_points_at_adoption_rather_than_at_a_blind_create() {
        let hint = next_action(&report(
            json!({ "run_count": 0, "runs": [] }),
            loaded(),
        ));
        assert!(hint.contains("workflow_adopt"), "{hint}");
        assert!(hint.contains("workflow_builtin_templates"), "{hint}");
    }

    #[test]
    fn drift_outranks_a_step_that_looks_runnable() {
        let hint = next_action(&report(
            json!({
                "run_count": 1,
                "runs": [{
                    "run_id": "dictation-u",
                    "run_status": "running",
                    "ready": ["generate_waveforms"],
                    "verification": { "drifted": ["generate_subtitles"] },
                }],
            }),
            loaded(),
        ));
        assert!(hint.contains("apply=true"), "{hint}");
        assert!(hint.contains("generate_subtitles"), "{hint}");
        assert!(!hint.contains("workflow_advance"), "{hint}");
    }

    #[test]
    fn a_ready_step_is_named_with_the_call_that_claims_it() {
        let hint = next_action(&report(
            json!({
                "run_count": 1,
                "runs": [{
                    "run_id": "dictation-u",
                    "run_status": "running",
                    "ready": ["sync_media"],
                    "failed": [],
                    "blocked": [],
                    "verification": { "drifted": [] },
                }],
            }),
            loaded(),
        ));
        assert!(hint.contains("sync_media"), "{hint}");
        assert!(hint.contains("workflow_advance"), "{hint}");
        assert!(hint.contains("workflow_record"), "{hint}");
    }

    #[test]
    fn a_failure_is_read_from_the_object_shape_the_engine_uses() {
        let hint = next_action(&report(
            json!({
                "run_count": 1,
                "runs": [{
                    "run_id": "dictation-u",
                    "run_status": "failed",
                    "ready": [],
                    "failed": [{ "id": "generate_subtitles", "reason": "no model" }],
                    "verification": { "drifted": [] },
                }],
            }),
            loaded(),
        ));
        assert!(hint.contains("generate_subtitles"), "{hint}");
        assert!(hint.contains("op=retry"), "{hint}");
    }

    #[test]
    fn an_unreadable_run_is_reported_instead_of_sinking_the_report() {
        let hint = next_action(&report(
            json!({
                "run_count": 1,
                "runs": [{ "run_id": "dictation-u", "error": "unreadable state.json" }],
            }),
            loaded(),
        ));
        assert!(hint.contains("cannot be read"), "{hint}");
        assert!(hint.contains("unreadable state.json"), "{hint}");
    }

    #[test]
    fn a_missing_model_is_said_out_loud() {
        let with_model = next_action(&report(
            json!({
                "run_count": 1,
                "runs": [{ "run_id": "r", "run_status": "completed", "ready": [],
                            "verification": { "drifted": [] } }],
            }),
            loaded(),
        ));
        assert!(!with_model.contains("no STT model"), "{with_model}");

        let without = next_action(&report(
            json!({
                "run_count": 1,
                "runs": [{ "run_id": "r", "run_status": "running", "ready": ["generate_subtitles"],
                            "verification": { "drifted": [] } }],
            }),
            json!({ "active_version": null }),
        ));
        assert!(without.contains("no STT model is loaded"), "{without}");
    }

    #[test]
    fn a_kind_without_a_provider_says_so_instead_of_looking_clean() {
        let hint = next_action(&json!({
            "audit": { "available": false, "reason": "no audit provider for 'book' datasets yet" },
            "workflow": { "run_count": 1, "runs": [{ "run_id": "r", "ready": ["x"] }] },
            "stt_model": loaded(),
        }));
        assert_eq!(hint, "no audit provider for 'book' datasets yet");
    }
}
