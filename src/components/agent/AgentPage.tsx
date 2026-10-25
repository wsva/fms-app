"use client";

import { useEffect, useState } from "react";
import { Cable, SlidersHorizontal } from "lucide-react";
import { PageHeader, TabBar, type TabBarItem } from "@/components/ui/toolbar";
import {
  ensureAgentLoaded,
  ensureSurfaceLoaded,
  useAgentControl,
} from "@/lib/agent/control";
import { isMobileApp } from "@/lib/platform";
import ConnectPanel from "./ConnectPanel";
import SurfacePanel from "./SurfacePanel";

const TABS = [
  { id: "connect", label: "Connect", icon: Cable },
  { id: "surface", label: "Surface", icon: SlidersHorizontal },
] as const;

type TabKey = (typeof TABS)[number]["id"];

/** Tools → Agent: the configuration surface for the one direction of agent
 *  integration FmS has — an agent attaching to this app's MCP server.
 *
 *  Configuration only, deliberately. The app does not dial an agent, so there is
 *  no conversation, no permission prompt and no host to approve; what is left is
 *  the copy-only attach instructions (Connect) and the switches that govern what
 *  the MCP surface will answer (Surface).
 *
 *  Desktop only: every command behind it is absent from the Android build. */
export default function AgentPage() {
  const [tab, setTab] = useState<TabKey>("connect");
  const { manual } = useAgentControl();
  // Same deferred platform read as Sidebar / SettingsPage: `isMobileApp()` is
  // false during prerendering and only true inside the Android WebView, so
  // reading it while rendering would desync the server HTML.
  const [mobile, setMobile] = useState(false);

  useEffect(() => {
    setMobile(isMobileApp());
  }, []);

  useEffect(() => {
    void ensureAgentLoaded();
    void ensureSurfaceLoaded();
  }, []);

  if (mobile) return null;

  const listening = Boolean(manual?.endpoints.running);
  const tabs: TabBarItem<TabKey>[] = TABS.map(({ id, label, icon }) => ({
    id,
    label,
    icon,
  }));

  return (
    <div className="flex flex-1 min-w-0 flex-col h-full bg-bg-base">
      <PageHeader
        title="Agent"
        badge={
          <span
            className={`text-[10px] font-medium uppercase tracking-wide px-1.5 py-0.5 rounded border shrink-0 ${
              listening
                ? "bg-accent-bg/15 border-accent-bg/30 text-accent-bg"
                : "bg-bg-muted border-border-default text-text-tertiary"
            }`}
            title={
              listening
                ? `Agents can attach at ${manual?.endpoints.mcp ?? "the MCP endpoint"}`
                : "The MCP server is not answering, so no agent can attach"
            }
          >
            {listening ? "listening" : "offline"}
          </span>
        }
      />

      {/* Tabs. Related configuration, split by what the user came to do rather
          than by backend module: attach an agent, or govern what it may do. */}
      <TabBar
        items={tabs}
        value={tab}
        onChange={setTab}
        label="Agent sections"
        className="shrink-0 px-4"
      />

      {/* Full width. What this page actually shows is code payloads and status
          rows — both improve with room, and a measure cap only wrapped them. The
          prose that does not improve keeps its own limit inside `hint`. */}
      <div className="flex-1 overflow-y-auto min-h-0 p-4">
        {tab === "connect" && <ConnectPanel />}
        {tab === "surface" && <SurfacePanel />}
      </div>
    </div>
  );
}
