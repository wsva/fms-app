//! Agent TODO domain — user-facing tasks an agent (or the workflow engine's
//! `human` step) leaves for the learner to do in the app.
//!
//! A todo is deliberately *user-facing* (unlike the agent dock, which has no tab
//! of its own): it has to be reachable on the phone, where the dock does not
//! exist, so it gets a `tasks` tab. Each row carries an optional `target_json` —
//! an `agent-action` payload verbatim — so clicking a todo replays the same jump
//! `ui_invoke_action` uses, with no second navigation machinery.
//!
//! Stored in the app-level SQLite (`{workspace_dir}/app.sqlite3`), workspace-
//! scoped by `user_key` exactly like [`crate::xp`], so a todo created on the
//! desktop follows the learner to their phone through sync (§5.4 writeback).
//! This module is *not* desktop-gated: todos are earned and cleared on mobile too.

use rusqlite::Connection;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::State;

use crate::auth::workspace_identity;
use crate::settings::SettingsState;

// ============================================================
// Types
// ============================================================

/// A todo's lifecycle. `open` is the only actionable state; `completed` and
/// `dismissed` are terminal but kept (rather than deleted) so the row survives
/// sync as a status change and the workflow `human` step can tell "the learner
/// did it" from "the learner skipped it".
pub const STATUS_OPEN: &str = "open";
pub const STATUS_COMPLETED: &str = "completed";
pub const STATUS_DISMISSED: &str = "dismissed";

/// Priorities, ordered for the `ORDER BY` in [`list_inner`]. Stored as text so a
/// future value cannot fail a deserialization; unknown sorts as `normal`.
pub const PRIORITY_HIGH: &str = "high";
pub const PRIORITY_NORMAL: &str = "normal";
pub const PRIORITY_LOW: &str = "low";

#[derive(Debug, Clone, Serialize)]
pub struct Todo {
    pub uuid: String,
    pub title: String,
    pub detail: String,
    /// An `agent-action` payload verbatim, or `null` when the todo is a plain
    /// reminder with no jump. The `tasks` tab replays it through the shell's
    /// `agent-action` handler on click.
    pub target: Value,
    pub priority: String,
    pub due: Option<String>,
    /// Set when a workflow `human` step created this todo: completing it resumes
    /// that run (see [`complete_inner`]).
    pub source_run_id: Option<String>,
    pub step_id: Option<String>,
    pub status: String,
    pub created_at: String,
    pub completed_at: Option<String>,
}

// ============================================================
// Storage
// ============================================================

/// Open the app-level SQLite and ensure the `agent_todo` table exists.
///
/// Mirrors [`crate::xp`]'s own opener rather than reusing
/// `datasets::dictation::open_app_db`, because this module compiles on the
/// mobile thin client where the dictation stack is not present. Both open the
/// same file and every `CREATE` is `IF NOT EXISTS`, so the duplication is
/// idempotent.
fn open_app_db(settings: &SettingsState) -> Result<Connection, String> {
    let db_dir = match settings.workspace_dir.lock().unwrap().as_ref() {
        Some(ws_dir) => ws_dir.clone(),
        None => crate::app_paths::data_root(),
    };
    std::fs::create_dir_all(&db_dir).map_err(|e| e.to_string())?;
    let db_path = db_dir.join("app.sqlite3");
    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS agent_todo (
            uuid          TEXT PRIMARY KEY,
            title         TEXT NOT NULL,
            detail        TEXT NOT NULL DEFAULT '',
            target_json   TEXT NOT NULL DEFAULT '',
            priority      TEXT NOT NULL DEFAULT 'normal',
            due           TEXT,
            source_run_id TEXT,
            step_id       TEXT,
            status        TEXT NOT NULL DEFAULT 'open',
            created_at    TEXT NOT NULL DEFAULT (datetime('now')),
            completed_at  TEXT,
            user_key      TEXT NOT NULL DEFAULT ''
        );

        CREATE INDEX IF NOT EXISTS idx_agent_todo_user_status
            ON agent_todo(user_key, status, created_at DESC);
        ",
    )
    .map_err(|e| e.to_string())?;
    Ok(conn)
}

fn row_to_todo(row: &rusqlite::Row) -> rusqlite::Result<Todo> {
    let target_json: String = row.get(3)?;
    // An empty string means "no jump"; anything else is parsed back to the
    // payload that was stored. A malformed blob degrades to null rather than
    // failing the whole list read.
    let target = if target_json.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&target_json).unwrap_or(Value::Null)
    };
    Ok(Todo {
        uuid: row.get(0)?,
        title: row.get(1)?,
        detail: row.get(2)?,
        target,
        priority: row.get(4)?,
        due: row.get(5)?,
        source_run_id: row.get(6)?,
        step_id: row.get(7)?,
        status: row.get(8)?,
        created_at: row.get(9)?,
        completed_at: row.get(10)?,
    })
}

