"use client";

// ---------------------------------------------------------------------------
// The tab chips that live in the title bar's middle region (see
// docs/design/tab-strip.md §4.2). This is not a separate chrome row — the shell
// renders it inside `TitleBar`, in the flexible slot that used to be a bare drag
// handle, so the menus stay on the left and the window controls on the right.
//
// The region opts out of the window drag handle (`data-tauri-drag-region="false"`
// is set by the parent in TitleBar); the leftover space after the last chip stays
// draggable. Chips scroll horizontally when they overflow, so the menus and
// window controls are never pushed off screen.
// ---------------------------------------------------------------------------

import { Pin, Plus, X } from "lucide-react";
import { TAB_DEFS, type TabId } from "@/components/layout/Sidebar";
import { MULTI_INSTANCE, type TabInstance } from "@/lib/shell/tabs";

interface TitleBarTabsProps {
  order: TabInstance[];
  focusedId: string;
  onFocus: (instanceId: string) => void;
  onClose: (instanceId: string) => void;
  onTogglePin: (instanceId: string) => void;
  /** Open a new instance of a multi-instance kind (the "+" picker). */
  onSpawn: (tabId: TabId) => void;
}

/** "books#2" → 2; a singleton id ("books") → 1. Used only to disambiguate the
 *  chip label when a kind is open more than once. */
function instanceNumber(instanceId: string): number {
  const hash = instanceId.indexOf("#");
  if (hash < 0) return 1;
  const n = Number(instanceId.slice(hash + 1));
  return Number.isFinite(n) ? n : 1;
}

export default function TitleBarTabs({
  order,
  focusedId,
  onFocus,
  onClose,
  onTogglePin,
  onSpawn,
}: TitleBarTabsProps) {
  const canSpawn = MULTI_INSTANCE.size > 0;

  return (
    <div
      className="flex min-w-0 flex-1 items-stretch overflow-x-auto overflow-y-hidden"
      role="tablist"
      aria-label="Open pages"
    >
      {order.map((inst) => {
        const def = TAB_DEFS[inst.tabId];
        const Icon = def?.icon;
        const focused = inst.instanceId === focusedId;
        const n = instanceNumber(inst.instanceId);
        const label = n > 1 ? `${inst.title} (${n})` : inst.title;
        return (
          <div
            key={inst.instanceId}
            role="tab"
            aria-selected={focused}
            title={label}
            data-tauri-drag-region="false"
            onClick={() => onFocus(inst.instanceId)}
            // Middle-click closes, the desktop tab convention.
            onMouseDown={(e) => {
              if (e.button === 1) {
                e.preventDefault();
                onClose(inst.instanceId);
              }
            }}
            className={`group flex shrink-0 cursor-pointer items-center gap-1.5 border-r border-border-default pl-2.5 pr-1.5 text-[12px] select-none ${
              focused
                ? "bg-bg-body text-text-primary"
                : "text-text-secondary hover:bg-bg-hover"
            }`}
          >
            {Icon && <Icon width={13} height={13} className="shrink-0" />}
            <span className="max-w-[9rem] truncate">{label}</span>

            {/* Pin ("stick") toggle — protects the tab from being replaced. */}
            <button
              type="button"
              aria-pressed={inst.pinned}
              title={inst.pinned ? "Unpin tab" : "Pin tab (keep it open)"}
              data-tauri-drag-region="false"
              onClick={(e) => {
                e.stopPropagation();
                onTogglePin(inst.instanceId);
              }}
              className={`flex h-4 w-4 shrink-0 items-center justify-center rounded-sm transition-colors ${
                inst.pinned
                  ? "text-accent hover:bg-bg-hover"
                  : "text-text-tertiary opacity-0 group-hover:opacity-100 hover:bg-bg-hover hover:text-text-primary focus:opacity-100"
              }`}
            >
              <Pin size={12} strokeWidth={1.75} fill={inst.pinned ? "currentColor" : "none"} />
            </button>

            {/* Close — hidden when only one tab is left (the shell never shows an
                empty content area). */}
            {order.length > 1 && (
              <button
                type="button"
                aria-label={`Close ${label}`}
                title="Close tab"
                data-tauri-drag-region="false"
                onClick={(e) => {
                  e.stopPropagation();
                  onClose(inst.instanceId);
                }}
                className="flex h-4 w-4 shrink-0 items-center justify-center rounded-sm text-text-tertiary opacity-0 group-hover:opacity-100 hover:bg-error-bg hover:text-error-text focus:opacity-100"
              >
                <X size={12} strokeWidth={1.75} />
              </button>
            )}
          </div>
        );
      })}

      {canSpawn && (
        <button
          type="button"
          aria-label="Open a page in a new tab"
          title="Open a page in a new tab"
          data-tauri-drag-region="false"
          onClick={() => {
            // Phase 1 ships no multi-instance kinds, so this button is hidden;
            // when one lands it opens the first multi-instance kind. A richer
            // picker replaces this per docs/design/tab-strip.md §4.2.
            const first = [...MULTI_INSTANCE][0];
            if (first) onSpawn(first);
          }}
          className="flex shrink-0 items-center justify-center px-2 text-text-tertiary hover:bg-bg-hover hover:text-text-primary"
        >
          <Plus size={14} strokeWidth={1.75} />
        </button>
      )}
    </div>
  );
}
