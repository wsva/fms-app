"use client";

import { createContext, useCallback, useContext, useEffect, useRef, useState, type ComponentType, type ReactNode } from "react";
import { Check } from "lucide-react";

// ---------------------------------------------------------------------------
// Bar item
// ---------------------------------------------------------------------------
// A strip of buttons, in the VS Code sense: each item fills the bar's height so
// its hover rectangle touches its neighbours, and clicking one raises a menu.
// Two bars use this primitive — the status bar at the bottom (menus open
// *above* their item) and the title bar at the top (menus open *below*, via
// `direction="down"`). Three consequences shape it:
//
//  * The panel sits at z-[110], over the agent dock (z-100) and the floating
//    button (z-101). The right-hand items live under the dock's own column, so
//    a lower panel would open behind the very surface the item reports on.
//  * No ancestor of an item may clip: `overflow-hidden` on a bar group would cut
//    the panel off — and `overflow-x: hidden` silently computes the other axis
//    to `auto`, so there is no "clip one way only" escape. Truncation therefore
//    belongs on the item's own text (`max-w-*` + `truncate`), never on a
//    wrapping div.
//  * The item opts out of the title bar's drag region with
//    `data-tauri-drag-region="false"` on its own wrapper, and that is not
//    cosmetic. Tauri's drag script walks the click's composed path and treats
//    any *non-clickable* node under a `deep` region as a handle: it prevents the
//    default and asks the window to start dragging. The tap-away scrim is a bare
//    div, so leaving it inside the handle meant a click "outside the menu"
//    started a window drag instead of closing it, and a second click on the item
//    (`e.detail === 2`) invoked `internal_toggle_maximize` — the window resized.
//    The button element itself was never the problem: a clickable node blocks a
//    drag on its own.
//  * Two open styles: `openOnHover` for a mouse-driven bar (title bar), and
//    click-plus-scrim for a bar that may be touched (status bar), because a
//    touch screen fires no trustworthy `mouseleave`, so a hover-opened menu
//    there could only be closed by tapping its own item again.
// ---------------------------------------------------------------------------

/** Closes the owning menu; a no-op for a row rendered outside one. */
const CloseContext = createContext<() => void>(() => {});

/** A bar icon: a Lucide icon or one of the app's own glyphs (the microphone in
 *  the navigation), which have nothing in common but the two props a bar passes.
 *  Named rather than inlined because `Sidebar.tsx` types its nav entries with the
 *  same shape, and the title bar's navigation menus hand those entries to these
 *  rows — widening the prop here is what keeps the call site free of a cast. */
export type BarIcon = ComponentType<{ size?: number; className?: string }>;

/** How long a hover-opened menu stays up after the pointer leaves its item.
 *  Long enough to cross the gap above the panel, short enough to read as
 *  instant. See the `cancelScheduledClose` comment in the component. */
const HOVER_CLOSE_MS = 150;

/** The hover-opened menu on screen right now, if any. Module scope, because the
 *  items are siblings each owning their own open state — nothing above them
 *  knows which panel is up — and moving from one label to the next has to take
 *  the first panel down at once instead of letting its grace timer run out with
 *  two menus floating over the pages.
 *
 *  Only the hover-opened style takes part: a click-opened menu already puts a
 *  full-window scrim under the pointer, so its neighbour cannot be entered at
 *  all — which is exactly why a hover-opened item drops the scrim. */
let hoveredCloser: (() => void) | null = null;

export type StatusItemProps = {
  /** Accessible name, and the tooltip unless `title` overrides it. Always a
   *  string, even when the visible body is custom `content`. */
  label: string;
  title?: string;
  /** Leading icon, sized to the bar. */
  icon?: BarIcon;
  /** Extra classes for that icon — how a switch shows its state without
   *  spending room on a word (`text-green-500` on, `text-text-tertiary` off).
   *  Merged after the size and opacity defaults, so a colour wins and the
   *  opacity rule still applies to it. */
  iconClassName?: string;
  /** Visible body. Defaults to `label`; pass custom nodes for a live value. */
  content?: ReactNode;
  /** Trailing node — a count or an unread dot, kept out of the a11y name. */
  trailing?: ReactNode;
  /** Persistent tint for "what this item reports on is on": dock open, service
   *  running. An open menu tints by itself and needs no help here. */
  active?: boolean;
  /** Which edge the panel aligns to: `start` for left-group items, `end` for
   *  right-group ones, so no menu is ever pushed off screen. */
  align?: "start" | "end";
  /** Which side of the item the panel opens on: `up` for a bottom bar, `down`
   *  for the title bar at the top of the window. */
  direction?: "up" | "down";
  /** Open on mouse enter and close when the pointer leaves, instead of only on
   *  click. The title bar sets it; a bar that gets touched must not (see the
   *  module comment), and it drops the tap-away scrim in favour of a document
   *  listener, because the scrim covers the *neighbouring* items and so would
   *  swallow their `mouseenter` — hover-to-switch would not work. */
  openOnHover?: boolean;
  /** Action for a plain item. Ignored when the item has menu children. */
  onClick?: () => void;
  /** Menu contents. With children the item owns a popover in `direction`;
   *  without them it is a bare button — which is what the dock toggle wants. */
  children?: ReactNode;
  className?: string;
};

