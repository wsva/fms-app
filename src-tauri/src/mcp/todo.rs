//! The agent-facing TODO tools: the same four operations the `tasks` tab and the
//! Tauri commands expose, so an agent can leave the learner work and read it back.
//!
//! A todo is user-facing storage, not a UI control: it survives sync (a row under
//! the `@app` scope, like XP) and completing one that a workflow `human` step
//! created resumes that run. Everything here is a thin wrapper over
//! [`crate::todo`]'s inner functions — the storage, journaling and resume logic
//! live there, so the command twin and the MCP tool cannot drift.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use serde::Deserialize;
use serde_json::Value;
use tauri::Manager;

use super::{DatasetMcpServer, JsonValue};
use crate::settings::SettingsState;
use crate::todo;

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct TodoAddParam {
    /// Short imperative summary shown in the tasks list, e.g. "Record a take of line 3".
    title: String,
    /// Longer explanation. Optional.
    #[serde(default)]
    detail: Option<String>,
    /// An `agent-action` payload the tasks tab replays when the learner taps the
    /// todo — the jump that puts them where the work is, e.g.
    /// `{"type":"invoke-action","id":"dictation.focus-cue","params":{"index":3}}`.
    /// Omit for a plain reminder with no jump.
    #[serde(default)]
    target: Option<JsonValue>,
    /// `high`, `normal` (default) or `low`.
    #[serde(default)]
    priority: Option<String>,
    /// Free-form due stamp, e.g. an ISO date. Optional.
    #[serde(default)]
    due: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct TodoListParam {
    /// Filter to `open`, `completed` or `dismissed`. Omit for every non-dismissed
    /// todo (open + completed), highest-priority and newest first.
    #[serde(default)]
    status: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct TodoIdParam {
    /// The todo uuid, from `todo_list` or the `todo_uuid` a `human` step returned.
    uuid: String,
}

#[tool_router(router = todo_router, vis = "pub(crate)")]
impl DatasetMcpServer {
    #[tool(name = "todo_add", description = "Leave the learner a task in the app's Tasks tab. Use it to hand off work that needs a human (record a take, review a card, approve a subtitle) or to remind them of a next step. `target` optionally carries an agent-action payload that the tab replays on tap, so the todo can deep-link the learner to the exact screen. Todos sync across the learner's devices. Returns the created todo with its uuid.")]
    async fn todo_add(
        &self,
        Parameters(param): Parameters<TodoAddParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] todo_add: {}", param.title);
        let settings = self.app.state::<SettingsState>();
        let user_key = crate::auth::workspace_identity(settings.inner());
        let title = param.title.trim().to_string();
        if title.is_empty() {
            return Err("a todo needs a non-empty title".to_string());
        }
        let target = param.target.map(|t| t.0).unwrap_or(Value::Null);
        let priority = todo::normalize_priority(param.priority.as_deref());
        let created = todo::upsert_inner(
            settings.inner(),
            &user_key,
            &uuid::Uuid::new_v4().to_string(),
            &title,
            param.detail.as_deref().unwrap_or(""),
            &target,
            &priority,
            param.due.as_deref(),
            None,
            None,
            todo::STATUS_OPEN,
            None,
            None,
        )?;
        Ok(serde_json::to_string_pretty(&created).unwrap_or_default())
    }

    #[tool(name = "todo_list", description = "Read the learner's todos. Optional `status` filter (open|completed|dismissed); omit it for every non-dismissed todo. Each entry carries its uuid, title, priority, status, and — when a workflow human step created it — source_run_id and step_id.")]
    async fn todo_list(
        &self,
        Parameters(param): Parameters<TodoListParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] todo_list: status={:?}", param.status);
        let settings = self.app.state::<SettingsState>();
        let user_key = crate::auth::workspace_identity(settings.inner());
        let todos = todo::list_inner(settings.inner(), &user_key, param.status.as_deref())?;
        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "count": todos.len(),
            "todos": todos,
        }))
        .unwrap_or_default())
    }

    #[tool(name = "todo_complete", description = "Mark a todo done. If a workflow `human` step created it (it carries source_run_id + step_id), completing it records that step as completed and resumes the run — so an agent waiting on a human step does not need to poll. Returns the updated todo.")]
    async fn todo_complete(
        &self,
        Parameters(param): Parameters<TodoIdParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] todo_complete: {}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let user_key = crate::auth::workspace_identity(settings.inner());
        let updated = todo::set_status_inner(
            settings.inner(),
            &user_key,
            &param.uuid,
            todo::STATUS_COMPLETED,
        )?;
        Ok(serde_json::to_string_pretty(&updated).unwrap_or_default())
    }

    #[tool(name = "todo_dismiss", description = "Dismiss a todo without completing it. A workflow `human` step's run stays parked (use workflow_intervene to skip or reset that step). Returns the updated todo.")]
    async fn todo_dismiss(
        &self,
        Parameters(param): Parameters<TodoIdParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] todo_dismiss: {}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let user_key = crate::auth::workspace_identity(settings.inner());
        let updated = todo::set_status_inner(
            settings.inner(),
            &user_key,
            &param.uuid,
            todo::STATUS_DISMISSED,
        )?;
        Ok(serde_json::to_string_pretty(&updated).unwrap_or_default())
    }
}
