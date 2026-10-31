"use client";

// ---------------------------------------------------------------------------
// One refresh mechanism for every page.
//
// The shell (`app/(app)/home/page.tsx`) keeps every page mounted and toggles it
// with `display`, so a page loads once at startup and then goes stale forever.
// Ten toolbars used to carry a "Refresh" button to paper over that; Dictation
// was the only page that re-read on activation. This module is that one correct
// behaviour, made shared:
//
//   activate  the page became the visible tab (a bottom-nav tap on a phone)
//   resume    the app/window came back to the foreground
//   event     a backend event that invalidates what the page shows
//   manual    F5 / Ctrl+R, or the `shell.refresh` agent action
//
// A page registers a closure that re-reads what it shows, plus a guard that
// returns false while unsaved edits would be destroyed. The registry is keyed
// on the tab id, exactly like `lib/agent/ui-actions.ts`, and for the same
// reason: pages are display-toggled rather than unmounted, so registrations
// survive navigation and the map means "everything the app can re-read".
//
// Refresh closures must only re-read. They must never navigate or clear a
// selection — selections are stored by uuid and resolved from the list, which
// is what keeps a re-read a no-op for anything the user is in the middle of.
//
// A refresh must also never blank what it is refreshing. Flipping the page's own
// `loading` flag on a visibility-triggered re-read is the usual way to make one
// destructive: the content unmounts, the pane's scroll offset is clamped away
// with it, and the page returns at the top. Show a spinner only when there is
// nothing on screen yet (`useBookData.refresh` is the worked example).
// ---------------------------------------------------------------------------

import { useCallback, useEffect, useRef, useSyncExternalStore } from "react";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "@/lib/tauri";
import { logError } from "@/lib/logger";

/** Why a refresh was requested. Mirrors the trigger table in the design note. */
export type RefreshReason = "activate" | "resume" | "manual" | "event";

export interface PageRefreshOptions {
  /** Re-read everything this view shows. Idempotent; never navigates. */
  refresh: () => void | Promise<void>;
  /**
   * False while a re-read would discard unsaved edits (a wiki draft, a pending
   * book reorder, half-typed cues). A manual request then comes back as
   * `blocked` so the shell can say why nothing happened.
   */
  canRefresh?: () => boolean;
  /** Backend events that invalidate this page, e.g. `["dataset-list-changed"]`. */
  events?: string[];
}

export interface RefreshOutcome {
  /** `ok` ran, `blocked` was refused by `canRefresh`, `none` had no page. */
  status: "ok" | "blocked" | "none";
  /** The tab the request resolved to, or null when none was active. */
  tab: string | null;
}

type Registration = {
  /** Taught by `usePageRefresh`; runs the page's current closure. */
  run: (reason: RefreshReason) => Promise<RefreshOutcome>;
  /**
   * The page's live "a re-read would discard unsaved edits" guard, read through a
   * ref so it never goes stale. The tab strip consults this before replacing a
   * tab (docs/design/tab-strip.md §4.4): a dirty page is auto-pinned, never
   * unmounted. Absent guard means "always clean".
   */
  canRefresh: () => boolean;
};

/** At most one registration per tab — a page mounts once and stays mounted. */
const registry = new Map<string, Registration>();

/**
 * The visible tab. Held at module scope because two consumers have no React
 * context to read: `refreshActivePage` (a keydown handler, and the agent action
 * that mirrors it) and the pages themselves, which subscribe through
 * `useSyncExternalStore` rather than a provider — the shell renders them as
 * plain siblings, so a provider would mean re-indenting the whole shell for a
 * one-boolean channel.
 */
let currentTab: string | null = null;
const tabSubscribers = new Set<() => void>();

/** A burst of app-resume signals (focus + visibilitychange) is one refresh. */
const MIN_RESUME_INTERVAL_MS = 2_000;

/** One mutation emits `dataset-list-changed` more than once; coalesce. */
const EVENT_DEBOUNCE_MS = 400;

