"use client";

import { useEffect, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  Bot,
  Copy,
  FolderKanban,
  FolderOpen,
  Globe,
  Info,
  Maximize,
  Minus,
  Palette,
  RefreshCw,
  RotateCcw,
  Settings,
  Square,
  Users,
  ExternalLink,
  X,
  ZoomIn,
  ZoomOut,
  type LucideIcon,
} from "lucide-react";
import {
  dispatchDockToggle,
  DOCK_STATE_DEFAULT,
  DOCK_STATE_EVENT,
  type DockStateDetail,
} from "@/lib/agent/dock-events";
import { isTauri } from "@/lib/platform";
import { CARD_BASE_URL } from "@/lib/cards";
import { logError } from "@/lib/logger";
import { THEMES } from "@/lib/theme";
import { useAgentActions, oneOf, reqBool, reqNum } from "@/lib/agent/ui-actions";
import { useTheme } from "@/hooks/useTheme";
import { useWindowControls } from "@/hooks/useWindowControls";
import {
  nudgeZoom,
  resetZoom,
  setZoomLevel,
  useZoomLevel,
  ZOOM_STEP,
} from "@/hooks/usePageZoom";
import {
  StatusItem,
  StatusMenuHeading,
  StatusMenuRow,
  StatusMenuSep,
} from "./StatusItem";
import { NAV_GROUPS, type TabId } from "./Sidebar";
import TitleBarTabs from "./TitleBarTabs";
import type { TabInstance } from "@/lib/shell/tabs";

/**
 * The window's title bar, drawn by the app.
 *
 * The window is created with `decorations: false` (src-tauri/tauri.conf.json),
 * so the native bar — app name, minimize, maximize, close — is gone and this row
 * is what the user drags, double-clicks and clicks instead. It is desktop-only:
 * the shell renders it when `!mobile`, and `decorations` is unsupported on
 * Android/iOS anyway, so the thin client never sees this component.
 *
 * What the platform gives back for free, and what this row therefore does not
 * implement:
 *  * Dragging. `data-tauri-drag-region="deep"` on the root makes every
 *    non-interactive part of the bar a drag handle. (Requires
 *    `core:window:allow-start-dragging`.) The menu items opt out of it — see the
 *    module comment in StatusItem.tsx, which explains why a menu's own dismiss
 *    area must not be part of the handle. The window controls need no opt-out:
 *    Tauri's script never starts a drag from a clickable node.
 *  * Double-click to maximize, likewise handled by that script through the
 *    already-allowed internal command — on the handle, not on a menu.
 *  * Resizing. tao hit-tests the edges of an undecorated resizable window
 *    itself, so there is no resize handle to draw here — and no `100vh`, since
 *    the app's own global zoom would overshoot the window (see globals.css).
 *
 * What is lost with the native bar and cannot be brought back from JS: the
 * Windows 11 snap-layouts flyout that hovers over the native maximize button.
 * Win+arrows and dragging to a screen edge still snap.
 */

interface TitleBarProps {
  /** Jump the shell to a tab, the same seam the status bar uses. */
  onNavigate: (tab: string) => void;
  /** The tab on screen, so the two navigation menus can check the row that is
   *  already open — the bar should not offer a page without saying so. */
  activeTab: TabId;
  /** Re-read the page on screen — the F5 path, guard and all. */
  onRefresh: () => void;
  /** Put the workspace chooser back on screen (the shell owns that state). */
  onWorkspaceChooser: () => void;
  /** The workspace uuid the shell has selected, or null. "Open workspace
   *  directory" needs it to say whether there is a directory to open at all —
   *  the path itself stays derived in Rust, never from this. */
  currentWorkspace: string | null;
  /** The open tab instances, drawn as chips in the bar's middle region (§4.2 of
   *  docs/design/tab-strip.md). */
  tabs: TabInstance[];
  /** The instance currently shown. */
  focusedId: string;
  /** Focus an already-open tab. */
  onFocusTab: (instanceId: string) => void;
  /** Close a tab. */
  onCloseTab: (instanceId: string) => void;
  /** Pin/unpin a tab, protecting it from being replaced. */
  onTogglePin: (instanceId: string) => void;
  /** Open a new instance of a multi-instance kind (the "+" picker). */
  onSpawnTab: (tab: TabId) => void;
}

