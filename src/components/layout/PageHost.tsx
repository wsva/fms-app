"use client";

// ---------------------------------------------------------------------------
// Renders the open tab instances, each display-toggled — the map that replaces
// the wall of ~21 `activeTab === "x"` blocks the shell used to carry inline.
//
// Every instance in `order` stays mounted (only a close/replace unmounts one), so
// switching tabs preserves each page's state exactly as before. The focused
// instance is `display:flex`, the rest `display:none`.
//
// A page receives `focused` where it previously received the ad-hoc `active`
// prop, and `onNavigate` for the one page (Datasets Sync) that drives the shell.
//
// Scroll offsets are the one piece of state a display toggle does not carry:
// a pane with no layout box has no scrolling box, so a hidden pane returns at
// the top. `usePreservedScroll` records and restores them per pane, which is
// what makes "switching tabs keeps your place" true for every page rather than
// only for the ones that hold their data in React state.
// ---------------------------------------------------------------------------

import { useRef } from "react";

import type { TabInstance, TabId } from "@/lib/shell/tabs";
import { usePreservedScroll } from "@/hooks/usePreservedScroll";

import ModelsPage from "@/components/listen_speak/models/ModelsPage";
import DictationPage from "@/components/listen_speak/dictation/DictationPage";
import WorkflowPage from "@/components/tools/WorkflowPage";
import DatasetsSyncPage from "@/components/listen_speak/datasets/DatasetsSyncPage";
import DevicesHubPage from "@/components/devices/DevicesHubPage";
import BooksPage from "@/components/books/BooksPage";
import ReadAloudPage from "@/components/read_aloud/ReadAloudPage";
import CardsPage from "@/components/cards/CardsPage";
import TasksPage from "@/components/tools/TasksPage";
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

interface PageHostProps {
  /** Open instances, in strip order. */
  order: TabInstance[];
  /** The instanceId currently shown. */
  focusedId: string;
  /** Focus-or-open a tab kind, the same seam the sidebar/menus use. */
  onNavigate: (tab: TabId) => void;
}

function PageFor({
  tabId,
  focused,
  onNavigate,
}: {
  tabId: TabId;
  focused: boolean;
  onNavigate: (tab: TabId) => void;
}) {
  switch (tabId) {
    case "dictation":
      return <DictationPage active={focused} />;
    case "workflow":
      return <WorkflowPage />;
    case "datasets-sync":
      return <DatasetsSyncPage onNavigate={(tab) => onNavigate(tab as TabId)} />;
    case "devices-hub":
      return <DevicesHubPage />;
    case "simple-words":
      return <SimpleWordsPage />;
    case "books":
      return <BooksPage />;
    case "read-aloud":
      return <ReadAloudPage />;
    case "cards":
      return <CardsPage />;
    case "tasks":
      return <TasksPage />;
    case "edge-tts":
      return <TtsPage />;
    case "models":
      return <ModelsPage />;
    case "agent":
      return <AgentPage />;
    case "settings":
      return <SettingsPage />;
    case "llm-chat":
      return <LLMChatPage />;
    case "chat":
      return <DeviceChatPage active={focused} />;
    case "ocr":
      return <OcrPage />;
    case "split-text":
      return <SplitTextPage />;
    case "pdf-reader":
      return <PdfReaderPage />;
    case "wiki":
      return <WikiPage />;
    case "logs":
      return <LogPage />;
    case "workspaces":
      return <WorkspacesPage />;
    default:
      return null;
  }
}

/** One open instance. Its own component so the scroll memory is per pane. */
function TabPane({
  instance,
  focused,
  onNavigate,
}: {
  instance: TabInstance;
  focused: boolean;
  onNavigate: (tab: TabId) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  usePreservedScroll(ref, focused);

  return (
    <div ref={ref} style={{ display: focused ? "flex" : "none" }} className="flex-1 min-h-0">
      <PageFor tabId={instance.tabId} focused={focused} onNavigate={onNavigate} />
    </div>
  );
}

export default function PageHost({ order, focusedId, onNavigate }: PageHostProps) {
  return (
    <>
      {order.map((inst) => (
        <TabPane
          key={inst.instanceId}
          instance={inst}
          focused={inst.instanceId === focusedId}
          onNavigate={onNavigate}
        />
      ))}
    </>
  );
}
