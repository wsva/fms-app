"use client";

import { useEffect, useState } from "react";
import { saveSurface, useAgentControl } from "@/lib/agent/control";
import { btnGhost, hint, inputCls } from "./styles";

/** Tab 3 — the two things here that are still settings: the policy mode that
 *  decides what an attached agent may call, whether the MCP surface is served at
 *  all, and whether the dock exists as a second way in.
 *
 *  These live on a page with its own Save rather than in Settings because an
 *  agent under `enforce` is refused the ability to change them: the surface that
 *  governs agents is not the surface they can reach. */
export default function SurfacePanel() {
  const { surface, manual, saving, note } = useAgentControl();
  const [draft, setDraft] = useState<{
    mcp_policy: string;
    mcp_enabled: boolean;
    goose_acp_enabled: boolean;
  } | null>(null);
  const [saved, setSaved] = useState(false);

  // Seed (and re-seed after every save) from what the backend holds, so the
  // controls never show an edit that was not written.
  useEffect(() => {
    if (!surface) return;
    setDraft({
      mcp_policy: surface.mcp_policy,
      mcp_enabled: surface.mcp_enabled,
      goose_acp_enabled: surface.goose_acp_enabled,
    });
  }, [surface]);

  const dirty =
    !!draft &&
    !!surface &&
    (draft.mcp_policy !== surface.mcp_policy ||
      draft.mcp_enabled !== surface.mcp_enabled ||
      draft.goose_acp_enabled !== surface.goose_acp_enabled);

  const patch = (next: Partial<NonNullable<typeof draft>>) => {
    setDraft((prev) => (prev ? { ...prev, ...next } : prev));
    setSaved(false);
  };

  const save = async () => {
    if (!draft) return;
    const ok = await saveSurface(draft);
    setSaved(ok);
    if (ok) setTimeout(() => setSaved(false), 2000);
  };

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-[1.05em] font-semibold text-text-primary mb-1">Agent surface</h2>
        <p className={hint}>
          What this build exposes to agents, and how much an attached agent may do. Live policy: <strong>{manual?.trust.policy.mode ?? surface?.mcp_policy ?? "unknown"}</strong>.{!draft && " Reading the settings…"}
        </p>
      </div>

      <div className="max-w-md">
        <label className="block font-medium mb-1" htmlFor="mcp-policy">
          Policy mode
        </label>
        <p className={`${hint} mb-2`}>
          <em>permissive</em> classifies and journals every MCP call but refuses nothing. <em>enforce</em> additionally refuses the destructive and human-handoff tiers, and refuses an agent&apos;s attempt to change this setting.
        </p>
        <select
          id="mcp-policy"
          className={inputCls}
          value={draft?.mcp_policy ?? "permissive"}
          onChange={(e) => patch({ mcp_policy: e.target.value })}
          disabled={!draft}
        >
          <option value="permissive">permissive — journal only</option>
          <option value="enforce">enforce — refuse destructive tools</option>
        </select>
      </div>

      {/* The MCP surface is a runtime switch, not a build feature. It is baked
          into the router, so saving this restarts the local web service — said
          out loud because an attached agent loses its tools for a moment. */}
      <label className="flex items-start gap-2 cursor-pointer max-w-2xl">
        <input
          type="checkbox"
          className="w-4 h-4 mt-0.5"
          checked={draft?.mcp_enabled ?? true}
          onChange={(e) => patch({ mcp_enabled: e.target.checked })}
          disabled={!draft}
        />
        <span>
          <span className="font-medium">Expose the MCP server (agents)</span>
          <span className={`block ${hint}`}>
            Serves <code>/mcp</code> and the generated <code>/agent.md</code> manual. Saving restarts the local web service, because the routes are chosen when the router is built: an agent attached right now loses its tools until the restart returns, and <code>/api/v1/status</code> keeps answering with <code>mcp_enabled: false</code> so a peer can tell why <code>/mcp</code> 404s.
          </span>
        </span>
      </label>

      <label className="flex items-start gap-2 cursor-pointer max-w-2xl">
        <input
          type="checkbox"
          className="w-4 h-4 mt-0.5"
          checked={draft?.goose_acp_enabled ?? false}
          onChange={(e) => patch({ goose_acp_enabled: e.target.checked })}
          disabled={!draft}
        />
        <span>
          <span className="font-medium">Show the agent dock</span>
          <span className={`block ${hint}`}>
            The dock is the second way in: it attaches FmS to an agent host. Slower to set up than attaching an agent to FmS, but it is the only window where FmS can show an agent&apos;s reasoning, tool calls and permission requests. Pick the host on the Hosting tab.
          </span>
        </span>
      </label>

      <div className="flex flex-wrap items-center gap-3">
        <button
          type="button"
          className={btnGhost}
          onClick={() => void save()}
          disabled={!dirty || saving}
        >
          {saving ? "Saving…" : "Save"}
        </button>
        {saved && <span className="text-sm text-green-500">Saved.</span>}
        {!saved && dirty && !saving && (
          <span className="text-sm text-text-tertiary">Unsaved changes.</span>
        )}
        {note?.kind === "err" && <span className="text-sm text-error-text">{note.text}</span>}
      </div>
    </div>
  );
}