// The theme ids, read off the same list the View menu draws — so the closed set
// an agent is handed and the choices a person sees cannot drift apart.
const THEME_IDS = THEMES.map((t) => t.id);

// The hint column of a menu row: right-aligned, non-interactive text, which is
// what `StatusMenuRow.trailing` exists for.
function KeyHint({ children }: { children: ReactNode }) {
  return <span className="shrink-0 text-[11px] text-text-tertiary">{children}</span>;
}

function WindowButton({
  label,
  icon: Icon,
  onClick,
  danger = false,
}: {
  label: string;
  icon: LucideIcon;
  onClick: () => void;
  danger?: boolean;
}) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
      className={`flex h-full w-[42px] shrink-0 items-center justify-center transition-colors ${
        danger
          ? "hover:bg-error-bg hover:text-error-text"
          : "text-text-secondary hover:bg-bg-hover hover:text-text-primary"
      }`}
    >
      <Icon size={14} strokeWidth={1.5} />
    </button>
  );
}

export default function TitleBar({
  onNavigate,
  activeTab,
  onRefresh,
  onWorkspaceChooser,
  currentWorkspace,
  tabs,
  focusedId,
  onFocusTab,
  onCloseTab,
  onTogglePin,
  onSpawnTab,
}: TitleBarProps) {
  const {
    available,
    maximized,
    fullscreen,
    minimize,
    toggleMaximize,
    close,
    setFullscreen,
    toggleFullscreen,
    readState,
  } = useWindowControls();
  const { theme, setTheme } = useTheme();
  const zoom = useZoomLevel();

  // The dock's presentation state, mirrored off the same broadcast the status
  // bar listens to, so the View menu can show whether the mailbox is open.
  const [dock, setDock] = useState<DockStateDetail>(DOCK_STATE_DEFAULT);
  useEffect(() => {
    const onState = (e: Event) => {
      const detail = (e as CustomEvent<DockStateDetail>).detail;
      if (detail) setDock(detail);
    };
    window.addEventListener(DOCK_STATE_EVENT, onState);
    return () => window.removeEventListener(DOCK_STATE_EVENT, onState);
  }, []);

  // F11 for full screen. The default is prevented because WebView2 would run its
  // own browser fullscreen, which is not the window state this bar reports.
  useEffect(() => {
    if (!available) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "F11" || e.repeat) return;
      e.preventDefault();
      toggleFullscreen();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [available, toggleFullscreen]);

  function openWebsite() {
    if (isTauri()) void openUrl(CARD_BASE_URL).catch(() => {});
    else window.open(CARD_BASE_URL, "_blank");
  }

  // Hand the current workspace's directory to the OS file explorer and answer
  // with the path that was revealed. The path comes from the backend
  // (`workspace_get_current_dir`) rather than being rebuilt from the uuid here,
  // so the on-disk layout stays the Rust's alone. The menu row sits under the
  // Workspace heading, which only makes sense once a workspace is selected —
  // with none, there is no directory to open.
  //
  // A failure is logged and rethrown rather than swallowed: the click would
  // otherwise do nothing and leave no trace, and an agent calling the same body
  // needs the reason as its answer. `logError` reaches the Logs page that the
  // Help menu's own "Logs & diagnostics" row points at.
  async function openWorkspaceDir(): Promise<string | null> {
    if (!isTauri() || !currentWorkspace) return null;
    try {
      const dir = await invoke<string>("workspace_get_current_dir");
      await revealItemInDir(dir);
      return dir;
    } catch (e) {
      logError(`Open workspace directory failed: ${String(e)}`, "titlebar");
      throw e;
    }
  }

  // ---- Publishing this bar's controls to an agent ----
  //
  // The agent-friendly rule covers the chrome as much as the pages: a button
  // drawn here has to be pressable from the outside too. It costs no Rust — the
  // registry below is read by the generic `ui_list_actions` / `ui_invoke_action`
  // pair — and a window command answers with the state read back off the native
  // window, so the agent gets what the window is, not what this render thinks it
  // is. Navigation and refresh are deliberately absent: `app_navigate` and
  // `shell.refresh` already own those verbs, and a second id for one action is
  // two ways to be wrong about it.
  //
  // `titlebar.quit` is declared `destructive`, which means it is listed and then
  // refused: closing the only window ends the app and takes whatever a page has
  // not written with it, and unlike the data tools there is no trash to undo it
  // from. A person clicks it; an agent is shown the boundary instead of stepping
  // over it.
  useAgentActions("titlebar", [
    {
      id: "titlebar.minimize",
      label: "Minimize the window to the taskbar.",
      kind: "mutate",
      run: async () => {
        await minimize();
        return await readState();
      },
    },
    {
      id: "titlebar.toggle-maximize",
      label: "Maximize the window, or restore it down if it is already maximized.",
      kind: "mutate",
      run: async () => {
        await toggleMaximize();
        return await readState();
      },
    },
    {
      id: "titlebar.set-fullscreen",
      label: "Put the window in or out of full screen — the F11 and the View menu row.",
      kind: "mutate",
      params_schema: {
        on: {
          type: "boolean",
          required: true,
          description: "true = full screen, false = back to a windowed state.",
        },
      },
      run: async (params) => {
        await setFullscreen(reqBool("titlebar.set-fullscreen", params, "on"));
        return await readState();
      },
    },
    {
      id: "titlebar.set-theme",
      label: "Switch the app theme, the same four schemes the View menu lists.",
      kind: "mutate",
      params_schema: {
        theme: {
          type: "string",
          required: true,
          enum: THEME_IDS,
          description: "The theme id: light, dark, solarized, code or gruvbox.",
        },
      },
      run: (params) => {
        const next = oneOf("titlebar.set-theme", params, "theme", THEME_IDS, theme);
        setTheme(next);
        return { theme: next };
      },
    },
    {
      id: "titlebar.set-zoom",
      label:
        "Set the page zoom of the whole window. Out-of-range levels are clamped, and the answer is what actually landed.",
      kind: "mutate",
      params_schema: {
        percent: {
          type: "number",
          required: true,
          description: "Zoom as a percentage — 100 is the default. Clamped to 50–200.",
        },
      },
      run: (params) => {
        const applied = setZoomLevel(reqNum("titlebar.set-zoom", params, "percent") / 100);
        return { zoom_percent: Math.round(applied * 100) };
      },
    },
    {
      id: "titlebar.toggle-agent-mailbox",
      label:
        "Open or close the agent mailbox dock — the View menu row and the floating button.",
      kind: "mutate",
      run: () => {
        dispatchDockToggle();
        return { mailbox_open: !dock.open };
      },
    },
    {
      id: "titlebar.open-workspace-dir",
      label:
        "Open the current workspace's directory in the OS file explorer — the File > Workspace menu row. Returns the path that was revealed.",
      // `mutate`, not `read`, to agree with the MCP twin `app_open_workspace_dir`
      // (UI_CONTROL): it raises a window on the user's desktop.
      kind: "mutate",
      run: async () => {
        if (!isTauri() || !currentWorkspace) {
          throw new Error(
            "titlebar.open-workspace-dir needs a selected workspace; none is active — switch or create one first"
          );
        }
        const path = await openWorkspaceDir();
        return { path, workspace_uuid: currentWorkspace };
      },
    },
    {
      id: "titlebar.quit",
      label:
        "Close the window and quit FmS. Refused through ui_invoke_action: unsaved edits are lost and nothing lands in the trash.",
      kind: "destructive",
      run: () => close(),
    },
  ]);

  return (
    <div
      data-tauri-drag-region="deep"
      // `select-none` because this is a drag handle: a stray double-click would
      // otherwise highlight the app name and the menu labels.
      //
      // `relative z-[120]` for two reasons: the bar has to stay above a
      // full-window modal scrim (the workspace chooser, z-50) so an undecorated
      // window is never impossible to drag or leave, and its menus (z-110 inside
      // this stacking context) have to clear the agent dock (z-100) and its
      // floating button (z-101), which hang on the right of the content below.
      //
      // Only the empty strip and the identity group are the handle: each menu
      // item marks itself `data-tauri-drag-region="false"`, and the window
      // controls are buttons, which Tauri's drag script skips on its own.
      className="relative z-[120] flex h-[var(--shell-titlebar-h)] shrink-0 items-stretch border-b border-border-default bg-bg-card text-[12px] select-none"
    >
      {/* Identity — what the native bar used to show. */}
      <div className="flex shrink-0 items-center gap-2 pl-3 pr-1">
        <img src="/logo.png" alt="" className="h-4 w-4 rounded-sm select-none" draggable={false} />
        <span className="font-semibold tracking-wide text-text-secondary">FmS</span>
      </div>

      {/* The menu row. Each item opens downward, over the pages, on hover as
          well as on click. */}
      <div className="flex shrink-0 items-stretch pl-1">
        <StatusItem
          label="File menu"
          title="File"
          content={<span>File</span>}
          direction="down"
          openOnHover
        >
          <StatusMenuHeading>Workspace</StatusMenuHeading>
          <StatusMenuRow
            icon={Users}
            text="Switch workspace…"
            onClick={onWorkspaceChooser}
          />
          <StatusMenuRow
            icon={FolderKanban}
            text="Workspaces page…"
            onClick={() => onNavigate("workspaces")}
          />
          <StatusMenuRow
            icon={FolderOpen}
            text="Open workspace directory"
            disabled={!currentWorkspace}
            // The empty catch only stops an unhandled rejection: `openWorkspaceDir`
            // has already logged the failure, and rethrows so the agent action
            // still gets it as its answer.
            onClick={() => void openWorkspaceDir().catch(() => {})}
          />
          <StatusMenuSep />
          <StatusMenuRow
            icon={Settings}
            text="Settings…"
            onClick={() => onNavigate("settings")}
          />
          <StatusMenuRow
            icon={RefreshCw}
            text="Reload this page"
            trailing={<KeyHint>F5</KeyHint>}
            onClick={onRefresh}
          />
          <StatusMenuSep />
          {/* Closing the only window quits the app — the same thing the native
              close button did, with the same consequence: what a page has
              written is on disk, what it has not is lost. A confirm step would
              be new behaviour, not a safety the app used to have. */}
          <StatusMenuRow
            icon={X}
            text="Quit FmS"
            danger
            disabled={!available}
            onClick={close}
          />
        </StatusItem>

        <StatusItem
          label="View menu"
          title="View"
          content={<span>View</span>}
          direction="down"
          openOnHover
        >
          <StatusMenuHeading>Theme</StatusMenuHeading>
          {THEMES.map((t) => (
            <StatusMenuRow
              key={t.id}
              icon={Palette}
              text={t.label}
              checked={theme === t.id}
              keepOpen
              onClick={() => setTheme(t.id)}
            />
          ))}
          <StatusMenuSep />
          <StatusMenuHeading>Zoom — {Math.round(zoom * 100)}%</StatusMenuHeading>
          <StatusMenuRow
            icon={ZoomIn}
            text="Zoom in"
            trailing={<KeyHint>Ctrl +</KeyHint>}
            keepOpen
            onClick={() => nudgeZoom(ZOOM_STEP)}
          />
          <StatusMenuRow
            icon={ZoomOut}
            text="Zoom out"
            trailing={<KeyHint>Ctrl −</KeyHint>}
            keepOpen
            onClick={() => nudgeZoom(-ZOOM_STEP)}
          />
          <StatusMenuRow
            icon={RotateCcw}
            text="Reset zoom"
            trailing={<KeyHint>Ctrl 0</KeyHint>}
            keepOpen
            onClick={() => resetZoom()}
          />
          <StatusMenuSep />
          <StatusMenuRow
            icon={Bot}
            text="Agent mailbox"
            checked={dock.open}
            keepOpen
            onClick={dispatchDockToggle}
          />
          <StatusMenuRow
            icon={Maximize}
            text="Full screen"
            trailing={<KeyHint>F11</KeyHint>}
            checked={fullscreen}
            keepOpen
            disabled={!available}
            onClick={toggleFullscreen}
          />
        </StatusItem>

        {/* Cross Device and Tools — the sidebar's two collapsible groups, read
            straight off its table (see the NAV_GROUPS comment in Sidebar.tsx),
            so a page added to a group appears in the menu with no second list to
            maintain. They exist because the sidebar can be collapsed and because
            a menu lists a whole group at once: the point is reaching any page
            from anywhere, the way VS Code's menus work.

            Nothing here publishes an agent action: every row navigates, and
            `app_navigate` already owns that verb — see the registry comment
            above, which keeps one id per action on purpose. */}
        {NAV_GROUPS.map((group) => (
          <StatusItem
            key={group.id}
            label={`${group.label} menu`}
            title={group.label}
            content={<span>{group.label}</span>}
            direction="down"
            openOnHover
          >
            {group.tabs.map((tab) => (
              <StatusMenuRow
                key={tab.id}
                icon={tab.icon}
                text={tab.label}
                checked={activeTab === tab.id}
                onClick={() => onNavigate(tab.id)}
              />
            ))}
          </StatusItem>
        ))}

        <StatusItem
          label="Help menu"
          title="Help"
          content={<span>Help</span>}
          direction="down"
          openOnHover
        >
          <StatusMenuRow icon={Globe} text="Website & manual…" onClick={openWebsite} />
          <StatusMenuRow
            icon={ExternalLink}
            text="Logs & diagnostics…"
            onClick={() => onNavigate("logs")}
          />
          <StatusMenuSep />
          <StatusMenuRow
            icon={Info}
            text="About & updates…"
            onClick={() => onNavigate("settings")}
          />
        </StatusItem>
      </div>

      {/* Middle region: the open-page tab chips, then empty space that stays a
          drag handle. Each chip opts out of the drag region on its own (see
          TitleBarTabs), while the container and the gap after the last chip do
          not — so the window is still grabbable here and double-clicks to
          maximize, exactly as the bare `flex-1` strip this replaces did. */}
      <div className="flex min-w-0 flex-1 items-stretch">
        <TitleBarTabs
          order={tabs}
          focusedId={focusedId}
          onFocus={onFocusTab}
          onClose={onCloseTab}
          onTogglePin={onTogglePin}
          onSpawn={onSpawnTab}
        />
      </div>

      {/* Window controls. Hidden outside Tauri (a browser preview has no window
          to command) and in full screen, where minimize and maximize have no
          meaning and close would be an easy mistake. */}
      {available && !fullscreen && (
        <div className="flex shrink-0 items-stretch">
          <WindowButton label="Minimize" icon={Minus} onClick={minimize} />
          <WindowButton
            label={maximized ? "Restore down" : "Maximize"}
            // Two offset squares is the restore glyph every desktop app draws;
            // an empty square is maximize.
            icon={maximized ? Copy : Square}
            onClick={toggleMaximize}
          />
          <WindowButton label="Close" icon={X} onClick={close} danger />
        </div>
      )}
    </div>
  );
}
