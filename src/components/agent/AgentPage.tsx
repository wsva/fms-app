"use client";

/**
 * Agent page — an ACP client front-end for a local `goose serve`.
 *
 * The Rust backend (`agent_acp.rs`) owns the WebSocket/ACP session; this page
 * only sends prompts, renders the streamed session updates, answers permission
 * requests, and reports the current UI state so the agent can drive the app.
 */

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Send,
  Bot,
  User,
  Loader2,
  Plug,
  PlugZap,
  Square,
  Wrench,
  Brain,
  ListChecks,
} from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { startRecording, stopRecording, type VoiceState } from "@/lib/voice-input";
import { VoiceMicButton } from "@/components/voice/VoiceMicButton";
import type { TabId } from "@/components/layout/Sidebar";
import type {
  AgentStatus,
  AgentMessageChunk,
  AgentThought,
  AgentToolCall,
  AgentToolUpdate,
  AgentPlan,
  AgentPlanEntry,
  AgentPermissionRequest,
  AgentDone,
  AgentError,
} from "@/lib/agent/types";

// ---------------------------------------------------------------------------
// View model — a flat list of rendered blocks.
// ---------------------------------------------------------------------------

type Block =
  | { kind: "user"; text: string }
  | { kind: "agent"; text: string }
  | { kind: "thought"; text: string }
  | {
      kind: "tool";
      id: string;
      title: string;
      toolKind?: string | null;
      status?: string | null;
      rawInput?: unknown;
      rawOutput?: unknown;
    }
  | { kind: "plan"; entries: AgentPlanEntry[] }
  | { kind: "error"; text: string };

