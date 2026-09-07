/**
 * Shared model status hook — polls the Rust backend every 3 seconds
 * and provides start/stop toggle. Uses module-level state so all
 * subscribers share a single polling interval.
 */

import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/tauri";

// ---------------------------------------------------------------------------
// Module-level state & subscription
// ---------------------------------------------------------------------------

interface ModelStatusState {
  activeVersion: string | null;
  activeStatus: string;
}

let state: ModelStatusState = { activeVersion: null, activeStatus: "Stopped" };
const subscribers = new Set<() => void>();

function notify() {
  subscribers.forEach((fn) => fn());
}

async function fetchStatus() {
  if (!isTauri()) return;
  try {
    const res = await invoke<{
      active_version: string | null;
      active_status: string;
      selected_version: string;
    }>("model_get_status");
    if (res.active_version !== state.activeVersion || res.active_status !== state.activeStatus) {
      state = { activeVersion: res.active_version, activeStatus: res.active_status };
      notify();
    }
  } catch {
    // silently ignore
  }
}

// Start polling once at module load
let pollingStarted = false;
function ensurePolling() {
  if (pollingStarted) return;
  pollingStarted = true;
  fetchStatus();
  setInterval(fetchStatus, 3000);
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

export function useModelStatus() {
  const { activeVersion, activeStatus } = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  const toggle = async () => {
    if (!isTauri()) return;
    try {
      if (activeVersion) {
        await invoke("model_stop");
      } else {
        await invoke("model_start");
      }
      await fetchStatus();
    } catch (e) {
      console.error("Model toggle failed:", e);
    }
  };

  return {
    activeModel: activeVersion,
    modelStatus: activeStatus,
    isRunning: activeStatus === "Running",
    toggle,
    refresh: fetchStatus,
  };
}
