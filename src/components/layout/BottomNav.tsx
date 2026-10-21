"use client";

import { useState } from "react";
import {
  Headphones,
  Layers,
  Menu,
  BookOpen,
  MessagesSquare,
  BookOpenText,
  FolderKanban,
  Settings,
  FileText,
  Box,
  ArrowUpDown,
  Users,
  User,
  LogOut,
  Star,
  X,
} from "lucide-react";
import type { TabId } from "./Sidebar";
import { MicGlyph } from "@/components/voice/MicGlyph";
import { useAuth } from "@/hooks/useAuth";
import LoginModal from "@/components/auth/LoginModal";

type IconType = React.ComponentType<{ width?: number; height?: number; className?: string }>;

type NavItem = {
  id: TabId;
  label: string;
  icon: IconType;
};

// Primary tabs shown directly in the bottom bar.
const PRIMARY_TABS: NavItem[] = [
  { id: "dictation", label: "Dictation", icon: Headphones },
  { id: "cards", label: "Cards", icon: Layers },
];

// Overflow tabs surfaced inside the "More" left drawer.
const OVERFLOW_TABS: NavItem[] = [
  { id: "read-book", label: "Read a Book", icon: BookOpen },
  { id: "read-aloud", label: "Read Aloud", icon: MicGlyph },
  { id: "devices-hub", label: "Devices & Hub", icon: Users },
  { id: "datasets-sync", label: "Datasets Sync", icon: ArrowUpDown },
  { id: "chat", label: "Cross-Device Chat", icon: MessagesSquare },
  { id: "wiki", label: "Wiki", icon: BookOpenText },
  { id: "workspaces", label: "Workspaces", icon: FolderKanban },
  { id: "logs", label: "Logs", icon: FileText },
  { id: "models", label: "Models", icon: Box },
  { id: "settings", label: "Settings", icon: Settings },
];

const OVERFLOW_IDS: TabId[] = OVERFLOW_TABS.map((t) => t.id);

/**
 * Mobile-only bottom navigation. Replaces the desktop sidebar on the Android
 * thin client: two primary tabs plus a "More" drawer (slides in from the left,
 * same idiom as CollapsibleSidebar's mobile drawer) holding the remaining
 * sections and the login/user/XP controls. Rendered as a normal flex child
 * (see page.tsx), so page content is never obscured by it.
 */
