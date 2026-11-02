"use client";

//! One clip plays at a time, app-wide.
//!
//! The rule: any mounted `<audio>` or `<video>` element that opts into the
//! manager (via `useMediaPlay().manage(ref.current)` — or the standalone
//! `manageMedia(el)` for non-hook callers) is paused the moment any other
//! managed element starts playing. This is what the `function playUrl()`
//! helpers in AttemptsPanel / PracticePanel still do wrong: `new Audio(url).play()`
//! there spawns a fresh element each call, so a second click overlaps the first.
//! Media render sites must go through this hook — do not add another `playUrl`.
//!
//! `playUrl(url)` on the hook is the fire-and-forget trigger for the reveal
//! case (a card flips and its anchored audio should start without a native
//! click). It goes through the same one-programmatic-element rule as the mounted
//! elements, so a reveal cannot talk over a clip a button started.
//!
//! `togglePlay(url, token)` is the version a play button uses: the same rule,
//! but each play is tagged with a token that says *which* control started it,
//! and the token is published, so that control shows Stop while its own clip
//! sounds and reverts on its own when the clip ends or something else takes it
//! over. Pressing the same control twice stops it — never a second overlapping
//! copy.

import { useCallback, useEffect, useRef, useSyncExternalStore } from "react";
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

/** Pause every managed element but the one that fired. Registered as a `play`
 *  listener, so `this` is the element that started. */
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

/** Absolute filesystem path (not a URL) → an asset URL, or null if this clip
 *  cannot be played here (no path, a non-desktop runtime, a bad path). */
function resolveSrc(absPath: string | null | undefined): string | null {
  if (!absPath || !isTauri()) return null;
  try {
    return convertFileSrc(absPath);
  } catch {
    return null;
  }
}

// The element behind programmatic plays (`playUrl`, `togglePlay`) — at most one
// alive at a time. Each play gets a *fresh* element and retires the previous
// one: swapping `src` on a playing element fires a `pause` and an `ended` of
// its own, which a play button would misread as "my clip stopped".
let active: Play | null = null;

type Play = {
  el: HTMLAudioElement;
  /** The token `togglePlay` started this with; null for a tokenless `playUrl`. */
  token: unknown;
  /** Drops the lifecycle listeners; filled in by whoever armed them. */
  detach: () => void;
};

// The token of the clip now sounding from a play button, and the subscribers
// that render Stop off it. Anything works as a token — a media path is a good
// one, since the question a button asks is exactly "is this the clip that is
// sounding?"
let playingToken: unknown = null;
const playbackListeners = new Set<() => void>();

function setPlayingToken(token: unknown): void {
  if (playingToken === token) return;
  playingToken = token;
  for (const fn of [...playbackListeners]) fn();
}

function subscribePlayback(fn: () => void): () => void {
  playbackListeners.add(fn);
  return () => {
    playbackListeners.delete(fn);
  };
}

function getPlayingToken(): unknown {
  return playingToken;
}

/** No server playback: nothing is sounding while the page is being rendered. */
function getPlayingTokenOnServer(): unknown {
  return null;
}

/** Pause and forget the live programmatic play. Its listeners leave with it, so
 *  the `pause` this provokes cannot be mistaken for a clip stopping elsewhere.
 *  `keepToken` is set only when a new play is taking over in the same breath. */
function retirePlay(keepToken = false): void {
  const play = active;
  if (!play) return;
  active = null;
  play.detach();
  unmanageMedia(play.el);
  try {
    play.el.pause();
  } catch {
    /* ignore */
  }
  if (!keepToken) setPlayingToken(null);
}

/** Start an element as the live play: registered with the manager, so its own
 *  `play` event pauses every mounted element and any mounted element starting
 *  pauses it. */
function startPlay(play: Play): void {
  retirePlay(play.token !== null);
  manageMedia(play.el);
  active = play;
}

export function playUrl(absPath: string | null | undefined): void {
  const src = resolveSrc(absPath);
  if (!src) return;
  const el = new Audio(src);
  startPlay({ el, token: null, detach: () => {} });
  el.play().catch(() => {
    /* a missing file, a blocked autoplay, or an aborted play all land here;
     * none is a user-facing error worth surfacing. */
  });
}

/**
 * The play/stop toggle a button drives: start `absPath` tagged with `token`, or
 * — if that same token is already what sounds — stop it. Pressing a control
 * twice therefore never produces two overlapping copies, and pressing another
 * one takes the single live play over (the first clip stops, and its button
 * reverts by itself, because `playingToken` moved on). A `token` unique to the
 * control is what makes the published token mean "*this* one is sounding".
 */
export function togglePlay(absPath: string | null | undefined, token: unknown): void {
  if (active && active.token === token) {
    retirePlay();
    return;
  }
  const src = resolveSrc(absPath);
  if (!src) return;
  const el = new Audio(src);
  const play: Play = { el, token, detach: () => {} };
  // However this clip stops — it ended, the button asked for Stop, a mounted
  // element took over — the token it was started with is no longer sounding.
  const stopped = () => {
    if (active === play) retirePlay();
  };
  el.addEventListener("ended", stopped);
  el.addEventListener("pause", stopped);
  play.detach = () => {
    el.removeEventListener("ended", stopped);
    el.removeEventListener("pause", stopped);
  };
  startPlay(play);
  setPlayingToken(token);
  el.play().catch(() => {
    if (active === play) retirePlay();
  });
}

export type UseMediaPlay = {
  /** Attach a mounted `<audio>` / `<video>` element to the manager. Call from
   *  an effect with the element ref; the hook unregisters on unmount. */
  manage: (el: HTMLMediaElement | null) => void;
  /** Fire-and-forget a resolved absolute path through the live play — used by
   *  `play_on: reveal` triggers. */
  playUrl: (absPath: string | null | undefined) => void;
  /** Start a clip tagged with `token`, or stop it if that token already
   *  sounds. Pair with `playing` to render a play/stop toggle. */
  toggle: (absPath: string | null | undefined, token: unknown) => void;
  /** The token of the clip currently sounding, or null — the value that lets a
   *  button say Stop for *its* clip only. */
  playing: unknown;
  /** Pause everything this hook knows about, mounted and programmatic. */
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

  const toggle = useCallback(
    (absPath: string | null | undefined, token: unknown) => {
      togglePlay(absPath, token);
    },
    []
  );

  // Subscribe to the live play's lifecycle, so a Stop button follows the audio
  // rather than the last click.
  const playing = useSyncExternalStore(subscribePlayback, getPlayingToken, getPlayingTokenOnServer);

  const stop = useCallback(() => {
    retirePlay();
    // The live play is managed too, but it is retired above; this loop is for
    // the mounted elements.
    for (const el of managed) {
      try {
        el.pause();
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

  return { manage, playUrl: playUrlCb, toggle, playing, stop };
}
