"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import {
  Box,
  Database,
  Table,
  Folder,
  SlidersHorizontal,
  Settings,
  Headphones,
  PanelLeftClose,
  PanelLeftOpen,
  MessageSquare,
  Volume2,
  Wrench,
  BookOpen,
  Globe,
  Layers,
  ScanText,
  FileText,
  AudioLines,
} from "lucide-react";

export type TabId =
  | "dictation"
  | "read-book"
  | "cards"
  | "datasets"
  | "dictation-dataset"
  | "studio"
  | "models"
  | "edge-tts"
  | "llm-chat"
  | "web-service"
  | "ocr"
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
    id: "datasets",
    label: "Datasets",
    icon: Database,
    tabs: [
      { id: "datasets", label: "Location", icon: Folder },
      { id: "dictation-dataset", label: "Modify", icon: Table },
      { id: "studio", label: "Studio", icon: SlidersHorizontal },
    ],
  },
  {
    id: "tools",
    label: "Tools",
    icon: Wrench,
    tabs: [
      { id: "models", label: "Models", icon: Box },
      { id: "edge-tts", label: "Edge TTS", icon: Volume2 },
      { id: "llm-chat", label: "LLM Chat", icon: MessageSquare },
      { id: "web-service", label: "Web Service", icon: Globe },
      { id: "ocr", label: "OCR", icon: ScanText },
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
        className="fixed left-0 top-0 flex flex-col h-screen border-r border-border-default items-center px-2 z-10 bg-bg-card"
        style={{
          width: sidebarWidth,
          transition: isDragging ? "none" : "width 200ms",
        }}
      >
      <nav className="flex flex-col w-full items-center gap-1 pt-4">
        {/* Root tabs above groups: Dictation, Read a Book, Cards */}
        {rootTabs.slice(0, 3).map((tab) => renderTab(tab, false))}

        {/* Navigation groups */}
        {navGroups.map(renderGroup)}

        {/* Settings at bottom of nav */}
        {rootTabs.slice(3).map((tab) => renderTab(tab, false))}
      </nav>

      <div className="mt-auto pb-4 w-full">
        <button
          className={`flex items-center p-2 w-full rounded-lg cursor-pointer transition-colors text-text-tertiary hover:bg-mid-gray/20 ${
            collapsed ? "justify-center" : "gap-2"
          }`}
          onClick={handleToggle}
          title={collapsed ? "Expand sidebar" : "Collapse sidebar"}
        >
          {collapsed ? <PanelLeftOpen size={20} /> : <PanelLeftClose size={20} />}
          {!collapsed && <span className="text-sm">Collapse</span>}
        </button>
      </div>

      {/* Resize handle */}
      {!collapsed && (
        <div
          className="absolute top-0 right-0 h-full w-1 cursor-col-resize hover:bg-accent/50 active:bg-accent transition-colors"
          onMouseDown={handleMouseDown}
        />
      )}
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
