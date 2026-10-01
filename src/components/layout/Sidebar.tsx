"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Box,
  SlidersHorizontal,
  Settings,
  Headphones,
  MessageSquare,
  Volume2,
  Wrench,
  BookOpen,
  BookOpenText,
  Layers,
  ScanText,
  FileText,
  AudioLines,
  User,
  LogOut,
  Star,
  FolderKanban,
} from "lucide-react";

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

interface AuthUser {
  name: string;
  email: string;
}

interface XpUser {
  user_id: string;
  lifetime_xp: number;
  level: number;
  updated_at: string;
}

interface XpAwardResult {
  xp_awarded: number;
  lifetime_xp: number;
  level: number;
  is_new: boolean;
}

export type TabId =
  | "dictation"
  | "read-book"
  | "cards"
  | "studio"
  | "models"
  | "edge-tts"
  | "llm-chat"
  | "ocr"
  | "wiki"
  | "workspaces"
  | "logs"
  | "settings";

type TabDef = {
  id: TabId;
  label: string;
  icon: React.ComponentType<{ width?: number; height?: number; className?: string }>;
};

type NavGroup = {
  id: string;
  label: string;
  icon: React.ComponentType<{ width?: number; height?: number; className?: string }>;
  tabs: TabDef[];
};

// Navigation structure with groups
const navGroups: NavGroup[] = [
  {
    id: "tools",
    label: "Tools",
    icon: Wrench,
    tabs: [
      { id: "workspaces", label: "Workspaces", icon: FolderKanban },
      { id: "studio", label: "Dataset Studio", icon: SlidersHorizontal },
      { id: "wiki", label: "Wiki", icon: BookOpenText },
      { id: "edge-tts", label: "Edge TTS", icon: Volume2 },
      { id: "llm-chat", label: "LLM Chat", icon: MessageSquare },
      { id: "ocr", label: "OCR", icon: ScanText },
      { id: "models", label: "Models", icon: Box },
      { id: "logs", label: "Logs", icon: FileText },
    ],
  },
];

// Root-level tabs (not in any group)
const rootTabs: TabDef[] = [
  { id: "dictation", label: "Dictation", icon: Headphones },
  { id: "read-book", label: "Read a Book", icon: BookOpen },
  { id: "cards", label: "Cards", icon: Layers },
  { id: "settings", label: "Settings", icon: Settings },
];

const SIDEBAR_WIDTH_KEY = "sidebar-width";
const DEFAULT_EXPANDED_WIDTH = 176; // w-44 = 11rem = 176px
const MIN_WIDTH = 120;
const MAX_WIDTH = 320;
const COLLAPSED_WIDTH = 48; // w-12 = 3rem = 48px

