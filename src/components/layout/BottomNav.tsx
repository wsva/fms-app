"use client";

import { useState } from "react";
import {
  Menu,
  User,
  LogOut,
  Star,
  X,
} from "lucide-react";
import type { TabId } from "./Sidebar";
import {
  NAV_CROSS_DEVICE_ICON,
  NAV_CROSS_DEVICE_LABEL,
  NAV_TOOLS_ICON,
  NAV_TOOLS_LABEL,
  useNavConfig,
  type NavMenu,
} from "@/lib/mobile-nav";
import { useAuth } from "@/hooks/useAuth";
import LoginModal from "@/components/auth/LoginModal";
import LevelAvatar from "@/components/auth/LevelAvatar";

/**
 * Mobile-only bottom navigation. Replaces the desktop sidebar on the Android
 * thin client: the primary tabs plus a "More" drawer (slides in from the left,
 * same idiom as CollapsibleSidebar's mobile drawer) holding the remaining
 * sections and the login/user/XP controls. The drawer mirrors the PC sidebar's
 * structure — root menus, collapsible Cross Device and Tools groups, Settings
 * at the bottom —
 * so the same mental model applies on both. Which menus sit in the bar versus
 * the drawer — and in what order inside each zone — is user-configurable in
 * Settings → Bottom Nav (see lib/mobile-nav). Rendered as a normal flex child
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
  // Same default the PC sidebar uses: both group headers start expanded.
  const [crossOpen, setCrossOpen] = useState(true);
  const [toolsOpen, setToolsOpen] = useState(true);
  const { barMenus, drawerMenus, drawerZones } = useNavConfig();
  const { authUser, xpUser, xpFlash, login, logout, loginOpen, closeLogin } = useAuth();

  const moreActive = drawerMenus.some((t) => t.id === activeTab);
  // The group headers light up like the PC sidebar's when a member is current.
  const CrossIcon = NAV_CROSS_DEVICE_ICON;
  const crossActive = drawerZones.crossDevice.some((t) => t.id === activeTab);
  const ToolsIcon = NAV_TOOLS_ICON;
  const toolsActive = drawerZones.tools.some((t) => t.id === activeTab);

  const go = (id: TabId) => {
    onTabChange(id);
    setDrawerOpen(false);
  };

  /** One drawer row; `isChild` gives Tools members the PC sidebar's indent. */
  const renderMenu = (tab: NavMenu, isChild = false) => {
    const Icon = tab.icon;
    const active = activeTab === tab.id;
    return (
      <button
        key={tab.id}
        onClick={() => go(tab.id)}
        title={tab.label}
        className={`flex items-center w-full p-2 rounded-lg cursor-pointer transition-colors ${
          isChild ? "gap-2 pl-10" : "gap-2"
        } ${
          active
            ? "bg-logo-primary/80 text-white"
            : "text-text-primary hover:bg-bg-hover"
        }`}
      >
        <Icon width={isChild ? 18 : 20} height={isChild ? 18 : 20} className="shrink-0" />
        <span className={`${isChild ? "text-xs" : "text-sm"} font-medium truncate`}>
          {tab.label}
        </span>
      </button>
    );
  };

  return (
    <>
      {/* Bottom bar */}
      <nav
        className="shrink-0 flex items-stretch border-t border-border-default bg-bg-card select-none"
        style={{ paddingBottom: "env(safe-area-inset-bottom)" }}
      >
        {barMenus.map((tab) => {
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
              {/* Same level ring the PC sidebar uses. The drawer already costs
                  a tap, so logout stays a one-tap button beside it. */}
              <LevelAvatar
                name={authUser.name}
                xp={xpUser?.lifetime_xp ?? 0}
                size={36}
                flash={xpFlash}
              />
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

        {/* Overflow navigation — the PC sidebar's structure: root menus, a
            collapsible Tools group, then the bottom menus (Settings). Scrolls
            when the list exceeds the drawer. */}
        <div className="flex-1 min-h-0 overflow-y-auto flex flex-col gap-1 p-2">
          {drawerZones.root.map((tab) => renderMenu(tab))}

          {drawerZones.crossDevice.length > 0 && (
            <>
              {/* Group header — same styling as a root row, with the chevron */}
              <button
                onClick={() => setCrossOpen((v) => !v)}
                title={NAV_CROSS_DEVICE_LABEL}
                className={`flex items-center gap-2 w-full p-2 rounded-lg cursor-pointer transition-colors ${
                  crossActive
                    ? "bg-logo-primary/80 text-white"
                    : "text-text-primary hover:bg-bg-hover"
                }`}
              >
                <CrossIcon width={20} height={20} className="shrink-0" />
                <span className="text-sm font-medium flex-1 truncate text-left">
                  {NAV_CROSS_DEVICE_LABEL}
                </span>
                <span
                  className={`text-xs opacity-70 transition-transform ${
                    crossOpen ? "rotate-90" : ""
                  }`}
                >
                  ▶
                </span>
              </button>
              {crossOpen && (
                <div className="flex flex-col gap-0.5 mt-0.5">
                  {drawerZones.crossDevice.map((tab) => renderMenu(tab, true))}
                </div>
              )}
            </>
          )}

          {drawerZones.tools.length > 0 && (
            <>
              {/* Group header — same styling as a root row, with the chevron */}
              <button
                onClick={() => setToolsOpen((v) => !v)}
                title={NAV_TOOLS_LABEL}
                className={`flex items-center gap-2 w-full p-2 rounded-lg cursor-pointer transition-colors ${
                  toolsActive
                    ? "bg-logo-primary/80 text-white"
                    : "text-text-primary hover:bg-bg-hover"
                }`}
              >
                <ToolsIcon width={20} height={20} className="shrink-0" />
                <span className="text-sm font-medium flex-1 truncate text-left">
                  {NAV_TOOLS_LABEL}
                </span>
                <span
                  className={`text-xs opacity-70 transition-transform ${
                    toolsOpen ? "rotate-90" : ""
                  }`}
                >
                  ▶
                </span>
              </button>
              {toolsOpen && (
                <div className="flex flex-col gap-0.5 mt-0.5">
                  {drawerZones.tools.map((tab) => renderMenu(tab, true))}
                </div>
              )}
            </>
          )}

          {drawerZones.bottom.map((tab) => renderMenu(tab))}
        </div>
      </div>

      <LoginModal open={loginOpen} onClose={closeLogin} />
    </>
  );
}
