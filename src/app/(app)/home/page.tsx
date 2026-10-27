"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { listen, emit } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { ask } from "@tauri-apps/plugin-dialog";
import { isMobileApp } from "@/lib/platform";
import Sidebar, { type TabId } from "@/components/layout/Sidebar";
import StatusBar from "@/components/layout/StatusBar";
import BottomNav from "@/components/layout/BottomNav";
import WorkspaceChooser from "@/components/workspace/WorkspaceChooser";
import ModelsPage from "@/components/listen_speak/models/ModelsPage";
import DictationPage from "@/components/listen_speak/dictation/DictationPage";
import WorkflowPage from "@/components/tools/WorkflowPage";
import DatasetsSyncPage from "@/components/listen_speak/datasets/DatasetsSyncPage";
import DevicesHubPage from "@/components/devices/DevicesHubPage";
import BooksPage from "@/components/books/BooksPage";
import ReadAloudPage from "@/components/read_aloud/ReadAloudPage";
import CardsPage from "@/components/cards/CardsPage";
import TasksPage from "@/components/tools/TasksPage";
import CardContextMenu from "@/components/cards/CardContextMenu";
import TtsPage from "@/components/listen_speak/edge_tts/TtsPage";
import SettingsPage from "@/components/settings/SettingsPage";
import AgentPage from "@/components/agent/AgentPage";
import LLMChatPage from "@/components/llm/chat/ChatPage";
import DeviceChatPage from "@/components/chat/ChatPage";
import OcrPage from "@/components/ocr/OcrPage";
import WikiPage from "@/components/wiki/WikiPage";
import LogPage from "@/components/tools/LogPage";
import WorkspacesPage from "@/components/workspace/WorkspacesPage";
import SimpleWordsPage from "@/components/tools/SimpleWordsPage";
import SplitTextPage from "@/components/tools/SplitTextPage";
import PdfReaderPage from "@/components/tools/PdfReaderPage";
import AgentDock from "@/components/agent/AgentDock";
import type { AgentAction } from "@/lib/agent/types";
import { setUiContext } from "@/lib/agent/ui-context";
import { jsonSafeDetail, runUiAction, useAgentActions } from "@/lib/agent/ui-actions";
import { refreshActivePage, useActiveTabSource } from "@/lib/refresh/page-refresh";

interface Workspace {
  uuid: string;
  name: string;
  user_id: string;
  avatar: string;
  created_at: string;
  last_accessed: string;
  auto_login: boolean;
}

// A pairing request emitted by the backend (pairing.rs) when an unknown device
// on an untrusted network asks to pair.
interface PairingRequest {
  request_id: string;
  device_id: string;
  name: string;
  fingerprint: string;
  // Identity the device declares; the PC binds writeback to it on approval.
  user_id?: string;
}

