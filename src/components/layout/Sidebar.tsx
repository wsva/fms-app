"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { isMobileApp } from "@/lib/platform";
import { useAuth } from "@/hooks/useAuth";
import LoginModal from "@/components/auth/LoginModal";
import {
  Box,
  SlidersHorizontal,
  Settings,
  Headphones,
  MessageSquare,
  MessagesSquare,
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
  Type,
  RefreshCw,
} from "lucide-react";
import { MicGlyph } from "@/components/voice/MicGlyph";

export type TabId =
  | "dictation"
  | "read-book"
  | "read-aloud"
  | "cards"
  | "studio"
  | "datasets-sync"
  | "simple-words"
  | "models"
  | "edge-tts"
  | "llm-chat"
  | "chat"
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
      { id: "datasets-sync", label: "Datasets Sync", icon: RefreshCw },
      { id: "simple-words", label: "Simple Words", icon: Type },
      { id: "wiki", label: "Wiki", icon: BookOpenText },
      { id: "edge-tts", label: "Edge TTS", icon: Volume2 },
      { id: "llm-chat", label: "LLM Chat", icon: MessageSquare },
      { id: "chat", label: "Device Chat", icon: MessagesSquare },
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
  { id: "read-aloud", label: "Read Aloud", icon: MicGlyph },
  { id: "cards", label: "Cards", icon: Layers },
  { id: "settings", label: "Settings", icon: Settings },
];

// Tabs reachable in the Android thin client. Everything else (OCR,
// Edge TTS, LLM Chat, Logs, Simple Words, Dataset Studio pipeline) is hidden.
// "models" is enabled to verify local STT (transcribe-rs/ONNX) on Android.
const MOBILE_VISIBLE_TABS: TabId[] = [
  "dictation",
  "read-book",
  "read-aloud",
  "cards",
  "wiki",
  "chat",
  "workspaces",
  "datasets-sync",
  "models",
  "settings",
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
  const [mobile, setMobile] = useState(false);
  const startXRef = useRef(0);
  const startWidthRef = useRef(0);
  const [expandedGroups, setExpandedGroups] = useState<Record<string, boolean>>({
    tools: true,
  });

  // ---- Auth state (shared with the mobile bottom nav via useAuth) ----
  const { authUser, xpUser, xpFlash, login, logout, loginOpen, closeLogin } = useAuth();

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

  useEffect(() => {
    setMobile(isMobileApp());
  }, []);

  const renderTab = (tab: TabDef, isChild: boolean = false) => {
    if (mobile && !MOBILE_VISIBLE_TABS.includes(tab.id)) return null;
    const Icon = tab.icon;
    const isActive = activeTab === tab.id;
    const label = tab.label;
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
        title={collapsed ? label : undefined}
      >
        <Icon width={isChild ? 18 : 20} height={isChild ? 18 : 20} className="shrink-0" />
        {!collapsed && (
          <p className={`${isChild ? "text-xs" : "text-sm"} font-medium truncate`} title={label}>
            {label}
          </p>
        )}
      </div>
    );
  };

  const renderGroup = (group: NavGroup) => {
    const GroupIcon = group.icon;
    const isExpanded = expandedGroups[group.id] !== false;
    const hasActiveTab = group.tabs.some((t) => t.id === activeTab);

    // Mobile: drop the collapsible group header and surface only the allowed
    // tabs as flat top-level entries (Workspaces, Wiki, Datasets).
    if (mobile) {
      const visible = group.tabs.filter((t) => MOBILE_VISIBLE_TABS.includes(t.id));
      if (visible.length === 0) return null;
      return (
        <div key={group.id} className="flex flex-col w-full gap-1">
          {visible.map((tab) => renderTab(tab, false))}
        </div>
      );
    }

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
              onClick={logout}
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
            onClick={login}
            className={`flex items-center w-full rounded-lg px-2 py-1.5 hover:bg-mid-gray/20 transition-colors cursor-pointer ${
              collapsed ? "justify-center" : "gap-2"
            }`}
            title="Login"
          >
            <User size={18} className="text-text-tertiary shrink-0" />
            {!collapsed && <span className="text-xs font-medium text-text-secondary">Login</span>}
          </button>
        )}

        <div className="w-full border-b border-border-default my-1" />

        {/* Root tabs above groups: Dictation, Read a Book, Read Aloud, Cards */}
        {rootTabs.slice(0, 4).map((tab) => renderTab(tab, false))}

        {/* Navigation groups */}
        {navGroups.map(renderGroup)}

        {/* Settings at bottom of nav */}
        {rootTabs.slice(4).map((tab) => renderTab(tab, false))}
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

      <LoginModal open={loginOpen} onClose={closeLogin} />
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
