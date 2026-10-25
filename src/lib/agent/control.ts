"use client";

// ---------------------------------------------------------------------------
// Agent control plane
// ---------------------------------------------------------------------------
// One shared snapshot behind the two surfaces that configure or approve agent
// hosting: the Agent page (Tools → Agent) and the dock, which is mounted on
// every desktop screen. The approval ask is why this is not page state: an
// agent calling `agent_host_connect` over MCP — or the dock's own auto-connect —
// raises it while the user may be on any tab, and a request that only exists
// inside a page the user is not looking at is a dead end for the caller. So the
// listener lives at module level and both surfaces read the same object;
// answering in either one clears it in both.
//
// Everything here is read back from the backend rather than derived: the port is
// the one the web service actually bound, the tool count comes from the live
// router, the copy payloads are the strings `/agent.md` serves, and the dial
// rule (`needs_confirmation`) is decided in Rust.

import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isMobileApp, isTauri } from "@/lib/platform";
import { logError } from "@/lib/logger";
import type {
  AgentConfirmRequest,
  AgentHostList,
  AgentStatus,
} from "@/lib/agent/types";

/** The generated agent manual (`agent_manual_get` → `mcp/manual.rs::render_json`).
 *  Only the fields the Connect panel renders are typed; the body carries much
 *  more (the full tool index, house rules, workflow templates) for `/agent.json`. */
export interface AgentManual {
  endpoints: {
    running: boolean;
    port: number;
    mcp: string;
    manual_markdown: string;
    note: string;
  };
  trust: {
    zone: string;
    policy: { mode: string; tiers: string[] };
  };
  tools: { count: number };
  connect: {
    pointer_sentence: string;
    pointer_when: string;
    inline_block: string;
    inline_when: string;
    never: string;
    snippets: { product: string; text: string; note?: string }[];
  };
}

/** Inline outcome of the last action, shown next to the control that started it. */
export type AgentNote = { kind: "ok" | "err" | "busy"; text: string };

/** The `AppSettings` fields the Surface panel edits, plus the untouched rest of
 *  the payload: `settings_set_global` replaces the whole object, so a write that
 *  only knew these fields would reset everything else. */
export interface AgentSurface {
  mcp_policy: string;
  mcp_enabled: boolean;
  goose_acp_enabled: boolean;
  goose_acp_url: string;
  raw: Record<string, unknown>;
}

/** Fire-and-forget notice that a Surface setting changed, so surfaces that keep
 *  their own copy of the settings (Settings, the dock) reload instead of
 *  writing back a stale `mcp_enabled` on their next save. */
export const AGENT_SURFACE_SAVED = "agent-surface-saved";

interface AgentState {
  manual: AgentManual | null;
  manualLoading: boolean;
  hosts: AgentHostList | null;
  note: AgentNote | null;
  confirm: AgentConfirmRequest | null;
  copied: string | null;
  surface: AgentSurface | null;
  saving: boolean;
}

const initial: AgentState = {
  manual: null,
  manualLoading: false,
  hosts: null,
  note: null,
  confirm: null,
  copied: null,
  surface: null,
  saving: false,
};

let state: AgentState = initial;
const subscribers = new Set<() => void>();

function set(patch: Partial<AgentState>) {
  state = { ...state, ...patch };
  subscribers.forEach((fn) => fn());
}

function subscribe(fn: () => void) {
  subscribers.add(fn);
  watchConfirms();
  return () => {
    subscribers.delete(fn);
  };
}

const getSnapshot = () => state;

