/**
 * Aggregated "which models are loaded" hook for the status bar.
 *
 * One shared 3s poll feeds every subscriber (same pattern as `useModelStatus`):
 * module-level state + `useSyncExternalStore`, refreshed immediately on the
 * `model-status-changed` event. It composes the per-family status commands —
 * STT (`model_get_status`, all platforms) and text-split (`text_split_status`,
 * desktop only) — into one normalized list so a future model type is a single
 * extra fetch + mapping branch here, with no change to the status bar layout.
 */

import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import { logError } from "@/lib/logger";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

export type LoadedModelState = "loaded" | "loading" | "available" | "none" | "error";

export interface LoadedModel {
  /** Stable family key: "stt" | "textsplit" | ... */
  type: string;
  /** Short chip label, e.g. "STT", "Split". */
  label: string;
  /** Full family name for the drawer, e.g. "Speech-to-text". */
  fullName: string;
  /** Active/selected model id, or null when nothing is present. */
  id: string | null;
  state: LoadedModelState;
  /** Human word for the drawer: "Loaded", "Loading…", "Available", "Not loaded", "Error". */
  stateText: string;
  /** Caption shown under the name in the drawer. */
  caption: string;
}

// ---------------------------------------------------------------------------
// Module-level state & subscription
// ---------------------------------------------------------------------------

interface SttStatus {
  active_version: string | null;
  active_status: string | { Error: string };
  selected_version: string;
}

interface TextSplitStatus {
  downloaded: boolean;
  loaded: boolean;
  model_id: string;
}

let state: LoadedModel[] = [];
const subscribers = new Set<() => void>();

function notify() {
  subscribers.forEach((fn) => fn());
}

/** Map an STT `model_get_status` payload to a normalized `LoadedModel`. */
function mapStt(res: SttStatus): LoadedModel {
  const status = res.active_status;
  const id = res.active_version ?? res.selected_version ?? null;
  let state_: LoadedModelState = "none";
  let stateText = "Not loaded";

  if (typeof status === "object" && status && "Error" in status) {
    state_ = "error";
    stateText = "Error";
  } else if (status === "Running") {
    state_ = "loaded";
    stateText = "Loaded";
  } else if (status === "Downloading") {
    state_ = "loading";
    stateText = "Loading…";
  } else if (status === "Downloaded" || status === "Stopped") {
    state_ = "available";
    stateText = "Available";
  }

  return {
    type: "stt",
    label: "STT",
    fullName: "Speech-to-text",
    id: res.active_version ?? null,
    state: state_,
    stateText,
    caption: res.active_version ? "Loaded in memory" : id ? "Downloaded, not loaded" : "No model downloaded",
  };
}

/** Map a `text_split_status` payload to a normalized `LoadedModel`. */
function mapTextSplit(res: TextSplitStatus): LoadedModel {
  const state_: LoadedModelState = res.loaded ? "loaded" : res.downloaded ? "available" : "none";
  const stateText = res.loaded ? "Loaded" : res.downloaded ? "Available" : "Not downloaded";
  return {
    type: "textsplit",
    label: "Split",
    fullName: "Text split",
    id: res.loaded || res.downloaded ? res.model_id : null,
    state: state_,
    stateText,
    caption: res.loaded
      ? "Resident in memory"
      : res.downloaded
        ? "Loads on first use"
        : "No model downloaded",
  };
}

async function fetchStatus() {
  if (!isTauri()) return;
  const models: LoadedModel[] = [];

  try {
    const stt = await invoke<SttStatus>("model_get_status");
    models.push(mapStt(stt));
  } catch (e) {
    logError(`useLoadedModels: model_get_status failed: ${String(e)}`, "models");
  }

  // `text_split_status` only exists in the desktop binary (the `textsplit`
  // cargo feature is off on the Android thin client).
  if (!isMobileApp()) {
    try {
      const ts = await invoke<TextSplitStatus>("text_split_status");
      models.push(mapTextSplit(ts));
    } catch (e) {
      logError(`useLoadedModels: text_split_status failed: ${String(e)}`, "models");
    }
  }

  // Cheap structural compare so we only re-render on a real change.
  if (JSON.stringify(models) !== JSON.stringify(state)) {
    state = models;
    notify();
  }
}

// Start polling once at module load.
let pollingStarted = false;
function ensurePolling() {
  if (pollingStarted) return;
  pollingStarted = true;
  fetchStatus();
  setInterval(fetchStatus, 3000);

  if (isTauri()) {
    listen("model-status-changed", () => fetchStatus());
  }
}

function subscribe(fn: () => void) {
  subscribers.add(fn);
  ensurePolling();
  return () => subscribers.delete(fn);
}

function getSnapshot() {
  return state;
}

// ---------------------------------------------------------------------------
// Public hook
// ---------------------------------------------------------------------------

export function useLoadedModels() {
  const models = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  /** Models worth a compact chip: loaded or in progress. */
  const active = models.filter((m) => m.state === "loaded" || m.state === "loading" || m.state === "error");
  /** Downloaded-but-idle models, summarized as "+N" in the bar. */
  const idleCount = models.filter((m) => m.state === "available").length;

  /**
   * Run the drawer action for a family: STT toggles load/stop; text-split
   * unloads its cached session to free memory (it auto-loads again on demand,
   * so there is nothing to do when it is not loaded).
   */
  const runAction = async (type: string) => {
    if (!isTauri()) return;
    const m = models.find((x) => x.type === type);
    if (!m) return;
    try {
      if (type === "stt") {
        if (m.state === "loaded") {
          await invoke("model_stop");
        } else {
          await invoke("model_start");
        }
      } else if (type === "textsplit") {
        if (m.state === "loaded") {
          await invoke("text_split_unload");
        } else {
          return; // loads on first use — no manual action
        }
      }
      await fetchStatus();
    } catch (e) {
      logError(`useLoadedModels: action for '${type}' failed: ${String(e)}`, "models");
    }
  };

  return { models, active, idleCount, runAction, refresh: fetchStatus };
}
