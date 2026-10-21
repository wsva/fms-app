"use client";

import type { LucideIcon } from "lucide-react";
import {
  tbGhost,
  tbDanger,
  tbSolid,
  tbToggle,
  tbToggleSoft,
  tbGhostStacked,
  tbDangerStacked,
  tbSolidStacked,
  tbToggleStacked,
  tbToggleSoftStacked,
  TB_ICON,
} from "@/lib/ui/toolbar";
import { useToolbarMobile } from "./mobile-context";
import { useIsMobile } from "@/hooks/useIsMobile";

export type ToolbarButtonVariant =
  | "ghost"
  | "danger"
  | "solid"
  | "toggle"
  | "toggleSoft";

type ToolbarButtonProps = {
  /** Visual semantics. Defaults to `ghost`. */
  variant?: ToolbarButtonVariant;
  /** Active state for the `toggle` / `toggleSoft` variants. */
  active?: boolean;
  /** Leading icon, rendered at the standardized labeled-button size (14). */
  icon?: LucideIcon;
  /** Extra props spread onto the icon (e.g. `{ strokeWidth: 3 }`). */
  iconProps?: Record<string, unknown>;
  /** Override the icon size (defaults to TB_ICON = 14). */
  iconSize?: number;
  /** Text label; hidden on mobile unless `collapseOnMobile={false}`. Always
   *  shown when `stacked`. */
  label?: string;
  /** Collapse the label to icon-only on mobile. Defaults to true. */
  collapseOnMobile?: boolean;
  /** Icon above the label instead of beside it. Lets a toolbar keep its words
   *  in the width a phone has; implies the label is shown. */
  stacked?: boolean;
  title?: string;
  /** Accessible name; auto-derived from `label`/`title` when omitted. */
  ariaLabel?: string;
  disabled?: boolean;
  /** Extra layout classes (e.g. `shrink-0`, `ml-auto`). */
  className?: string;
  onClick?: () => void;
};

function variantClass(
  variant: ToolbarButtonVariant,
  active: boolean,
  stacked: boolean
): string {
  switch (variant) {
    case "danger":
      return stacked ? tbDangerStacked : tbDanger;
    case "solid":
      return stacked ? tbSolidStacked : tbSolid;
    case "toggle":
      return stacked ? tbToggleStacked(active) : tbToggle(active);
    case "toggleSoft":
      return stacked ? tbToggleSoftStacked(active) : tbToggleSoft(active);
    default:
      return stacked ? tbGhostStacked : tbGhost;
  }
}

/**
 * A labeled toolbar action button. Wraps the shared `tb*` tokens so every
 * dataset page gets identical padding, gap, icon size, and mobile label
 * collapsing. Use a plain `<button type="button">` underneath (never HeroUI
 * `<Button>`) so nothing reintroduces the component's own font-weight/padding.
 */
export function ToolbarButton({
  variant = "ghost",
  active = false,
  icon: Icon,
  iconProps,
  iconSize = TB_ICON,
  label,
  collapseOnMobile = true,
  stacked = false,
  title,
  ariaLabel,
  disabled,
  className = "",
  onClick,
}: ToolbarButtonProps) {
  const ctx = useToolbarMobile();
  const detected = useIsMobile();
  const mobile = ctx ?? detected;
  // A label-only button (no icon) must never collapse, or it would render empty
  // on mobile — collapsing only makes sense when the icon remains. A stacked
  // button shows its label by definition: the vertical layout is chosen precisely
  // because the words are worth keeping.
  const showLabel = !!label && (stacked || !mobile || !collapseOnMobile || !Icon);

  return (
    <button
      type="button"
      className={`${variantClass(variant, active, stacked)} ${className}`.trim()}
      disabled={disabled}
      title={title ?? label}
      aria-label={ariaLabel ?? label ?? title}
      aria-pressed={
        variant === "toggle" || variant === "toggleSoft" ? active : undefined
      }
      onClick={onClick}
    >
      {Icon && <Icon size={iconSize} {...(iconProps ?? {})} />}
      {showLabel && label}
    </button>
  );
}

export default ToolbarButton;
