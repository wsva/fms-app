"use client";

import { copyAgentText, refreshAgent, useAgentControl } from "@/lib/agent/control";
import { btnGhost, codeBox, hint } from "./styles";

/** Tab 1 — how to attach an agent to FmS.
 *
 *  Read-only by design: the port is the one the web service bound, the tool
 *  count comes from the live router, and every payload below is the string
 *  `/agent.md` serves. One generator, so this panel cannot disagree with the
 *  manual an agent fetches. Nothing here writes another product's config file —
 *  FmS hands the line over and the user pastes it. */
export default function ConnectPanel() {
  const { manual, manualLoading, hosts, surface, copied } = useAgentControl();

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
            : " — /mcp and the manual answer 404 until you switch it back on (Surface tab)."}
        </p>
        <p className="text-text-secondary">
          Agent dock:{" "}
          {hosts?.connected ? (
            <>
              hosting <strong>{hosts.session?.display_name ?? hosts.active_id ?? "a host"}</strong>{" "}
              via <strong>{hosts.session?.transport ?? "?"}</strong>
              {hosts.session?.mcp_transport ? (
                <>
                  , FmS tools over <strong>{hosts.session.mcp_transport}</strong>
                </>
              ) : null}
              .
            </>
          ) : (
            "no host hosted — see the Hosting tab."
          )}
        </p>
        {manual && !manual.endpoints.running && (
          <p className="text-error-text">{manual.endpoints.note}</p>
        )}
      </div>

      {/* Payload A — the pointer sentence. One line, and the manual it points at
          teaches the rest. */}
      <div>
        <label className="block font-medium mb-1">1 · Paste this sentence (preferred)</label>
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

      {/* Payload B — the inline block. "Can attach MCP" is not the same as "can
          fetch a URL", and without this the whole second class of agents fails
          silently. */}
      <div>
        <label className="block font-medium mb-1">2 · Or paste this block</label>
        <p className={`${hint} mb-2`}>{manual?.connect.inline_when ?? "For an agent that can attach an MCP server but cannot fetch a URL."}</p>
        <div className="flex items-start gap-2">
          <pre className={`flex-1 whitespace-pre-wrap ${codeBox}`}>
            {manual?.connect.inline_block ?? "—"}
          </pre>
          <button
            type="button"
            className={btnGhost}
            onClick={() => void copyAgentText("inline", manual?.connect.inline_block ?? "")}
            disabled={!manual}
          >
            {copied === "inline" ? "Copied" : "Copy"}
          </button>
        </div>
      </div>

      {/* Per-product config lines, still copy-only. */}
      {(manual?.connect.snippets.length ?? 0) > 0 && (
        <div>
          <label className="block font-medium mb-1">Config lines</label>
          <p className={`${hint} mb-2`}>{manual?.connect.never}</p>
          <ul className="space-y-4">
            {manual?.connect.snippets.map((s) => (
              <li key={s.product} className="space-y-1">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-sm text-text-secondary">{s.product}</span>
                  <button
                    type="button"
                    className={btnGhost}
                    onClick={() => void copyAgentText(s.product, s.text)}
                  >
                    {copied === s.product ? "Copied" : "Copy"}
                  </button>
                </div>
                {/* The payload owns the whole width under its product name: a
                    side-by-side label column made the config lines narrow and the
                    product names wrap. */}
                <pre className={`whitespace-pre-wrap ${codeBox}`}>{s.text}</pre>
                {/* Product-specific footwork the config file alone cannot say —
                    WorkBuddy loads nothing until the connector is trusted. */}
                {s.note && <p className={hint}>{s.note}</p>}
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
