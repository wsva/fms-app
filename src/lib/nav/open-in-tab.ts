// ---------------------------------------------------------------------------
// Open a page in the shell, then drive it — in that order, and with a wait in
// between.
//
// The gap is the whole reason this exists. `PageHost` mounts a page instance
// only when its tab opens, and a page publishes its UI actions when it mounts
// (`useAgentActions`), so `navigate(t); runUiAction(id)` throws for any tab that
// was not already open: the registry has no entry yet. The shell's own
// `start-dictation` bridge has exactly that race.
//
// Waiting for the action to *appear* beats waiting a fixed beat, and the wait
// has to cover the action's own data too: `dictation.open-dataset` and
// `cards.select-dataset` both refuse a uuid that is not in their loaded list
// yet, and both say so with "retry once". So one deadline covers registration
// and the first successful run.
// ---------------------------------------------------------------------------

import { listUiActions, runUiAction } from "@/lib/agent/ui-actions";
import type { TabId } from "@/lib/shell/tabs";

const POLL_MS = 50;
const TIMEOUT_MS = 2_000;

/**
 * Open `tab` and run `actionId` against `datasetUuid` once that page can.
 *
 * @param navigate The shell's `openTab`, threaded down as `onNavigate`.
 * @param tab The page that owns the action.
 * @param actionId A registered UI action taking `dataset_uuid`.
 * @param datasetUuid Passed as `dataset_uuid`; `extra` is merged over it.
 * @returns Whatever the action returned.
 * @throws when the action never becomes runnable inside {@link TIMEOUT_MS}. The
 *   message says which of the two it was, because "the page did not mount" and
 *   "the dataset is not in this page" have different fixes.
 */
export async function openDatasetInTab(
  navigate: (tab: TabId) => void,
  tab: TabId,
  actionId: string,
  datasetUuid: string,
  extra: Record<string, unknown> = {}
): Promise<unknown> {
  navigate(tab);

  const deadline = Date.now() + TIMEOUT_MS;
  let lastError: unknown = null;
  let registered = false;

  for (;;) {
    registered = listUiActions().some((a) => a.id === actionId);
    if (registered) {
      try {
        return await runUiAction(actionId, { dataset_uuid: datasetUuid, ...extra });
      } catch (err) {
        // Most often "the list is still loading", which the action itself
        // invites a retry for. A genuinely wrong uuid costs at most the
        // remaining deadline, and the message it produced is kept for the throw.
        lastError = err;
      }
    }
    if (Date.now() >= deadline) break;
    await new Promise((resolve) => setTimeout(resolve, POLL_MS));
  }

  if (!registered) {
    throw new Error(
      `'${actionId}' never registered after opening '${tab}' — the page did not mount`
    );
  }
  throw lastError instanceof Error
    ? lastError
    : new Error(`'${actionId}' did not accept '${datasetUuid}': ${String(lastError)}`);
}
