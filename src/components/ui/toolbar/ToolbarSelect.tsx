"use client";

import { useState, type ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import { ToolbarButton } from "./ToolbarButton";
import { OverflowContext } from "./overflow-context";

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

  return (
    <OverflowContext.Provider value={() => setOpen(false)}>
      <div className={`relative shrink-0 ${className}`.trim()}>
        <ToolbarButton
          variant="toggleSoft"
          active={open}
          icon={icon}
          label={iconOnly ? undefined : value}
          title={title ?? `${label}: ${value}`}
          ariaLabel={iconOnly ? (title ?? `${label}: ${value}`) : `${label}: ${value}`}
          disabled={disabled}
          onClick={() => setOpen((v) => !v)}
        />
        {open && (
          <>
            <div className="fixed inset-0 z-40" onClick={() => setOpen(false)} />
            <div
              className="absolute right-0 top-full mt-1 z-50 w-64 rounded-lg border border-border-default bg-bg-card shadow-xl py-1"
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