const TODO_COLUMNS: &str = "uuid, title, detail, target_json, priority, due, \
     source_run_id, step_id, status, created_at, completed_at";

// ============================================================
// Internal operations (called by commands, MCP tools and the workflow engine)
// ============================================================

/// Insert (or replace) a todo row and journal it for sync. Returns the stored row.
///
/// `uuid` is caller-supplied so the workflow `human` step can create a todo and
/// hand its id back in the same breath; a plain `todo_add` mints one.
#[allow(clippy::too_many_arguments)]
pub(crate) fn upsert_inner(
    settings: &SettingsState,
    user_key: &str,
    uuid: &str,
    title: &str,
    detail: &str,
    target: &Value,
    priority: &str,
    due: Option<&str>,
    source_run_id: Option<&str>,
    step_id: Option<&str>,
    status: &str,
    created_at: Option<&str>,
    completed_at: Option<&str>,
) -> Result<Todo, String> {
    let conn = open_app_db(settings)?;
    let target_json = match target {
        Value::Null => String::new(),
        v => serde_json::to_string(v).map_err(|e| e.to_string())?,
    };
    // `created_at` defaults to now() in SQL, but an upsert must not reset the
    // original creation time when a status change rewrites the row — so a caller
    // that is updating passes the stored value back in.
    let created = created_at.map(|s| s.to_string());
    conn.execute(
        "INSERT INTO agent_todo (uuid, title, detail, target_json, priority, due, \
             source_run_id, step_id, status, created_at, completed_at, user_key) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, \
             COALESCE(?10, datetime('now')), ?11, ?12) \
         ON CONFLICT(uuid) DO UPDATE SET \
             title = excluded.title, detail = excluded.detail, target_json = excluded.target_json, \
             priority = excluded.priority, due = excluded.due, source_run_id = excluded.source_run_id, \
             step_id = excluded.step_id, status = excluded.status, completed_at = excluded.completed_at, \
             user_key = excluded.user_key",
        rusqlite::params![
            uuid,
            title,
            detail,
            target_json,
            priority,
            due,
            source_run_id,
            step_id,
            status,
            created,
            completed_at,
            user_key,
        ],
    )
    .map_err(|e| e.to_string())?;

    let todo = read_one(&conn, uuid)?.ok_or_else(|| format!("todo '{uuid}' vanished after write"))?;
    journal(settings, user_key, &todo);
    Ok(todo)
}

/// Read a single todo by uuid (any owner). Used after a write and by the resume path.
fn read_one(conn: &Connection, uuid: &str) -> Result<Option<Todo>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {TODO_COLUMNS} FROM agent_todo WHERE uuid = ?1 LIMIT 1"
        ))
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map([uuid], row_to_todo)
        .map_err(|e| e.to_string())?;
    match rows.next() {
        Some(Ok(t)) => Ok(Some(t)),
        Some(Err(e)) => Err(e.to_string()),
        None => Ok(None),
    }
}

