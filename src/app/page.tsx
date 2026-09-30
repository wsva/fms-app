"use client";

import { useState, useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import Sidebar, { type TabId } from "@/components/layout/Sidebar";
import StatusBar from "@/components/layout/StatusBar";
import ModelsPage from "@/components/listen_speak/models/ModelsPage";
import DatasetsPage from "@/components/listen_speak/datasets/DatasetsPage";
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

export default function Home() {
  const [activeTab, setActiveTab] = useState<TabId>("dictation");
  const [sidebarWidth, setSidebarWidth] = useState(176); // default expanded width

  // Listen for wiki deep link navigation (fms-app://wiki/path/to/file.md)
  useEffect(() => {
    const unlisten = listen<string>("wiki-navigate", () => {
      setActiveTab("wiki");
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

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
        <div style={{ display: activeTab === "datasets" ? "flex" : "none" }} className="flex-1 min-h-0">
          <DatasetsPage />
        </div>
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

        <StatusBar />
      </div>

      {/* Global text-selection → "Add to Card" menu */}
      <CardContextMenu />
    </div>
  );
}

