"use client";

import { copyAgentText, refreshAgent, useAgentControl } from "@/lib/agent/control";
import { btnGhost, codeBox, hint } from "./styles";

/** Section 1 of the Agent page — how to attach an agent to FmS.
 *
 *  Read-only by design: the port is the one the web service bound, the tool
 *  count comes from the live router, and the payload below is the string
 *  `/agent.md` serves. One generator, so this panel cannot disagree with the
 *  manual an agent fetches. Nothing here writes another product's config file —
 *  FmS hands the line over and the user pastes it.
 *
 *  One payload, not three. The pointer sentence is the whole instruction: this
 *  page hands over the address, `/agent.md` teaches the rest, and the only thing
 *  it keeps back is the per-product registration lines — for a human adding a
 *  second client, not for the agent this page is meant to start. Carrying them
 *  here as well made the page two scrolls of near-identical copy for a reader who
 *  only ever needs the first. */
export default function ConnectPanel() {
  const { manual, manualLoading, surface, copied } = useAgentControl();

  return (
    <div className="space-y-6">
      <div>
        <div className="flex items-start justify-between gap-3 mb-1">
          <h2 className="text-[1.05em] font-semibold text-text-primary">Attach an agent</h2>
          <button
            type="button"
            className={btnGhost}
            onClick={() => void refreshAgent()}
            disabled={manualLoading}
          >
            {manualLoading ? "Reading…" : "Refresh"}
          </button>
        </div>
        <p className={hint}>
          FmS runs a Model Context Protocol server on the loopback interface. Copy one line below into any MCP-capable agent and it configures itself: the manual it then reads is generated from this build ({manual ? `${manual.tools.count} tools` : "…"}), so it cannot drift from the tools that actually exist. The endpoint is not editable — it follows the port the service really bound.
        </p>
      </div>

      {/* Status half: everything read back, nothing inferred. */}
      <div className="rounded-md border border-border-default p-3 text-sm space-y-1">
        <div className="flex flex-wrap items-center gap-2">
          <span
            className={manual?.endpoints.running ? "text-green-500" : "text-error-text"}
            aria-hidden
          >
            ●
          </span>
          <span className="font-medium">
            {manual
              ? manual.endpoints.running
                ? `Web service running on port ${manual.endpoints.port}`
                : "Web service stopped"
              : "Web service status unknown"}
          </span>
        </div>
        <p className="text-text-secondary">
          MCP endpoint: <code>{manual?.endpoints.mcp ?? "—"}</code>
        </p>
        <p className="text-text-secondary">
          Manual: <code>{manual?.endpoints.manual_markdown ?? "—"}</code>
        </p>
        <p className="text-text-secondary">
          This page sits in the <strong>{manual?.trust.zone ?? "unknown"}</strong> trust zone
          {manual?.trust.zone === "loopback"
            ? " — a local agent needs no credentials at all."
            : "."}
        </p>
        <p className="text-text-secondary">
          MCP surface: <strong>{surface?.mcp_enabled ?? true ? "exposed" : "off"}</strong>
          {surface?.mcp_enabled ?? true
            ? " — the endpoint below answers."
            : " — /mcp and the manual answer 404 until you switch it back on (Agent surface, below)."}
        </p>
        {manual && !manual.endpoints.running && (
          <p className="text-error-text">{manual.endpoints.note}</p>
        )}
      </div>

      {/* The pointer sentence — one line, and the manual it points at teaches
          the rest. */}
      <div>
        <label className="block font-medium mb-1">Paste this sentence into your agent</label>
        <p className={`${hint} mb-2`}>{manual?.connect.pointer_when ?? "For any agent that can fetch a URL."}</p>
        <div className="flex items-start gap-2">
          <code className={`flex-1 ${codeBox}`}>{manual?.connect.pointer_sentence ?? "—"}</code>
          <button
            type="button"
            className={btnGhost}
            onClick={() => void copyAgentText("pointer", manual?.connect.pointer_sentence ?? "")}
            disabled={!manual}
          >
            {copied === "pointer" ? "Copied" : "Copy"}
          </button>
        </div>
      </div>
    </div>
  );
}
