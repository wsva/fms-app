"use client";

import type { ReactNode } from "react";

/**
 * Breadcrumb bar shared by every dataset page: a full-width, edge-to-edge
 * `bg-bg-card` bar with a bottom border, sitting above the toolbar. Centralizes
 * the crumb classes that were previously duplicated (and slightly drifted)
 * across ReadBook / Cards / Dictation / ReadAloud / Wiki.
 */

const NAV =
  "shrink-0 flex items-center justify-start gap-2 px-4 py-2 text-sm select-none min-w-0 overflow-x-auto border-b border-border-default bg-bg-card";

type BreadcrumbProps = { children: ReactNode; className?: string };

export function Breadcrumb({ children, className = "" }: BreadcrumbProps) {
  return <nav className={`${NAV} ${className}`.trim()}>{children}</nav>;
}

type LinkProps = {
  children: ReactNode;
  onClick?: () => void;
  title?: string;
  /** Add the truncated, width-capped treatment used for a mid-trail crumb. */
  truncate?: boolean;
  /**
   * `link` (default) is accent-colored; `current` renders the primary-colored,
   * row-filling treatment but stays clickable — for a tail crumb that still
   * does something (e.g. Dictation's dataset crumb reloads on click).
   */
  variant?: "link" | "current";
  className?: string;
};

/** An accent, underlined crumb that navigates back to a parent level. */
Breadcrumb.Link = function BreadcrumbLink({
  children,
  onClick,
  title,
  truncate = false,
  variant = "link",
  className = "",
}: LinkProps) {
  const cls =
    variant === "current"
      ? "cursor-pointer hover:underline text-left text-text-primary font-medium truncate min-w-0 flex-1"
      : `cursor-pointer hover:underline text-left text-accent ${
          truncate ? "truncate min-w-0 max-w-[240px] shrink-0" : "shrink-0"
        }`;
  return (
    <button
      type="button"
      title={title}
      onClick={onClick}
      className={`${cls} ${className}`.trim()}
    >
      {children}
    </button>
  );
};

type CurrentProps = {
  children: ReactNode;
  title?: string;
  /**
   * Fill the remaining row width (the last crumb grows, earlier crumbs stay
   * packed left). Pass `false` for a plain root crumb that should not grow.
   */
  grow?: boolean;
  className?: string;
};

/** The current (non-clickable) crumb — plain primary-colored text. */
Breadcrumb.Current = function BreadcrumbCurrent({
  children,
  title,
  grow = true,
  className = "",
}: CurrentProps) {
  return (
    <span
      title={title}
      className={`text-text-primary font-medium truncate min-w-0 text-left ${
        grow ? "flex-1" : "shrink-0"
      } ${className}`.trim()}
    >
      {children}
    </span>
  );
};

/** The `›` separator between crumbs. */
Breadcrumb.Sep = function BreadcrumbSep() {
  return <span className="shrink-0 text-text-tertiary">&rsaquo;</span>;
};

export default Breadcrumb;