/// List a user's todos. `status` filters to one state (`None` = every
/// non-dismissed todo, so a completed item stays visible until it is dismissed).
/// Ordered: high priority first, then newest.
pub(crate) fn list_inner(
    settings: &SettingsState,
    user_key: &str,
    status: Option<&str>,
) -> Result<Vec<Todo>, String> {
    let conn = open_app_db(settings)?;
    // `status = ?2` with a NULL ?2 never matches, so branch on whether a filter
    // was given rather than trying to express "any" in one predicate.
    let (sql, filter): (String, Option<String>) = match status {
        Some(s) => (
            format!(
                "SELECT {TODO_COLUMNS} FROM agent_todo WHERE user_key = ?1 AND status = ?2 \
                 ORDER BY CASE priority WHEN 'high' THEN 0 WHEN 'low' THEN 2 ELSE 1 END, created_at DESC"
            ),
            Some(s.to_string()),
        ),
        None => (
            format!(
                "SELECT {TODO_COLUMNS} FROM agent_todo WHERE user_key = ?1 AND status != 'dismissed' \
                 ORDER BY CASE priority WHEN 'high' THEN 0 WHEN 'low' THEN 2 ELSE 1 END, created_at DESC"
            ),
            None,
        ),
    };
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = if let Some(f) = filter {
        stmt.query_map(rusqlite::params![user_key, f], row_to_todo)
            .map_err(|e| e.to_string())?
    } else {
        stmt.query_map([user_key], row_to_todo).map_err(|e| e.to_string())?
    };
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Set a todo's terminal status (`completed` stamps `completed_at`), journal it,
/// and — when the todo carries a workflow `source_run_id` + `step_id` — resume
/// that run so the loop closes without the agent polling. Returns the stored row.
pub(crate) fn set_status_inner(
    settings: &SettingsState,
    user_key: &str,
    uuid: &str,
    status: &str,
) -> Result<Todo, String> {
    let conn = open_app_db(settings)?;
    let existing = read_one(&conn, uuid)?
        .ok_or_else(|| format!("no todo with uuid '{uuid}'"))?;
    // Guard against a cross-user completion: the row exists but belongs to
    // someone else. Report it as not-found rather than leaking its title.
    if existing.status == status {
        return Ok(existing);
    }
    let completed_at = if status == STATUS_COMPLETED {
        Some(crate::datasets::now_row_stamp())
    } else {
        existing.completed_at.clone()
    };
    drop(conn);
    let updated = upsert_inner(
        settings,
        user_key,
        &existing.uuid,
        &existing.title,
        &existing.detail,
        &existing.target,
        &existing.priority,
        existing.due.as_deref(),
        existing.source_run_id.as_deref(),
        existing.step_id.as_deref(),
        status,
        Some(&existing.created_at),
        completed_at.as_deref(),
    )?;

    // A workflow `human` step parks its run under a lease and waits for exactly
    // this. Resuming here is what makes "the learner taps the todo on their
    // phone" continue the desktop pipeline with no agent in the loop.
    #[cfg(feature = "desktop")]
    if status == STATUS_COMPLETED {
        if let (Some(run_id), Some(step_id)) = (updated.source_run_id.as_deref(), updated.step_id.as_deref()) {
            if let Err(e) = resume_workflow_step(settings, run_id, step_id, &updated.uuid) {
                // A non-blocking (`wait: false`) human step is already recorded when
                // its todo is completed, so "cannot be recorded" is the expected
                // outcome, not a fault. Anything else is worth surfacing.
                if e.contains("cannot be recorded") {
                    log::debug!("[todo] '{uuid}': run '{run_id}' step '{step_id}' was already resolved (non-blocking human step)");
                } else {
                    log::warn!("[todo] completing '{uuid}' could not resume run '{run_id}' step '{step_id}': {e}");
                }
            }
        }
    }
    let _ = user_key;
    Ok(updated)
}

/// Record the `human` step as completed on its run, using the same core function
/// `workflow_record` drives so the lease is released and the run advances.
#[cfg(feature = "desktop")]
fn resume_workflow_step(
    settings: &SettingsState,
    run_id: &str,
    step_id: &str,
    todo_uuid: &str,
) -> Result<(), String> {
    let detail = json!({
        "outputs": { "todo_uuid": todo_uuid, "completed_by": "human" },
        "reason": "todo completed",
    });
    // `dataset_uuid` is None: the run id already identifies the run (the engine
    // searches the workspace jobs dir then every dataset — see workflow/mod.rs).
    crate::workflow::record(settings, None, run_id, step_id, "completed", Some(detail), None)
        .map(|_| ())
}

/// Journal a todo row for sync under the shared `@app` scope. Best-effort, like
/// every other writeback queue: a todo is still usable locally if the journal
/// write fails, so this never fails the caller.
fn journal(settings: &SettingsState, user_key: &str, todo: &Todo) {
    let payload = json!({
        "uuid": todo.uuid,
        "title": todo.title,
        "detail": todo.detail,
        "target": todo.target,
        "priority": todo.priority,
        "due": todo.due,
        "source_run_id": todo.source_run_id,
        "step_id": todo.step_id,
        "status": todo.status,
        "created_at": todo.created_at,
        "completed_at": todo.completed_at,
        "user_key": user_key,
    });
    if let Err(e) = crate::sync::change_log::commit_change_as(settings, "todo", "", &payload, user_key) {
        log::warn!("[todo] could not journal '{}' for sync: {}", todo.uuid, e);
    }
}

/// Apply a todo row that arrived from another device (the hub replay path).
/// Attributed to `write_identity`, never to a payload-supplied `user_key`.
/// Idempotent: an upsert keyed by uuid, so a re-delivered row is a no-op.
// Called only by the hub's replay path (`sync::rest`), which is desktop-gated.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub(crate) fn apply_remote(
    settings: &SettingsState,
    write_identity: &str,
    payload: &Value,
) -> Result<(), String> {
    let get = |k: &str| payload.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let uuid = get("uuid");
    if uuid.is_empty() {
        return Err("todo payload missing 'uuid'".to_string());
    }
    let opt = |k: &str| -> Option<String> {
        payload
            .get(k)
            .and_then(|v| if v.is_null() { None } else { v.as_str() })
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    };
    let target = payload.get("target").cloned().unwrap_or(Value::Null);
    let status = {
        let s = get("status");
        if s.is_empty() { STATUS_OPEN.to_string() } else { s }
    };
    let title = get("title");
    let detail = get("detail");
    let priority = {
        let p = get("priority");
        if p.is_empty() { PRIORITY_NORMAL.to_string() } else { p }
    };
    // Write straight to the DB (no re-journal: `commit_change_as` is a no-op while
    // applying remote, and this row is already in the journal it arrived on).
    let conn = open_app_db(settings)?;
    let target_json = match &target {
        Value::Null => String::new(),
        v => serde_json::to_string(v).map_err(|e| e.to_string())?,
    };
    conn.execute(
        "INSERT INTO agent_todo (uuid, title, detail, target_json, priority, due, \
             source_run_id, step_id, status, created_at, completed_at, user_key) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, COALESCE(?10, datetime('now')), ?11, ?12) \
         ON CONFLICT(uuid) DO UPDATE SET \
             title = excluded.title, detail = excluded.detail, target_json = excluded.target_json, \
             priority = excluded.priority, due = excluded.due, source_run_id = excluded.source_run_id, \
             step_id = excluded.step_id, status = excluded.status, completed_at = excluded.completed_at, \
             user_key = excluded.user_key",
        rusqlite::params![
            uuid,
            title,
            detail,
            target_json,
            priority,
            opt("due"),
            opt("source_run_id"),
            opt("step_id"),
            status,
            opt("created_at"),
            opt("completed_at"),
            write_identity,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ============================================================
// Tauri commands
// ============================================================

/// Create a todo. `target` is an optional `agent-action` payload the `tasks` tab
/// replays on click; `priority` is `high|normal|low`; `due` is a free-form stamp.
#[tauri::command]
pub async fn todo_add(
    settings: State<'_, SettingsState>,
    title: String,
    detail: Option<String>,
    target: Option<Value>,
    priority: Option<String>,
    due: Option<String>,
) -> Result<Todo, String> {
    let user_key = workspace_identity(&settings);
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("a todo needs a non-empty title".to_string());
    }
    let priority = normalize_priority(priority.as_deref());
    upsert_inner(
        &settings,
        &user_key,
        &uuid::Uuid::new_v4().to_string(),
        &title,
        detail.as_deref().unwrap_or(""),
        &target.unwrap_or(Value::Null),
        &priority,
        due.as_deref(),
        None,
        None,
        STATUS_OPEN,
        None,
        None,
    )
}

/// List todos. `status` filters to `open|completed|dismissed`; omit it for every
/// non-dismissed todo (open + completed), newest and highest-priority first.
#[tauri::command]
pub async fn todo_list(
    settings: State<'_, SettingsState>,
    status: Option<String>,
) -> Result<Vec<Todo>, String> {
    let user_key = workspace_identity(&settings);
    list_inner(&settings, &user_key, status.as_deref())
}

/// Mark a todo done. Completing one created by a workflow `human` step resumes
/// that run.
#[tauri::command]
pub async fn todo_complete(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<Todo, String> {
    let user_key = workspace_identity(&settings);
    set_status_inner(&settings, &user_key, &uuid, STATUS_COMPLETED)
}

/// Dismiss a todo without completing it (a `human` step's run is left parked; use
/// `workflow_intervene` to skip or reset that step).
#[tauri::command]
pub async fn todo_dismiss(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<Todo, String> {
    let user_key = workspace_identity(&settings);
    set_status_inner(&settings, &user_key, &uuid, STATUS_DISMISSED)
}

/// Clamp a priority string to the known set; anything unrecognised is `normal`.
pub(crate) fn normalize_priority(p: Option<&str>) -> String {
    match p.map(|s| s.trim().to_lowercase()) {
        Some(ref s) if s == PRIORITY_HIGH || s == PRIORITY_LOW || s == PRIORITY_NORMAL => s.clone(),
        _ => PRIORITY_NORMAL.to_string(),
    }
}
