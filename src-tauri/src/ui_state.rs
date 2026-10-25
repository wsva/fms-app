//! What the user is currently looking at — the agent's eyes on the UI.
//!
//! The webview reports a *patch* of its context whenever something meaningful
//! changes ([`agent_report_ui_state`]); this module caches the merged result so
//! `app_get_ui_state` can answer an agent synchronously, without a round-trip
//! into the webview. Living here rather than in `ai::agent_acp` matters: the
//! cache belongs to whichever agent asks, and a third-party agent attached over
//! `/mcp` has no ACP client at all.
//!
//! Merge semantics are deliberate — a patch key *present with a value* sets,
//! *present with `null`* clears, *absent* leaves untouched. Pages are
//! display-toggled rather than unmounted, so several of them own disjoint
//! slices of the context and their partial reports have to compose instead of
//! clobbering each other.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};
use tauri::State;
use tokio::sync::oneshot;

/// The keys a patch may carry. Unknown keys are rejected with an actionable
/// error (the `app_navigate` house pattern) rather than silently stored, so a
/// frontend typo surfaces during development instead of reading back as a
/// permanent `null` to every agent.
const KNOWN_KEYS: [&str; 6] = [
    "active_tab",
    "active_dataset_uuid",
    "active_media_uuid",
    "cue_index",
    "selection",
    "scroll",
];

/// Cached UI context plus the registry of things the UI can do.
pub struct UiState {
    /// Merged context patch, `{}` until the first report. The presence of
    /// `updated_at` is how a reader tells "UI is mounted" from "nothing has
    /// ever reported".
    context: Mutex<Value>,
    /// Actions registered by mounted pages, `{}` until the first registration.
    /// Kept here rather than in `context` because it is a capability manifest,
    /// not part of "what the user is looking at".
    actions: Mutex<Value>,
    /// The workflow step-op vocabulary the Workflow page published (from
    /// `lib/workflow/step-ops.ts`), `{}` until the first report. Separate from
    /// `actions` because it describes *authored YAML* — the step ids and form
    /// fields a definition may name — not clickable controls. Sourced from the
    /// frontend so Rust never keeps a second copy of that vocabulary.
    workflow_actions: Mutex<Value>,
    /// In-flight `ui_invoke_action` calls, keyed by correlation id.
    ///
    /// `agent-action` is a fire-and-forget event, so without this the tool could
    /// only say "emitted" and leave the agent guessing whether the click landed.
    /// The tool parks a sender, the webview runs the action and reports back
    /// through [`agent_report_action_result`], and the tool resolves.
    pending: Mutex<HashMap<String, oneshot::Sender<Value>>>,
}

impl Default for UiState {
    /// Both caches start as *objects*, not `Value::Null` (serde's default), so
    /// the merge below can always take `as_object_mut`.
    fn default() -> Self {
        Self {
            context: Mutex::new(json!({})),
            actions: Mutex::new(json!({})),
            workflow_actions: Mutex::new(json!({})),
            pending: Mutex::new(HashMap::new()),
        }
    }
}

/// Managed state handle.
#[derive(Clone, Default)]
pub struct UiStateHandle {
    inner: Arc<UiState>,
}

impl UiStateHandle {
    /// Merge one reported patch into the cached context and stamp `updated_at`.
    /// Returns the merged snapshot.
    pub fn report_context(&self, patch: Value) -> Result<Value, String> {
        let obj = patch
            .as_object()
            .ok_or_else(|| "patch must be a JSON object of context keys".to_string())?;

        let mut context = self.inner.context.lock().unwrap();
        let map = context.as_object_mut().expect("context is always an object");
        for (key, value) in obj {
            if !KNOWN_KEYS.contains(&key.as_str()) {
                return Err(format!(
                    "unknown ui context key '{}'. Valid keys: {}",
                    key,
                    KNOWN_KEYS.join(", ")
                ));
            }
            if value.is_null() {
                map.remove(key);
            } else {
                map.insert(key.clone(), value.clone());
            }
        }
        map.insert(
            "updated_at".to_string(),
            json!(chrono::Utc::now().to_rfc3339()),
        );
        Ok(context.clone())
    }

    /// The merged context snapshot. `{}` before the first report.
    pub fn context(&self) -> Value {
        self.inner.context.lock().unwrap().clone()
    }

    /// Whether the webview has ever reported — i.e. whether the registry and
    /// the context below can be trusted at all.
    pub fn ui_mounted(&self) -> bool {
        self
            .inner
            .context
            .lock()
            .unwrap()
            .get("updated_at")
            .is_some()
    }

    /// Replace the registered UI actions wholesale. Each page re-registers its
    /// own slice on mount, so this takes the full array the frontend assembled.
    pub fn report_actions(&self, actions: Value) -> Result<usize, String> {
        let arr = actions
            .as_array()
            .ok_or_else(|| "actions must be a JSON array".to_string())?;
        let count = arr.len();
        *self.inner.actions.lock().unwrap() = json!({
            "actions": arr,
            "updated_at": chrono::Utc::now().to_rfc3339(),
        });
        Ok(count)
    }

    /// The registered actions plus when they were registered.
    pub fn actions(&self) -> Value {
        self.inner.actions.lock().unwrap().clone()
    }

    /// Replace the published workflow step-op vocabulary wholesale. `actions`
    /// is the per-step doc array; `commands` is the flat set of Tauri commands a
    /// step may name. Stored together so a reader gets a self-consistent pair.
    pub fn report_workflow_actions(&self, actions: Value, commands: Value) -> Result<usize, String> {
        let arr = actions
            .as_array()
            .ok_or_else(|| "actions must be a JSON array".to_string())?;
        let count = arr.len();
        *self.inner.workflow_actions.lock().unwrap() = json!({
            "actions": arr,
            "commands": commands,
            "updated_at": chrono::Utc::now().to_rfc3339(),
        });
        Ok(count)
    }

