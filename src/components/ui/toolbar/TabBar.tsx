"use client";

import { useRef, type ComponentType, type KeyboardEvent } from "react";
import { TB_TAB_ICON, tabBar, tabItem } from "@/lib/ui/toolbar";

/** One tab of a strip. The generic id keeps `value` / `onChange` typed per page,
 *  so `useState<"connect" | "hosting">` needs no cast at the call site. */
export type TabBarItem<Id extends string = string> = {
  id: Id;
  label: string;
  /** Lucide icon rendered at `TB_TAB_ICON`, in front of the word. */
  icon?: ComponentType<{ size?: number | string; className?: string }>;
  /** Trailing count ("Cards 12"). Rendered even when 0 — a zero is information. */
  count?: number;
  /** Attention dot for state the label cannot carry (pending approval, unread). */
  dot?: boolean;
  disabled?: boolean;
};

type TabBarProps<Id extends string> = {
  items: readonly TabBarItem<Id>[];
  value: Id;
  onChange: (id: Id) => void;
  /** Accessible name for the strip, e.g. "Agent sections". */
  label?: string;
  /** Extra classes on the strip — usually the padding (e.g. `px-4`). */
  className?: string;
};

/**
 * The underline tab strip: one row of views inside a single page, sitting
 * directly under the header (and, if there is one, above the scrolling body).
 *
 * Sibling of `Breadcrumb` and `Toolbar` in the page top-area pattern, and
 * deliberately narrow: it switches which view a page shows, nothing else. A tab
 * is never an action — an action belongs in `Toolbar` — and a strip never
 * replaces the breadcrumb, because the breadcrumb answers "where am I" while the
 * strip answers "which view of it".
 *
 * Keyboard follows the ARIA tab pattern, minus the panels: the strip owns one
 * tab stop (the selected tab) and Left/Right/Home/End move selection with
 * focus, skipping disabled tabs. Panels are the caller's business — every view
 * here is expected to be mounted by the page itself, as on the Agent page.
 */
export function TabBar<Id extends string>({
  items,
  value,
  onChange,
  label,
  className = "",
}: TabBarProps<Id>) {
  const strip = useRef<HTMLDivElement>(null);

  const select = (index: number) => {
    const item = items[index];
    if (!item || item.disabled) return;
    onChange(item.id);
    strip.current?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[index]?.focus();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const count = items.length;
    if (!count) return;
    const current = Math.max(
      items.findIndex((item) => item.id === value),
      0,
    );

    if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      select(e.key === "Home" ? 0 : count - 1);
      return;
    }

    const step = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
    if (!step) return;
    e.preventDefault();
    // Walk past disabled tabs instead of parking focus on one.
    for (let offset = 1; offset <= count; offset += 1) {
      const next = ((current + step * offset) % count + count) % count;
      if (!items[next].disabled) {
        select(next);
        return;
      }
    }
  };

  return (
    <div
      ref={strip}
      role="tablist"
      aria-label={label}
      onKeyDown={onKeyDown}
      className={`${tabBar} ${className}`.trim()}
    >
      {items.map(({ id, label: text, icon: Icon, count, dot, disabled }) => {
        const active = id === value;
        return (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={active}
            tabIndex={active ? 0 : -1}
            disabled={disabled}
            onClick={() => onChange(id)}
            className={tabItem(active)}
          >
            {Icon && <Icon size={TB_TAB_ICON} />}
            {text}
            {count !== undefined && <span className="text-xs opacity-70">{count}</span>}
            {dot && <span className="w-1.5 h-1.5 rounded-full bg-warning-text" aria-hidden />}
          </button>
        );
      })}
    </div>
  );
}

export default TabBar;
