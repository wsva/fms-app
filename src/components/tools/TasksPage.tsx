"use client";

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { Check, Circle, ListTodo, X } from "lucide-react";
import { PageHeader, Toolbar, ToolbarButton } from "@/components/ui/toolbar";
import { usePageRefresh } from "@/lib/refresh/page-refresh";

/** Mirror of `src-tauri/src/todo.rs::Todo` (serialized field names). */
interface Todo {
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

const PRIORITY_BADGE: Record<string, string> = {
  high: "bg-red-600/25 text-red-300",
  normal: "bg-blue-600/25 text-blue-300",
  low: "bg-gray-600/25 text-text-secondary",
};

/** A todo's `target` is an `agent-action` payload verbatim. Anything carrying a
 *  string `type` is replayed through the shell's existing handler (the same one
 *  `ui_invoke_action` drives), so clicking a todo lands the learner exactly
 *  where the agent meant — no second navigation machinery. */
function replayTarget(target: unknown): void {
  if (target && typeof target === "object" && typeof (target as { type?: unknown }).type === "string") {
    void emit("agent-action", target);
  }
}

/**
 * The `tasks` tab: user-facing todos an agent, or a workflow `human` step, left
 * for the learner. Deliberately a tab (unlike the agent dock) so it is reachable
 * on the phone, where the dock does not exist. Clicking an open todo replays its
 * `target` through the shell and marks it done; completing a todo a workflow step
 * is waiting on resumes that run on the backend.
 */
export default function TasksPage() {
  const [todos, setTodos] = useState<Todo[]>([]);
  const [loading, setLoading] = useState(true);
  const [hideCompleted, setHideCompleted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      // No `status` filter: the backend returns every non-dismissed todo
      // (open + completed) ordered high-priority-first then newest.
      const rows = await invoke<Todo[]>("todo_list", {});
      setTodos(rows);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // The task list is written by workflow runs, agent actions and the user's own
  // clicks, so it goes stale silently. The shared hook re-reads it whenever this
  // page becomes the visible tab, the app resumes, or a dataset is imported.
  usePageRefresh("tasks", {
    refresh,
    events: ["dataset-list-changed"],
  });

  const complete = useCallback(
    async (todo: Todo) => {
      if (busy) return;
      setBusy(todo.uuid);
      // Jump first: navigation is fire-and-forget and must not wait on the IPC
      // round trip. Then mark done — which, for a workflow hand-off, resumes the
      // parked run on the backend.
      replayTarget(todo.target);
      try {
        await invoke("todo_complete", { uuid: todo.uuid });
        await refresh();
        setError(null);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setBusy(null);
      }
    },
    [busy, refresh]
  );

  const dismiss = useCallback(
    async (todo: Todo) => {
      if (busy) return;
      setBusy(todo.uuid);
      try {
        await invoke("todo_dismiss", { uuid: todo.uuid });
        await refresh();
        setError(null);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setBusy(null);
      }
    },
    [busy, refresh]
  );

  const visible = hideCompleted ? todos.filter((t) => t.status !== "completed") : todos;
  const openCount = todos.filter((t) => t.status === "open").length;
  const doneCount = todos.length - openCount;

  return (
    <div className="flex flex-1 min-w-0 flex-col h-full bg-bg-base">
      <PageHeader title="Tasks" />
      <Toolbar className="flex-wrap">
        <span className="text-xs text-text-secondary">
          {openCount} open{doneCount > 0 ? ` \u00b7 ${doneCount} done` : ""}
        </span>
        <div className="flex-1 min-w-0" />
        <ToolbarButton
          variant="toggle"
          active={hideCompleted}
          icon={Check}
          label="Open only"
          onClick={() => setHideCompleted((v) => !v)}
          title={hideCompleted ? "Show completed tasks too" : "Hide completed tasks"}
        />
      </Toolbar>

      {error && (
        <div className="mx-3 mt-2 px-3 py-2 rounded-md bg-red-600/15 border border-red-600/30 text-xs text-red-300 break-words">
          {error}
        </div>
      )}

      <div className="flex-1 min-h-0 overflow-y-auto p-3">
        {loading ? (
          <p className="text-sm text-text-secondary">Loading&hellip;</p>
        ) : visible.length === 0 ? (
          <div className="flex flex-col items-center justify-center gap-2 py-16 text-center">
            <ListTodo size={36} className="text-text-tertiary" />
            <p className="text-sm text-text-secondary max-w-sm">
              {todos.length === 0
                ? "No tasks yet. An agent or a workflow hand-off will leave one here."
                : "Nothing open \u2014 you are all caught up."}
            </p>
          </div>
        ) : (
          <ul className="flex flex-col gap-2 max-w-3xl">
            {visible.map((todo) => {
              const done = todo.status === "completed";
              const isBusy = busy === todo.uuid;
              return (
                <li
                  key={todo.uuid}
                  className={`flex items-start gap-3 rounded-lg border border-border-default bg-bg-card px-3 py-2.5 ${
                    done ? "opacity-60" : ""
                  }`}
                >
                  {done ? (
                    <span className="mt-0.5 shrink-0 text-green-500" title="Completed">
                      <Check size={18} />
                    </span>
                  ) : (
                    <button
                      type="button"
                      onClick={() => void complete(todo)}
                      disabled={isBusy}
                      title="Do this task and open where it leads"
                      className="mt-0.5 shrink-0 text-text-tertiary hover:text-accent cursor-pointer"
                      aria-label="Complete task"
                    >
                      <Circle size={18} />
                    </button>
                  )}

                  <button
                    type="button"
                    onClick={() => {
                      if (!done) void complete(todo);
                    }}
                    disabled={done || isBusy}
                    className={`flex-1 min-w-0 text-left ${done ? "" : "cursor-pointer"}`}
                  >
                    <span
                      className={`block text-sm font-medium text-text-primary break-words ${
                        done ? "line-through" : ""
                      }`}
                    >
                      {todo.title}
                    </span>
                    {todo.detail && (
                      <span className="block text-xs text-text-secondary mt-0.5 break-words">
                        {todo.detail}
                      </span>
                    )}
                    <span className="flex flex-wrap items-center gap-2 mt-1">
                      <span
                        className={`text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded ${
                          PRIORITY_BADGE[todo.priority] ?? PRIORITY_BADGE.normal
                        }`}
                      >
                        {todo.priority}
                      </span>
                      {todo.due && (
                        <span className="text-[11px] text-text-tertiary">due {todo.due}</span>
                      )}
                      {todo.source_run_id && (
                        <span className="text-[11px] text-text-tertiary">
                          from workflow{todo.step_id ? ` \u00b7 ${todo.step_id}` : ""}
                        </span>
                      )}
                    </span>
                  </button>

                  <button
                    type="button"
                    onClick={() => void dismiss(todo)}
                    disabled={isBusy}
                    title="Dismiss without completing"
                    className="shrink-0 p-1 rounded text-text-tertiary hover:text-red-400 hover:bg-bg-hover cursor-pointer"
                    aria-label="Dismiss task"
                  >
                    <X size={16} />
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </div>
  );
}
