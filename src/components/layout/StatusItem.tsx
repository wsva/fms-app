"use client";

import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { Check, type LucideIcon } from "lucide-react";

// ---------------------------------------------------------------------------
// Status bar item
// ---------------------------------------------------------------------------
// The status bar is a strip of buttons, in the VS Code sense: each item fills
// the bar's height so its hover rectangle touches its neighbours, and clicking
// one raises a menu *above* it. Two consequences shape this primitive:
//
//  * The panel sits at z-[110], over the agent dock (z-100) and the floating
//    button (z-101). The right-hand items live under the dock's own column, so
//    a lower panel would open behind the very surface the item reports on.
//  * No ancestor of an item may clip: `overflow-hidden` on a bar group would cut
//    the panel off — and `overflow-x: hidden` silently computes the other axis
//    to `auto`, so there is no "clip one way only" escape. Truncation therefore
//    belongs on the item's own text (`max-w-*` + `truncate`), never on a
//    wrapping div.
// ---------------------------------------------------------------------------

/** Closes the owning menu; a no-op for a row rendered outside one. */
const CloseContext = createContext<() => void>(() => {});

export type StatusItemProps = {
  /** Accessible name, and the tooltip unless `title` overrides it. Always a
   *  string, even when the visible body is custom `content`. */
  label: string;
  title?: string;
  /** Leading icon, sized to the bar. */
  icon?: LucideIcon;
  /** Visible body. Defaults to `label`; pass custom nodes for a live value. */
  content?: ReactNode;
  /** Trailing node — a count or an unread dot, kept out of the a11y name. */
  trailing?: ReactNode;
  /** Persistent tint for "what this item reports on is on": dock open, service
   *  running. An open menu tints by itself and needs no help here. */
  active?: boolean;
  /** Which edge the panel aligns to: `start` for left-group items, `end` for
   *  right-group ones, so no menu is ever pushed off screen. */
  align?: "start" | "end";
  /** Action for a plain item. Ignored when the item has menu children. */
  onClick?: () => void;
  /** Menu contents. With children the item owns an upward popover; without
   *  them it is a bare button — which is what the dock toggle wants. */
  children?: ReactNode;
  className?: string;
};

/**
 * One item of the status bar: a full-height button, optionally with an upward
 * menu. Owns the open state, the Escape handler and the tap-away backdrop —
 * the backdrop because a WebView fires no `mouseleave`, so hover dismissal
 * would leave a menu stuck open on touch.
 */
export function StatusItem({
  label,
  title,
  icon: Icon,
  content,
  trailing,
  active = false,
  align = "start",
  onClick,
  children,
  className = "",
}: StatusItemProps) {
  const [open, setOpen] = useState(false);
  const hasMenu = Boolean(children);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [open]);

  const hint = title ?? label;
  const tint =
    open || active
      ? "bg-bg-hover text-text-primary"
      : "text-text-secondary hover:bg-bg-hover hover:text-text-primary";

  return (
    <div className={`relative flex shrink-0 ${className}`}>
      <button
        type="button"
        title={hint}
        aria-label={hint}
        aria-haspopup={hasMenu || undefined}
        aria-expanded={hasMenu ? open : undefined}
        aria-pressed={hasMenu ? undefined : active}
        onClick={() => (hasMenu ? setOpen((v) => !v) : onClick?.())}
        className={`flex h-full items-center gap-1.5 px-1.5 cursor-pointer transition-colors ${tint}`}
      >
        {Icon && <Icon size={13} className="shrink-0 opacity-80" />}
        <span className="flex min-w-0 items-center gap-1.5">{content ?? label}</span>
        {trailing}
      </button>

      {hasMenu && open && (
        <>
          <div className="fixed inset-0 z-[105]" onClick={() => setOpen(false)} />
          <div
            role="menu"
            aria-label={label}
            className={`absolute bottom-full mb-1 z-[110] min-w-[220px] max-w-[380px] rounded-lg border border-border-default bg-bg-card py-1 text-sm font-normal shadow-xl ${
              align === "end" ? "right-0" : "left-0"
            }`}
          >
            <CloseContext.Provider value={() => setOpen(false)}>{children}</CloseContext.Provider>
          </div>
        </>
      )}
    </div>
  );
};

// ---------------------------------------------------------------------------
// Menu rows
// ---------------------------------------------------------------------------

/** A menu line: icon, text, optional detail, and either an action or a check. */
export function StatusMenuRow({
  icon: Icon,
  text,
  detail,
  onClick,
  danger = false,
  disabled = false,
  checked,
  trailing,
  keepOpen = false,
}: {
  icon?: LucideIcon;
  text: ReactNode;
  detail?: ReactNode;
  onClick?: () => void;
  danger?: boolean;
  disabled?: boolean;
  /** Renders the check column, for a selectable list. */
  checked?: boolean;
  /** Extra right-aligned content. Rendered *inside* the menu button, so it must
   *  be non-interactive (a state word, a keyboard hint, a count) — nesting a
   *  button here would be invalid HTML. A row that acts is its own row. */
  trailing?: ReactNode;
  /** Skip the automatic dismiss, for a row whose action opens something that
   *  needs the menu's context kept (a sub-panel, a second click). */
  keepOpen?: boolean;
}) {
  const close = useContext(CloseContext);
  const interactive = Boolean(onClick) && !disabled;

  return (
    <button
      type="button"
      role="menuitem"
      disabled={!interactive}
      onClick={() => {
        if (!interactive) return;
        onClick?.();
        if (!keepOpen) close();
      }}
      className={`flex w-full items-center gap-2 px-3 py-1 text-left ${
        interactive ? "cursor-pointer hover:bg-bg-hover" : "cursor-default"
      } ${danger ? "text-red-500" : "text-text-secondary"} ${disabled ? "opacity-50" : ""}`}
    >
      <span className="w-3 shrink-0 text-accent-bg">
        {checked === undefined ? null : checked ? <Check size={12} /> : null}
      </span>
      {Icon && <Icon size={13} className="shrink-0 opacity-70" />}
      <span className="flex min-w-0 flex-1 items-baseline gap-2">
        <span className="truncate">{text}</span>
        {detail && <span className="truncate text-[11px] text-text-tertiary">{detail}</span>}
      </span>
      {trailing}
    </button>
  );
}

/** A non-interactive heading, so one menu can hold more than one subject. */
export function StatusMenuHeading({ children }: { children: ReactNode }) {
  return (
    <p className="px-3 pt-1 pb-0.5 text-[10px] uppercase tracking-wider text-text-tertiary">
      {children}
    </p>
  );
}

export function StatusMenuSep() {
  return <div className="my-1 border-t border-border-default" />;
}

/** Free-form block for the rare content that is not a command: a list of rows,
 *  a URL, a state line. */
export function StatusMenuBlock({ children }: { children: ReactNode }) {
  return <div className="px-3 py-1 text-text-secondary">{children}</div>;
}
