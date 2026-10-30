// ---------------------------------------------------------------------------
// The text-selection context menu's on/off switch
// ---------------------------------------------------------------------------
// One preference, two readers: the menu itself (`CardContextMenu`, mounted once
// in the shell) and the status bar's toggle. It has to survive a restart, so it
// lives in localStorage; it has to reach two components with no common parent,
// so it is a module-level store read through `useSyncExternalStore` — the shape
// `useLoadedModels` uses for the same reason.
//
// Off by default: the menu appears over text the user is in the middle of
// reading, which is an interruption until they ask for it. Android is the
// exception — there is no status bar there, and a preference whose switch is
// not on screen must not silently take a feature away, so an absent stored
// choice means "on" on a phone.
// ---------------------------------------------------------------------------

import { useSyncExternalStore } from "react";
import { isMobileApp } from "@/lib/platform";

const STORAGE_KEY = "cards.context-menu";

let enabled = false;
let hydrated = false;
const subscribers = new Set<() => void>();

function notify() {
  subscribers.forEach((fn) => fn());
}

/**
 * Read the stored choice, once. An absent entry is "never chosen", so the
 * per-platform default applies and nothing is written — that keeps the default
 * free to change later (the rule the dictation page's collapsed flag follows).
 */
function hydrate() {
  if (hydrated || typeof window === "undefined") return;
  hydrated = true;
  let stored: string | null = null;
  try {
    stored = localStorage.getItem(STORAGE_KEY);
  } catch {
    /* private mode / storage disabled */
  }
  const next = stored === null ? isMobileApp() : stored === "1";
  if (next !== enabled) {
    enabled = next;
    notify();
  }
}

/** The choice as it stands right now, for a click handler or an agent action
 *  that wants the other value. */
export function getContextMenuEnabled(): boolean {
  return enabled;
}

function subscribe(fn: () => void) {
  subscribers.add(fn);
  // Hydration notifies the set this subscriber just joined, so the first paint
  // (which had to assume the default) is corrected without a second effect.
  hydrate();
  return () => {
    subscribers.delete(fn);
  };
}

/** Whether the menu is offered. Matches the server render until hydration. */
export function useContextMenuEnabled(): boolean {
  return useSyncExternalStore(subscribe, getContextMenuEnabled, () => false);
}

/** Set it, remember it, and tell both readers. */
export function setContextMenuEnabled(on: boolean) {
  hydrate();
  try {
    localStorage.setItem(STORAGE_KEY, on ? "1" : "0");
  } catch {
    /* private mode / storage disabled — the choice just will not survive */
  }
  if (enabled === on) return;
  enabled = on;
  notify();
}
