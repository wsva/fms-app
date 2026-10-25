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
  /** The host record the live session belongs to (null before records existed). */
  host_id?: string | null;
  /** That record's display name, for the dock header and the Connect card. */
  display_name?: string | null;
  /** Which carrier the session runs on: a websocket and a spawned child are
   *  different trust propositions, so the UI always says which one it is. */
  transport?: AgentTransportKind | null;
  /** Present on `agent_connect`: whether the fms-app MCP server was registered
   *  with the session (the agent advertised http MCP capability). */
  mcp_registered?: boolean;
  /** The loopback MCP url offered to the agent, when registered. */
  mcp_url?: string;
  /** `http` when the agent took the url, `stdio` when it was handed the bridge. */
  mcp_transport?: string | null;
}

export type AgentTransportKind = "ws" | "stdio";

/** One agent host record (`AppSettings::agent_hosts`), plus the app's own dial
 *  rule. `needs_confirmation` is decided in Rust — never re-derived here — so the
 *  Connect card cannot disagree with the dialler. */
export interface AgentHost {
  id: string;
  display_name: string;
  enabled: boolean;
  kind: AgentTransportKind;
  /** ws: `host:port`; stdio: the program. Safe to show: no credentials. */
  endpoint: string;
  /** stdio always, ws only when it is not loopback. */
  needs_confirmation: boolean;
  /** The user has already answered the confirm dialog this session. */
  approved: boolean;
  last_session?: string | null;
}

/** `agent_host_list` — the records plus the live session folded in. */
export interface AgentHostList {
  hosts: AgentHost[];
  /** The record `agent_host_connect` dials when asked without an id. */
  active_id: string;
  connected: boolean;
  session?: AgentStatus;
}

/** `agent-confirm-request` payload / `agent_confirm_request` result: the host the
 *  user is being asked to approve, and why. */
export interface AgentConfirmRequest {
  host_id: string;
  display_name: string;
  kind: AgentTransportKind;
  endpoint: string;
  reason: string;
}

/** `agent-log` — one stderr line from a spawned ACP host, forwarded so a stdio
 *  host that dies is visible in the dock instead of failing silently. */
export interface AgentLog {
  source: string;
  text: string;
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
