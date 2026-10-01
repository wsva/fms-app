"use client";

import { useState, useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import Sidebar, { type TabId } from "@/components/layout/Sidebar";
import StatusBar from "@/components/layout/StatusBar";
import WorkspaceChooser from "@/components/workspace/WorkspaceChooser";
import ModelsPage from "@/components/listen_speak/models/ModelsPage";
import DictationPage from "@/components/listen_speak/dictation/DictationPage";
import DictationDatasetPage from "@/components/listen_speak/dictation_dataset/DictationDatasetPage";
import StudioPage from "@/components/listen_speak/studio/StudioPage";
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

interface Workspace {
  uuid: string;
  name: string;
  user_id: string;
  avatar: string;
  created_at: string;
  last_accessed: string;
  auto_login: boolean;
}

export default function Home() {
  const [activeTab, setActiveTab] = useState<TabId>("dictation");
  const [sidebarWidth, setSidebarWidth] = useState(176); // default expanded width
  const [showWorkspaceChooser, setShowWorkspaceChooser] = useState(false);
  const [currentWorkspace, setCurrentWorkspace] = useState<Workspace | null>(null);

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

  function handleWorkspaceSelect(ws: Workspace) {
    setCurrentWorkspace(ws);
    setShowWorkspaceChooser(false);
  }

  // Show workspace chooser if needed
  if (showWorkspaceChooser) {
    return <WorkspaceChooser onSelect={handleWorkspaceSelect} />;
  }

  return (
    <div className="flex h-screen">
      <Sidebar
        activeTab={activeTab}
        onTabChange={setActiveTab}
        onWidthChange={setSidebarWidth}
      />

      <div
        className="flex-1 flex flex-col overflow-hidden transition-[margin] duration-200"
        style={{ marginLeft: sidebarWidth }}
      >
        <div style={{ display: activeTab === "dictation" ? "flex" : "none" }} className="flex-1 min-h-0">
          <DictationPage />
        </div>
        <div style={{ display: activeTab === "dictation-dataset" ? "flex" : "none" }} className="flex-1 min-h-0">
          <DictationDatasetPage />
        </div>
        <div style={{ display: activeTab === "studio" ? "flex" : "none" }} className="flex-1 min-h-0">
          <StudioPage />
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

        <StatusBar />
      </div>

      {/* Global text-selection → "Add to Card" menu */}
      <CardContextMenu />
    </div>
  );
}

