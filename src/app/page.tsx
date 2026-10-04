"use client";

import { useState, useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
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
import CardsPage from "@/components/cards/CardsPage";
import CardContextMenu from "@/components/cards/CardContextMenu";
import TtsPage from "@/components/listen_speak/edge_tts/TtsPage";
import SettingsPage from "@/components/settings/SettingsPage";
import LLMChatPage from "@/components/llm/chat/ChatPage";
import OcrPage from "@/components/ocr/OcrPage";
import WikiPage from "@/components/wiki/WikiPage";
import LogPage from "@/components/tools/LogPage";
import WorkspacesPage from "@/components/workspace/WorkspacesPage";
import SimpleWordsPage from "@/components/tools/SimpleWordsPage";

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

  useEffect(() => {
    setMobile(isMobileApp());
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
    <div className={`flex h-screen ${mobile ? "flex-col" : ""}`}>
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
          {mobile ? <DatasetsSyncPage /> : <StudioPage />}
        </div>
        <div style={{ display: activeTab === "simple-words" ? "flex" : "none" }} className="flex-1 min-h-0">
          <SimpleWordsPage />
        </div>
        <div style={{ display: activeTab === "read-book" ? "flex" : "none" }} className="flex-1 min-h-0">
          <ReadBookPage />
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
    </div>
  );
}