/**
 * One item of a shell bar: a full-height button, optionally with a menu.
 * Owns the open state, the Escape handler, the hover policy and the dismiss —
 * by a tap-away backdrop when the item is click-opened, by a document listener
 * when it is hover-opened.
 */
export function StatusItem({
  label,
  title,
  icon: Icon,
  iconClassName = "",
  content,
  trailing,
  active = false,
  align = "start",
  direction = "up",
  openOnHover = false,
  onClick,
  children,
  className = "",
}: StatusItemProps) {
  const [open, setOpen] = useState(false);
  const hasMenu = Boolean(children);
  const hoverable = hasMenu && openOnHover;
  const rootRef = useRef<HTMLDivElement | null>(null);
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // A grace period, not an instant close: the panel hangs 4px below the item
  // (`mt-1`), and while the pointer crosses that gap it is over neither the item
  // nor the panel, which would otherwise close the menu the moment the user aims
  // for it. Entering the panel cancels the pending close. Stable identity, so the
  // unmount cleanup below can depend on it honestly.
  const cancelScheduledClose = useCallback(() => {
    if (closeTimer.current) {
      clearTimeout(closeTimer.current);
      closeTimer.current = null;
    }
  }, []);

  // Stable identities, so the module-level claim can be compared and released
  // without a new closure on every render.
  const closeNow = useCallback(() => setOpen(false), []);

  /** The one way out of an open menu: release the claim, then close. */
  const dismiss = useCallback(() => {
    if (hoveredCloser === closeNow) hoveredCloser = null;
    cancelScheduledClose();
    setOpen(false);
  }, [cancelScheduledClose, closeNow]);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") dismiss();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [open, dismiss]);

  // Dismiss on a click anywhere outside this item, while a hover-opened menu is
  // up. Bubbling phase, so the item's own toggle and the rows' handlers run
  // first. The click then continues into the page below: absorbing it would need
  // a full-window scrim, and a scrim is exactly what breaks hover.
  useEffect(() => {
    if (!open || !hoverable) return;
    const onDocMouseDown = (e: MouseEvent) => {
      const root = rootRef.current;
      if (root && e.target instanceof Node && !root.contains(e.target)) dismiss();
    };
    document.addEventListener("mousedown", onDocMouseDown);
    return () => document.removeEventListener("mousedown", onDocMouseDown);
  }, [open, hoverable, dismiss]);

  // A menu left open by hover must not outlive the pointer, and an unmount must
  // not leave a pending timer, nor a claim pointing at a component that is gone.
  useEffect(
    () => () => {
      cancelScheduledClose();
      if (hoveredCloser === closeNow) hoveredCloser = null;
    },
    [cancelScheduledClose, closeNow],
  );

  const hint = title ?? label;
  const tint =
    open || active
      ? "bg-bg-hover text-text-primary"
      : "text-text-secondary hover:bg-bg-hover hover:text-text-primary";

  // Entering the panel itself counts as entering the item — it is a descendant
  // of this wrapper — so the pending close is cancelled on the way in. Both the
  // hover and the click path open through here, so "one panel at a time" holds
  // however this one came up.
  const openItem = () => {
    cancelScheduledClose();
    if (hoverable) {
      // Take the screen over from a sibling's panel at once, rather than
      // stacking two of them for the length of the grace period.
      if (hoveredCloser && hoveredCloser !== closeNow) hoveredCloser();
      hoveredCloser = closeNow;
    }
    setOpen(true);
  };

  const scheduleClose = () => {
    cancelScheduledClose();
    closeTimer.current = setTimeout(dismiss, HOVER_CLOSE_MS);
  };

  return (
    <div
      ref={rootRef}
      // See the module comment: inside the title bar's `deep` drag region this
      // is what keeps the item, its panel and its dismiss area out of the handle.
      data-tauri-drag-region="false"
      onMouseEnter={hoverable ? openItem : undefined}
      onMouseLeave={hoverable ? scheduleClose : undefined}
      className={`relative flex shrink-0 ${className}`}
    >
      <button
        type="button"
        title={hint}
        aria-label={hint}
        aria-haspopup={hasMenu || undefined}
        aria-expanded={hasMenu ? open : undefined}
        aria-pressed={hasMenu ? undefined : active}
        onClick={() => {
          if (!hasMenu) {
            onClick?.();
            return;
          }
          // Click toggles even where hover does: it is how a menu whose pointer
          // has already left, or one opened on a touch screen, gets shut.
          if (open) dismiss();
          else openItem();
        }}
        className={`flex h-full items-center gap-1.5 px-1.5 cursor-pointer transition-colors ${tint}`}
      >
        {Icon && <Icon size={13} className={`shrink-0 opacity-80 ${iconClassName}`} />}
        <span className="flex min-w-0 items-center gap-1.5">{content ?? label}</span>
        {trailing}
      </button>

      {hasMenu && open && (
        <>
          {!openOnHover && (
            <div className="fixed inset-0 z-[105]" onClick={dismiss} />
          )}
          <div
            role="menu"
            aria-label={label}
            className={`absolute ${
              direction === "down" ? "top-full mt-1" : "bottom-full mb-1"
            } z-[110] min-w-[220px] max-w-[380px] rounded-lg border border-border-default bg-bg-card py-1 text-sm font-normal shadow-xl ${
              align === "end" ? "right-0" : "left-0"
            }`}
          >
            <CloseContext.Provider value={dismiss}>{children}</CloseContext.Provider>
          </div>
        </>
      )}
    </div>
  );
};

