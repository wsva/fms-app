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
