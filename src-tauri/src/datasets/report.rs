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

use std::path::Path;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::datasets::{find_dataset_dir_typed, read_info, DatasetType};
use crate::settings::SettingsState;

/// Offenders one check lists in a *report*. `dataset_audit` caps at 100 because
/// the page renders a table of them; a report is read by a model, and 47 copies
/// of one sentence about 47 different files teach it nothing the exact `count`
/// has not already told it — for the price of a few thousand context tokens.
const REPORT_ITEM_CAP: usize = 5;

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
        "workflow": workflow_section(&settings, uuid, dictation, ty.as_str())?,
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
    Ok(slim_audit(value))
}

/// Project the audit down for a report reader: every check keeps its verdict, its
/// exact count and its advice, but lists only a sample of offenders and says a
/// finding shared by several files once (`item_detail`) instead of once per file.
/// The full list is one `dataset_audit` call away, which each check's `advice`
/// already tells the reader to want.
fn slim_audit(mut value: Value) -> Value {
    if let Some(checks) = value.get_mut("checks").and_then(Value::as_array_mut) {
        for check in checks.iter_mut() {
            slim_check(check);
        }
    }
    value
}

fn slim_check(check: &mut Value) {
    let Some(map) = check.as_object_mut() else {
        return;
    };
    let items = match map.remove("items") {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    };
    // `truncated` speaks for the audit's own cap, which this projection replaces.
    map.remove("truncated");
    let shown: Vec<Value> = items.iter().take(REPORT_ITEM_CAP).cloned().collect();
    if items.len() > shown.len() {
        map.insert("items_omitted".into(), json!(items.len() - shown.len()));
    }
    match shared_detail(&shown) {
        // One finding, many files: name the finding once, keep the files a list.
        Some(detail) => {
            map.insert("item_detail".into(), json!(detail));
            map.insert(
                "items".into(),
                Value::Array(
                    shown
                        .iter()
                        .filter_map(|i| i.get("source").and_then(Value::as_str))
                        .map(|s| Value::String(s.to_string()))
                        .collect(),
                ),
            );
        }
        // Different reasons per file — those are the findings worth reading.
        None => {
            map.insert("items".into(), Value::Array(shown));
        }
    }
}

/// The one finding behind every shown offender, with each file's own name
/// replaced by `<file>`. `None` when they differ after all, or when there is only
/// one item and hoisting would buy nothing.
fn shared_detail(items: &[Value]) -> Option<String> {
    if items.len() < 2 {
        return None;
    }
    // Generalize everything first, then compare: the finding sentence has to be
    // read once per file either way, and this way no borrow straddles a loop.
    let candidates: Option<Vec<String>> = items
        .iter()
        .map(|item| {
            let obj = item.as_object()?;
            let detail = obj.get("detail").and_then(Value::as_str)?;
            let source = obj.get("source").and_then(Value::as_str)?;
            Some(generalize(detail, source))
        })
        .collect();
    let candidates = candidates?;
    let first = candidates.first()?;
    candidates.iter().all(|c| c == first).then(|| first.clone())
}

/// Replace every spelling of one finding's file — the source path and its
/// extension-less stem, which is how the artefacts beside it are named — with
/// `<file>`.
fn generalize(detail: &str, source: &str) -> String {
    let stem = Path::new(source).with_extension("");
    let stem = stem.to_string_lossy();
    let out = if source.is_empty() {
        detail.to_string()
    } else {
        detail.replace(source, "<file>")
    };
    if stem.is_empty() || stem.as_ref() == source {
        out
    } else {
        out.replace(stem.as_ref(), "<file>")
    }
}

/// Every run this dataset carries, scored against the audit, plus what is
/// runnable next with its action and params already resolved.
fn workflow_section(
    settings: &SettingsState,
    uuid: &str,
    dictation: bool,
    kind: &str,
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
        slim_run(&mut scored);
        runs.push(scored);
    }
    order_runs(uuid, &mut runs);
    let on_disk: Vec<&str> = runs
        .iter()
        .filter_map(|r| r.get("run_id").and_then(Value::as_str))
        .collect();
    // One entry per built-in template of this dataset's kind, whether or not its
    // run exists: a caller who means the *sync cue times* workflow has to be told
    // that its run is `dictation-sync_cue_times-<uuid>` and not the build run, or
    // it will record real work in the wrong history.
    let expected_runs: Vec<Value> = crate::workflow::expected_runs(kind, uuid)
        .into_iter()
        .map(|(template_category, template_id, run_id)| {
            json!({
                "category": template_category,
                "template_id": template_id,
                "run_id": run_id,
                "present": on_disk.contains(&run_id.as_str()),
            })
        })
        .collect();
    Ok(json!({
        "run_count": runs.len(),
        // The id the app's own page would use, whether or not it exists: with more
        // than one run in play, a reader needs to know which one the UI shows.
        "canonical_run_id": crate::workflow::canonical_run_id(uuid),
        "expected_runs": expected_runs,
        "runs": runs,
    }))
}

