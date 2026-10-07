"use client";

import { useState, useEffect } from "react";
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
import StudioPage from "@/components/listen_speak/studio/StudioPage";
import DatasetsSyncPage from "@/components/listen_speak/datasets/DatasetsSyncPage";
import ReadBookPage from "@/components/read_book/ReadBookPage";
import ReadAloudPage from "@/components/read_aloud/ReadAloudPage";
import CardsPage from "@/components/cards/CardsPage";
import CardContextMenu from "@/components/cards/CardContextMenu";
import TtsPage from "@/components/listen_speak/edge_tts/TtsPage";
import SettingsPage from "@/components/settings/SettingsPage";
import LLMChatPage from "@/components/llm/chat/ChatPage";
import DeviceChatPage from "@/components/chat/ChatPage";
import OcrPage from "@/components/ocr/OcrPage";
import WikiPage from "@/components/wiki/WikiPage";
import LogPage from "@/components/tools/LogPage";
import WorkspacesPage from "@/components/workspace/WorkspacesPage";
import SimpleWordsPage from "@/components/tools/SimpleWordsPage";
import AgentDock from "@/components/agent/AgentDock";
import type { AgentAction } from "@/lib/agent/types";

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

  // Agent app-control bridge: the goose agent drives the UI through the `app_*`
  // MCP tools, which emit `agent-action`. Perform the shell-level action here
  // (navigation + toast) and re-emit page-specific intents for the target page.
  useEffect(() => {
    const unlisten = listen<AgentAction>("agent-action", (event) => {
      const action = event.payload;
      switch (action.type) {
        case "navigate":
          // The agent no longer has a dedicated tab (it lives in an always-on
          // dock). Ignore a legacy `navigate to agent` so in-flight sessions
          // don't blank the content area.
          if ((action.tab as string) === "agent") break;
          setActiveTab(action.tab);
          break;
        case "open-review":
          setActiveTab("cards");
          void emit("agent-open-review", { dataset_uuid: action.dataset_uuid ?? null });
          break;
        case "start-dictation":
          setActiveTab("dictation");
          void emit("agent-start-dictation", {
            dataset_uuid: action.dataset_uuid ?? null,
            media_uuid: action.media_uuid ?? null,
          });
          break;
        case "notify":
          setToast(action.message);
          window.setTimeout(() => setToast(null), 4000);
          break;
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

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
      className={`flex h-screen ${mobile ? "flex-col" : ""}`}
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
        <div style={{ display: activeTab === "studio" ? "flex" : "none" }} className="flex-1 min-h-0">
          <StudioPage />
        </div>
        <div style={{ display: activeTab === "datasets-sync" ? "flex" : "none" }} className="flex-1 min-h-0">
          <DatasetsSyncPage />
        </div>
        <div style={{ display: activeTab === "simple-words" ? "flex" : "none" }} className="flex-1 min-h-0">
          <SimpleWordsPage />
        </div>
        <div style={{ display: activeTab === "read-book" ? "flex" : "none" }} className="flex-1 min-h-0">
          <ReadBookPage />
        </div>
        <div style={{ display: activeTab === "read-aloud" ? "flex" : "none" }} className="flex-1 min-h-0">
          <ReadAloudPage />
        </div>
        <div style={{ display: activeTab === "cards" ? "flex" : "none" }} className="flex-1 min-h-0">
          <CardsPage />
        </div>
        <div style={{ display: activeTab === "edge-tts" ? "flex" : "none" }} className="flex-1 min-h-0">
          <TtsPage />
        </div>
        <div style={{ display: activeTab === "models" ? "flex" : "none" }} className="flex-1 min-h-0">
          <ModelsPage />
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
          `booted` is required as well as `!mobile`: the platform is unknown on
          the prerendered first paint, so without it Android would mount the
          dock for one commit and its effects would call the `agent_*` commands
          that only exist in the desktop binary. */}
      {booted && !mobile && <AgentDock activeTab={activeTab} />}

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

