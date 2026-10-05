// ---------------------------------------------------------------------------
// Agent (Goose ACP) types — mirror the Rust payloads emitted by agent_acp.rs.
// All events are namespaced under `agent-*` and correlated by session_id.
// ---------------------------------------------------------------------------

import type { TabId } from "@/components/layout/Sidebar";

/** Payload of the `agent-status` event and the `agent_status`/`agent_connect`
 *  command results. */
export interface AgentStatus {
  connected: boolean;
  session_id?: string | null;
  /** Present on `agent_connect`: whether the fms-app MCP server was registered
   *  with the session (goose advertised http MCP capability). */
  mcp_registered?: boolean;
  /** The loopback MCP url offered to goose, when registered. */
  mcp_url?: string;
}

/** `agent-message-chunk` — a streamed agent text delta. */
export interface AgentMessageChunk {
  session_id: string;
  text: string;
}

/** `agent-thought` — a streamed agent reasoning/thinking delta. */
export interface AgentThought {
  session_id: string;
  text: string;
}

/** `agent-tool-call` — the agent started (or announced) a tool call. */
export interface AgentToolCall {
  session_id: string;
  id: string;
  title?: string | null;
  kind?: string | null;
  status?: string | null;
  raw_input?: unknown;
}

/** `agent-tool-update` — progress/result for a previously announced tool call. */
export interface AgentToolUpdate {
  session_id: string;
  id: string;
  status?: string | null;
  raw_output?: unknown;
}

/** A single entry of an `agent-plan` update. */
export interface AgentPlanEntry {
  content: string;
  priority?: string | null;
  status?: string | null;
}

/** `agent-plan` — the agent's current plan. */
export interface AgentPlan {
  session_id: string;
  entries: AgentPlanEntry[];
}

/** One selectable option in a permission request. */
export interface AgentPermissionOption {
  optionId: string;
  name?: string | null;
  kind?: string | null;
}

/** `agent-permission-request` — the agent asks to run a tool that our policy
 *  says needs human approval. Respond via `agent_respond_permission`. */
export interface AgentPermissionRequest {
  request_id: number;
  session_id: string;
  tool_call: Record<string, unknown>;
  options: AgentPermissionOption[];
}

/** `agent-done` — a prompt turn finished. */
export interface AgentDone {
  request_id: number;
  stop_reason?: string | null;
}

/** `agent-error` — a prompt turn failed. */
export interface AgentError {
  request_id: number;
  message: string;
}

/** `agent-update` — any other session/update we did not map explicitly. */
export interface AgentGenericUpdate {
  session_id: string;
  update_type: string;
  update: Record<string, unknown>;
}

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
  | { type: "notify"; message: string };