/// Drop what a report reader cannot act on: `verdicts` for steps that have not
/// claimed to be finished. A `verified` verdict on a `ready` step only says the
/// evidence agrees with work nobody has done yet, and the audit's counts already
/// say that — while every verdict worth reading (`drifted`, `undecidable`) sits on
/// a `completed` step, which is what this keeps.
fn slim_run(run: &mut Value) {
    if let Some(Value::Object(verdicts)) = run.get_mut("verdicts") {
        verdicts.retain(|_, v| v.get("status").and_then(Value::as_str) == Some("completed"));
    }
}

/// Put the run a caller should act on first: the one the app's page drives, then
/// the most advanced, then by id so two reports of one dataset read identically.
///
/// Directory order is not a ranking, and treating it as one cost a real session
/// three hours: an agent worked in a stray run the report happened to list first
/// while the canonical run — the one the user looks at — never moved, and both
/// `workflow_advance` and `workflow_record` were perfectly happy about that.
fn order_runs(dataset_uuid: &str, runs: &mut Vec<Value>) {
    let canonical = crate::workflow::canonical_run_id(dataset_uuid);
    // An owned rank key, not a borrowed one: the id is read out of the run the
    // comparator is handed, so a `sort_by_key` closure cannot return it.
    runs.sort_by(|a, b| rank(a, &canonical).cmp(&rank(b, &canonical)));
}

