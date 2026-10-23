"use client";

import { invoke } from "@tauri-apps/api/core";

/**
 * Thin-client auto-flush controller: pushes the writeback queue to the hub on
 * the phone's own, so a local edit (card, book, wiki, read-aloud, dictation)
 * no longer waits for the user to open the Sync page and tap "Sync".
 *
 * This is a module-level singleton on purpose. Every dataset page mounts its
 * own `<PendingWritebackBanner>` → its own `useWritebackPending` poll, but they
 * all read the SAME device-wide queue, so five instances must behave as one.
 * All of them just call `notifyWritebackCount(n)`; the shared state below makes
 * at most one flush run at a time and spaces attempts out.
 *
 * Policy (the hybrid the app agreed on, not "flush every keystroke" nor an
 * OS cron — Android's Doze makes background timers unreliable):
 *  - Debounced push: once pending changes appear, wait `DEBOUNCE_MS`, then
 *    push — so a burst of edits (which the queue already coalesces per object)
 *    lands as a single upload rather than one request per save.
 *  - Throttled: attempts are at least `MIN_INTERVAL_MS` apart; a failed or
 *    offline push leaves rows queued and the next detected change retries.
 *  - On resume: returning to the app (or focusing the window) flushes promptly,
 *    which is what actually catches edits made while offline or backgrounded.
 *  - Only the push leg runs here. The heavier full "Sync round" (which also
 *    pulls deltas and can swap dataset dirs) stays a deliberate manual action.
 */

// Coalesce a burst of edits into one push shortly after the last one lands.
const DEBOUNCE_MS = 6_000;
// Floor on time between flush attempts, so retries never hammer the hub.
const MIN_INTERVAL_MS = 15_000;

let listenersAttached = false;
let inFlight = false;
let lastAttemptAt = 0;
let pendingKnown = 0;
let debounceTimer: ReturnType<typeof setTimeout> | null = null;

function arm(delayMs: number): void {
  if (debounceTimer != null) return; // one scheduled trigger is enough
  debounceTimer = setTimeout(() => {
    debounceTimer = null;
    void runFlush();
  }, delayMs);
}

async function runFlush(): Promise<void> {
  if (inFlight || pendingKnown <= 0) return;
  const since = Date.now() - lastAttemptAt;
  if (since < MIN_INTERVAL_MS) {
    // Too soon since the last attempt: re-check once the gap elapses rather
    // than firing a second request.
    arm(MIN_INTERVAL_MS - since);
    return;
  }
  inFlight = true;
  lastAttemptAt = Date.now();
  try {
    const pushed = await invoke<number>("writeback_flush");
    // Best-effort decrement; the poll refreshes the authoritative count soon.
    if (pushed > 0) pendingKnown = Math.max(0, pendingKnown - pushed);
  } catch {
    // Transport/offline failure: rows stay queued. The next detected change or
    // an app resume retries. Swallowed on purpose — the banner already shows
    // that the changes are still pending.
  } finally {
    inFlight = false;
  }
}

function ensureResumeListeners(): void {
  if (listenersAttached) return;
  if (typeof document === "undefined" || typeof window === "undefined") return;
  listenersAttached = true;
  const onResume = () => {
    if (document.visibilityState === "visible") arm(0);
  };
  document.addEventListener("visibilitychange", onResume);
  window.addEventListener("focus", onResume);
  // Coming back online should push edits that queued while disconnected, even
  // if the app was already foregrounded (so no visibilitychange fires).
  window.addEventListener("online", () => arm(0));
}

/**
 * Feed the shared controller the latest queue depth (from any banner poll).
 * When it reports pending changes, the controller schedules a debounced push
 * and starts listening for app-resume so offline edits get retried promptly.
 */
export function notifyWritebackCount(count: number): void {
  pendingKnown = count;
  if (count > 0) {
    ensureResumeListeners();
    arm(DEBOUNCE_MS);
  }
}
