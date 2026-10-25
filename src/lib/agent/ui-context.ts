// ---------------------------------------------------------------------------
// What the user is currently looking at, reported to the backend so an agent
// can answer "do something with what I'm on".
//
// Pages here are display-toggled rather than unmounted, and several of them own
// disjoint slices of the context (the shell knows the tab, Dictation knows the
// dataset/media/cue). So this is a *module singleton* that merges patches and
// forwards the delta, not a prop drilled down from the shell: any component can
// claim the keys it owns without the shell having to know about them.
//
// Wire contract (mirrors `src-tauri/src/ui_state.rs`):
//   key present with a value -> set
//   key present with `null`  -> clear
//   key absent               -> untouched
// ---------------------------------------------------------------------------

import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isAgentHost } from "./host";

/** The context keys the backend accepts. An unknown key is rejected there, so
 *  this interface is the compile-time half of that check. */
export interface UiContextPatch {
  /** Sidebar tab id, e.g. `"dictation"`. */
  active_tab?: string | null;
  active_dataset_uuid?: string | null;
  active_media_uuid?: string | null;
  /** Index of the cue the user is on within the open media item. */
  cue_index?: number | null;
  /** Highlighted text, when the page has a meaningful selection. */
  selection?: string | null;
  /** Scroll offsets the page considers worth reporting. */
  scroll?: Record<string, number> | null;
}

/** Merged local mirror of what the backend holds. */
let current: UiContextPatch = {};
/** Keys changed since the last flush — the only thing worth sending. */
let dirty: UiContextPatch = {};
let timer: ReturnType<typeof setTimeout> | null = null;

const DEBOUNCE_MS = 200;

/** The context as this window currently understands it. */
export function getUiContext(): UiContextPatch {
  return { ...current };
}

/** Merge `patch` into the reported context and schedule a flush. Passing `null`
 *  for a key clears it (the page no longer has that thing open). */
export function setUiContext(patch: UiContextPatch): void {
  for (const [key, value] of Object.entries(patch)) {
    // `undefined` would vanish from the JSON payload and read as "untouched" on
    // the other side, so it is normalised to an explicit clear.
    const v = value === undefined ? null : value;
    const k = key as keyof UiContextPatch;
    if (v === null) delete current[k];
    else (current as Record<string, unknown>)[key] = v;
    (dirty as Record<string, unknown>)[key] = v;
  }
  scheduleFlush();
}

function scheduleFlush(): void {
  if (!isAgentHost() || timer) return;
  timer = setTimeout(flush, DEBOUNCE_MS);
}

function flush(): void {
  timer = null;
  const patch = dirty;
  dirty = {};
  if (!isAgentHost() || Object.keys(patch).length === 0) return;
  // Fire and forget: the backend cache is a convenience for agents, never a
  // source of truth the UI waits on.
  invoke("agent_report_ui_state", { patch }).catch(() => {});
}

/** Report `patch` whenever it changes by value. Convenience wrapper over
 *  {@link setUiContext} for components that would otherwise write the effect
 *  (and its dependency bookkeeping) by hand. */
export function useReportUiContext(patch: UiContextPatch): void {
  const key = JSON.stringify(patch);
  useEffect(() => {
    setUiContext(JSON.parse(key) as UiContextPatch);
  }, [key]);
}