/**
 * The guard a root overlay installs while it is open — see `setOverlayGuard`.
 * Held beside `currentTab` rather than in the registry because an overlay is not
 * a tab: the card editor sits above whichever tab the shell is showing (it is
 * reached from the wiki's text-selection menu as much as from the Cards list),
 * so it belongs to no registration and to *all* of them at once.
 */
let overlayGuard: (() => boolean) | null = null;

/**
 * Let an overlay over the shell participate in the dirty-page policy.
 *
 * `openTab` reads `canRefreshInstance(focused)` to decide whether to replace the
 * focused tab, and `refreshActivePage` reads it to decide whether F5 is safe.
 * Both ask the *page* — but a page whose editor lives above it in a root overlay
 * is not where the unsaved work is. An overlay that can lose work installs a
 * predicate here; every answer then ANDs it in, so a dirty editor over a clean
 * tab blocks the replace and the refresh like a dirty tab would.
 *
 * Pass `null` on unmount. The predicate must read through a ref: it is called
 * from event handlers, long after the render that installed it.
 */
export function setOverlayGuard(fn: (() => boolean) | null): void {
  overlayGuard = fn;
}

/** Is whatever is on screen safe to re-read? False only when an overlay says so. */
function overlayIsClean(): boolean {
  return overlayGuard ? overlayGuard() : true;
}

/**
 * Tell the module which tab the shell is showing. Called by the shell during
 * render — before its page children render — so a page resolving `active` in
 * the same pass sees the value that is about to be committed.
 */
export function useActiveTabSource(tab: string): void {
  if (currentTab !== tab) {
    currentTab = tab;
    // The pages re-render right after this call and read the new value, so the
    // notification is only a backstop for a subscriber React chose to skip. It
    // is deferred for that reason: notifying synchronously would be a React
    // update scheduled from inside another component's render.
    queueMicrotask(() => {
      if (currentTab !== tab) return;
      tabSubscribers.forEach((notify) => notify());
    });
  }
}

function subscribeTab(notify: () => void): () => void {
  tabSubscribers.add(notify);
  return () => {
    tabSubscribers.delete(notify);
  };
}

/**
 * Is this tab the one on screen? For a page that needs the fact for its own
 * reason — `DatasetsSyncPage` and `DevicesHubPage` gate a slow poll on it —
 * rather than for refreshing.
 */
export function useIsActiveTab(tab: string): boolean {
  const getSnapshot = useCallback(() => currentTab === tab, [tab]);
  return useSyncExternalStore(subscribeTab, getSnapshot, () => false);
}

/**
 * Which tab the shell is showing, for a caller outside React. The card editor is
 * a root overlay and has to know what it is drawn over — both to decorate the
 * right tab chip and to answer the agent's "which card is on screen" against the
 * page it is actually on.
 */
export function getActiveTabId(): string | null {
  return currentTab;
}

/**
 * Keep this page's data current for as long as it is mounted.
 *
 * The options object is rebuilt on every render (its closures capture state),
 * so it is held in a ref and the registration is keyed on the tab alone —
 * re-registering per render would be pure churn and would not change what the
 * dispatcher runs.
 */
