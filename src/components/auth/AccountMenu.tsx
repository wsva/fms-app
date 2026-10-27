"use client";

import { useEffect, useState } from "react";
import { FolderKanban, LogOut } from "lucide-react";
import LevelAvatar from "./LevelAvatar";
import { levelProgress } from "@/lib/xp";

/**
 * The sidebar's account footer: a read-only identity trigger (avatar ring, name,
 * level/XP) that opens a panel with the details and the two account actions.
 *
 * Deliberately NOT using `useAuth()` here: the sidebar already runs it for the
 * logged-in / checking / logged-out choice, and a second instance would double
 * the `auth_get_user` round trip and every `auth-*` listener. Values come in as
 * props from that single hook instance instead.
 *
 * Logout is the only destructive control and it lives inside the panel, not on
 * the trigger — the old markup made the identity row itself the logout button,
 * so glancing at who you were signed in as could sign you out.
 */
export default function AccountMenu({
  collapsed,
  name,
  email,
  xp,
  pendingXp,
  flash,
  onLogout,
  onWorkspaces,
}: {
  collapsed: boolean;
  name: string;
  email: string;
  xp: number;
  pendingXp: number;
  flash: number | null;
  onLogout: () => void;
  onWorkspaces: () => void;
}) {
  const [open, setOpen] = useState(false);
  const { level, toNext, pct } = levelProgress(xp);

  // Escape closes, matching what a menu is expected to do. The tap-away
  // backdrop below covers the mouse/touch path (a WebView fires no mouseleave).
  useEffect(() => {
    if (!open) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [open]);

  const run = (action: () => void) => () => {
    setOpen(false);
    action();
  };

  return (
    <div className="relative w-full">
      <button
        onClick={() => setOpen((v) => !v)}
        aria-haspopup="menu"
        aria-expanded={open}
        title={`${name} — level ${level}, ${xp} XP`}
        className={`flex items-center w-full rounded-lg px-2 py-1.5 hover:bg-mid-gray/20 transition-colors cursor-pointer ${
          collapsed ? "justify-center" : "gap-2"
        }`}
      >
        <LevelAvatar name={name} xp={xp} size={collapsed ? 24 : 28} flash={flash} />
        {!collapsed && (
          <span className="flex flex-col min-w-0 flex-1 text-left">
            <span className="text-xs font-medium text-text-primary truncate">{name}</span>
            <span className="text-[10px] leading-none text-text-tertiary truncate mt-0.5">
              Lv.{level} · {xp} XP
            </span>
          </span>
        )}
      </button>

      {open && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setOpen(false)} />
          <div
            role="menu"
            aria-label="Account"
            className={`absolute z-50 w-64 rounded-lg border border-border-default bg-bg-card shadow-xl py-1 ${
              // The sidebar is narrower than the panel, so it opens towards the
              // content area: upwards from the footer, or rightwards out of the
              // collapsed rail. `aside` sets no overflow, so neither is clipped.
              collapsed ? "left-full ml-1 bottom-0" : "left-0 bottom-full mb-1"
            }`}
          >
            <div className="px-3 py-2">
              <p className="text-sm font-semibold text-text-primary truncate">{name}</p>
              {email && (
                <p className="text-[11px] text-text-tertiary truncate" title={email}>
                  {email}
                </p>
              )}
            </div>

            <div className="px-3 pb-2">
              <div className="flex items-center justify-between text-[11px] text-text-secondary mb-1">
                <span>Level {level}</span>
                <span className="text-text-tertiary">{xp} XP</span>
              </div>
              <div className="h-1.5 rounded-full bg-border-default overflow-hidden">
                <div
                  className="h-full rounded-full bg-accent"
                  style={{ width: `${(pct * 100).toFixed(1)}%` }}
                />
              </div>
              <p className="mt-1 text-[10px] text-text-tertiary">
                {toNext > 0 ? `${toNext} XP to level ${level + 1}` : "Top of the level curve"}
              </p>
              {pendingXp > 0 && (
                <p className="mt-0.5 text-[10px] text-text-tertiary/80">
                  {pendingXp} XP awaiting hub sync
                </p>
              )}
            </div>

            <div className="border-t border-border-default my-1" />
            <button
              role="menuitem"
              onClick={run(onWorkspaces)}
              className="flex items-center gap-2 w-full px-3 py-1.5 text-xs text-text-secondary hover:bg-bg-hover cursor-pointer"
            >
              <FolderKanban size={14} className="shrink-0" />
              Switch workspace
            </button>

            <div className="border-t border-border-default my-1" />
            <button
              role="menuitem"
              onClick={run(onLogout)}
              className="flex items-center gap-2 w-full px-3 py-1.5 text-xs text-red-500 hover:bg-red-500/10 cursor-pointer"
            >
              <LogOut size={14} className="shrink-0" />
              Log out
            </button>
          </div>
        </>
      )}
    </div>
  );
}
