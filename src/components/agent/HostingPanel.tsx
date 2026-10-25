"use client";

import {
  showInDock,
  testDockConnection,
  useAgentControl,
} from "@/lib/agent/control";
import AgentConfirmCard from "./AgentConfirmCard";
import { btnGhost, hint, inputCls } from "./styles";

/** Tab 2 — which agent the dock hosts, i.e. FmS attached *to* an agent instead
 *  of the other way round.
 *
 *  The select offers the app's host records and nothing else: a free-text
 *  address is exactly what those records replaced, and whether dialling one needs
 *  approval is decided in Rust (`needs_confirmation`), never re-derived here. */
export default function HostingPanel() {
  const { hosts, surface, note } = useAgentControl();
  const busy = note?.kind === "busy";
  const talkingTo = hosts?.session?.display_name ?? hosts?.active_id ?? null;

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-[1.05em] font-semibold text-text-primary mb-1">Host an agent in the dock</h2>
        <p className={hint}>
          The dock is the second way in: instead of attaching to FmS, it attaches FmS to an agent host — a <code>goose serve</code> you started yourself (<code>ws</code>), or a program FmS spawns and stops (<code>stdio</code>) {surface?.goose_acp_url ? <> (default <code>{surface.goose_acp_url}</code>)</> : null}. It is the only window where FmS can show an agent&apos;s reasoning, tool calls and permission requests — which is also why the dock stays visible on every page and this page never hosts the conversation itself.
        </p>
      </div>

      {/* Live session, read back from `agent_host_list`. */}
      <div className="rounded-md border border-border-default p-3 text-sm space-y-1">
        <p className="text-text-secondary">
          Session:{" "}
          {hosts?.connected ? (
            <>
              hosting <strong>{talkingTo ?? "a host"}</strong> via{" "}
              <strong>{hosts.session?.transport ?? "?"}</strong>
              {hosts.session?.session_id ? (
                <>
                  {" "}
                  — session <code>{hosts.session.session_id}</code>
                </>
              ) : null}
              {hosts.session?.mcp_transport ? (
                <>
                  , FmS tools over <strong>{hosts.session.mcp_transport}</strong>
                </>
              ) : null}
              .
            </>
          ) : (
            "nothing hosted — FmS is on its own."
          )}
        </p>
      </div>

      {/* The switch: which record the dock shows. Two commands behind one select
          — `agent_host_disconnect` for "FmS only", `agent_host_connect` for a
          record. */}
      <div className="max-w-md">
        <label className="block font-medium mb-1" htmlFor="dock-host">
          Shown in
        </label>
        <p className={`${hint} mb-2`}>
          The dock hosts one agent at a time. Hosts come from the app&apos;s records — an agent adds them with <code>agent_host_register</code>. Whether dialling one needs your approval is decided in the backend, never here.
        </p>
        <select
          id="dock-host"
          className={inputCls}
          value={hosts?.connected ? hosts?.session?.host_id ?? "" : ""}
          onChange={(e) => void showInDock(e.target.value)}
          disabled={busy}
        >
          <option value="">FmS only — no agent hosted</option>
          {(hosts?.hosts ?? []).map((h) => (
            <option key={h.id} value={h.id}>
              {h.display_name} ({h.kind})
              {h.needs_confirmation && !h.approved ? " — needs your approval" : ""}
            </option>
          ))}
        </select>
        {(hosts?.hosts.length ?? 0) === 0 && (
          <p className={`${hint} mt-2`}>
            No host records yet. Until one exists the dock has nothing to dial — use the copy-only lines on the Connect tab, or have an agent register one.
          </p>
        )}
      </div>

      {/* The ask, inline. The dock renders the same card from the same store, so
          an agent-triggered connect can be approved from either place. */}
      <AgentConfirmCard />

      <div className="flex flex-wrap items-center gap-3">
        <button
          type="button"
          className={btnGhost}
          onClick={() => void testDockConnection()}
          disabled={busy}
        >
          {busy ? "Working…" : "Test dock connection"}
        </button>
        {note && note.kind !== "busy" && (
          <span
            className={`text-sm ${note.kind === "ok" ? "text-green-500" : "text-error-text"}`}
          >
            {note.text}
          </span>
        )}
      </div>
    </div>
  );
}
