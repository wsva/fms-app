"use client";

import { useEffect, useRef, useState, type CSSProperties } from "react";

/**
 * A draggable seam between two columns.
 *
 * Both tabs of the standard-audio page are two panes — a list and the thing the
 * list is about — and in both the right split is a taste: a long file name wants
 * room on the left, a long waveform wants room on the right. So the seam moves,
 * it is remembered per device, and it never lets either side become a sliver.
 *
 * The width lives in a hook rather than the component because the grid is the
 * caller's: both tabs own their own layout, and only the seam is shared.
 */

/** The gutter the handle sits in; the grid's middle column. */
export const HANDLE_WIDTH = 12;

/** Used until there is a laid-out container to measure. */
const FALLBACK_WIDTH = 420;

export type SplitOptions = {
  /** Remembered per device, so the seam survives a reload. */
  storageKey: string;
  /**
   * The seam before anyone drags: `"even"` halves the room, a number is a
   * pixel width. Both are also what a double-click returns to.
   */
  initial: "even" | number;
  /** Neither side may be dragged into a sliver. */
  leftMin: number;
  rightMin: number;
};

/**
 * Keep both sides usable.
 *
 * Without a floor on each, a drag can leave a column so thin that its content is
 * a sliver, and the state that follows is a page that looks broken rather than
 * resized.
 */
function clampTo(
  next: number,
  container: number,
  leftMin: number,
  rightMin: number
): number {
  const room = container > 0 ? container - rightMin - HANDLE_WIDTH : next;
  return Math.round(Math.min(Math.max(next, leftMin), Math.max(leftMin, room)));
}

export function useSplitWidth({ storageKey, initial, leftMin, rightMin }: SplitOptions) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(() => (initial === "even" ? FALLBACK_WIDTH : initial));

  const measured = () => containerRef.current?.clientWidth ?? 0;

  /** An even split of the room actually available. */
  const even = () => {
    const container = measured();
    return container > 0 ? Math.round((container - HANDLE_WIDTH) / 2) : FALLBACK_WIDTH;
  };

  useEffect(() => {
    const stored = Number(window.localStorage.getItem(storageKey));
    // Chosen once is chosen: a remembered seam wins over the default, which is
    // only the shape of a page that has never been dragged.
    const wanted = Number.isFinite(stored) && stored > 0 ? stored : initial === "even" ? even() : initial;
    // A window that is already narrow must not open with the right pane
    // squeezed below its floor — and a container that has not been laid out yet
    // (width 0) must not be read as "very narrow".
    const container = measured();
    setWidth(container > 0 ? clampTo(wanted, container, leftMin, rightMin) : wanted);
    // The options are literals at every call site; the seam is read once and
    // owned by the state from then on.
  }, [storageKey, initial, leftMin, rightMin]);

  // Whatever takes the room away — the window, the sidebar, the page's own
  // padding — the floor is re-applied to the width already chosen rather than
  // the width being thrown away.
  useEffect(() => {
    const node = containerRef.current;
    if (!node || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      const container = node.clientWidth;
      // A view hidden by the shell reports 0; it has no width to divide yet.
      if (container <= 0) return;
      setWidth((current) => clampTo(current, container, leftMin, rightMin));
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, [leftMin, rightMin]);

  const resize = (next: number) => setWidth(clampTo(next, measured(), leftMin, rightMin));

  /** Written on release rather than on every pointer move of a drag. */
  const store = (next: number) => {
    try {
      window.localStorage.setItem(storageKey, String(clampTo(next, measured(), leftMin, rightMin)));
    } catch {
      // Storage off: the seam still works, it just forgets between visits.
    }
  };

  const commit = (next: number) => {
    resize(next);
    store(next);
  };

  /** The double-click: back to this page's own default. */
  const reset = () => {
    const next = initial === "even" ? even() : initial;
    resize(next);
    store(next);
  };

  const columns: CSSProperties = {
    gridTemplateColumns: `${width}px ${HANDLE_WIDTH}px minmax(0, 1fr)`,
  };

  return { containerRef, width, columns, resize, commit, reset };
}

/**
 * The handle itself.
 *
 * Drag it, or use the arrow keys when it has focus, and a double-click puts it
 * back to the page's default.
 */
export function Splitter({
  width,
  label,
  onChange,
  onCommit,
  onReset,
}: {
  width: number;
  /** What the two panes are, for a screen reader. */
  label: string;
  onChange: (next: number) => void;
  onCommit: (width: number) => void;
  onReset: () => void;
}) {
  const drag = useRef<{ x: number; from: number; to: number } | null>(null);

  const stop = () => {
    drag.current = null;
    // Both were set on the body for the length of the drag, so the pointer keeps
    // its shape over the two panes and nothing under it gets selected.
    document.body.style.userSelect = "";
    document.body.style.cursor = "";
  };

  // A drag that outlives the seam — the tab is switched from under it — would
  // otherwise leave the whole window in col-resize.
  useEffect(
    () => () => {
      document.body.style.userSelect = "";
      document.body.style.cursor = "";
    },
    []
  );

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-valuenow={Math.round(width)}
      tabIndex={0}
      title="Drag to resize · double-click to reset"
      className="group hidden lg:flex items-center justify-center touch-none cursor-col-resize outline-none"
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        event.preventDefault();
        event.currentTarget.setPointerCapture(event.pointerId);
        drag.current = { x: event.clientX, from: width, to: width };
        document.body.style.userSelect = "none";
        document.body.style.cursor = "col-resize";
      }}
      onPointerMove={(event) => {
        const start = drag.current;
        if (!start) return;
        start.to = start.from + (event.clientX - start.x);
        onChange(start.to);
      }}
      onPointerUp={(event) => {
        const start = drag.current;
        if (!start) return;
        if (event.currentTarget.hasPointerCapture(event.pointerId)) {
          event.currentTarget.releasePointerCapture(event.pointerId);
        }
        stop();
        onCommit(start.to);
      }}
      onPointerCancel={stop}
      onDoubleClick={onReset}
      onKeyDown={(event) => {
        if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
        event.preventDefault();
        const next = width + (event.key === "ArrowLeft" ? -24 : 24);
        onChange(next);
        onCommit(next);
      }}
    >
      <span className="h-12 w-[3px] rounded-full bg-border-default transition-colors group-hover:bg-accent-bg group-focus-visible:bg-accent-bg" />
    </div>
  );
}