export default function Sidebar({
  activeTab,
  onTabChange,
  onCollapseChange,
  onWidthChange,
}: {
  activeTab: TabId;
  onTabChange: (id: TabId) => void;
  onCollapseChange?: (collapsed: boolean) => void;
  onWidthChange?: (width: number) => void;
}) {
  const [collapsed, setCollapsed] = useState(false);
  const [expandedWidth, setExpandedWidth] = useState(DEFAULT_EXPANDED_WIDTH);
  const [isDragging, setIsDragging] = useState(false);
  const startXRef = useRef(0);
  const startWidthRef = useRef(0);
  const [expandedGroups, setExpandedGroups] = useState<Record<string, boolean>>({
    tools: true,
  });

  // ---- Auth state ----
  const [authUser, setAuthUser] = useState<AuthUser | null>(null);
  const [xpUser, setXpUser] = useState<XpUser | null>(null);
  const [xpFlash, setXpFlash] = useState<number | null>(null);

  const checkAuth = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const user = await invoke<AuthUser | null>("auth_get_user");
      setAuthUser(user);
      if (user) {
        const xp = await invoke<XpUser | null>("xp_get_user");
        setXpUser(xp);
      } else {
        setXpUser(null);
      }
    } catch {
      /* ignore */
    }
  }, []);

  useEffect(() => {
    checkAuth();
    if (!isTauri()) return;
    const unlistenLogin = listen<AuthUser>("auth-login-success", (event) => {
      setAuthUser(event.payload);
      // Fetch XP after login.
      invoke<XpUser | null>("xp_get_user").then(setXpUser).catch(() => {});
    });
    const unlistenLogout = listen("auth-logout", () => {
      setAuthUser(null);
      setXpUser(null);
    });
    // Auth is per-workspace: on workspace switch, clear the old user and
    // reload auth/XP from the newly selected workspace's auth.json.
    const unlistenWorkspace = listen("workspace-selected", () => {
      setAuthUser(null);
      setXpUser(null);
      checkAuth();
    });
    const unlistenXp = listen<XpAwardResult>("xp-earned", (event) => {
      setXpUser((prev) =>
        prev
          ? { ...prev, lifetime_xp: event.payload.lifetime_xp, level: event.payload.level }
          : { user_id: "", lifetime_xp: event.payload.lifetime_xp, level: event.payload.level, updated_at: new Date().toISOString() }
      );
      // Flash the XP gain.
      setXpFlash(event.payload.xp_awarded);
      setTimeout(() => setXpFlash(null), 2000);
    });
    return () => {
      unlistenLogin.then((fn) => fn());
      unlistenLogout.then((fn) => fn());
      unlistenWorkspace.then((fn) => fn());
      unlistenXp.then((fn) => fn());
    };
  }, [checkAuth]);

  async function handleLogin() {
    if (!isTauri()) return;
    try {
      await invoke("auth_open_login");
    } catch {
      /* ignore */
    }
  }

  async function handleLogout() {
    if (!isTauri()) return;
    try {
      await invoke("auth_logout");
      setAuthUser(null);
    } catch {
      /* ignore */
    }
  }

  // Load saved width from localStorage
  useEffect(() => {
    const saved = localStorage.getItem(SIDEBAR_WIDTH_KEY);
    if (saved) {
      const w = parseInt(saved, 10);
      if (w >= MIN_WIDTH && w <= MAX_WIDTH) setExpandedWidth(w);
    }
  }, []);

  // Notify parent of width changes
  useEffect(() => {
    const currentWidth = collapsed ? COLLAPSED_WIDTH : expandedWidth;
    onWidthChange?.(currentWidth);
  }, [collapsed, expandedWidth, onWidthChange]);

  // Handle resize drag
  const handleMouseDown = useCallback((e: React.MouseEvent) => {
    if (collapsed) return;
    e.preventDefault();
    startXRef.current = e.clientX;
    startWidthRef.current = expandedWidth;
    setIsDragging(true);
  }, [collapsed, expandedWidth]);

  useEffect(() => {
    if (!isDragging) return;

    const handleMouseMove = (e: MouseEvent) => {
      const delta = e.clientX - startXRef.current;
      const newWidth = Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, startWidthRef.current + delta));
      setExpandedWidth(newWidth);
    };

    const handleMouseUp = () => {
      setIsDragging(false);
      localStorage.setItem(SIDEBAR_WIDTH_KEY, String(expandedWidth));
    };

    document.addEventListener("mousemove", handleMouseMove);
    document.addEventListener("mouseup", handleMouseUp);
    return () => {
      document.removeEventListener("mousemove", handleMouseMove);
      document.removeEventListener("mouseup", handleMouseUp);
    };
  }, [isDragging, expandedWidth]);

  const sidebarWidth = collapsed ? COLLAPSED_WIDTH : expandedWidth;

  const handleToggle = () => {
    const next = !collapsed;
    setCollapsed(next);
    onCollapseChange?.(next);
  };

  const toggleGroup = (groupId: string) => {
    if (collapsed) return; // Don't toggle groups when sidebar is collapsed
    setExpandedGroups((prev) => ({ ...prev, [groupId]: !prev[groupId] }));
  };

  const renderTab = (tab: TabDef, isChild: boolean = false) => {
    const Icon = tab.icon;
    const isActive = activeTab === tab.id;
    return (
      <div
        key={tab.id}
        className={`flex items-center p-2 w-full rounded-lg cursor-pointer transition-colors ${
          collapsed ? "justify-center" : isChild ? "gap-2 pl-10" : "gap-2"
        } ${
          isActive
            ? "bg-logo-primary/80"
            : "hover:bg-mid-gray/20 hover:opacity-100 opacity-85"
        }`}
        onClick={() => onTabChange(tab.id)}
        title={collapsed ? tab.label : undefined}
      >
        <Icon width={isChild ? 18 : 20} height={isChild ? 18 : 20} className="shrink-0" />
        {!collapsed && (
          <p className={`${isChild ? "text-xs" : "text-sm"} font-medium truncate`} title={tab.label}>
            {tab.label}
          </p>
        )}
      </div>
    );
  };

  const renderGroup = (group: NavGroup) => {
    const GroupIcon = group.icon;
    const isExpanded = expandedGroups[group.id] !== false;
    const hasActiveTab = group.tabs.some((t) => t.id === activeTab);

    return (
      <div key={group.id} className="w-full">
        {/* Group header - same style as top-level tabs */}
        <div
          className={`flex items-center p-2 w-full rounded-lg cursor-pointer transition-colors ${
            collapsed ? "justify-center" : "gap-2"
          } ${
            hasActiveTab
              ? "bg-logo-primary/80"
              : "hover:bg-mid-gray/20 hover:opacity-100 opacity-85"
          }`}
          onClick={() => toggleGroup(group.id)}
          title={collapsed ? group.label : undefined}
        >
          <GroupIcon width={20} height={20} className="shrink-0" />
          {!collapsed && (
            <>
              <p className="text-sm font-medium flex-1 truncate" title={group.label}>
                {group.label}
              </p>
              <span
                className={`text-xs text-text-tertiary transition-transform ${
                  isExpanded ? "rotate-90" : ""
                }`}
              >
                ▶
              </span>
            </>
          )}
        </div>

        {/* Group tabs (only when expanded and not collapsed) */}
        {!collapsed && isExpanded && (
          <div className="flex flex-col gap-0.5 mt-0.5">
            {group.tabs.map((tab) => renderTab(tab, true))}
          </div>
        )}
      </div>
    );
  };

  return (
    <>
      <aside
        className="fixed left-0 top-0 flex flex-col h-screen border-r border-border-default items-center px-2 z-10 bg-bg-card select-none"
        style={{
          width: sidebarWidth,
          transition: isDragging ? "none" : "width 200ms",
        }}
        onDoubleClick={(e) => {
          // Only toggle when double-clicking blank space (not buttons/tabs).
          const target = e.target as HTMLElement;
          if (target.closest("button, a, input, select, [role='tab']")) return;
          handleToggle();
        }}
      >
      <nav className="flex flex-col w-full items-center gap-1 pt-2">
        {/* Auth button at top */}
        {authUser ? (
          <div className="flex flex-col w-full items-center gap-0.5">
            {/* Logout button */}
            <button
              onClick={handleLogout}
              className={`flex items-center w-full rounded-lg px-2 py-1 hover:bg-mid-gray/20 transition-colors cursor-pointer gap-2 ${
                collapsed ? "justify-center" : ""
              }`}
              title={collapsed ? `${authUser.name} (click to logout)` : "Click to logout"}
            >
              <span className="w-6 h-6 rounded-full bg-accent-bg text-white flex items-center justify-center text-xs font-bold shrink-0">
                {authUser.name.charAt(0).toUpperCase()}
              </span>
              {!collapsed && (
                <>
                  <span className="text-xs font-medium text-text-primary truncate flex-1 text-left">
                    {authUser.name}
                  </span>
                  <LogOut size={14} className="text-text-tertiary shrink-0" />
                </>
              )}
            </button>
            {/* XP status */}
            {!collapsed && (
              <div className="flex items-center justify-center gap-2 px-2 py-0.5 text-[10px] leading-none text-text-tertiary">
                <span className="flex items-center gap-0.5 leading-none">
                  Lv.{xpUser?.level ?? 1}
                </span>
                <span className="flex items-center gap-0.5 leading-none">
                  <Star size={10} className="text-yellow-500" />
                  {xpUser?.lifetime_xp ?? 0} XP
                </span>
                {xpFlash !== null && (
                  <span className="text-yellow-500 font-bold animate-bounce">
                    +{xpFlash}
                  </span>
                )}
              </div>
            )}
            {collapsed && xpFlash !== null && (
              <span className="text-[9px] text-yellow-500 font-bold text-center animate-bounce">
                +{xpFlash}
              </span>
            )}
          </div>
        ) : (
          <button
            onClick={handleLogin}
            className={`flex items-center w-full rounded-lg px-2 py-1.5 hover:bg-mid-gray/20 transition-colors cursor-pointer ${
              collapsed ? "justify-center" : "gap-2"
            }`}
            title={collapsed ? "Login" : "Login in browser"}
          >
            <User size={18} className="text-text-tertiary shrink-0" />
            {!collapsed && <span className="text-xs font-medium text-text-secondary">Login</span>}
          </button>
        )}

        <div className="w-full border-b border-border-default my-1" />

        {/* Root tabs above groups: Dictation, Read a Book, Cards */}
        {rootTabs.slice(0, 3).map((tab) => renderTab(tab, false))}

        {/* Navigation groups */}
        {navGroups.map(renderGroup)}

        {/* Settings at bottom of nav */}
        {rootTabs.slice(3).map((tab) => renderTab(tab, false))}
      </nav>

      {/* Resize handle */}
      <div
        className={`absolute top-0 right-0 h-full ${collapsed ? "w-2" : "w-1 cursor-col-resize hover:bg-accent/50 active:bg-accent"} transition-colors`}
        onMouseDown={!collapsed ? handleMouseDown : undefined}
      />
    </aside>

      {/* Overlay to capture mouse events during drag */}
      {isDragging && (
        <div className="fixed inset-0 z-50 cursor-col-resize" />
      )}
    </>
  );
}

/** Width constants for sidebar states */
export const SIDEBAR_WIDTH = {
  collapsed: COLLAPSED_WIDTH,
  defaultExpanded: DEFAULT_EXPANDED_WIDTH,
  min: MIN_WIDTH,
  max: MAX_WIDTH,
} as const;