/** The current snapshot. Re-renders whenever any action below changes it. */
export function useAgentControl(): AgentState {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

const reason = (e: unknown) => (e instanceof Error ? e.message : String(e));

// ---- The approval ask ------------------------------------------------------

let watching = false;

/** Register the `agent-confirm-request` listener once, on the first subscriber.
 *  The dock is always mounted on desktop, so in practice this fires at shell
 *  boot — before any page that would render the card exists. */
function watchConfirms() {
  if (watching || !isTauri() || isMobileApp()) return;
  watching = true;
  listen<AgentConfirmRequest>("agent-confirm-request", (e) => {
    set({
      confirm: e.payload,
      note: {
        kind: "busy",
        text: `Waiting for your approval of ${e.payload.display_name}.`,
      },
    });
  }).catch((e) => {
    watching = false;
    logError(`agent: listening for approval requests failed — ${reason(e)}`, "settings");
  });
}

// ---- Reads -----------------------------------------------------------------

/** Reload the manual and the host records. Idempotent; safe to call often. */
export async function refreshAgent(): Promise<void> {
  if (!isTauri() || isMobileApp()) return;
  set({ manualLoading: true });
  try {
    const [manual, hosts] = await Promise.all([
      invoke<AgentManual>("agent_manual_get"),
      // The dock half of the status line: which host records exist, which one
      // is active, and whether a session is live right now.
      invoke<AgentHostList>("agent_host_list").catch(() => null),
    ]);
    set({ manual, hosts, manualLoading: false });
  } catch (e) {
    set({ manualLoading: false });
    logError(`agent: reading the agent manual failed — ${reason(e)}`, "settings");
  }
}

let loaded = false;

/** First read only — the dock and the page both mount at shell boot, and two
 *  identical reads of a static payload would just be noise. */
export async function ensureAgentLoaded(): Promise<void> {
  if (loaded) return;
  loaded = true;
  await refreshAgent();
}

/** Read the global settings the Surface panel edits (once). */
export async function ensureSurfaceLoaded(): Promise<void> {
  if (state.surface || !isTauri()) return;
  try {
    const raw = await invoke<Record<string, unknown>>("settings_get_global");
    set({
      surface: {
        mcp_policy: String(raw.mcp_policy ?? "permissive"),
        mcp_enabled: raw.mcp_enabled !== false,
        goose_acp_enabled: raw.goose_acp_enabled === true,
        goose_acp_url: String(raw.goose_acp_url ?? ""),
        raw,
      },
    });
  } catch (e) {
    logError(`agent: reading the global settings failed — ${reason(e)}`, "settings");
  }
}

// ---- Writes ----------------------------------------------------------------

/** Copy-only, by design: FmS never writes another product's config file (see
 *  `manual.connect.never`). The key names the button so only it flips to
 *  "Copied". */
export async function copyAgentText(key: string, text: string): Promise<void> {
  if (!text) return;
  try {
    await navigator.clipboard.writeText(text);
    set({ copied: key });
    setTimeout(() => {
      if (state.copied === key) set({ copied: null });
    }, 2000);
  } catch (e) {
    logError(`agent: copying to the clipboard failed — ${reason(e)}`, "settings");
    set({ copied: null });
  }
}

/** Probe the dock's ACP host and report the outcome inline. `agent_connect` is
 *  idempotent, so this doubles as a status refresh when already connected. */
export async function testDockConnection(): Promise<void> {
  if (!isTauri()) return;
  set({ note: { kind: "busy", text: "Connecting…" } });
  try {
    const status = await invoke<AgentStatus>("agent_connect");
    set({
      note: status.connected
        ? {
            kind: "ok",
            text: `Connected (session ${status.session_id ?? "?"})${
              status.mcp_registered ? " · FmS tools registered" : ""
            }.`,
          }
        : { kind: "err", text: "Not connected." },
    });
    await refreshAgent();
  } catch (e) {
    set({ note: { kind: "err", text: reason(e) } });
  }
}

/** The "Shown in" switch. Two commands, one decision: a host the Rust trust rule
 *  marks `needs_confirmation` is asked about first (the approval card), and the
 *  approval it records is what unblocks the connect. No address is accepted
 *  here — the records are the only dial targets. */
export async function showInDock(hostId: string): Promise<void> {
  if (!isTauri()) return;
  set({ note: { kind: "busy", text: hostId ? "Connecting…" : "Disconnecting…" } });
  try {
    if (!hostId) {
      await invoke("agent_host_disconnect");
      set({ confirm: null, note: { kind: "ok", text: "No agent hosted — FmS is on its own." } });
      await refreshAgent();
      return;
    }
    const host = state.hosts?.hosts.find((h) => h.id === hostId);
    if (host?.needs_confirmation && !host.approved) {
      const req = await invoke<{ state: string; host?: AgentConfirmRequest }>(
        "agent_confirm_request",
        { id: hostId }
      );
      if (req.state === "pending" && req.host) {
        set({ confirm: req.host, note: { kind: "busy", text: "Waiting for your approval…" } });
        return;
      }
    }
    await connectHost(hostId);
  } catch (e) {
    set({ note: { kind: "err", text: reason(e) } });
  }
}

async function connectHost(hostId: string): Promise<void> {
  const status = await invoke<AgentStatus>("agent_host_connect", { id: hostId });
  set({
    note: {
      kind: status.connected ? "ok" : "err",
      text: status.connected
        ? `Hosting ${status.display_name ?? hostId} over ${status.transport ?? "?"} — session ${
            status.session_id ?? "?"
          }${status.mcp_transport ? `, FmS tools over ${status.mcp_transport}` : ""}.`
        : "Not connected.",
    },
  });
  await refreshAgent();
}

/** Answer a pending approval. Shared by the page and the dock, so the same click
 *  is available from whichever one the user is looking at. */
export async function answerHostConfirm(approve: boolean): Promise<void> {
  const host = state.confirm;
  if (!host) return;
  set({ confirm: null, note: { kind: "busy", text: approve ? "Connecting…" : "Cancelling…" } });
  try {
    await invoke("agent_confirm_respond", { hostId: host.host_id, approve });
    if (!approve) {
      set({ note: { kind: "err", text: `${host.display_name} stays unhosted.` } });
      await refreshAgent();
      return;
    }
    await connectHost(host.host_id);
  } catch (e) {
    set({ note: { kind: "err", text: reason(e) } });
  }
}

/** Save one or more Surface fields. `mcp_enabled` is baked into the router, so
 *  the save restarts the local web service; the manual is re-read afterwards
 *  because the status line should report what the service now serves, not the
 *  field we just wrote. */
export async function saveSurface(
  patch: Partial<Pick<AgentSurface, "mcp_policy" | "mcp_enabled" | "goose_acp_enabled">>
): Promise<boolean> {
  if (!isTauri() || !state.surface) return false;
  set({ saving: true });
  try {
    const raw = { ...state.surface.raw, ...patch };
    await invoke("settings_set_global", { global: raw });
    set({ surface: { ...state.surface, ...patch, raw }, saving: false });
    window.dispatchEvent(new Event(AGENT_SURFACE_SAVED));
    await refreshAgent();
    return true;
  } catch (e) {
    set({ saving: false, note: { kind: "err", text: reason(e) } });
    return false;
  }
}
