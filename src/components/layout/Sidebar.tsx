"use client";

import { useState } from "react";
import {
  Box,
  LayoutDashboard,
  Database,
  Settings,
  Headphones,
  PanelLeftClose,
  PanelLeftOpen,
  MessageSquare,
  Languages,
  Volume2,
  Wrench,
  Share2,
} from "lucide-react";

export type TabId =
  | "dashboard"
  | "dictation"
  | "datasets"
  | "models"
  | "edge-tts"
  | "llm-chat"
  | "p2p-share"
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
    id: "listen-speak",
    label: "Listen & Speak",
    icon: Languages,
    tabs: [
      { id: "dictation", label: "Dictation", icon: Headphones },
      { id: "edge-tts", label: "TTS", icon: Volume2 },
      { id: "datasets", label: "Datasets", icon: Database },
      { id: "models", label: "Models", icon: Box },
    ],
  },
  {
    id: "llm",
    label: "LLM",
    icon: MessageSquare,
    tabs: [
      { id: "llm-chat", label: "Chat", icon: MessageSquare },
    ],
  },
  {
    id: "tools",
    label: "Tools",
    icon: Wrench,
    tabs: [
      { id: "p2p-share", label: "P2P Share", icon: Share2 },
    ],
  },
];

// Root-level tabs (not in any group)
const rootTabs: TabDef[] = [
  { id: "dashboard", label: "Dashboard", icon: LayoutDashboard },
  { id: "settings", label: "Settings", icon: Settings },
];

export default function Sidebar({
  activeTab,
  onTabChange,
  onCollapseChange,
}: {
  activeTab: TabId;
  onTabChange: (id: TabId) => void;
  onCollapseChange?: (collapsed: boolean) => void;
}) {
  const [collapsed, setCollapsed] = useState(false);
  const [expandedGroups, setExpandedGroups] = useState<Record<string, boolean>>({
    "listen-speak": true,
    llm: true,
    tools: true,
  });

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
    <aside
      className={`fixed left-0 top-0 flex flex-col h-screen border-r border-border-default items-center px-2 z-10 bg-bg-card transition-all duration-200 ${
        collapsed ? "w-12" : "w-44"
      }`}
    >
      <nav className="flex flex-col w-full items-center gap-1 pt-4">
        {/* Root tabs */}
        {rootTabs.slice(0, 1).map((tab) => renderTab(tab, false))} {/* Dashboard first */}

        {/* Navigation groups */}
        {navGroups.map(renderGroup)}

        {/* Settings at bottom of nav */}
        {rootTabs.slice(1).map((tab) => renderTab(tab, false))}
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
    </aside>
  );
}

/** Width in px corresponding to the sidebar's current state — used for ml offset */
export const SIDEBAR_WIDTH = { expanded: "ml-44", collapsed: "ml-12" } as const;