export default function AgentPage({
  active,
  activeTab,
}: {
  active: boolean;
  activeTab: TabId;
}) {
  const [blocks, setBlocks] = useState<Block[]>([]);
  const [input, setInput] = useState("");
  const [status, setStatus] = useState<AgentStatus>({ connected: false });
  const [enabled, setEnabled] = useState(false);
  const [busy, setBusy] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const [permission, setPermission] = useState<AgentPermissionRequest | null>(null);
  const [voiceState, setVoiceState] = useState<VoiceState>("idle");
  const [voiceError, setVoiceError] = useState("");

  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const connectAttempted = useRef(false);

  // ---- Auto-scroll ----
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [blocks]);

  // ---- Block helpers ----
  const appendStream = useCallback((kind: "agent" | "thought", text: string) => {
    setBlocks((prev) => {
      const copy = [...prev];
      const last = copy[copy.length - 1];
      if (last && last.kind === kind) {
        copy[copy.length - 1] = { ...last, text: last.text + text } as Block;
      } else {
        copy.push({ kind, text } as Block);
      }
      return copy;
    });
  }, []);

  // ---- Load settings + connection status ----
  const refreshStatus = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const g = await invoke<{ goose_acp_enabled?: boolean }>("settings_get_global");
      setEnabled(!!g.goose_acp_enabled);
      const s = await invoke<AgentStatus>("agent_status");
      setStatus(s);
    } catch (e) {
      console.error("agent: failed to read status/settings", e);
    }
  }, []);

  const doConnect = useCallback(async () => {
    if (!isTauri() || connecting) return;
    setConnecting(true);
    try {
      const s = await invoke<AgentStatus>("agent_connect");
      setStatus(s);
      setBlocks((prev) => [
        ...prev,
        { kind: "agent", text: `Connected to goose (session ${s.session_id ?? "?"}).` },
      ]);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setBlocks((prev) => [...prev, { kind: "error", text: `Connection failed: ${msg}` }]);
    } finally {
      setConnecting(false);
    }
  }, [connecting]);

  const doDisconnect = useCallback(async () => {
    if (!isTauri()) return;
    try {
      await invoke("agent_disconnect");
      setStatus({ connected: false });
    } catch (e) {
      console.error("agent: disconnect failed", e);
    }
  }, []);

  useEffect(() => {
    refreshStatus();
  }, [refreshStatus]);

  // Auto-connect when the page becomes visible, is enabled, and not connected.
  useEffect(() => {
    if (!active || !enabled || status.connected || connectAttempted.current) return;
    connectAttempted.current = true;
    void doConnect();
  }, [active, enabled, status.connected, doConnect]);

  // Re-arm the auto-connect attempt once a connection drops.
  useEffect(() => {
    if (!status.connected) connectAttempted.current = false;
  }, [status.connected]);

  // ---- Report UI state so app_get_ui_state can answer the agent ----
  useEffect(() => {
    if (!isTauri() || !active) return;
    invoke("agent_report_ui_state", { activeTab, activeDatasetUuid: null }).catch(() => {});
  }, [activeTab, active]);

  // ---- Event listeners (registered once) ----
  useEffect(() => {
    if (!isTauri()) return;
    const unlisteners: (() => void)[] = [];
    let cancelled = false;
    const reg = <T,>(event: string, handler: (payload: T) => void) => {
      listen<T>(event, (e) => handler(e.payload)).then((fn) => {
        if (cancelled) fn();
        else unlisteners.push(fn);
      });
    };

    reg<AgentStatus>("agent-status", (p) => setStatus(p));
    reg<AgentMessageChunk>("agent-message-chunk", (p) => appendStream("agent", p.text));
    reg<AgentThought>("agent-thought", (p) => appendStream("thought", p.text));
    reg<AgentToolCall>("agent-tool-call", (p) =>
      setBlocks((prev) => {
        const idx = prev.findIndex((b) => b.kind === "tool" && b.id === p.id);
        if (idx >= 0) {
          const copy = [...prev];
          const existing = copy[idx] as Extract<Block, { kind: "tool" }>;
          copy[idx] = {
            ...existing,
            title: p.title ?? existing.title,
            toolKind: p.kind ?? existing.toolKind,
            status: p.status ?? existing.status,
            rawInput: p.raw_input ?? existing.rawInput,
          };
          return copy;
        }
        return [
          ...prev,
          {
            kind: "tool",
            id: p.id,
            title: p.title ?? p.id,
            toolKind: p.kind,
            status: p.status,
            rawInput: p.raw_input,
          },
        ];
      })
    );
    reg<AgentToolUpdate>("agent-tool-update", (p) =>
      setBlocks((prev) => {
        const idx = prev.findIndex((b) => b.kind === "tool" && b.id === p.id);
        if (idx < 0) return prev;
        const copy = [...prev];
        const existing = copy[idx] as Extract<Block, { kind: "tool" }>;
        copy[idx] = {
          ...existing,
          status: p.status ?? existing.status,
          rawOutput: p.raw_output ?? existing.rawOutput,
        };
        return copy;
      })
    );
    reg<AgentPlan>("agent-plan", (p) =>
      setBlocks((prev) => {
        const idx = prev.map((b) => b.kind).lastIndexOf("plan");
        if (idx >= 0) {
          const copy = [...prev];
          copy[idx] = { kind: "plan", entries: p.entries ?? [] };
          return copy;
        }
        return [...prev, { kind: "plan", entries: p.entries ?? [] }];
      })
    );
    reg<AgentPermissionRequest>("agent-permission-request", (p) => setPermission(p));
    reg<AgentDone>("agent-done", () => setBusy(false));
    reg<AgentError>("agent-error", (p) => {
      setBlocks((prev) => [...prev, { kind: "error", text: p.message }]);
      setBusy(false);
    });

    return () => {
      cancelled = true;
      unlisteners.forEach((fn) => fn());
    };
  }, [appendStream]);

  // ---- Send prompt ----
  const handleSend = useCallback(async () => {
    const text = input.trim();
    if (!text || busy || !status.connected) return;
    setBlocks((prev) => [...prev, { kind: "user", text }]);
    setInput("");
    setBusy(true);
    try {
      await invoke("agent_send_prompt", { text });
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setBlocks((prev) => [...prev, { kind: "error", text: msg }]);
      setBusy(false);
    }
  }, [input, busy, status.connected]);

  const handleCancel = useCallback(async () => {
    if (!isTauri()) return;
    try {
      await invoke("agent_cancel");
    } catch (e) {
      console.error("agent: cancel failed", e);
    }
  }, []);

  const respondPermission = useCallback(
    async (optionId: string | null) => {
      if (!permission) return;
      try {
        await invoke("agent_respond_permission", {
          requestId: permission.request_id,
          optionId,
        });
      } catch (e) {
        console.error("agent: permission response failed", e);
      } finally {
        setPermission(null);
      }
    },
    [permission]
  );

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void handleSend();
    }
  }

  // ---- Voice input (mirrors ChatPage.handleVoiceToggle) ----
  async function handleVoiceToggle() {
    if (voiceState === "processing") return;
    if (voiceState === "recording") {
      stopRecording();
      return;
    }
    setVoiceError("");
    if (isTauri()) {
      try {
        const st = await invoke<{ active_version: string | null }>("model_get_status");
        if (!st.active_version) {
          setVoiceState("processing");
          await invoke("model_start");
        }
      } catch {
        setVoiceError("Failed to load STT model");
        return;
      }
    }
    await startRecording({
      onStateChange: setVoiceState,
      onResult: (text) => {
        setInput((prev) => (prev ? prev + " " : "") + text.trim());
        inputRef.current?.focus();
        setVoiceState("idle");
      },
      onError: (err) => {
        setVoiceError(err);
        setVoiceState("idle");
      },
    });
  }

  // ---- Render ----
  return (
    <main className="flex-1 flex flex-col h-full">
      {/* Header / connection banner */}
      <div className="p-4 border-b border-border-default flex items-center gap-3">
        <h1 className="text-lg font-semibold flex items-center gap-2">
          <Bot size={18} /> Agent
        </h1>
        <span
          className={`text-[10px] font-medium uppercase tracking-wide px-1.5 py-0.5 rounded border ${
            status.connected
              ? "bg-accent-bg/15 border-accent-bg/30 text-accent-bg"
              : "bg-bg-muted border-border-default text-text-tertiary"
          }`}
        >
          {status.connected ? "connected" : "disconnected"}
        </span>
        <div className="flex-1" />
        {status.connected ? (
          <button
            type="button"
            onClick={doDisconnect}
            className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-bg-muted border border-border-default text-xs font-medium text-text-secondary hover:bg-bg-hover transition-colors"
          >
            <PlugZap size={14} /> Disconnect
          </button>
        ) : (
          <button
            type="button"
            onClick={doConnect}
            disabled={connecting}
            className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-accent-bg text-white text-xs font-medium hover:opacity-90 transition-opacity disabled:opacity-50"
          >
            {connecting ? <Loader2 size={14} className="animate-spin" /> : <Plug size={14} />}
            Connect
          </button>
        )}
      </div>

      {!enabled && !status.connected && (
        <div className="px-4 py-2 text-xs text-text-tertiary bg-bg-muted border-b border-border-default">
          The Agent is disabled. Enable it and set the goose ACP URL/secret in
          Settings → Agent (Goose ACP), then start <code>goose serve</code>.
        </div>
      )}

      {/* Message list */}
      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        {blocks.length === 0 && (
          <div className="flex flex-col items-center justify-center h-full text-center text-text-tertiary">
            <Bot size={48} className="mb-4 opacity-50" />
            <p className="text-sm max-w-md">
              {status.connected
                ? 'Ask the agent to do something — try "give me a quiz" or "open the cards page".'
                : "Connect to goose to start controlling the app with natural messages."}
            </p>
          </div>
        )}

        {blocks.map((b, i) => (
          <BlockView key={i} block={b} streaming={busy && i === blocks.length - 1} />
        ))}

        {/* Permission prompt */}
        {permission && (
          <div className="p-3 rounded-xl border border-accent-bg/40 bg-accent-bg/10 space-y-2">
            <p className="text-sm font-medium text-text-primary flex items-center gap-2">
              <Wrench size={14} /> The agent wants to run a tool
            </p>
            <p className="text-xs text-text-secondary break-words">
              {String(
                (permission.tool_call?.title as string) ??
                  (permission.tool_call?.name as string) ??
                  "a tool"
              )}
            </p>
            <div className="flex flex-wrap gap-2">
              {permission.options.map((o) => (
                <button
                  key={o.optionId}
                  type="button"
                  onClick={() => respondPermission(o.optionId)}
                  className={`px-3 py-1.5 rounded-lg text-xs font-medium transition-colors ${
                    o.kind?.startsWith("reject")
                      ? "bg-bg-muted border border-border-default text-text-secondary hover:bg-bg-hover"
                      : "bg-accent-bg text-white hover:opacity-90"
                  }`}
                >
                  {o.name ?? o.optionId}
                </button>
              ))}
              <button
                type="button"
                onClick={() => respondPermission(null)}
                className="px-3 py-1.5 rounded-lg text-xs font-medium bg-bg-muted border border-border-default text-text-tertiary hover:bg-bg-hover"
              >
                Cancel
              </button>
            </div>
          </div>
        )}

        <div ref={messagesEndRef} />
      </div>

      {/* Input row */}
      <div className="p-4 border-t border-border-default">
        {voiceError && <p className="text-xs text-error-text mb-2">{voiceError}</p>}
        <div className="flex gap-2 items-end">
          <textarea
            ref={inputRef}
            className="flex-1 px-4 py-2.5 rounded-lg bg-bg-muted border border-border-default text-text-primary text-sm resize-none focus:outline-none focus:border-accent"
            placeholder={
              status.connected
                ? "Message the agent… (Enter to send, Shift+Enter for newline)"
                : "Connect to goose to send a message"
            }
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            rows={1}
            disabled={!status.connected || busy}
          />
          <VoiceMicButton
            state={voiceState}
            surface="composer"
            onPress={handleVoiceToggle}
            disabled={!status.connected}
          />
          {busy ? (
            <button
              type="button"
              className="p-2.5 rounded-lg bg-bg-muted border border-border-default text-text-secondary hover:bg-bg-hover transition-colors"
              onClick={handleCancel}
              title="Cancel the current turn"
            >
              <Square size={18} />
            </button>
          ) : (
            <button
              type="button"
              className="p-2.5 rounded-lg bg-accent-bg text-white hover:opacity-90 transition-opacity disabled:opacity-50 disabled:cursor-not-allowed"
              onClick={handleSend}
              disabled={!input.trim() || !status.connected || busy}
            >
              <Send size={18} />
            </button>
          )}
        </div>
      </div>
    </main>
  );
}

