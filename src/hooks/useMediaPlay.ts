"use client";

//! One clip plays at a time, app-wide.
//!
//! The rule: any mounted `<audio>` or `<video>` element that opts into the
//! manager (via `useMediaPlay().manage(ref.current)` — or the standalone
//! `manageMedia(el)` for non-hook callers) is paused the moment any other
//! managed element starts playing. This is what the four `function playUrl()`
//! helpers in AttemptsPanel / ParagraphList / PracticePanel / useBookData were
//! supposed to be doing but were not: `new Audio(url).play()` there spawns a
//! fresh element each call, so a second click overlaps the first. Card media
//! render sites must go through this hook — do not add a fifth `playUrl`.
//!
//! `playUrl(url)` on the hook is the fire-and-forget trigger for the reveal
//! case (a card flips and its anchored audio should start without a native
//! click). It goes through a module-scoped singleton `<audio>` so even
//! programmatic plays coexist with the mounted elements the manager watches.

import { useCallback, useEffect, useRef } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/tauri";

// Set of currently-managed elements. Kept at module scope so two independent
// render sites (e.g. the review panel and the editor's Library drawer) share
// the one-at-a-time rule instead of each managing their own.
const managed = new Set<HTMLMediaElement>();

/** Register an element with the manager. Its own `play` event pauses every
 *  other registered element. Idempotent. */
export function manageMedia(el: HTMLMediaElement): void {
  if (managed.has(el)) return;
  managed.add(el);
  el.addEventListener("play", onPlay);
}

/** Unregister; call from an unmount effect. Safe if the element was never
 *  registered (e.g. a null ref). */
export function unmanageMedia(el: HTMLMediaElement | null | undefined): void {
  if (!el) return;
  el.removeEventListener("play", onPlay);
  managed.delete(el);
}

/** Pause every managed element except `keep` (used by the reveal hook so a
 *  newly playing element does not pause itself before `play()` resolves). */
function onPlay(this: HTMLMediaElement) {
  const self = this;
  for (const other of managed) {
    if (other !== self && !other.paused) {
      try {
        other.pause();
      } catch {
        /* a detached element may throw; ignore */
      }
    }
  }
}

// One hidden `<audio>` element backs `playUrl` so programmatic plays respect
// the same one-at-a-time rule as the mounted elements.
let singleton: HTMLAudioElement | null = null;

/** Absolute filesystem path (not a URL) → `convertFileSrc` → play. Returns
 *  nothing; a missing file or a non-desktop runtime is a silent no-op, which
 *  is what the reveal trigger wants (a card flip should not throw because a
 *  video file lives on another device). */
export function playUrl(absPath: string | null | undefined): void {
  if (!absPath || !isTauri()) return;
  let src: string;
  try {
    src = convertFileSrc(absPath);
  } catch {
    return;
  }
  if (!singleton) singleton = new Audio();
  // Pause any mounted element that was mid-play; the singleton is not in the
  // `managed` set so it does not fire `onPlay` on itself.
  for (const other of managed) {
    if (!other.paused) {
      try {
        other.pause();
      } catch {
        /* ignore */
      }
    }
  }
  singleton.src = src;
  singleton.play().catch(() => {
    /* a missing file, a blocked autoplay, or an aborted play all land here;
     * none is a user-facing error worth surfacing. */
  });
}

export type UseMediaPlay = {
  /** Attach a mounted `<audio>` / `<video>` element to the manager. Call from
   *  an effect with the element ref; the hook unregisters on unmount. */
  manage: (el: HTMLMediaElement | null) => void;
  /** Fire-and-forget a resolved absolute path through the singleton — used by
   *  `play_on: reveal` triggers. */
  playUrl: (absPath: string | null | undefined) => void;
  /** Pause everything this hook knows about, mounted and singleton. */
  stop: () => void;
};

/** Per-component access to the shared one-at-a-time manager. The hook holds no
 *  state itself — the sets live at module scope so the rule is app-wide. */
export function useMediaPlay(): UseMediaPlay {
  // Track which element this hook instance registered so we can unregister it
  // on unmount without disturbing other components' registrations.
  const owned = useRef<HTMLMediaElement[]>([]);

  const manage = useCallback((el: HTMLMediaElement | null) => {
    if (!el) return;
    if (!owned.current.includes(el)) owned.current.push(el);
    manageMedia(el);
  }, []);

  const playUrlCb = useCallback((absPath: string | null | undefined) => {
    playUrl(absPath);
  }, []);

  const stop = useCallback(() => {
    for (const el of managed) {
      try {
        el.pause();
      } catch {
        /* ignore */
      }
    }
    if (singleton) {
      try {
        singleton.pause();
      } catch {
        /* ignore */
      }
    }
  }, []);

  useEffect(() => {
    const list = owned.current;
    return () => {
      for (const el of list) unmanageMedia(el);
      list.length = 0;
    };
  }, []);

  return { manage, playUrl: playUrlCb, stop };
}
