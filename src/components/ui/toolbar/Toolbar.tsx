"use client";

import type { ReactNode } from "react";
import { MobileContext } from "./mobile-context";
import { useIsMobile } from "@/hooks/useIsMobile";

type ToolbarProps = {
  children: ReactNode;
  /** Extra layout classes appended to the bar (e.g. `@container`). */
  className?: string;
  /**
   * Override the mobile flag handed to descendant buttons. Pages that already
   * track `mobile` (BooksPage via useBookData) pass it here; when omitted the
   * bar resolves it with the shared `useIsMobile()` hook.
   */
  mobile?: boolean;
};

/**
 * The full-width toolbar bar that sits under the breadcrumb on every dataset
 * page. Standardizes the container box and the `gap-2` spacing (Dictation
 * previously used `gap-3`), and provides the mobile flag to its buttons.
 */
export function Toolbar({ children, className = "", mobile }: ToolbarProps) {
  const detected = useIsMobile();
  const value = mobile ?? detected;
  return (
    <MobileContext.Provider value={value}>
      <div
        className={`shrink-0 flex items-center gap-2 px-4 py-0 border-b border-border-default bg-bg-card ${className}`.trim()}
      >
        {children}
      </div>
    </MobileContext.Provider>
  );
}

export default Toolbar;