// ---------------------------------------------------------------------------
// Block rendering
// ---------------------------------------------------------------------------

function BlockView({ block, streaming }: { block: Block; streaming: boolean }) {
  switch (block.kind) {
    case "user":
      return (
        <div className="flex gap-3 justify-end">
          <div className="max-w-[70%] px-4 py-2.5 rounded-2xl whitespace-pre-wrap bg-accent-bg text-white text-sm">
            {block.text}
          </div>
          <div className="w-8 h-8 rounded-full bg-bg-muted flex items-center justify-center shrink-0">
            <User size={16} className="text-text-secondary" />
          </div>
        </div>
      );
    case "agent":
      return (
        <div className="flex gap-3 justify-start">
          <div className="w-8 h-8 rounded-full bg-accent-bg/20 flex items-center justify-center shrink-0">
            <Bot size={16} className="text-accent-bg" />
          </div>
          <div className="max-w-[70%] px-4 py-2.5 rounded-2xl whitespace-pre-wrap bg-bg-muted text-text-primary text-sm">
            {block.text === "" && streaming ? (
              <Loader2 size={16} className="animate-spin text-text-tertiary" />
            ) : (
              block.text
            )}
          </div>
        </div>
      );
    case "thought":
      return (
        <div className="flex gap-3 justify-start">
          <div className="w-8 shrink-0" />
          <details className="max-w-[70%] text-xs text-text-tertiary">
            <summary className="cursor-pointer flex items-center gap-1.5 select-none">
              <Brain size={12} /> Thinking
            </summary>
            <div className="mt-1 px-3 py-2 rounded-lg bg-bg-card border border-border-default whitespace-pre-wrap">
              {block.text}
            </div>
          </details>
        </div>
      );
    case "tool":
      return (
        <div className="flex gap-3 justify-start">
          <div className="w-8 shrink-0" />
          <div className="max-w-[80%] px-3 py-2 rounded-lg bg-bg-card border border-border-default text-xs space-y-1">
            <div className="flex items-center gap-2">
              <Wrench size={12} className="text-text-tertiary shrink-0" />
              <span className="font-medium text-text-primary break-words">{block.title}</span>
              {block.status && (
                <span className="ml-auto px-1.5 py-0.5 rounded bg-bg-muted text-[10px] uppercase tracking-wide text-text-tertiary">
                  {block.status}
                </span>
              )}
            </div>
            {(block.rawInput != null || block.rawOutput != null) && (
              <details className="text-text-tertiary">
                <summary className="cursor-pointer select-none">details</summary>
                <pre className="mt-1 p-2 rounded bg-bg-muted overflow-x-auto whitespace-pre-wrap break-words">
                  {JSON.stringify(
                    { input: block.rawInput ?? null, output: block.rawOutput ?? null },
                    null,
                    2
                  )}
                </pre>
              </details>
            )}
          </div>
        </div>
      );
    case "plan":
      return (
        <div className="flex gap-3 justify-start">
          <div className="w-8 shrink-0" />
          <div className="max-w-[80%] px-3 py-2 rounded-lg bg-bg-card border border-border-default text-xs space-y-1">
            <p className="flex items-center gap-2 font-medium text-text-primary">
              <ListChecks size={12} /> Plan
            </p>
            <ul className="space-y-0.5">
              {(block.entries ?? []).map((e, i) => (
                <li key={i} className="flex items-center gap-2 text-text-secondary">
                  <span
                    className={`w-1.5 h-1.5 rounded-full shrink-0 ${
                      e.status === "completed"
                        ? "bg-accent-bg"
                        : e.status === "in_progress"
                        ? "bg-yellow-500"
                        : "bg-text-tertiary"
                    }`}
                  />
                  <span className="break-words">{e.content}</span>
                </li>
              ))}
            </ul>
          </div>
        </div>
      );
    case "error":
      return (
        <div className="flex gap-3 justify-start">
          <div className="w-8 shrink-0" />
          <div className="max-w-[80%] px-3 py-2 rounded-lg bg-error-bg text-error-text text-xs break-words">
            {block.text}
          </div>
        </div>
      );
  }
}