    /// The published workflow step-op vocabulary plus when it was published.
    pub fn workflow_actions(&self) -> Value {
        self.inner.workflow_actions.lock().unwrap().clone()
    }

    /// One registered action by id, or `None`. Used to check `kind` and the
    /// declared parameters *before* an event is emitted, so a bad call fails
    /// immediately instead of timing out in the webview.
    pub fn action(&self, id: &str) -> Option<Value> {
        self.inner
            .actions
            .lock()
            .unwrap()
            .get("actions")
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                arr.iter()
                    .find(|a| a.get("id").and_then(|v| v.as_str()) == Some(id))
                    .cloned()
            })
    }

    /// Park a correlation id and hand back the half the caller awaits.
    pub fn park(&self, correlation_id: String) -> oneshot::Receiver<Value> {
        let (tx, rx) = oneshot::channel();
        self.inner
            .pending
            .lock()
            .unwrap()
            .insert(correlation_id, tx);
        rx
    }

    /// Deliver a result to a parked invocation. `false` means nothing was
    /// waiting — the id is unknown, or the tool already gave up on it.
    pub fn resolve(&self, correlation_id: &str, result: Value) -> bool {
        match self
            .inner
            .pending
            .lock()
            .unwrap()
            .remove(correlation_id)
        {
            // A receiver that was dropped (the tool timed out) makes `send`
            // fail; that is still "nothing was waiting".
            Some(tx) => tx.send(result).is_ok(),
            None => false,
        }
    }

    /// Drop a parked invocation without delivering anything, so a timed-out call
    /// cannot leak its sender if the webview answers late.
    pub fn abandon(&self, correlation_id: &str) {
        self.inner.pending.lock().unwrap().remove(correlation_id);
    }
}

/// Report the frontend's current UI context so `app_get_ui_state` can answer an
/// agent without a round-trip to the webview.
///
/// `patch` is merged, not replaced: present keys set (or clear, when `null`),
/// absent keys are left alone. Valid keys: `active_tab`,
/// `active_dataset_uuid`, `active_media_uuid`, `cue_index`, `selection`,
/// `scroll`. Returns the merged snapshot.
#[tauri::command]
pub async fn agent_report_ui_state(
    state: State<'_, UiStateHandle>,
    patch: Value,
) -> Result<Value, String> {
    state.report_context(patch)
}

/// Register the actions the mounted UI can perform, so `ui_list_actions` and
/// `ui_invoke_action` can drive them generically.
#[tauri::command]
pub async fn agent_report_ui_actions(
    state: State<'_, UiStateHandle>,
    actions: Value,
) -> Result<Value, String> {
    let count = state.report_actions(actions)?;
    Ok(json!({ "ok": true, "registered": count }))
}

/// The action-registry snapshot, for the shell's own diagnostics.
#[tauri::command]
pub async fn agent_get_ui_actions(state: State<'_, UiStateHandle>) -> Result<Value, String> {
    Ok(state.actions())
}

/// Register the Workflow page's step-op vocabulary (from `step-ops.ts`) so
/// `workflow_list_actions` and the agent manual can describe what an authored
/// step may name without Rust keeping a second copy of it. `actions` is the
/// per-step doc array; `commands` is the flat set of command names a step's
/// `action` may take.
#[tauri::command]
pub async fn agent_report_workflow_actions(
    state: State<'_, UiStateHandle>,
    actions: Value,
    commands: Value,
) -> Result<Value, String> {
    let count = state.report_workflow_actions(actions, commands)?;
    Ok(json!({ "ok": true, "registered": count }))
}

/// Resolve one `ui_invoke_action` call. The shell invokes this after running the
/// registered action, with whatever the action returned (or the error it threw),
/// so the tool can answer the agent with the page's own result instead of a bare
/// "emitted".
///
/// `correlation_id` comes straight off the `agent-action` payload. An id nothing
/// is waiting on is reported rather than ignored: it means the invocation already
/// timed out, and silently succeeding would hide that from whoever is debugging.
#[tauri::command]
pub async fn agent_report_action_result(
    state: State<'_, UiStateHandle>,
    correlation_id: String,
    ok: bool,
    detail: Value,
) -> Result<Value, String> {
    let delivered = state.resolve(&correlation_id, json!({ "ok": ok, "detail": detail }));
    if delivered {
        Ok(json!({ "ok": true, "correlation_id": correlation_id }))
    } else {
        Ok(json!({
            "ok": false,
            "correlation_id": correlation_id,
            "error": format!(
                "no invocation is waiting on correlation_id '{}'. It either timed out or was \
                 never issued; the result was dropped. Re-invoke with ui_invoke_action to retry.",
                correlation_id
            ),
        }))
    }
}

/// A flat, agent-readable view of the merged context. Kept separate from
/// [`UiStateHandle::context`] so the JSON shape an agent sees is defined in one
/// place even as the cache grows.
pub fn context_json(handle: &UiStateHandle) -> Map<String, Value> {
    let ctx = handle.context();
    let mut out = Map::new();
    for key in KNOWN_KEYS {
        out.insert(
            key.to_string(),
            ctx.get(key).cloned().unwrap_or(Value::Null),
        );
    }
    out.insert(
        "updated_at".to_string(),
        ctx.get("updated_at").cloned().unwrap_or(Value::Null),
    );
    out
}