// ---------------------------------------------------------------------------
// Menu rows
// ---------------------------------------------------------------------------

/** A menu line: icon, text, optional detail, and either an action or a check. */
export function StatusMenuRow({
  icon: Icon,
  text,
  detail,
  onClick,
  danger = false,
  disabled = false,
  checked,
  trailing,
  keepOpen = false,
}: {
  icon?: BarIcon;
  text: ReactNode;
  detail?: ReactNode;
  onClick?: () => void;
  danger?: boolean;
  disabled?: boolean;
  /** Renders the check column, for a selectable list. */
  checked?: boolean;
  /** Extra right-aligned content. Rendered *inside* the menu button, so it must
   *  be non-interactive (a state word, a keyboard hint, a count) — nesting a
   *  button here would be invalid HTML. A row that acts is its own row. */
  trailing?: ReactNode;
  /** Skip the automatic dismiss, for a row whose action opens something that
   *  needs the menu's context kept (a sub-panel, a second click). */
  keepOpen?: boolean;
}) {
  const close = useContext(CloseContext);
  const interactive = Boolean(onClick) && !disabled;

  return (
    <button
      type="button"
      role="menuitem"
      disabled={!interactive}
      onClick={() => {
        if (!interactive) return;
        onClick?.();
        if (!keepOpen) close();
      }}
      className={`flex w-full items-center gap-2 px-3 py-1 text-left ${
        interactive ? "cursor-pointer hover:bg-bg-hover" : "cursor-default"
      } ${danger ? "text-red-500" : "text-text-secondary"} ${disabled ? "opacity-50" : ""}`}
    >
      <span className="w-3 shrink-0 text-accent-bg">
        {checked === undefined ? null : checked ? <Check size={12} /> : null}
      </span>
      {Icon && <Icon size={13} className="shrink-0 opacity-70" />}
      <span className="flex min-w-0 flex-1 items-baseline gap-2">
        <span className="truncate">{text}</span>
        {detail && <span className="truncate text-[11px] text-text-tertiary">{detail}</span>}
      </span>
      {trailing}
    </button>
  );
}

/** A non-interactive heading, so one menu can hold more than one subject. */
export function StatusMenuHeading({ children }: { children: ReactNode }) {
  return (
    <p className="px-3 pt-1 pb-0.5 text-[10px] uppercase tracking-wider text-text-tertiary">
      {children}
    </p>
  );
}

export function StatusMenuSep() {
  return <div className="my-1 border-t border-border-default" />;
}

/** Free-form block for the rare content that is not a command: a list of rows,
 *  a URL, a state line. */
export function StatusMenuBlock({ children }: { children: ReactNode }) {
  return <div className="px-3 py-1 text-text-secondary">{children}</div>;
}
