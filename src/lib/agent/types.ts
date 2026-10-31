// ---------------------------------------------------------------------------
// Agent types — the payloads the shell and the agent surfaces share.
//
// There is no conversation stream here: the app does not dial an agent. Agents
// attach to FmS's MCP server and drive it through the `app_*` tools, whose
// effects arrive as `agent-action` events.
// ---------------------------------------------------------------------------

import type { TabId } from "@/components/layout/Sidebar";

// ---------------------------------------------------------------------------
// App-control actions (emitted by the app_* MCP tools as `agent-action`)
// ---------------------------------------------------------------------------

/** `agent-action` union — the shell (page.tsx) performs these UI actions. */
export type AgentAction =
  | { type: "navigate"; tab: TabId }
  | { type: "open-review"; dataset_uuid?: string | null }
  | {
      type: "start-dictation";
      dataset_uuid?: string | null;
      media_uuid?: string | null;
    }
  | { type: "notify"; message: string }
  /** Open the card editor on a deck, optionally prefilled. A capture todo
   *  replays this: the reader finishes the card here and `todo_uuid` closes on
   *  save. */
  | OpenCardAction
  /** Run one action from the registry in `lib/agent/ui-actions`, then report the
   *  outcome back through `agent_report_action_result` so `ui_invoke_action` can
   *  answer the agent with the page's own result. `correlation_id` ties the two
   *  halves together; without it the tool could only say "emitted". */
  | {
      type: "invoke-action";
      id: string;
      params?: Record<string, unknown> | null;
      correlation_id: string;
    };

/**
 * Open the card editor on `dataset_uuid`.
 *
 * `prefill` seeds the fields of a card that does not exist yet; `card_uuid`
 * reopens one that does. A capture sets both `todo_uuid` and
 * `completes: "on_save"`, which is the only case where saving completes a
 * todo — see `tapCompletes` in `lib/agent/tasks`.
 */
export type OpenCardAction = {
  type: "open-card";
  dataset_uuid: string;
  dataset_name?: string;
  card_uuid?: string | null;
  prefill?: {
    question?: string;
    answer?: string;
    suggestion?: string;
    note?: string;
  } | null;
  /** The paragraph the selection sat in, when the card came from a capture. */
  context?: string | null;
  /** Where it came from, as one readable line. */
  source?: { display: string } | null;
  todo_uuid?: string | null;
  completes?: "on_save" | null;
};
