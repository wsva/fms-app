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
} from "lucide-react";

export type TabId = "dashboard" | "datasets" | "dictation" | "models" | "settings";

const tabs: { id: TabId; label: string; icon: React.ComponentType<{ width?: number; height?: number; className?: string }> }[] = [
  { id: "dashboard", label: "Dashboard", icon: LayoutDashboard },
  { id: "datasets", label: "Datasets", icon: Database },
  { id: "dictation", label: "Dictation", icon: Headphones },
  { id: "models", label: "Models", icon: Box },
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

  const handleToggle = () => {
    const next = !collapsed;
    setCollapsed(next);
    onCollapseChange?.(next);
  };

  return (
    <aside
      className={`fixed left-0 top-0 flex flex-col h-screen border-r border-border-default items-center px-2 z-10 bg-bg-card transition-all duration-200 ${
        collapsed ? "w-12" : "w-40"
      }`}
    >
      <nav className="flex flex-col w-full items-center gap-1 pt-4">
        {tabs.map((tab) => {
          const Icon = tab.icon;
          const isActive = activeTab === tab.id;
          return (
            <div
              key={tab.id}
              className={`flex items-center p-2 w-full rounded-lg cursor-pointer transition-colors ${
                collapsed ? "justify-center" : "gap-2"
              } ${
                isActive
                  ? "bg-logo-primary/80"
                  : "hover:bg-mid-gray/20 hover:opacity-100 opacity-85"
              }`}
              onClick={() => onTabChange(tab.id)}
              title={collapsed ? tab.label : undefined}
            >
              <Icon width={24} height={24} className="shrink-0" />
              {!collapsed && (
                <p className="text-sm font-medium truncate" title={tab.label}>
                  {tab.label}
                </p>
              )}
            </div>
          );
        })}
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
export const SIDEBAR_WIDTH = { expanded: "ml-40", collapsed: "ml-12" } as const;
