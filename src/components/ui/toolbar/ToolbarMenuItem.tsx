"use client";

import type { ReactNode } from "react";
import { Check, type LucideIcon } from "lucide-react";
import { TB_MENU_ICON } from "@/lib/ui/toolbar";
import { useOverflowClose } from "./overflow-context";

type ToolbarMenuItemProps = {
  children: ReactNode;
  icon?: LucideIcon;
  /** Destructive item — error-colored text. */
  danger?: boolean;
  /** Renders a trailing check mark (ml-auto) when true. */
  checked?: boolean;
  disabled?: boolean;
  onClick?: () => void;
};

/**
 * A row inside a `ToolbarOverflow` menu. Single source of truth for the menu
 * item classes (previously duplicated as ReadAloud's local `menuItem` /
 * `menuItemDanger` consts and inlined in Cards). Closes the owning overflow on
 * click via context.
 */
export function ToolbarMenuItem({
  children,
  icon: Icon,
  danger = false,
  checked = false,
  disabled = false,
  onClick,
}: ToolbarMenuItemProps) {
  const close = useOverflowClose();
  const base =
    "w-full text-left px-3 py-2 text-sm hover:bg-bg-hover cursor-pointer flex items-center gap-2 disabled:opacity-50 disabled:cursor-not-allowed";
  return (
    <button
      type="button"
      className={`${base} ${danger ? "text-error-text" : "text-text-primary"}`}
      disabled={disabled}
      onClick={() => {
        close?.();
        onClick?.();
      }}
    >
      {Icon && <Icon size={TB_MENU_ICON} />}
      {children}
      {checked && <Check size={14} className="ml-auto" />}
    </button>
  );
}

export default ToolbarMenuItem;
