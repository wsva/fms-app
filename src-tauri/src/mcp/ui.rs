//! Driving the UI: two tools over the action registry the webview publishes.
//!
//! The registry lives in `ui_state.rs` and is filled by `useAgentActions` on
//! every mounted page, so it means "everything the app can do right now" — a
//! capability manifest rather than a fixed list baked into this file. That is
//! why there are two generic tools here instead of one hand-written tool per
//! control: adding a button to a page adds it to the agent's surface with no
//! Rust change at all.
//!
//! Both tools sit on the existing `agent-action` channel and the shell bridge in
//! `app/(app)/home/page.tsx`. `ui_invoke_action` closes the loop with a
//! correlation id, because a fire-and-forget event would leave the agent
//! guessing whether the click landed.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{Emitter, Manager};
use tokio::time::{timeout, Duration};

use super::{DatasetMcpServer, JsonValue};
use crate::ui_state::UiStateHandle;

/// How long `ui_invoke_action` waits for the webview to report back. Generous
/// enough for an action that awaits a backend command, short enough that a
/// closed window fails the call instead of hanging a tool the agent is waiting
/// on.
const INVOKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The `kind` vocabulary. Declared here rather than read off the registry so a
/// filter typo is caught instead of silently matching nothing.
const KINDS: [&str; 3] = ["read", "mutate", "destructive"];

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct UiListActionsParam {
    /// Restrict to one page's actions (a `TabId`, e.g. `"cards"`).
    #[serde(default)]
    tab: Option<String>,
    /// Restrict to one kind: `read`, `mutate` or `destructive`.
    #[serde(default)]
    kind: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct UiInvokeActionParam {
    /// The action id from `ui_list_actions`, e.g. `"cards.start-review"`.
    id: String,
    /// Arguments matching the action's `params_schema`. Omit when it has none.
    /// `JsonValue` rather than `serde_json::Value` so the generated schema is an
    /// object and not a boolean `true` (see `mcp/mod.rs`).
    #[serde(default)]
    params: Option<JsonValue>,
}

/// The declared parameters of one action, as `{ name: { type, description,
/// required?, enum? } }`.
fn param_specs(action: &Value) -> Vec<(String, Value)> {
    action
        .get("params_schema")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// Check a call against the action's declared parameters, in the house style:
/// the error names the valid set and what to pass.
fn check_params(action: &Value, params: &Value) -> Result<(), String> {
    let id = action
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("<action>");
    let specs = param_specs(action);
    let given = params.as_object();

    if specs.is_empty() {
        if given.map(|g| !g.is_empty()).unwrap_or(false) {
            return Err(format!(
                "action '{}' takes no parameters, but got {}. Call it with params omitted.",
                id,
                given
                    .map(|g| g.keys().cloned().collect::<Vec<_>>().join(", "))
                    .unwrap_or_default()
            ));
        }
        return Ok(());
    }

    let given = given.ok_or_else(|| {
        format!(
            "action '{}' needs parameters ({}), but got none",
            id,
            specs
                .iter()
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;

    for (name, spec) in &specs {
        if !given.contains_key(name) {
            if spec.get("required").and_then(|v| v.as_bool()) == Some(true) {
                return Err(format!(
                    "action '{}' is missing required parameter '{}': {}. Parameters: {}",
                    id,
                    name,
                    spec.get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default(),
                    specs
                        .iter()
                        .map(|(k, _)| k.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            continue;
        }
        // A declared `enum` is a closed set; naming it beats a page-level
        // rejection the agent cannot see the reason for.
        if let Some(allowed) = spec.get("enum").and_then(|v| v.as_array()) {
            let value = given.get(name).cloned().unwrap_or(Value::Null);
            if !value.is_null() && !allowed.contains(&value) {
                return Err(format!(
                    "action '{}' parameter '{}' must be one of {}, got {}",
                    id,
                    name,
                    allowed
                        .iter()
                        .map(|v| v.to_string())
                        .collect::<Vec<_>>()
                        .join(" | "),
                    value
                ));
            }
        }
    }

    if let Some(unknown) = given
        .keys()
        .find(|k| !specs.iter().any(|(name, _)| name == *k))
    {
        return Err(format!(
            "action '{}' has no parameter '{}'. Valid parameters: {}",
            id,
            unknown,
            specs
                .iter()
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    Ok(())
}

/// The "nothing is registered" answer. Deliberately not `[]`: an empty list
/// reads as "this app has no UI capabilities", when what it actually means is
/// "nobody has reported yet".
fn unmounted_error(ui: &UiStateHandle) -> String {
    if ui.ui_mounted() {
        "no UI actions are registered, although the app window has reported its state. \
         Pages register their actions on mount, so this usually means the build has no \
         `useAgentActions` calls yet. app_get_ui_state still shows where the user is, and \
         the data tools (dataset_*, card_*, workflow_*) do not depend on the registry."
            .to_string()
    } else {
        "no UI actions are registered because the app window has not reported yet \
         (ui_mounted=false): it is still starting, or the window is closed. This is not an \
         empty capability list. Call app_get_ui_state to confirm, then retry — and if the UI \
         is genuinely unavailable, work through the data tools instead, which do not need a \
         window at all."
            .to_string()
    }
}

#[tool_router(router = ui_router, vis = "pub(crate)")]
impl DatasetMcpServer {
    #[tool(name = "ui_list_actions", description = "List what the app's UI can do right now: every action the mounted pages have registered, with its id, label, tab, kind (read = only changes what is on screen, mutate = changes data through the page's own code path, destructive = refused here by design) and params_schema. This is a live registry, not a fixed list — it grows as pages mount, so re-read it rather than caching it. Optional filters: tab (a page id such as \"cards\") and kind. An error instead of an empty list means the webview has not reported yet; the message says which. Use ui_invoke_action with an id from here.")]
    async fn ui_list_actions(
        &self,
        Parameters(param): Parameters<UiListActionsParam>,
    ) -> Result<String, String> {
        let kind_filter = match param.kind.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(raw) => {
                if !KINDS.contains(&raw) {
                    return Err(format!(
                        "unknown kind '{}'. Valid kinds: {}",
                        raw,
                        KINDS.join(", ")
                    ));
                }
                Some(raw.to_string())
            }
        };
        let tab_filter = param.tab.as_deref().map(str::trim).filter(|t| !t.is_empty());

        let ui = self.app.state::<UiStateHandle>();
        let snapshot = ui.actions();
        let all = snapshot
            .get("actions")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        if all.is_empty() {
            return Err(unmounted_error(&ui));
        }

        let actions: Vec<Value> = all
            .into_iter()
            .filter(|a| match (tab_filter, a.get("tab").and_then(|v| v.as_str())) {
                (None, _) => true,
                (Some(_), None) => false,
                (Some(want), Some(have)) => want == have,
            })
            .filter(|a| match (kind_filter.as_deref(), a.get("kind").and_then(|v| v.as_str())) {
                (None, _) => true,
                (Some(_), None) => false,
                (Some(want), Some(have)) => want == have,
            })
            .collect();

        // Name the tabs that exist, so a filter that matched nothing says what
        // would have matched instead of returning a bare zero.
        let mut tabs: Vec<String> = snapshot
            .get("actions")
            .and_then(|v| v.as_array())
            .map(|arr| {
                let mut seen: Vec<String> = arr
                    .iter()
                    .filter_map(|a| a.get("tab").and_then(|v| v.as_str()).map(String::from))
                    .collect();
                seen.sort();
                seen.dedup();
                seen
            })
            .unwrap_or_default();
        tabs.sort();

        let count = actions.len();
        if count == 0 {
            return Err(format!(
                "no registered action matches tab={:?} kind={:?}. Tabs with actions: {}. \
                 Call ui_list_actions with no filters to see the whole registry.",
                tab_filter, kind_filter, tabs.join(", ")
            ));
        }

        Ok(json!({
            "status": "ok",
            "count": count,
            "ui_mounted": ui.ui_mounted(),
            "registered_at": snapshot.get("updated_at").cloned().unwrap_or(Value::Null),
            "tabs": tabs,
            "actions": actions,
            "note": "kind=destructive actions are listed for completeness but refused by ui_invoke_action — destructive operations go through the data tools, which have a trash.",
        })
        .to_string())
    }

    #[tool(name = "ui_invoke_action", description = "Perform one action from ui_list_actions in the app window — the equivalent of the user clicking that button — and wait for the page's own result. Pass the action id and any params its params_schema declares. Returns {ok, detail} where detail is whatever the page returned, or an error naming the reason. kind=destructive actions are refused outright: destructive work goes through the data tools, which have a trash, so there is one undo path and not two. Waits up to 10s; on a timeout the call may still have landed, so confirm the effect with app_get_ui_state (or dataset_get / workflow_status) rather than retrying blind.")]
    async fn ui_invoke_action(
        &self,
        Parameters(param): Parameters<UiInvokeActionParam>,
    ) -> Result<String, String> {
        let id = param.id.trim();
        let params = param
            .params
            .map(|p| p.0)
            .unwrap_or_else(|| json!({}));

        let ui = self.app.state::<UiStateHandle>();
        let action = ui.action(id).ok_or_else(|| {
            let known: Vec<String> = ui
                .actions()
                .get("actions")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|a| a.get("id").and_then(|v| v.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            if known.is_empty() {
                unmounted_error(&ui)
            } else {
                format!(
                    "unknown ui action '{}'. Registered actions: {}. Call ui_list_actions for \
                     their parameters.",
                    id,
                    known.join(", ")
                )
            }
        })?;

        // One undo path, not two: the data tools have a trash, a UI click does
        // not, so a destructive action is refused here rather than proxied.
        if action.get("kind").and_then(|v| v.as_str()) == Some("destructive") {
            return Err(format!(
                "action '{}' is kind=destructive and is refused by design. Destructive operations \
                 go through the data tools (dataset_delete, card_delete, …), which move things to \
                 the app trash so the user can undo them. See app_bootstrap's house rules.",
                id
            ));
        }

        check_params(&action, &params)?;

        // Park before emitting: an action that resolves synchronously would
        // otherwise report back to a correlation id nobody is listening on yet.
        let correlation_id = uuid::Uuid::new_v4().to_string();
        let rx = ui.park(correlation_id.clone());

        let label = action
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        log::info!("[MCP] ui_invoke_action: {} ({})", id, label);

        if let Err(e) = self.app.emit(
            "agent-action",
            json!({
                "type": "invoke-action",
                "id": id,
                "params": params,
                "correlation_id": correlation_id,
            }),
        ) {
            ui.abandon(&correlation_id);
            return Err(format!("failed to emit the action to the webview: {}", e));
        }

        match timeout(INVOKE_TIMEOUT, rx).await {
            Ok(Ok(result)) => Ok(json!({
                "status": "ok",
                "action": id,
                "correlation_id": correlation_id,
                "ok": result.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
                "detail": result.get("detail").cloned().unwrap_or(Value::Null),
            })
            .to_string()),
            // The webview answered "the action threw"; that is a result, not a
            // transport failure, so it comes back as a tool-level error carrying
            // the page's own message.
            Ok(Err(_)) => Err(format!(
                "action '{}' was invoked but its result channel closed before an answer arrived. \
                 Confirm the effect with app_get_ui_state before retrying.",
                id
            )),
            Err(_) => {
                ui.abandon(&correlation_id);
                Err(format!(
                    "action '{}' did not report back within {}s (correlation_id {}). It may still \
                     have run — the event is fire-and-forget once emitted. Do not retry blind: \
                     confirm the effect with app_get_ui_state (for navigation and selection), \
                     dataset_get or workflow_status (for data), and only then decide.",
                    id,
                    INVOKE_TIMEOUT.as_secs(),
                    correlation_id
                ))
            }
        }
    }
}