export function usePageRefresh(tab: string, options: PageRefreshOptions): void {
  const latest = useRef(options);
  latest.current = options;

  const active = useIsActiveTab(tab);
  const activeRef = useRef(active);
  activeRef.current = active;

  // Last re-read time, for the resume throttle. Seeded with the mount time, not
  // zero: a window that opens focused fires `focus` immediately, and the page
  // has just loaded itself.
  const lastAt = useRef(Date.now());

  const run = useCallback(
    async (reason: RefreshReason): Promise<RefreshOutcome> => {
      const opts = latest.current;
      if (opts.canRefresh && !opts.canRefresh()) {
        return { status: "blocked", tab };
      }
      try {
        await opts.refresh();
      } catch (err) {
        // A failed re-read must not become an unhandled rejection: the page
        // still shows its previous list, and the next trigger tries again.
        logError(`page refresh (${reason}) failed on '${tab}': ${String(err)}`, "refresh");
      }
      lastAt.current = Date.now();
      return { status: "ok", tab };
    },
    [tab]
  );

  // Publish this page so `refreshActivePage` (F5, `shell.refresh`) can reach it.
  useEffect(() => {
    const registration: Registration = {
      run,
      // Read through `latest` so the guard reflects this render's state, not the
      // closure captured when the registration was first made.
      canRefresh: () => latest.current.canRefresh?.() ?? true,
    };
    registry.set(tab, registration);
    return () => {
      // Identity check, not id check: a second mount of the same tab (strict
      // mode) must not have its registration dropped by the first cleanup.
      if (registry.get(tab) === registration) registry.delete(tab);
    };
  }, [tab, run]);

  // Become visible → re-read. Mount is skipped: the page just loaded itself.
  const wasActive = useRef(active);
  useEffect(() => {
    if (active && !wasActive.current) void run("activate");
    wasActive.current = active;
  }, [active, run]);

  // Back to the foreground → re-read. Both signals, because Android WebView
  // does not reliably fire `window.focus` and desktop does not always fire
  // `visibilitychange` (see `lib/sync/autoFlush.ts`, which listens for both).
  useEffect(() => {
    const resume = () => {
      if (!activeRef.current) return;
      if (Date.now() - lastAt.current < MIN_RESUME_INTERVAL_MS) return;
      void run("resume");
    };
    const onVisibility = () => {
      if (document.visibilityState === "visible") resume();
    };
    window.addEventListener("focus", resume);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.removeEventListener("focus", resume);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [run]);

  // Data changed elsewhere (another page, an agent over MCP) → re-read, but
  // only while this page is the one on screen; activation covers the rest.
  const eventKey = JSON.stringify(options.events ?? []);
  useEffect(() => {
    if (!isTauri()) return;
    const names = JSON.parse(eventKey) as string[];
    if (names.length === 0) return;

    let alive = true;
    const unlisteners: Array<() => void> = [];
    let timer: ReturnType<typeof setTimeout> | null = null;

    const fire = () => {
      if (!activeRef.current) return;
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => {
        timer = null;
        if (activeRef.current) void run("event");
      }, EVENT_DEBOUNCE_MS);
    };

    for (const name of names) {
      listen(name, fire)
        .then((unlisten) => {
          if (alive) unlisteners.push(unlisten);
          else unlisten();
        })
        .catch(() => {
          /* the bridge refused the subscription; activation still refreshes */
        });
    }

    return () => {
      alive = false;
      if (timer) clearTimeout(timer);
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [eventKey, run]);
}

/**
 * Re-read the page that is on screen. Used by the shell's F5 / Ctrl+R handler
 * and by `shell.refresh`, so a keyboard user and an agent share one path.
 */
export async function refreshActivePage(reason: RefreshReason): Promise<RefreshOutcome> {
  if (!currentTab) return { status: "none", tab: null };
  // An overlay's unsaved work outranks the page beneath it, whatever asked.
  if (!overlayIsClean()) return { status: "blocked", tab: currentTab };
  const registration = registry.get(currentTab);
  if (!registration) return { status: "none", tab: currentTab };
  return await registration.run(reason);
}

/**
 * Can the instance keyed by `id` be re-read without discarding unsaved edits?
 * `true` when nothing is registered (no page, so nothing to lose) or the page
 * declares no guard. The tab strip's dirty guard calls this before replacing a
 * tab: a `false` here means "auto-pin, do not unmount" (docs/design/tab-strip.md
 * §4.4). `id` is the same key `usePageRefresh` registered under — the focused
 * instance id, which equals the `TabId` for every singleton page.
 *
 * A root overlay over the instance is consulted as well: an open card editor
 * with unsaved text answers `false` for *whichever* tab it is drawn on, which is
 * what auto-pins that tab instead of replacing it.
 */
export function canRefreshInstance(id: string): boolean {
  if (!overlayIsClean()) return false;
  const registration = registry.get(id);
  if (!registration) return true;
  return registration.canRefresh();
}
