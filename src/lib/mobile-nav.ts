// Mobile bottom-navigation customization: which menus sit in the bottom bar
// and in which order. Shared between BottomNav (renders it) and the Settings
// page's "Bottom Nav" section (edits it).
//
// Persisted in localStorage, not in the backend settings file: the bar is a
// per-device UI preference — a phone must not inherit a tablet's (or another
// machine's) layout, same tier as the theme and the pinned microphone. The
// `order` list is the single source of sequence: the bar shows the checked
// ids left-to-right in list order, the "More" drawer shows the rest top-to-
// bottom in the same order — so one reorder control drives both surfaces.

import { useEffect, useMemo, useState } from "react";
import {
  ArrowUpDown,
  BookOpen,
  BookOpenText,
  Box,
  FileText,
  FolderKanban,
  Headphones,
  Layers,
  MessagesSquare,
  Settings,
  Users,
} from "lucide-react";
import { MicGlyph } from "@/components/voice/MicGlyph";
import type { TabId } from "@/components/layout/Sidebar";

export type NavMenu = {
  id: TabId;
  label: string;
  icon: React.ComponentType<{ width?: number; height?: number; className?: string }>;
};

/** The full menu universe the bar/drawer split applies to, in default order. */
export const NAV_MENUS: NavMenu[] = [
  { id: "dictation", label: "Dictation", icon: Headphones },
  { id: "cards", label: "Cards", icon: Layers },
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

const MENU_BY_ID = new Map<TabId, NavMenu>(NAV_MENUS.map((m) => [m.id, m]));

/** Hard cap on bar buttons so labels stay legible on narrow phones; the
 *  "More" button always adds one more slot, so the bar peaks at 5 buttons. */
export const MAX_BAR_MENUS = 4;

export type NavConfig = {
  /** Every menu id, editing order == display order for both surfaces. */
  order: TabId[];
  /** Menu ids shown in the bottom bar (membership only; order comes from `order`). */
  bar: TabId[];
};

export const DEFAULT_NAV_CONFIG: NavConfig = {
  order: NAV_MENUS.map((m) => m.id),
  bar: ["dictation", "cards"],
};

const STORAGE_KEY = "bottom-nav-config";
const CHANGE_EVENT = "bottom-nav-config-changed";

const isTabId = (v: unknown): v is TabId =>
  typeof v === "string" && MENU_BY_ID.has(v as TabId);

/** Read + validate the stored layout. Tolerates a missing/corrupt entry and
 *  merges ids the saved list predates (the menu universe grew), so no device
 *  can end up without a route to some page. */
export function loadNavConfig(): NavConfig {
  if (typeof window === "undefined") return DEFAULT_NAV_CONFIG;
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT_NAV_CONFIG;
    const parsed = JSON.parse(raw);
    const saved: TabId[] = Array.isArray(parsed?.order) ? parsed.order.filter(isTabId) : [];
    const order = [...new Set([...saved, ...NAV_MENUS.map((m) => m.id)])];
    const barRaw: TabId[] = Array.isArray(parsed?.bar) ? parsed.bar.filter(isTabId) : [];
    // Dedupe, clamp to the cap, and keep only ids that survive in `order`.
    const bar: TabId[] = [];
    for (const id of new Set<TabId>(barRaw)) {
      if (order.includes(id) && bar.length < MAX_BAR_MENUS) bar.push(id);
    }
    if (bar.length === 0) bar.push(...DEFAULT_NAV_CONFIG.bar.slice(0, 1));
    return { order, bar };
  } catch {
    return DEFAULT_NAV_CONFIG;
  }
}

/** Persist and notify every `useNavConfig` consumer (the live bar re-renders
 *  while the Settings section is still open — no app restart needed). */
export function saveNavConfig(config: NavConfig): void {
  if (typeof window === "undefined") return;
  localStorage.setItem(STORAGE_KEY, JSON.stringify(config));
  window.dispatchEvent(new Event(CHANGE_EVENT));
}

/** The config as live state: re-reads on change events and exposes the two
 *  derived menu lists in display order. */
export function useNavConfig() {
  const [config, setConfig] = useState<NavConfig>(DEFAULT_NAV_CONFIG);

  useEffect(() => {
    setConfig(loadNavConfig());
    const onChange = () => setConfig(loadNavConfig());
    window.addEventListener(CHANGE_EVENT, onChange);
    return () => window.removeEventListener(CHANGE_EVENT, onChange);
  }, []);

  const barMenus = useMemo(
    () => config.order.filter((id) => config.bar.includes(id)).map((id) => MENU_BY_ID.get(id)!),
    [config]
  );
  const drawerMenus = useMemo(
    () => config.order.filter((id) => !config.bar.includes(id)).map((id) => MENU_BY_ID.get(id)!),
    [config]
  );

  return { config, barMenus, drawerMenus };
}
