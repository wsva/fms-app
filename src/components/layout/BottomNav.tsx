"use client";

import { useState } from "react";
import {
  Headphones,
  Layers,
  Menu,
  BookOpen,
  MessagesSquare,
  SlidersHorizontal,
  BookOpenText,
  FolderKanban,
  Settings,
  FileText,
  Box,
  User,
  LogOut,
  Star,
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

// Overflow tabs surfaced inside the "More" bottom sheet.
const OVERFLOW_TABS: NavItem[] = [
  { id: "read-book", label: "Read a Book", icon: BookOpen },
  { id: "read-aloud", label: "Read Aloud", icon: MicGlyph },
  { id: "studio", label: "Datasets", icon: SlidersHorizontal },
  { id: "wiki", label: "Wiki", icon: BookOpenText },
  { id: "chat", label: "Device Chat", icon: MessagesSquare },
  { id: "workspaces", label: "Workspaces", icon: FolderKanban },
  { id: "logs", label: "Logs", icon: FileText },
  { id: "models", label: "Models", icon: Box },
  { id: "settings", label: "Settings", icon: Settings },
];

const OVERFLOW_IDS: TabId[] = OVERFLOW_TABS.map((t) => t.id);

/**
 * Mobile-only bottom navigation. Replaces the desktop sidebar on the Android
 * thin client: two primary tabs plus a "More" sheet holding the remaining
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
  const [sheetOpen, setSheetOpen] = useState(false);
  const { authUser, xpUser, xpFlash, login, logout, loginOpen, closeLogin } = useAuth();

  const moreActive = OVERFLOW_IDS.includes(activeTab);

  const go = (id: TabId) => {
    onTabChange(id);
    setSheetOpen(false);
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
          onClick={() => setSheetOpen(true)}
          className={`flex-1 h-14 flex flex-col items-center justify-center gap-1 cursor-pointer transition-colors ${
            moreActive ? "text-accent" : "text-text-secondary"
          }`}
        >
          <Menu width={22} height={22} />
          <span className="text-[11px] font-medium leading-none">More</span>
        </button>
      </nav>

      {/* "More" bottom sheet */}
      {sheetOpen && (
        <div
          className="fixed inset-0 z-50 flex items-end bg-black/40"
          onClick={() => setSheetOpen(false)}
        >
          <div
            className="w-full bg-bg-card rounded-t-2xl border-t border-border-default shadow-2xl flex flex-col"
            style={{ paddingBottom: "env(safe-area-inset-bottom)" }}
            onClick={(e) => e.stopPropagation()}
          >
            {/* Grab handle */}
            <div className="flex justify-center pt-2 pb-1">
              <div className="w-10 h-1 rounded-full bg-border-default" />
            </div>

            {/* User / login section */}
            <div className="px-4 py-2 border-b border-border-default">
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

            {/* Overflow navigation */}
            <div className="flex flex-col p-2">
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
        </div>
      )}

      <LoginModal open={loginOpen} onClose={closeLogin} />
    </>
  );
}
