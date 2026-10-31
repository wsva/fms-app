// ---------------------------------------------------------------------------
// The agent TODO surface, frontend side.
//
// One place for the `todo_*` commands and the `Todo` shape, because two
// consumers now read the same ledger: the Tasks page (the full list, reachable
// on the phone) and the status bar (the count and the top of the list, desktop).
// Before that split each spelled `invoke("todo_list")` out on its own.
// ---------------------------------------------------------------------------

import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";

/** Mirror of `src-tauri/src/todo.rs::Todo` (serialized field names). */
export interface AgentTask {
  uuid: string;
  title: string;
  detail: string;
  target: unknown;
  priority: string;
  due: string | null;
  source_run_id: string | null;
  step_id: string | null;
  status: string;
  created_at: string;
  completed_at: string | null;
}

export type AgentTaskStatus = "open" | "completed" | "dismissed";

/** List tasks. Omit `status` for every non-dismissed one (open + completed),
 *  highest priority first, newest next — the backend's own ordering. */
export function listTasks(status?: AgentTaskStatus): Promise<AgentTask[]> {
  // Tauri deserialises command arguments as camelCase, so the snake_case value
  // is spelled out here rather than at every call site.
  return invoke<AgentTask[]>("todo_list", { status: status ?? null });
}

/** Mark one done. Completing a task a workflow `human` step is waiting on
 *  resumes that run on the backend. */
export function completeTask(uuid: string): Promise<void> {
  return invoke("todo_complete", { uuid });
}

export function dismissTask(uuid: string): Promise<void> {
  return invoke("todo_dismiss", { uuid });
}

/** A task's `target` is an `agent-action` payload verbatim. Anything carrying a
 *  string `type` is replayed through the shell's existing handler (the same one
 *  `ui_invoke_action` drives), so clicking a task lands the learner exactly where
 *  the agent meant — no second navigation machinery.
 *
 *  `todoUuid` is handed in by the caller because the payload cannot carry it: a
 *  capture's target is written by `todo_add`, which is what mints the uuid. It
 *  is injected only into `open-card`, whose `completes` rule needs to know which
 *  todo to close. */
export function replayTaskTarget(target: unknown, todoUuid?: string): void {
  if (
    target &&
    typeof target === "object" &&
    typeof (target as { type?: unknown }).type === "string"
  ) {
    const action = target as { type: string };
    const payload =
      todoUuid && action.type === "open-card"
        ? { ...action, todo_uuid: todoUuid }
        : target;
    void emit("agent-action", payload);
  }
}

/**
 * Whether tapping a task also completes it.
 *
 * Almost always: a todo that deep-links somewhere is done once you get there.
 * A capture is the exception — it exists to be finished, so its payload says so
 * and the card editor's save closes it instead. An absent or odd `target` keeps
 * the old behaviour, because a todo with nothing to open still needs one tap to
 * get rid of. */
export function tapCompletes(target: unknown): boolean {
  if (!target || typeof target !== "object") return true;
  return (target as { completes?: unknown }).completes !== "on_save";
}
