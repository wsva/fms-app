"use client";

import type { LucideIcon } from "lucide-react";
import {
  tbIcon,
  tbIconToggle,
  tbIconToggleSoft,
  TB_ICON_SIZE,
} from "@/lib/ui/toolbar";

export type ToolbarIconButtonVariant = "ghost" | "toggle" | "toggleSoft";

type ToolbarIconButtonProps = {
  icon: LucideIcon;
  /** Extra props spread onto the icon (e.g. `{ strokeWidth: 3 }`). */
  iconProps?: Record<string, unknown>;
  variant?: ToolbarIconButtonVariant;
  active?: boolean;
  title?: string;
  /** Required: icon-only buttons must always carry an accessible name. */
  ariaLabel: string;
  /** Override the icon size (defaults to TB_ICON_SIZE = 16). */
  size?: number;
  disabled?: boolean;
  className?: string;
  onClick?: () => void;
};

function variantClass(
  variant: ToolbarIconButtonVariant,
  active: boolean
): string {
  switch (variant) {
    case "toggle":
      return tbIconToggle(active);
    case "toggleSoft":
      return tbIconToggleSoft(active);
    default:
      return tbIcon;
  }
}

/**
 * An icon-only toolbar button (Back / Forward / Search / cue-step arrows).
 * Wraps the shared icon tokens; standardized to a 16px glyph and always
 * requires an accessible name.
 */
export function ToolbarIconButton({
  icon: Icon,
  iconProps,
  variant = "ghost",
  active = false,
  title,
  ariaLabel,
  size = TB_ICON_SIZE,
  disabled,
  className = "",
  onClick,
}: ToolbarIconButtonProps) {
  return (
    <button
      type="button"
      className={`${variantClass(variant, active)} ${className}`.trim()}
      disabled={disabled}
      title={title ?? ariaLabel}
      aria-label={ariaLabel}
      aria-pressed={variant === "ghost" ? undefined : active}
      onClick={onClick}
    >
      <Icon size={size} {...(iconProps ?? {})} />
    </button>
  );
}

export default ToolbarIconButton;
