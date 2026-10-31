"use client";

import { useRef, useState, type ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import { ToolbarButton } from "./ToolbarButton";
import { OverflowContext } from "./overflow-context";

/** Width of the panel (`w-64`), needed to decide which side it opens on. */
const MENU_WIDTH = 256;
/** Tallest the panel is allowed to be, in px — about eight of its rows. A deck
 *  list grows with the user's decks, and a menu taller than the screen puts its
 *  own last rows out of reach; past this it scrolls. */
const MENU_MAX_HEIGHT = 320;
/** Room kept between the panel and the window's edges. */
const EDGE_MARGIN = 8;

type ToolbarSelectProps = {
  /** Leading icon naming the control (Type for a script, Globe for a language…). */
  icon: LucideIcon;
  /** Name of the control; the current value is appended to it. */
  label: string;
  /** Text of the chosen option, shown on the trigger. */
  value: string;
  /** Tooltip; defaults to `<label>: <value>`. */
  title?: string;
  /** The options - `ToolbarMenuItem`s, which close the menu as they fire. */
  children: ReactNode;
  disabled?: boolean;
  className?: string;
  /** Trigger as the glyph alone, with the value left to the title. For controls
   *  that repeat down a page (a per-row menu) where the words are the noise. */
  iconOnly?: boolean;
};

/**
 * A pick-one control in the toolbar: a labeled button that opens a small menu.
 *
 * `ToolbarOverflow` is the same mechanism but presents as a bare three-dot
 * trigger, which is right for "some actions live here" and wrong for a setting
 * whose current value belongs in the bar itself - the user should be able to
 * see what the page is set to without opening anything. This keeps the panel,
 * the tap-away backdrop (mobile WebViews fire no mouseleave) and the
 * `OverflowContext` that lets a plain `ToolbarMenuItem` close it, and adds the
 * value to the trigger.
 */
export function ToolbarSelect({
  icon,
  label,
  value,
  title,
  children,
  disabled = false,
  className = "",
  iconOnly = false,
}: ToolbarSelectProps) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  // Where the panel goes, measured when it opens — it is placed against the
  // window, not the trigger. Two things the CSS cannot know:
  //
  //  - The panel used to hang off the trigger's right edge, right for a control
  //    at the end of a row (font size, spacing) and wrong for one near the left
  //    (the card editor's deck), which opened 256px to the left of itself and
  //    lost its first characters past the window's edge — where nothing scrolls
  //    them back.
  //  - Height is bounded by the room it finds, not by its content: a deck list
  //    grows with the user's decks, and an open menu running off the bottom hides
  //    its own last decks. The fix for "my deck is missing" must not look like
  //    "my deck is gone", so the remainder scrolls.
  const [place, setPlace] = useState({ start: false, above: false, maxHeight: MENU_MAX_HEIGHT });

  const toggle = () => {
    if (open) {
      setOpen(false);
      return;
    }
    const rect = rootRef.current?.getBoundingClientRect();
    if (rect) {
      const roomBelow = window.innerHeight - rect.bottom - EDGE_MARGIN;
      const roomAbove = rect.top - EDGE_MARGIN;
      // A cramped side is only worth flipping when the other one is roomier; a
      // menu that jumps to fit a single row is worse than one that scrolls.
      const above = roomBelow < 200 && roomAbove > roomBelow;
      setPlace({
        start: rect.right < MENU_WIDTH + EDGE_MARGIN,
        above,
        maxHeight: Math.min(MENU_MAX_HEIGHT, Math.max(96, above ? roomAbove : roomBelow)),
      });
    }
    setOpen(true);
  };

  // `overflow-y-auto` needs a definite height, so the cap is inline and the rest
  // is classes; `overscroll-contain` keeps a flick at the end of the list from
  // scrolling the page behind it.
  const panelClass = [
    "absolute z-50 w-64 max-w-[calc(100vw-1rem)]",
    "overflow-y-auto overscroll-contain rounded-lg border border-border-default",
    "bg-bg-card shadow-xl py-1",
    place.start ? "left-0" : "right-0",
    place.above ? "bottom-full mb-1" : "top-full mt-1",
  ].join(" ");

  return (
    <OverflowContext.Provider value={() => setOpen(false)}>
      <div ref={rootRef} className={`relative shrink-0 ${className}`.trim()}>
        <ToolbarButton
          variant="toggleSoft"
          active={open}
          icon={icon}
          label={iconOnly ? undefined : value}
          title={title ?? `${label}: ${value}`}
          ariaLabel={iconOnly ? (title ?? `${label}: ${value}`) : `${label}: ${value}`}
          disabled={disabled}
          onClick={toggle}
        />
        {open && (
          <>
            <div className="fixed inset-0 z-40" onClick={() => setOpen(false)} />
            <div
              className={panelClass}
              style={{ maxHeight: place.maxHeight }}
              onMouseLeave={() => setOpen(false)}
            >
              {children}
            </div>
          </>
        )}
      </div>
    </OverflowContext.Provider>
  );
}

export default ToolbarSelect;
