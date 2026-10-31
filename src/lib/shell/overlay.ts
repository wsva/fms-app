"use client";

// ---------------------------------------------------------------------------
// Whether something is covering the shell.
//
// A full-window surface draws its own close control at the top-right of its
// toolbar, which is a few pixels from the window's own close button in the
// title bar above it — close enough that a miss does not scroll the wrong
// card, it quits the app. The bar therefore needs to know that a surface is
// up, and the surface is the only thing that knows.
//
// Module-scope rather than a provider, for the same reason `tab-subject` and
// `lib/agent/ui-actions` are: the title bar and the overlay are siblings in the
// shell with no common owner below it, and a provider would mean re-indenting
// the shell for one boolean.
//
// A depth counter, not a boolean: two overlays can be up at once (a card editor
// with the About card over it, the workspace chooser over a page), and the one
// that closes first must not report the window free while the other still
// covers it.
// ---------------------------------------------------------------------------

import { useSyncExternalStore } from "react";

let depth = 0;
const subscribers = new Set<() => void>();

function subscribe(notify: () => void): () => void {
  subscribers.add(notify);
  return () => {
    subscribers.delete(notify);
  };
}

function emit(): void {
  subscribers.forEach((notify) => notify());
}

/**
 * Announce that a surface covering the shell is on screen. Returns the release
 * function — call it on unmount, or from an effect's cleanup, so a surface that
 * goes away by any route still releases its hold.
 *
 * Cheap and idempotent per call: two holds are two, even from the same surface,
 * since the count is what keeps the last one honest.
 */
export function holdOverlay(): () => void {
  depth += 1;
  emit();
  let released = false;
  return () => {
    if (released) return;
    released = true;
    depth = Math.max(0, depth - 1);
    emit();
  };
}

/** Is a surface covering the shell right now? */
export function useOverlayOpen(): boolean {
  return useSyncExternalStore(
    subscribe,
    () => depth > 0,
    () => false,
  );
}