export default function Home() {
  const [activeTab, setActiveTab] = useState<TabId>("dictation");
  const [sidebarWidth, setSidebarWidth] = useState(176); // default expanded width
  const [showWorkspaceChooser, setShowWorkspaceChooser] = useState(false);
  const [currentWorkspace, setCurrentWorkspace] = useState<Workspace | null>(null);
  const [mobile, setMobile] = useState(false);
  // Android paints the prerendered desktop shell while the JS bundle is still
  // loading, which reads as "wrong app" for a second or more on a phone. The
  // splash is therefore rendered from the very first (prerendered) paint and
  // only dropped once the client has committed the platform-correct layout.
  // `booted` starts false on server and client alike, so hydration matches.
  const [booted, setBooted] = useState(false);
  // Ephemeral notification banner for the agent's `app_notify` action (the app
  // has no toast system, so the shell renders a self-dismissing one).
  const [toast, setToast] = useState<string | null>(null);

  // One banner, two sources: `app_notify` and the refresh shortcut. A second
  // message restarts the dismissal instead of letting the first timer clear it.
  const toastTimer = useRef<number | null>(null);
  const showToast = useCallback((message: string) => {
    setToast(message);
    if (toastTimer.current !== null) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => {
      toastTimer.current = null;
      setToast(null);
    }, 4000);
  }, []);

  useEffect(() => {
    setMobile(isMobileApp());
    setBooted(true);
  }, []);

  // Check workspace state on mount
  useEffect(() => {
    async function checkWorkspace() {
      try {
        const ws = await invoke<Workspace | null>("workspace_get_current");
        if (ws) {
          setCurrentWorkspace(ws);
        } else {
          // No workspace selected, show chooser
          setShowWorkspaceChooser(true);
        }
      } catch (e) {
        console.error("Failed to get current workspace:", e);
        setShowWorkspaceChooser(true);
      }
    }
    checkWorkspace();

    // Listen for workspace chooser event from backend
    const unlistenChooser = listen("workspace-show-chooser", () => {
      setShowWorkspaceChooser(true);
    });

    // Keep the shell in sync when the workspace is switched (e.g. from the
    // Workspaces page) so the current-workspace state stays fresh.
    const unlistenSelected = listen<Workspace>("workspace-selected", (event) => {
      setCurrentWorkspace(event.payload);
    });

    return () => {
      unlistenChooser.then((fn) => fn());
      unlistenSelected.then((fn) => fn());
    };
  }, []);

  // Listen for wiki deep link navigation (fms-app://wiki/path/to/file.md)
  useEffect(() => {
    const unlisten = listen<string>("wiki-navigate", () => {
      setActiveTab("wiki");
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // The shell's slice of the agent-visible UI context: which tab is showing.
  // The pages own the rest (Dictation reports the dataset/media/cue) and merge
  // into the same backend cache — see `lib/agent/ui-context`.
  useEffect(() => {
    setUiContext({ active_tab: activeTab });
  }, [activeTab]);

  // Agent app-control bridge: an attached agent drives the UI through the `app_*`
  // MCP tools, which emit `agent-action`. Perform the shell-level action here
  // (navigation + toast) and re-emit page-specific intents for the target page.
  useEffect(() => {
    const unlisten = listen<AgentAction>("agent-action", (event) => {
      const action = event.payload;
      switch (action.type) {
        case "navigate":
          // `agent` is a settings page, not a view an agent should drive, and
          // honouring it would yank the user off whatever they were reading.
          // Ignore it so a stale prompt cannot blank the content area.
          if ((action.tab as string) === "agent") break;
          setActiveTab(action.tab);
          break;
        case "open-review":
          setActiveTab("cards");
          void emit("agent-open-review", { dataset_uuid: action.dataset_uuid ?? null });
          break;
        case "start-dictation": {
          setActiveTab("dictation");
          // Drive the page through the action registry instead of re-emitting an
          // event: `agent-start-dictation` had no listener, so the dataset and
          // media an agent named were silently dropped on the floor.
          const datasetUuid = action.dataset_uuid ?? null;
          const mediaUuid = action.media_uuid ?? null;
          void (async () => {
            try {
              if (datasetUuid) {
                await runUiAction("dictation.open-dataset", { dataset_uuid: datasetUuid });
              }
              if (mediaUuid) {
                await runUiAction("dictation.open-media", {
                  dataset_uuid: datasetUuid,
                  media_uuid: mediaUuid,
                });
              }
            } catch (e) {
              // Nothing to report back to: `app_start_dictation` is
              // fire-and-forget by design. The agent can confirm the effect
              // with app_get_ui_state, which is what its description says.
              console.error("agent start-dictation failed:", e);
            }
          })();
          break;
        }
        case "notify":
          showToast(action.message);
          break;
        case "invoke-action": {
          // The response half of `ui_invoke_action`. The tool is parked on
          // `correlation_id` waiting for this, so every path — success, thrown
          // error, IPC failure — has to report something or the agent waits out
          // the full timeout for no reason.
          const correlationId = action.correlation_id;
          void (async () => {
            let ok = false;
            let detail: unknown = null;
            try {
              detail = (await runUiAction(action.id, action.params ?? {})) ?? null;
              ok = true;
            } catch (e) {
              detail = e instanceof Error ? e.message : String(e);
            }
            try {
              await invoke("agent_report_action_result", {
                correlationId,
                ok,
                detail: jsonSafeDetail(detail),
              });
            } catch (e) {
              console.error("agent_report_action_result failed:", e);
            }
          })();
          break;
        }
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [showToast]);

  // Pairing: ask the PC owner about each request, one at a time. This dialog is
  // the only way a device is ever approved — no bearer secret can replace the
  // click. Only the PC shows dialogs.
  useEffect(() => {
    if (isMobileApp()) return;
    const queue: PairingRequest[] = [];
    let running = false;
    const drain = async () => {
      if (running) return;
      running = true;
      while (queue.length > 0) {
        const req = queue.shift()!;
        const asWho = req.user_id && req.user_id !== "local"
          ? `\n\nIt will sync progress as: ${req.user_id}`
          : "";
        const approve = await ask(
          `FmS on ${req.name} wants to pair.${asWho}\n\nConfirm the code ${req.fingerprint} matches the one shown on the device, then allow or deny.`,
          { title: "Pairing request", kind: "warning", okLabel: "Allow", cancelLabel: "Deny" },
        );
        try {
          await invoke("pairing_respond", { requestId: req.request_id, approve });
        } catch (e) {
          console.error("pairing_respond failed:", e);
        }
      }
      running = false;
    };
    const unlisten = listen<PairingRequest>("pairing-request", (event) => {
      queue.push(event.payload);
      void drain();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Tell the refresh module which tab is on screen, so a page knows when it
  // becomes visible. See lib/refresh/page-refresh.tsx for why this is not a
  // provider: the pages are plain siblings, and one boolean is not worth
  // re-indenting the shell.
  useActiveTabSource(activeTab);

  // F5 / Ctrl+R / Cmd+R re-read the page on screen. `preventDefault` is not
  // politeness: a WebView-level reload remounts every page and destroys state
  // the app deliberately keeps alive — including dictation answers, which are
  // never persisted. The page's own guard decides whether a re-read is safe.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.repeat) return;
      const isRefreshKey =
        e.key === "F5" ||
        ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === "r");
      if (!isRefreshKey) return;
      e.preventDefault();
      void refreshActivePage("manual").then((outcome) => {
        if (outcome.status === "blocked") {
          showToast("Not refreshed: this page has unsaved edits.");
        } else if (outcome.status === "none") {
          showToast("Nothing to refresh on this page.");
        }
      });
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [showToast]);

  // The same path the keyboard takes, for an agent: refresh is otherwise
  // UI-only, and every feature is supposed to be reachable through MCP.
  useAgentActions("shell", [
    {
      id: "shell.refresh",
      label: "Re-read the page on screen from disk",
      kind: "read",
      run: () => refreshActivePage("manual"),
    },
  ]);

  function handleWorkspaceSelect(ws: Workspace) {
    setCurrentWorkspace(ws);
    setShowWorkspaceChooser(false);
  }

  // Show workspace chooser if needed
  if (showWorkspaceChooser) {
    return <WorkspaceChooser onSelect={handleWorkspaceSelect} />;
  }

  return (
    <div
      // `h-full` rather than `h-screen` — see the html/body rule in globals.css:
      // a viewport unit is multiplied by the global page zoom while a percentage
      // resolves against a viewport already divided by it, so 100vh overshoots
      // the window at any zoom above 1 and the window itself starts scrolling.
      className={`flex h-full ${mobile ? "flex-col" : ""}`}
      // Edge-to-edge WebView draws under the status bar; push the shell below it.
      style={mobile ? { paddingTop: "env(safe-area-inset-top)" } : undefined}
    >
      {!mobile && (
        <Sidebar
          activeTab={activeTab}
          onTabChange={setActiveTab}
          onWidthChange={setSidebarWidth}
        />
      )}

      <div
        className="flex-1 flex flex-col overflow-hidden transition-[margin] duration-200 min-h-0"
        style={{ marginLeft: mobile ? 0 : sidebarWidth }}
      >
        <div style={{ display: activeTab === "dictation" ? "flex" : "none" }} className="flex-1 min-h-0">
          <DictationPage active={activeTab === "dictation"} />
        </div>
        <div style={{ display: activeTab === "workflow" ? "flex" : "none" }} className="flex-1 min-h-0">
          <WorkflowPage />
        </div>
        <div style={{ display: activeTab === "datasets-sync" ? "flex" : "none" }} className="flex-1 min-h-0">
          <DatasetsSyncPage onNavigate={(tab) => setActiveTab(tab as TabId)} />
        </div>
        <div style={{ display: activeTab === "devices-hub" ? "flex" : "none" }} className="flex-1 min-h-0">
          <DevicesHubPage />
        </div>
        <div style={{ display: activeTab === "simple-words" ? "flex" : "none" }} className="flex-1 min-h-0">
          <SimpleWordsPage />
        </div>
        <div style={{ display: activeTab === "books" ? "flex" : "none" }} className="flex-1 min-h-0">
          <BooksPage />
        </div>
        <div style={{ display: activeTab === "read-aloud" ? "flex" : "none" }} className="flex-1 min-h-0">
          <ReadAloudPage />
        </div>
        <div style={{ display: activeTab === "cards" ? "flex" : "none" }} className="flex-1 min-h-0">
          <CardsPage />
        </div>
        <div style={{ display: activeTab === "tasks" ? "flex" : "none" }} className="flex-1 min-h-0">
          <TasksPage />
        </div>
        <div style={{ display: activeTab === "edge-tts" ? "flex" : "none" }} className="flex-1 min-h-0">
          <TtsPage />
        </div>
        <div style={{ display: activeTab === "models" ? "flex" : "none" }} className="flex-1 min-h-0">
          <ModelsPage />
        </div>
        <div style={{ display: activeTab === "agent" ? "flex" : "none" }} className="flex-1 min-h-0">
          <AgentPage />
        </div>
        <div style={{ display: activeTab === "settings" ? "flex" : "none" }} className="flex-1 min-h-0">
          <SettingsPage />
        </div>
        <div style={{ display: activeTab === "llm-chat" ? "flex" : "none" }} className="flex-1 min-h-0">
          <LLMChatPage />
        </div>
        <div style={{ display: activeTab === "chat" ? "flex" : "none" }} className="flex-1 min-h-0">
          <DeviceChatPage active={activeTab === "chat"} />
        </div>
        <div style={{ display: activeTab === "ocr" ? "flex" : "none" }} className="flex-1 min-h-0">
          <OcrPage />
        </div>
        <div style={{ display: activeTab === "split-text" ? "flex" : "none" }} className="flex-1 min-h-0">
          <SplitTextPage />
        </div>
        <div style={{ display: activeTab === "pdf-reader" ? "flex" : "none" }} className="flex-1 min-h-0">
          <PdfReaderPage />
        </div>
        <div style={{ display: activeTab === "wiki" ? "flex" : "none" }} className="flex-1 min-h-0">
          <WikiPage />
        </div>
        <div style={{ display: activeTab === "logs" ? "flex" : "none" }} className="flex-1 min-h-0">
          <LogPage />
        </div>
        <div style={{ display: activeTab === "workspaces" ? "flex" : "none" }} className="flex-1 min-h-0">
          <WorkspacesPage />
        </div>

        {!mobile && <StatusBar />}
      </div>

      {mobile && <BottomNav activeTab={activeTab} onTabChange={setActiveTab} />}

      {/* Global text-selection → "Add to Card" menu */}
      <CardContextMenu />

      {/* Always-accessible agent dock (desktop only) — overlays the content
          area on the right, toggled by the floating Bot button or Ctrl+Space.
          It is the outbox of the agent→FmS link (notifications, task count),
          so it only navigates; it never holds a session.
          `booted` is required as well as `!mobile`: the platform is unknown on
          the prerendered first paint, so without it Android would mount the
          dock for one commit and its effects would call desktop-only commands. */}
      {booted && !mobile && <AgentDock onNavigate={(tab) => setActiveTab(tab as TabId)} />}

      {/* Agent `app_notify` toast */}
      {toast && (
        <div className="fixed bottom-6 left-1/2 -translate-x-1/2 z-[9998] px-4 py-2.5 rounded-lg bg-bg-card border border-border-default text-sm text-text-primary shadow-lg max-w-[80%] break-words">
          {toast}
        </div>
      )}

      {/* Boot splash — visible on Android only, see .app-boot-splash in globals.css.
          No `flex` class: display is owned by that rule so the pre-paint
          data-mobile tag can decide it. /logo.png is a copy of
          src-tauri/icons/icon.png (the launcher artwork); it lives in public/ so
          the WebView can reach it, and is rounded to echo the adaptive icon. */}
      {!booted && (
        <div className="app-boot-splash fixed inset-0 z-[9999] flex-col items-center justify-center bg-bg-body">
          <img src="/logo.png" alt="" className="h-36 w-36 select-none rounded-2xl" draggable={false} />
        </div>
      )}
    </div>
  );
}

