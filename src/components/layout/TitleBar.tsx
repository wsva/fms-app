"use client";

import { useEffect, useState, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  Bot,
  Copy,
  FolderKanban,
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
 *    non-interactive part of the bar a drag handle, and Tauri's own script
 *    refuses to start a drag from a button, a link or an input — so the menus
 *    and the window controls sit inside the handle without fighting it.
 *    (Requires `core:window:allow-start-dragging`.)
 *  * Double-click to maximize, likewise handled by that script through the
 *    already-allowed internal command.
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
  /** Re-read the page on screen — the F5 path, guard and all. */
  onRefresh: () => void;
  /** Put the workspace chooser back on screen (the shell owns that state). */
  onWorkspaceChooser: () => void;
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

export default function TitleBar({ onNavigate, onRefresh, onWorkspaceChooser }: TitleBarProps) {
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
          description: "The theme id: light, dark, solarized or gruvbox.",
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
      className="relative z-[120] flex h-[var(--shell-titlebar-h)] shrink-0 items-stretch border-b border-border-default bg-bg-card text-[12px] select-none"
    >
      {/* Identity — what the native bar used to show. */}
      <div className="flex shrink-0 items-center gap-2 pl-3 pr-1">
        <img src="/logo.png" alt="" className="h-4 w-4 rounded-sm select-none" draggable={false} />
        <span className="font-semibold tracking-wide text-text-secondary">FmS</span>
      </div>

      {/* The menu row. Each item opens downward, over the pages. */}
      <div className="flex shrink-0 items-stretch pl-1">
        <StatusItem label="File menu" title="File" content={<span>File</span>} direction="down">
          <StatusMenuHeading>Workspace</StatusMenuHeading>
          <StatusMenuRow
            icon={Users}
            text="Switch workspace…"
            detail="Change the account and folder set this window works on"
            onClick={onWorkspaceChooser}
          />
          <StatusMenuRow
            icon={FolderKanban}
            text="Workspaces page…"
            onClick={() => onNavigate("workspaces")}
          />
          <StatusMenuSep />
          <StatusMenuRow
            icon={Settings}
            text="Settings…"
            detail="Theme, directories, sync, agent"
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

        <StatusItem label="View menu" title="View" content={<span>View</span>} direction="down">
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
            detail={dock.open ? "open" : "closed"}
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

        <StatusItem label="Help menu" title="Help" content={<span>Help</span>} direction="down">
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
            detail="Build version and the latest release"
            onClick={() => onNavigate("settings")}
          />
        </StatusItem>
      </div>

      {/* The rest of the bar is empty handle — the part you grab to move the
          window, and double-click to maximize. */}
      <div className="min-w-0 flex-1" />

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
