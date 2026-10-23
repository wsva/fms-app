"use client";

import type { ReactNode } from "react";
import type { LucideIcon } from "lucide-react";

// The header row for a page that has NO breadcrumb. It reuses the exact
// `Breadcrumb` bar classes so the top strip is identical whether it carries
// crumbs (the dataset pages) or a plain title (every tool / aux page). A page
// header is always a single compact row: optional icon, the page name filling
// the row, and a trailing non-action status slot (a badge, a spinner). Actionable
// controls never live here — they belong in the `Toolbar` row directly below.
const HEADER =
  "shrink-0 flex items-center gap-2 px-4 py-2 border-b border-border-default bg-bg-card";

type PageHeaderProps = {
  /** The page name — taken from the Sidebar/BottomNav `TabId` label. */
  title: string;
  /** Optional leading icon (OCR's scanner, Settings' gear…). */
  icon?: LucideIcon;
  /** Trailing, non-actionable status: a provider/model badge or a spinner. */
  badge?: ReactNode;
  className?: string;
};

/**
 * A single-row page title bar — the no-breadcrumb counterpart of `Breadcrumb`.
 * Place it as the first `shrink-0` child of a page's `flex flex-col h-full`
 * root, above the content (and above a `Toolbar` row, if the page has one).
 */
export function PageHeader({ title, icon: Icon, badge, className = "" }: PageHeaderProps) {
  return (
    <div className={`${HEADER} ${className}`.trim()}>
      {Icon && <Icon size={20} className="text-text-secondary shrink-0" />}
      <h1 className="text-base font-semibold text-text-primary flex-1 min-w-0 truncate">{title}</h1>
      {badge}
    </div>
  );
}

export default PageHeader;
