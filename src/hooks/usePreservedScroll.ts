"use client";

// ---------------------------------------------------------------------------
// Scroll memory for a pane that is hidden with `display` and shown again.
//
// The shell keeps every open tab mounted and hides the inactive ones with
// `display:none` (`components/layout/PageHost.tsx`), because that is what keeps
// a page's state alive across a tab switch. Scroll offsets are the one piece of
// state a display toggle does not keep: an element with no layout box has no
// scrolling box, so the reading pane comes back at the top and the learner
// loses the line they were on.
//
// Offsets are recorded while the user scrolls rather than read on the way out —
// by the time the pane is hidden the value is already gone. One capture-phase
// listener on the pane sees every scroller inside it, because `scroll` does not
// bubble but still travels down through the capture phase.
// ---------------------------------------------------------------------------

import { useEffect, useLayoutEffect, useRef } from "react";
import type { RefObject } from "react";

interface Offset {
  top: number;
  left: number;
}

/**
 * Keep the scroll positions of everything inside `paneRef` across a
 * `display:none` → visible cycle. `active` is whether the pane is the one on
 * screen; the hook restores on the transition into `active`, never on the way
 * out.
 *
 * A pane that *unmounts* (a replaced or closed tab) has no offsets to restore,
 * which is the intended behaviour: a freshly mounted page starts at its start.
 */
export function usePreservedScroll(
  paneRef: RefObject<HTMLDivElement | null>,
  active: boolean
): void {
  /** Last offset seen inside this pane, per scroll container it contains. */
  const memory = useRef(new Map<Element, Offset>());

  const activeRef = useRef(active);
  activeRef.current = active;

  useEffect(() => {
    const root = paneRef.current;
    if (!root) return;
    const offsets = memory.current;
    const record = (e: Event) => {
      const el = e.target;
      if (!(el instanceof Element)) return;
      const top = el.scrollTop;
      const left = el.scrollLeft;
      // Losing the layout box clamps a scroller to 0 and can report that as a
      // scroll; a pane resetting to the top as it hides must not overwrite the
      // place the user left. Any *other* offset a hidden pane picks up is real
      // — a chat pane following a new message to the bottom — and is kept.
      if (!activeRef.current && top === 0 && left === 0) return;
      offsets.set(el, { top, left });
    };
    root.addEventListener("scroll", record, true);
    return () => root.removeEventListener("scroll", record, true);
  }, [paneRef]);

  useLayoutEffect(() => {
    if (!active) return;
    const offsets = memory.current;
    const restore = () => {
      for (const [el, off] of offsets) {
        // A subtree that remounted (a re-read swapping its rows out) leaves
        // stale nodes behind; drop them rather than guessing an equivalent.
        if (!el.isConnected) {
          offsets.delete(el);
          continue;
        }
        el.scrollTop = off.top;
        el.scrollLeft = off.left;
      }
    };
    // Before the first paint of the newly shown pane — this forces the layout
    // the pane only just got back — and once more on the next frame, for a pane
    // whose re-read settles its row heights after that first pass.
    restore();
    const frame = requestAnimationFrame(restore);
    return () => cancelAnimationFrame(frame);
  }, [active]);
}

export default usePreservedScroll;
