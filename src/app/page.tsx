"use client";

import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/tauri";
import Sidebar, { type TabId } from "@/components/layout/Sidebar";
import StatusBar from "@/components/layout/StatusBar";
import ModelsPage from "@/components/listen_speak/models/ModelsPage";
import DatasetsPage from "@/components/listen_speak/datasets/DatasetsPage";
import DictationPage from "@/components/listen_speak/dictation/DictationPage";
import TtsPage from "@/components/listen_speak/edge_tts/TtsPage";
import SettingsPage from "@/components/settings/SettingsPage";
import LLMChatPage from "@/components/llm/chat/ChatPage";
import P2PPage from "@/components/tools/p2p/P2PPage";

export default function Home() {
  const [activeTab, setActiveTab] = useState<TabId>("dashboard");
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);

  return (
    <div className="flex h-screen">
      <Sidebar
        activeTab={activeTab}
        onTabChange={setActiveTab}
        onCollapseChange={setSidebarCollapsed}
      />

      <div className={`flex-1 flex flex-col overflow-hidden transition-all duration-200 ${sidebarCollapsed ? "ml-12" : "ml-44"}`}>
        <div style={{ display: activeTab === "dashboard" ? "flex" : "none" }} className="flex-1">
          <DashboardContent />
        </div>
        <div style={{ display: activeTab === "datasets" ? "flex" : "none" }} className="flex-1">
          <DatasetsPage />
        </div>
        <div style={{ display: activeTab === "dictation" ? "flex" : "none" }} className="flex-1 min-h-0">
          <DictationPage />
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
        <div style={{ display: activeTab === "p2p-share" ? "flex" : "none" }} className="flex-1 min-h-0">
          <P2PPage />
        </div>

        <StatusBar />
      </div>
    </div>
  );
}

function DashboardContent() {
  const [userName, setUserName] = useState<string>("");

  useEffect(() => {
    if (!isTauri()) return;
    invoke<{ name: string; email: string } | null>("auth_get_user")
      .then((user) => {
        if (user) setUserName(user.name);
      })
      .catch(() => {});
  }, []);

  return (
    <main className="flex-1 flex flex-col items-center justify-center p-8">
      <div className="max-w-lg text-center space-y-6">
        <div className="flex justify-center">
          <img src="/icon.png" alt="fms" className="w-48 h-48 drop-shadow-lg" />
        </div>
        {userName && (
          <p className="text-lg text-text-secondary">
            Willkommen, <span className="font-semibold text-accent">{userName}</span>!
          </p>
        )}
        <p className="text-sm text-text-tertiary">
          Select a tab from the sidebar to get started.
        </p>
      </div>
    </main>
  );
}
