/**
 * The agent task ledger for the status bar.
 *
 * The bar shows a count and the top of the open list, so it needs the same
 * refresh discipline the Tasks page has: read once on mount, then keep current.
 * Tasks are written by workflow runs, agent actions and the user's own clicks,
 * and none of those emit an event the bar could subscribe to, so a slow poll is
 * the honest mechanism — 8s, only while the window is visible, via the shared
 * {@link useVisiblePolling}.
 */

import { useCallback, useEffect, useState } from "react";
import { isTauri } from "@/lib/tauri";
import {
  completeTask,
  dismissTask,
  listTasks,
  replayTaskTarget,
  type AgentTask,
} from "@/lib/agent/tasks";
import { useVisiblePolling } from "./useVisiblePolling";

export function useAgentTasks(active: boolean) {
  const [open, setOpen] = useState<AgentTask[]>([]);
  /** The task a verb is in flight for, so its row can disable itself. */
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!isTauri()) return;
    try {
      setOpen(await listTasks("open"));
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    if (active) void refresh();
  }, [active, refresh]);

  useVisiblePolling(active, () => void refresh(), 8000);

  /** Do the task: jump first (navigation is fire-and-forget and must not wait
   *  on the IPC round trip), then mark it done — which resumes a workflow run
   *  parked on a `human` step. */
  const complete = useCallback(
    async (task: AgentTask) => {
      if (busy) return;
      setBusy(task.uuid);
      replayTaskTarget(task.target);
      try {
        await completeTask(task.uuid);
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
    async (task: AgentTask) => {
      if (busy) return;
      setBusy(task.uuid);
      try {
        await dismissTask(task.uuid);
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

  return { open, busy, error, complete, dismiss, refresh };
}

export default useAgentTasks;
