"use client";

import { useState, type ReactNode } from "react";
import { MoreVertical, type LucideIcon } from "lucide-react";
import { ToolbarIconButton } from "./ToolbarIconButton";
import { OverflowContext } from "./overflow-context";

type ToolbarOverflowProps = {
  children: ReactNode;
  /** Controlled open state; omit to let the menu manage its own. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** Trigger icon (defaults to MoreVertical). */
  icon?: LucideIcon;
  /** Accessible name / tooltip for the trigger. */
  label?: string;
  className?: string;
};

/**
 * The three-dot overflow menu. Owns the trigger, the tap-away backdrop (mobile
 * WebViews fire no mouseleave), and the dropdown panel. Previously Cards and
 * ReadAloud each hand-rolled this with different z-index and a missing backdrop
 * in ReadAloud; this unifies both.
 */
export function ToolbarOverflow({
  children,
  open,
  onOpenChange,
  icon = MoreVertical,
  label = "More actions",
  className = "",
}: ToolbarOverflowProps) {
  const [internal, setInternal] = useState(false);
  const isControlled = open !== undefined;
  const isOpen = isControlled ? open : internal;
  const setOpen = (v: boolean) => {
    if (isControlled) onOpenChange?.(v);
    else setInternal(v);
  };

  return (
    <OverflowContext.Provider value={() => setOpen(false)}>
      <div className={`relative shrink-0 ${className}`.trim()}>
        <ToolbarIconButton
          icon={icon}
          variant="toggleSoft"
          active={isOpen}
          title={label}
          ariaLabel={label}
          onClick={() => setOpen(!isOpen)}
        />
        {isOpen && (
          <>
            <div className="fixed inset-0 z-40" onClick={() => setOpen(false)} />
            {/* Bounded like `ToolbarSelect`'s panel: these menus sit at the end of
                a page toolbar, so the height they can rely on is the viewport's —
                past it the list scrolls rather than running off the screen. */}
            <div
              className="absolute right-0 top-full mt-1 z-50 w-56 max-h-[min(60vh,20rem)] overflow-y-auto overscroll-contain rounded-lg border border-border-default bg-bg-card shadow-xl py-1"
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

export default ToolbarOverflow;
