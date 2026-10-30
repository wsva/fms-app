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
  /** Full family name as the status menu shows it, e.g. "Speech-to-text". */
  fullName: string;
  /** Active/selected model id, or null when nothing is present. */
  id: string | null;
  state: LoadedModelState;
  /** Human word for the status menu: "Loaded", "Loading…", "Available", "Not loaded", "Error".
   *  The menu states the state once — dot and word — and never repeats it in a
   *  second caption, so there is nothing else to derive here. */
  stateText: string;
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
    fullName: "Speech-to-text",
    // `selected_version` when nothing is active: it names the model that would
    // load, which is what the row wants beside "Available". With no model at
    // all there is nothing honest to show, so the row drops the id instead of
    // printing a name that is not on disk.
    id: state_ === "none" ? null : res.active_version || res.selected_version || null,
    state: state_,
    stateText,
  };
}

/** Map a `text_split_status` payload to a normalized `LoadedModel`. */
function mapTextSplit(res: TextSplitStatus): LoadedModel {
  const state_: LoadedModelState = res.loaded ? "loaded" : res.downloaded ? "available" : "none";
  const stateText = res.loaded ? "Loaded" : res.downloaded ? "Available" : "Not downloaded";
  return {
    type: "textsplit",
    fullName: "Text split",
    id: res.loaded || res.downloaded ? res.model_id : null,
    state: state_,
    stateText,
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

  /**
   * Run the one action a family offers. The menu decides which verb to show
   * (`actionLabel` in StatusBar.tsx); this only carries it out, and it refuses
   * the same states the menu does, so the two cannot drift into calling
   * `model_start` on a model that was never downloaded. text-split unloads its
   * cached session to free memory — it has no load verb, because it loads
   * itself on first use.
   */
  const runAction = async (type: string) => {
    if (!isTauri()) return;
    const m = models.find((x) => x.type === type);
    if (!m) return;
    try {
      if (type === "stt") {
        if (m.state === "loaded") {
          await invoke("model_stop");
        } else if (m.state === "available" || m.state === "error") {
          await invoke("model_start");
        } else {
          return; // nothing on disk to load, or a load already in flight
        }
      } else if (type === "textsplit") {
        if (m.state !== "loaded") return; // loads on first use — no manual action
        await invoke("text_split_unload");
      }
      await fetchStatus();
    } catch (e) {
      logError(`useLoadedModels: action for '${type}' failed: ${String(e)}`, "models");
    }
  };

  return { models, runAction, refresh: fetchStatus };
}
