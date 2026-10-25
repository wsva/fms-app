"use client";

import { useEffect, useState } from "react";
import { Cable, PlugZap, SlidersHorizontal } from "lucide-react";
import { PageHeader } from "@/components/ui/toolbar";
import {
  ensureAgentLoaded,
  ensureSurfaceLoaded,
  useAgentControl,
} from "@/lib/agent/control";
import { isMobileApp } from "@/lib/platform";
import ConnectPanel from "./ConnectPanel";
import HostingPanel from "./HostingPanel";
import SurfacePanel from "./SurfacePanel";

const TABS = [
  { key: "connect", label: "Connect", icon: Cable },
  { key: "hosting", label: "Hosting", icon: PlugZap },
  { key: "surface", label: "Surface", icon: SlidersHorizontal },
] as const;

type TabKey = (typeof TABS)[number]["key"];

/** Tools → Agent: the configuration surface for both directions of agent
 *  integration.
 *
 *  Configuration only, deliberately. The conversation, the permission prompts and
 *  the host-approval ask live in the dock, which is mounted on every screen: an
 *  earlier tab-based agent panel was removed for exactly that reason — it went
 *  invisible whenever the agent navigated the user elsewhere. This page can
 *  therefore be a normal tab without losing anything that has to be seen.
 *
 *  Desktop only: every command behind it is absent from the Android build. */
export default function AgentPage() {
  const [tab, setTab] = useState<TabKey>("connect");
  const { hosts, confirm } = useAgentControl();
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

  const hosted = hosts?.connected ? hosts.session?.display_name ?? "a host" : null;

  return (
    <div className="flex flex-1 min-w-0 flex-col h-full bg-bg-base">
      <PageHeader
        title="Agent"
        badge={
          <span
            className={`text-[10px] font-medium uppercase tracking-wide px-1.5 py-0.5 rounded border shrink-0 ${
              hosted
                ? "bg-accent-bg/15 border-accent-bg/30 text-accent-bg"
                : "bg-bg-muted border-border-default text-text-tertiary"
            }`}
            title={hosted ? `The dock is hosting ${hosted}` : "No agent hosted in the dock"}
          >
            {hosted ? "hosting" : "not hosted"}
          </span>
        }
      />

      {/* Tabs. Related configuration, split by what the user came to do rather
          than by backend module: paste a line, pick a host, govern the surface. */}
      <div className="flex gap-1 px-4 border-b border-border-default shrink-0">
        {TABS.map(({ key, label, icon: Icon }) => (
          <button
            key={key}
            type="button"
            onClick={() => setTab(key)}
            className={`flex items-center gap-1.5 px-3 py-2 text-sm font-medium border-b-2 -mb-px transition-colors ${
              tab === key
                ? "border-accent-bg text-text-primary"
                : "border-transparent text-text-tertiary hover:text-text-secondary"
            }`}
          >
            <Icon size={15} />
            {label}
            {key === "hosting" && confirm && (
              <span className="w-1.5 h-1.5 rounded-full bg-yellow-500" aria-hidden />
            )}
          </button>
        ))}
      </div>

      <div className="flex-1 overflow-y-auto min-h-0 p-4">
        <div className="max-w-3xl">
          {tab === "connect" && <ConnectPanel />}
          {tab === "hosting" && <HostingPanel />}
          {tab === "surface" && <SurfacePanel />}
        </div>
      </div>
    </div>
  );
}
