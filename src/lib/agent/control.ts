"use client";

// ---------------------------------------------------------------------------
// Agent control plane
// ---------------------------------------------------------------------------
// One shared snapshot behind the Agent page (Tools → Agent): the generated
// manual that tells an agent how to attach, and the two app-side switches that
// decide whether the MCP surface answers at all.
//
// Everything here is read back from the backend rather than derived: the port is
// the one the web service actually bound, the tool count comes from the live
// router, and the copy payloads are the strings `/agent.md` serves. Two sections
// of one page read this object (Attach an agent, Agent surface), so a copy or a
// save made on one is visible on the other.

import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isMobileApp, isTauri } from "@/lib/platform";
import { logError } from "@/lib/logger";

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
  /** Whether the dock's floating Bot button is drawn. Off by default — opt-in
   *  chrome, not part of the agent surface, but it is the dock's own switch so
   *  it belongs beside the other two. */
  agent_fab_visible: boolean;
  raw: Record<string, unknown>;
}

/** Fire-and-forget notice that a Surface setting changed, so surfaces that keep
 *  their own copy of the settings (Settings) reload instead of writing back a
 *  stale `mcp_enabled` on their next save. */
export const AGENT_SURFACE_SAVED = "agent-surface-saved";

interface AgentState {
  manual: AgentManual | null;
  manualLoading: boolean;
  note: AgentNote | null;
  copied: string | null;
  surface: AgentSurface | null;
  saving: boolean;
}

const initial: AgentState = {
  manual: null,
  manualLoading: false,
  note: null,
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

// ---- Reads -----------------------------------------------------------------

/** Reload the manual. Idempotent; safe to call often. */
export async function refreshAgent(): Promise<void> {
  if (!isTauri() || isMobileApp()) return;
  set({ manualLoading: true });
  try {
    const manual = await invoke<AgentManual>("agent_manual_get");
    set({ manual, manualLoading: false });
  } catch (e) {
    set({ manualLoading: false });
    logError(`agent: reading the agent manual failed — ${reason(e)}`, "settings");
  }
}

let loaded = false;

/** First read only — the page remounts on every tab switch, and re-reading a
 *  static payload each time would just be noise. */
export async function ensureAgentLoaded(): Promise<void> {
  if (loaded) return;
  loaded = true;
  await refreshAgent();
}

/** Read the global settings the Surface panel edits, *and* the switch the dock
 *  draws its floating button from. Called once on load and again whenever the
 *  backend says the settings changed. */
async function readSurface(): Promise<void> {
  try {
    const raw = await invoke<Record<string, unknown>>("settings_get_global");
    set({
      surface: {
        mcp_policy: String(raw.mcp_policy ?? "permissive"),
        mcp_enabled: raw.mcp_enabled !== false,
        // Absent reads as off: `#[serde(default)]` on the Rust side, and the
        // button is meant to be opt-in, so silence must mean "no button".
        agent_fab_visible: raw.agent_fab_visible === true,
        raw,
      },
    });
  } catch (e) {
    logError(`agent: reading the global settings failed — ${reason(e)}`, "settings");
  }
}

let watchingSettings = false;

/** Mirror writes made elsewhere into this store: the Settings page's Save, or an
 *  agent's `settings_set`. Without it the dock would keep drawing the button
 *  after someone switched it off, which reads as the checkbox being ignored. */
function watchSettingsChanges(): void {
  if (watchingSettings) return;
  watchingSettings = true;
  void listen("settings-changed", () => {
    void readSurface();
  }).catch((e) => {
    logError(`agent: watching the settings failed — ${reason(e)}`, "settings");
  });
}

/** Read the global settings the Surface panel edits (once, then on every
 *  `settings-changed` event). */
export async function ensureSurfaceLoaded(): Promise<void> {
  if (!isTauri()) return;
  watchSettingsChanges();
  if (state.surface) return;
  await readSurface();
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

/** Save one or more Surface fields. `mcp_enabled` is baked into the router, so
 *  the save restarts the local web service; the manual is re-read afterwards
 *  because the status line should report what the service now serves, not the
 *  field we just wrote. */
export async function saveSurface(
  patch: Partial<Pick<AgentSurface, "mcp_policy" | "mcp_enabled" | "agent_fab_visible">>
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
