"use client";

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { Menu, PanelLeftClose, PanelLeftOpen } from "lucide-react";
import { isMobileApp } from "@/lib/platform";

/**
 * Shared collapsible left sidebar used by Read a Book, Read Aloud and Wiki.
 *
 * Anchored inside a `relative flex flex-1 min-h-0 overflow-hidden` body
 * wrapper placed after the page's top bar (never `fixed` to the viewport, so
 * it stays below the Android status bar and never covers the bar that opens
 * it). The wrapper is a flex ROW so the desktop column can take layout space.
 *
 *  * Mobile: slide-in drawer over the content — dimmed backdrop,
 *    `w-[80vw] max-w-[300px]`, closes on backdrop click and on item
 *    selection (`closeOnSelect`).
 *  * Desktop: in-flow split column — the page content sits beside it instead
 *    of being covered. Drag-resizable right edge with the width persisted per
 *    page in localStorage; closes via the toggle button.
 */

export type SidebarController = {
  mobile: boolean;
  visible: boolean;
  width: number;
  toggle: () => void;
  close: () => void;
  /** Closes the drawer after an item selection — mobile only; on desktop the
   *  split column stays open so users can pick repeatedly. */
  closeOnSelect: () => void;
  startDrag: (e: React.MouseEvent) => void;
};

type Options = {
  /** localStorage key the dragged width is persisted under. */
  storageKey: string;
  defaultWidth: number;
  minWidth?: number;
  /** Defaults to 80% of the window width, evaluated during drag. */
  maxWidth?: number;
};

export function useCollapsibleSidebar({
  storageKey,
  defaultWidth,
  minWidth = 160,
  maxWidth,
}: Options): SidebarController {
  // Deferred platform flag so SSR/prerender HTML matches the first client
  // paint (desktop layout); on mobile the drawer starts closed.
  const [mobile, setMobile] = useState(false);
  const [visible, setVisible] = useState(true);
  const [width, setWidth] = useState(defaultWidth);
  const widthRef = useRef(defaultWidth);

  const clampWidth = useCallback(
    (w: number) => {
      const max = maxWidth ?? window.innerWidth * 0.8;
      return Math.min(max, Math.max(minWidth, w));
    },
    [minWidth, maxWidth]
  );

  useEffect(() => {
    const m = isMobileApp();
    setMobile(m);
    if (m) setVisible(false);
    const saved = parseInt(localStorage.getItem(storageKey) ?? "", 10);
    if (Number.isFinite(saved)) {
      const w = clampWidth(saved);
      widthRef.current = w;
      setWidth(w);
    }
  }, [storageKey, clampWidth]);

  const toggle = useCallback(() => setVisible((v) => !v), []);
  const close = useCallback(() => setVisible(false), []);
  const closeOnSelect = useCallback(() => {
    if (isMobileApp()) setVisible(false);
  }, []);

  const startDrag = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      const startX = e.clientX;
      const startWidth = widthRef.current;
      let final = startWidth;
      const onMove = (ev: MouseEvent) => {
        final = clampWidth(startWidth + ev.clientX - startX);
        widthRef.current = final;
        setWidth(final);
      };
      const onUp = () => {
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
        localStorage.setItem(storageKey, String(final));
      };
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
    },
    [clampWidth, storageKey]
  );

  return { mobile, visible, width, toggle, close, closeOnSelect, startDrag };
}

export function CollapsibleSidebar({
  sidebar,
  children,
}: {
  sidebar: SidebarController;
  children: ReactNode;
}) {
  const { mobile, visible } = sidebar;

  // Desktop: in-flow column — the content splits the page beside it.
  if (!mobile) {
    if (!visible) return null;
    return (
      <div
        className="relative shrink-0 min-h-0 self-stretch flex flex-col bg-bg-card border-r border-border-default"
        style={{ width: `${sidebar.width}px` }}
      >
        {children}
        {/* Resize handle (mouse-drag only; not useful on touch) */}
        <div
          className="absolute top-0 right-0 h-full w-3 cursor-col-resize flex items-center justify-center group z-10"
          onMouseDown={sidebar.startDrag}
        >
          <div className="w-0.5 h-12 rounded-full bg-border-default group-hover:bg-accent transition-colors" />
        </div>
      </div>
    );
  }

  // Mobile: slide-in drawer with a dimmed backdrop over the content.
  return (
    <>
      <div
        className={`absolute inset-0 z-30 bg-black/40 transition-opacity duration-200 ${
          visible ? "opacity-100" : "opacity-0 pointer-events-none"
        }`}
        onClick={sidebar.close}
        aria-hidden="true"
      />
      <div
        className={`absolute inset-y-0 left-0 z-40 w-[80vw] max-w-[300px] flex flex-col bg-bg-card border-r border-border-default shadow-xl transition-transform duration-200 ${
          visible ? "translate-x-0" : "-translate-x-full"
        }`}
      >
        {children}
      </div>
    </>
  );
}

export function SidebarToggleButton({
  sidebar,
  title,
}: {
  sidebar: SidebarController;
  title?: string;
}) {
  const label = title ?? (sidebar.visible ? "Hide sidebar" : "Show sidebar");
  return (
    <button
      className="p-1.5 rounded-md text-text-secondary hover:text-text-primary hover:bg-bg-hover transition-colors cursor-pointer"
      title={label}
      aria-label={label}
      aria-expanded={sidebar.visible}
      onClick={sidebar.toggle}
    >
      {sidebar.mobile ? (
        <Menu size={18} />
      ) : sidebar.visible ? (
        <PanelLeftClose size={18} />
      ) : (
        <PanelLeftOpen size={18} />
      )}
    </button>
  );
}
