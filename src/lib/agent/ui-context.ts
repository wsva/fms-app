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
  /**
   * The card on screen, while the card editor overlay is open. The editor is a
   * root above every tab rather than a page of its own, so without this key an
   * agent could not tell "this card" from "the deck the list happens to show".
   */
  active_card_uuid?: string | null;
  /**
   * The document the page has open, `<kind>:<uuid>:<ref>` — `wiki:<dataset>:
   * <rel>`, `book:<book>:<chapter>`, `read_aloud:<dataset>:<text>`. Cleared
   * when that page closes it. A capture reads it to say where it came from;
   * nothing splits it apart, so a `ref` may contain ':' freely.
   */
  active_doc?: string | null;
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

/**
 * Clear `active_doc` only when the value on record is still this page's own.
 *
 * Several document pages are mounted at once (they are display-toggled), so a
 * page that loses focus runs its effect in the same commit as the page that
 * gained it. Without this identity check the order of the two effects would
 * decide whether the newly opened document survives, and it would sometimes be
 * wiped by the page that just went off screen.
 */
export function clearDocIfOwned(kind: string): void {
  const cur = current.active_doc;
  if (!cur || cur.startsWith(`${kind}:`)) setUiContext({ active_doc: null });
}

/**
 * Clear `active_dataset_uuid` only while it still names `uuid`.
 *
 * Same race as {@link clearDocIfOwned}, for the key Dictation and the card
 * editor already share: a page must not clear a dataset that another page has
 * since claimed.
 */
export function clearDatasetUuid(uuid: string | null | undefined): void {
  if (!uuid) return;
  if (current.active_dataset_uuid === uuid) {
    setUiContext({ active_dataset_uuid: null });
  }
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