fn rank(run: &Value, canonical: &str) -> (i32, std::cmp::Reverse<u64>, String) {
    let id = run.get("run_id").and_then(Value::as_str).unwrap_or("");
    (
        if id == canonical { 0 } else { 1 },
        // An unreadable run has no `counts`, so it ranks last — a run whose
        // progress is unknown is never the one to tell an agent to work in.
        std::cmp::Reverse(
            run.get("counts")
                .and_then(|c| c.get("completed"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
        ),
        id.to_string(),
    )
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
    let canonical = workflow["canonical_run_id"].as_str().unwrap_or("");
    if workflow["run_count"].as_u64().unwrap_or(0) == 0 {
        return format!(
            "no workflow run recorded for this dataset — call workflow_builtin_templates, pick the \
             template the task needs, then workflow_adopt with that YAML as definition_yaml and that \
             template's run id (workflow.expected_runs lists them; '{canonical}' is the primary \
             workflow's, the one the app's Workflow page uses by default) to open a run whose \
             statuses come from the audit above: every step whose adopt: checks were measured clean \
             is marked completed without being run"
        );
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
                other => {
                    // Nothing ready and the run is neither complete nor blocked: a step
                    // is claimed and nobody reported its outcome. That is the one dead
                    // end which looks from here like the app being busy, so name the
                    // claim and the call that clears it — a session that reads "stalled"
                    // as "in progress" ends its turn and leaves the run claimed forever.
                    let claimed = claimed_steps(run);
                    if claimed.is_empty() {
                        format!(
                            "run '{run_id}' is '{other}' with nothing ready — call workflow_status \
                             for the per-step reasons"
                        )
                    } else {
                        format!(
                            "run '{run_id}' is stalled: {}; its outcome was never reported, so \
                             nothing else can start. workflow_record(run_id='{run_id}', step, \
                             event=completed) if you did the work, event=failed if you did not \
                             (a reaped lease returns the step to ready and workflow_record accepts \
                             a ready step), workflow_intervene(op=reset) only to hand the claim over",
                            claimed.join("; ")
                        )
                    }
                }
            }
        }
    };

    // Several runs on one dataset is a real state, not a detail: they are separate
    // histories, and progress recorded in one is invisible in the others.
    if let Some(note) = multi_run_note(workflow) {
        hint.push_str(&note);
    }

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

/// Steps claimed and not yet reported, phrased the way the hint names them.
///
/// Reads the `steps` view rather than `next.claimed` because the view is what the
/// engine writes after reaping: a step whose lease has expired is already back to
/// `pending` there, so a hint built this way never tells an agent to report a claim
/// nobody holds.
fn claimed_steps(run: &Value) -> Vec<String> {
    run.get("steps")
        .and_then(Value::as_object)
        .map(|steps| {
            steps
                .iter()
                .filter(|(_, view)| view.get("status").and_then(Value::as_str) == Some("running"))
                .map(|(id, view)| {
                    let holder = view
                        .get("lease")
                        .and_then(Value::as_str)
                        .unwrap_or("an agent");
                    match view.get("started_at").and_then(Value::as_str) {
                        Some(started) => format!("'{id}' is claimed by '{holder}' since {started}"),
                        None => format!("'{id}' is claimed by '{holder}'"),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The sentence explaining which run the hint follows, when a dataset carries more
/// than one. `None` when there is only one, which is the normal case and needs no
/// qualification of the hint above it.
///
/// "More than one" splits in two, and the advice differs: a run whose id belongs to
/// a built-in template is a *second workflow* over this dataset — legitimately its
/// own history, and nothing to consolidate — while any other id is one the caller
/// (or an agent) invented, which is the case that used to waste whole sessions.
fn multi_run_note(workflow: &Value) -> Option<String> {
    let runs = workflow.get("runs").and_then(Value::as_array)?;
    if runs.len() < 2 {
        return None;
    }
    let chosen = runs[0].get("run_id").and_then(Value::as_str).unwrap_or("?");
    let others: Vec<&str> = runs
        .iter()
        .skip(1)
        .filter_map(|r| r.get("run_id").and_then(Value::as_str))
        .collect();
    let canonical = workflow
        .get("canonical_run_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let template_runs: Vec<&str> = workflow
        .get("expected_runs")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| e.get("run_id").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    let strays: Vec<&str> = others
        .iter()
        .copied()
        .filter(|id| !template_runs.contains(id))
        .collect();
    let followed = if canonical.is_empty() || canonical == chosen {
        format!(
            "'{chosen}' is the run of this dataset's primary workflow{} (every template keeps its \
             own run — see workflow.expected_runs)",
            if canonical.is_empty() {
                ""
            } else {
                ", which the app's Workflow page uses by default"
            }
        )
    } else {
        format!(
            "this hint follows '{chosen}'; the primary workflow's run is '{canonical}', which the \
             app's Workflow page uses by default and is either behind or does not exist yet \
             (workflow_adopt with run_id='{canonical}' opens it)"
        )
    };
    let note = if strays.is_empty() {
        format!(
            " Note: this dataset carries {} runs of named workflows and they are separate \
             histories — {} — so work recorded in '{chosen}' does not show up in [{}]. Act on the \
             run of the workflow you mean, never by copying statuses between them.",
            runs.len(),
            followed,
            others.join(", "),
        )
    } else {
        format!(
            " Note: this dataset carries {} runs and they are separate histories — {} — so work \
             recorded in '{chosen}' does not show up in [{}]. Consolidate by finishing or deleting \
             the stray run(s) [{}], never by copying statuses between them.",
            runs.len(),
            followed,
            others.join(", "),
            strays.join(", "),
        )
    };
    Some(note)
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

    #[test]
    fn a_stalled_run_names_the_claimed_step_and_the_call_that_clears_it() {
        let hint = next_action(&report(
            json!({
                "run_count": 1,
                "runs": [{
                    "run_id": "dictation-u",
                    "run_status": "in_progress",
                    "ready": [],
                    "failed": [],
                    "blocked": [],
                    "verification": { "drifted": [] },
                    "steps": {
                        "generate_subtitles": {
                            "status": "running", "lease": "agent", "started_at": "2026-10-11T03:54:33Z"
                        }
                    },
                }],
            }),
            loaded(),
        ));
        assert!(hint.contains("stalled"), "{hint}");
        assert!(hint.contains("generate_subtitles"), "{hint}");
        assert!(hint.contains("workflow_record"), "{hint}");
    }

    /// A reaped lease is not a claim, and telling an agent to report one would
    /// send it at an error — the step is `ready` again and only needs doing.
    #[test]
    fn a_reaped_claim_is_not_reported_as_still_claimed() {
        let hint = next_action(&report(
            json!({
                "run_count": 1,
                "runs": [{
                    "run_id": "dictation-u",
                    "run_status": "in_progress",
                    "ready": [],
                    "failed": [],
                    "blocked": [],
                    "verification": { "drifted": [] },
                    "steps": {
                        "generate_subtitles": {
                            "status": "pending", "reason": "interrupted: lease expired after 300s"
                        }
                    },
                }],
            }),
            loaded(),
        ));
        assert!(!hint.contains("stalled"), "{hint}");
        assert!(hint.contains("workflow_status"), "{hint}");
    }

    /// The bug this ordering exists for: an orphan run sorted first by accident of
    /// the filesystem, and a whole session ran inside it while the run the user
    /// looks at stayed where it was.
    #[test]
    fn the_run_the_page_drives_is_ordered_first_even_when_it_is_behind() {
        let mut runs = vec![
            json!({ "run_id": "dictation-stray", "counts": { "completed": 4 } }),
            json!({ "run_id": "dictation-u", "counts": { "completed": 1 } }),
        ];
        order_runs("u", &mut runs);
        assert_eq!(runs[0]["run_id"], json!("dictation-u"));

        let hint = next_action(&report(
            json!({
                "run_count": 2,
                "canonical_run_id": "dictation-u",
                "runs": [runs[0].clone(), runs[1].clone()],
            }),
            loaded(),
        ));
        assert!(hint.contains("dictation-stray"), "{hint}");
        assert!(hint.contains("separate histories"), "{hint}");
        assert!(hint.contains("Workflow page uses 'dictation-u'") || hint.contains("'dictation-u' is the run"), "{hint}");
    }

    /// A second workflow over one dataset is not an accident. Since `dictation`
    /// ships two templates, a dataset carrying both their runs is the expected
    /// state — so the hint must name them as workflows and stop telling the caller
    /// to consolidate a legitimate history away. Only an id no template owns is a
    /// stray, which the test above pins.
    #[test]
    fn a_second_templates_run_is_a_workflow_and_not_a_stray() {
        let hint = next_action(&report(
            json!({
                "run_count": 2,
                "canonical_run_id": "dictation-u",
                "expected_runs": [
                    { "template_id": "build_dictation_dataset", "run_id": "dictation-u", "present": true },
                    { "template_id": "sync_dictation_cue_times", "run_id": "dictation-sync_cue_times-u", "present": true },
                ],
                "runs": [
                    { "run_id": "dictation-u", "counts": { "completed": 3 }, "ready": ["adjust_cue_times"] },
                    { "run_id": "dictation-sync_cue_times-u", "counts": { "completed": 1 } },
                ],
            }),
            loaded(),
        ));
        assert!(hint.contains("runs of named workflows"), "{hint}");
        assert!(hint.contains("Act on the run of the workflow you mean"), "{hint}");
        assert!(hint.contains("dictation-sync_cue_times-u"), "{hint}");
        assert!(!hint.contains("stray"), "{hint}");
    }

    #[test]
    fn with_no_canonical_run_the_hint_follows_the_most_advanced_one() {
        let mut runs = vec![
            json!({ "run_id": "alpha", "counts": { "completed": 2 } }),
            json!({ "run_id": "beta", "counts": { "completed": 5 } }),
            json!({ "run_id": "gamma", "error": "unreadable state.json" }),
        ];
        order_runs("u", &mut runs);
        assert_eq!(runs[0]["run_id"], json!("beta"));
        assert_eq!(runs[1]["run_id"], json!("alpha"));
        // An unreadable run has no known progress, so it is never the one to work in.
        assert_eq!(runs[2]["run_id"], json!("gamma"));
    }

    #[test]
    fn a_report_keeps_only_verdicts_that_claim_progress() {
        let mut run = json!({
            "verdicts": {
                "a": { "status": "ready", "verdict": "verified" },
                "b": { "status": "completed", "verdict": "drifted" },
            },
        });
        slim_run(&mut run);
        assert!(run["verdicts"].get("a").is_none());
        assert_eq!(run["verdicts"]["b"]["verdict"], json!("drifted"));
    }

    /// Seventy-odd files sharing one finding is one sentence, not seventy: the
    /// count stays exact and the sample stays readable. The whole source —
    /// directory included — folds away, since the artefacts beside it are named
    /// the same way and the un-folded names stay in `items`.
    #[test]
    fn a_shared_finding_is_said_once_and_the_rest_are_counted() {
        let mut check = json!({
            "id": "waveforms_missing",
            "level": "problem",
            "count": 7,
            "truncated": false,
            "items": (0..7)
                .map(|i| json!({
                    "source": format!("de/b{i}.mp3"),
                    "detail": format!("no waveform/de/b{i}.json — the player has no peaks"),
                }))
                .collect::<Vec<_>>(),
        });
        slim_check(&mut check);
        assert_eq!(check["item_detail"], json!("no waveform/<file>.json — the player has no peaks"));
        assert_eq!(check["items"].as_array().unwrap().len(), REPORT_ITEM_CAP);
        assert_eq!(check["items_omitted"], json!(7 - REPORT_ITEM_CAP));
        assert_eq!(check["count"], json!(7));
        assert!(check["items"][0].is_string());
        assert!(check.get("truncated").is_none());
    }

    #[test]
    fn findings_that_differ_stay_per_file() {
        let mut check = json!({
            "id": "cue_sanity",
            "count": 2,
            "truncated": false,
            "items": [
                { "source": "a.mp3", "detail": "cue 3 overlaps cue 4" },
                { "source": "b.mp3", "detail": "cue 2 is empty" },
            ],
        });
        slim_check(&mut check);
        assert!(check.get("item_detail").is_none());
        assert!(check.get("items_omitted").is_none());
        assert_eq!(check["items"][0]["detail"], json!("cue 3 overlaps cue 4"));
    }
}