export default function BottomNav({
  activeTab,
  onTabChange,
}: {
  activeTab: TabId;
  onTabChange: (id: TabId) => void;
}) {
  const [drawerOpen, setDrawerOpen] = useState(false);
  const { authUser, xpUser, xpFlash, login, logout, loginOpen, closeLogin } = useAuth();

  const moreActive = OVERFLOW_IDS.includes(activeTab);

  const go = (id: TabId) => {
    onTabChange(id);
    setDrawerOpen(false);
  };

  return (
    <>
      {/* Bottom bar */}
      <nav
        className="shrink-0 flex items-stretch border-t border-border-default bg-bg-card select-none"
        style={{ paddingBottom: "env(safe-area-inset-bottom)" }}
      >
        {PRIMARY_TABS.map((tab) => {
          const Icon = tab.icon;
          const active = activeTab === tab.id;
          return (
            <button
              key={tab.id}
              onClick={() => onTabChange(tab.id)}
              className={`flex-1 h-14 flex flex-col items-center justify-center gap-1 cursor-pointer transition-colors ${
                active ? "text-accent" : "text-text-secondary"
              }`}
            >
              <Icon width={22} height={22} />
              <span className="text-[11px] font-medium leading-none">{tab.label}</span>
            </button>
          );
        })}
        <button
          onClick={() => setDrawerOpen(true)}
          className={`flex-1 h-14 flex flex-col items-center justify-center gap-1 cursor-pointer transition-colors ${
            moreActive ? "text-accent" : "text-text-secondary"
          }`}
        >
          <Menu width={22} height={22} />
          <span className="text-[11px] font-medium leading-none">More</span>
        </button>
      </nav>

      {/* "More" left drawer: dimmed backdrop + panel sliding in from the left,
          the same idiom as CollapsibleSidebar's mobile drawer. Always mounted
          so the transform transition runs both ways; pointer-events toggle it
          out of the hit-test when closed. */}
      <div
        className={`fixed inset-0 z-50 bg-black/40 transition-opacity duration-200 ${
          drawerOpen ? "opacity-100" : "opacity-0 pointer-events-none"
        }`}
        onClick={() => setDrawerOpen(false)}
        aria-hidden="true"
      />
      <div
        className={`fixed inset-y-0 left-0 z-50 w-[80vw] max-w-[300px] flex flex-col bg-bg-card border-r border-border-default shadow-2xl transition-transform duration-200 ${
          drawerOpen ? "translate-x-0" : "-translate-x-full"
        }`}
        style={{
          paddingTop: "env(safe-area-inset-top)",
          paddingBottom: "env(safe-area-inset-bottom)",
        }}
        role="dialog"
        aria-modal="true"
        aria-label="More menus"
      >
        {/* Header: title + close */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-border-default shrink-0">
          <span className="text-sm font-semibold text-text-primary">More</span>
          <button
            onClick={() => setDrawerOpen(false)}
            className="p-1.5 rounded-md text-text-secondary hover:bg-bg-hover cursor-pointer"
            title="Close"
          >
            <X size={18} />
          </button>
        </div>

        {/* User / login section */}
        <div className="px-4 py-2 border-b border-border-default shrink-0">
          {authUser ? (
            <div className="flex items-center gap-3">
              <span className="w-9 h-9 rounded-full bg-accent-bg text-white flex items-center justify-center text-sm font-bold shrink-0">
                {authUser.name.charAt(0).toUpperCase()}
              </span>
              <div className="flex flex-col min-w-0 flex-1">
                <span className="text-sm font-semibold text-text-primary truncate">
                  {authUser.name}
                </span>
                <span className="flex items-center gap-2 text-[11px] text-text-tertiary">
                  <span className="leading-none">Lv.{xpUser?.level ?? 1}</span>
                  <span className="flex items-center gap-0.5 leading-none">
                    <Star size={11} className="text-yellow-500" />
                    {xpUser?.lifetime_xp ?? 0} XP
                  </span>
                  {(xpUser?.pending_xp ?? 0) > 0 && (
                    <span
                      className="text-text-tertiary/70 leading-none"
                      title={`${xpUser?.pending_xp} XP awaiting hub sync`}
                    >
                      (+{xpUser?.pending_xp}⋯)
                    </span>
                  )}
                  {xpFlash !== null && (
                    <span className="text-yellow-500 font-bold animate-bounce">
                      +{xpFlash}
                    </span>
                  )}
                </span>
              </div>
              <button
                onClick={logout}
                className="flex items-center gap-1.5 px-3 py-1.5 rounded-md text-xs text-text-secondary hover:bg-bg-hover cursor-pointer shrink-0"
                title="Logout"
              >
                <LogOut size={15} />
                Logout
              </button>
            </div>
          ) : (
            <button
              onClick={login}
              className="flex items-center gap-2 w-full px-3 py-2 rounded-md text-sm font-medium text-text-secondary hover:bg-bg-hover cursor-pointer"
            >
              <User size={18} className="text-text-tertiary" />
              Login
            </button>
          )}
        </div>

        {/* Overflow navigation — scrolls when the list exceeds the drawer */}
        <div className="flex-1 min-h-0 overflow-y-auto flex flex-col p-2">
          {OVERFLOW_TABS.map((tab) => {
            const Icon = tab.icon;
            const active = activeTab === tab.id;
            return (
              <button
                key={tab.id}
                onClick={() => go(tab.id)}
                className={`flex items-center gap-3 px-3 py-3 rounded-lg cursor-pointer transition-colors ${
                  active
                    ? "bg-logo-primary/80 text-white"
                    : "text-text-primary hover:bg-bg-hover"
                }`}
              >
                <Icon width={20} height={20} className="shrink-0" />
                <span className="text-sm font-medium">{tab.label}</span>
              </button>
            );
          })}
        </div>
      </div>

      <LoginModal open={loginOpen} onClose={closeLogin} />
    </>
  );
}
