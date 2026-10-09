"use client";

/**
 * Agent dock — an ACP client front-end for a local `goose serve`, rendered as
 * an always-accessible right-side panel instead of a regular tab.
 *
 * The Rust backend (`agent_acp.rs`) owns the WebSocket/ACP session; this dock
 * only sends prompts, renders the streamed session updates, answers permission
 * requests, and reports the current UI state so the agent can drive the app
 * from whichever page the user is on. The dock overlays the content area, is
 * toggled via the floating Bot button or Ctrl+Space, and auto-opens when the
 * agent needs attention (permission request, or first output of a turn).
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
  X,
} from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
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

// Dock presentation state, persisted across restarts. Like `dictation.collapsed`,
// localStorage only ever holds an explicit user choice (written in the toggle
// handler), never today's default, so the default rule can evolve.
const DOCK_OPEN_KEY = "agent.dockOpen";
const DOCK_WIDTH_KEY = "agent.dockWidth";
const DEFAULT_DOCK_WIDTH = 380;
const MIN_DOCK_WIDTH = 300;
const MAX_DOCK_WIDTH = 640;

// Floating toggle button geometry. Persisted so users can park it out of the
// way when it covers page content (waveform, video, cue list) and the choice
// survives reloads. Null = default bottom-right corner (matches the pre-drag
// `bottom-4 right-4` styling), so a fresh install looks unchanged.
const FAB_SIZE = 48;
const FAB_MARGIN = 16;
const FAB_EDGE_PAD = 4;
const FAB_POS_KEY = "agent.fabPos";
// Movement past this threshold (in CSS px, summed |dx|+|dy|) is treated as a
// drag and suppresses the click-to-open handler on pointerup.
const FAB_DRAG_THRESHOLD = 4;

/**
 * True when this build actually hosts the agent backend.
 *
 * `ai/agent_acp.rs` is gated behind the Rust `desktop` feature and is absent
 * from the Android binary, so invoking any `agent_*` command on the phone fails
 * with "Command agent_status not found". The shell does not mount this dock on
 * mobile, but it renders the desktop layout for the first commit (the platform
 * is only knowable after mount, see the `mobile` state below), so effects can
 * still fire once on Android — hence every agent IPC call is gated here too.
 *
 * Safe to call from effects and handlers only: `isMobileApp()` falls back to
 * `false` during prerendering, so using it while rendering would break hydration.
 */
function isAgentHost(): boolean {
  return isTauri() && !isMobileApp();
}

export default function AgentDock({ activeTab }: { activeTab: TabId }) {
  const [blocks, setBlocks] = useState<Block[]>([]);
  const [input, setInput] = useState("");
  const [status, setStatus] = useState<AgentStatus>({ connected: false });
  const [enabled, setEnabled] = useState(false);
  const [busy, setBusy] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const [permission, setPermission] = useState<AgentPermissionRequest | null>(null);
  const [voiceState, setVoiceState] = useState<VoiceState>("idle");
  const [voiceError, setVoiceError] = useState("");

  // ---- Dock presentation state ----
  const [mobile, setMobile] = useState(false);
  const [open, setOpenState] = useState(false);
  const [unread, setUnread] = useState(false);
  const [dockWidth, setDockWidth] = useState(DEFAULT_DOCK_WIDTH);
  // One auto-reopen per turn: if the user closes the dock mid-stream we stop
  // fighting them until the next turn starts. Reset on send and on done/error.
  const reopenedThisTurn = useRef(false);

  // ---- Floating toggle button: position + drag state ----
  const [fabPos, setFabPosState] = useState<{ x: number; y: number } | null>(null);
  // Mirror the latest pos so pointerup can persist without re-reading state.
  const fabPosRef = useRef<{ x: number; y: number } | null>(null);
  const setFabPos = (p: { x: number; y: number } | null) => {
    fabPosRef.current = p;
    setFabPosState(p);
  };
  // Live drag session; kept in a ref so pointermove doesn't force a rerender
  // of the whole dock (which would still be mounted while closed, running the
  // ACP listeners and effects). `moved` gates the click handler.
  const fabDrag = useRef({ active: false, startX: 0, startY: 0, startLeft: 0, startTop: 0, moved: 0 });

  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const connectAttempted = useRef(false);

  // setOpen — single funnel for every open/close so the user's explicit choice
  // is persisted; auto-opens pass persist=false.
  const setOpen = useCallback((v: boolean, persist: boolean = true) => {
    setOpenState(v);
    if (v) setUnread(false);
    if (persist) localStorage.setItem(DOCK_OPEN_KEY, v ? "1" : "0");
  }, []);

  // ---- Load persisted presentation state + platform flag ----
  useEffect(() => {
    setMobile(isMobileApp());
    setOpenState(localStorage.getItem(DOCK_OPEN_KEY) === "1");
    const w = parseInt(localStorage.getItem(DOCK_WIDTH_KEY) ?? "", 10);
    if (Number.isFinite(w)) {
      setDockWidth(Math.min(MAX_DOCK_WIDTH, Math.max(MIN_DOCK_WIDTH, w)));
    }
    // Restore FAB position if we have a stored one; anything else falls back
    // to the default bottom-right corner at render time (see `effectivePos`).
    try {
      const raw = localStorage.getItem(FAB_POS_KEY);
      if (raw) {
        const p = JSON.parse(raw);
        if (p && Number.isFinite(p.x) && Number.isFinite(p.y)) {
          setFabPos(clampFabPos(p.x, p.y));
        }
      }
    } catch {
      /* ignore malformed storage, fall back to default corner */
    }
  }, []);

  // Re-clamp when the viewport shrinks (window resize, dev tools dock, etc.)
  // so the button never drifts off-screen and becomes unreachable.
  useEffect(() => {
    const onResize = () => {
      setFabPosState((prev) => {
        if (!prev) return prev;
        const next = clampFabPos(prev.x, prev.y);
        if (next.x === prev.x && next.y === prev.y) return prev;
        fabPosRef.current = next;
        return next;
      });
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  // Effective FAB styling: until a position is stored or dragged, the button
  // keeps its default bottom-right corner expressed as `right`/`bottom` insets.
  // Those are viewport-independent, which matters: deriving the corner from
  // `window.innerWidth/innerHeight` makes the prerendered HTML (no window, so a
  // placeholder size) disagree with the client's real viewport on the style
  // attribute — a hydration mismatch React warns about and deliberately does not
  // patch up, leaving the FAB in the wrong corner.
  const fabStyle: React.CSSProperties = fabPos
    ? { left: fabPos.x, top: fabPos.y, width: FAB_SIZE, height: FAB_SIZE }
    : { right: FAB_MARGIN, bottom: FAB_MARGIN, width: FAB_SIZE, height: FAB_SIZE };

  const handleFabPointerDown = (e: React.PointerEvent<HTMLButtonElement>) => {
    // Ignore right/middle mouse; let text-selection / context menu still work.
    if (e.pointerType === "mouse" && e.button !== 0) return;
    // Handlers run on the client only, so reading the viewport here is safe; it
    // mirrors the `right`/`bottom` default into `left`/`top` for the drag.
    const cur = fabPosRef.current ?? {
      x: window.innerWidth - FAB_MARGIN - FAB_SIZE,
      y: window.innerHeight - FAB_MARGIN - FAB_SIZE,
    };
    fabDrag.current = {
      active: true,
      startX: e.clientX,
      startY: e.clientY,
      startLeft: cur.x,
      startTop: cur.y,
      moved: 0,
    };
    // Capture so we keep getting moves even if the pointer leaves the button.
    e.currentTarget.setPointerCapture(e.pointerId);
    // Snap the FAB from the default corner to real coords immediately on the
    // first drag frame so subsequent moves track the pointer one-to-one.
    if (!fabPosRef.current) setFabPos(cur);
  };

  const handleFabPointerMove = (e: React.PointerEvent<HTMLButtonElement>) => {
    const d = fabDrag.current;
    if (!d.active) return;
    const dx = e.clientX - d.startX;
    const dy = e.clientY - d.startY;
    d.moved = Math.max(d.moved, Math.abs(dx) + Math.abs(dy));
    setFabPos(clampFabPos(d.startLeft + dx, d.startTop + dy));
  };

  const handleFabPointerUp = (e: React.PointerEvent<HTMLButtonElement>) => {
    const d = fabDrag.current;
    if (!d.active) return;
    d.active = false;
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {
      /* capture may already be released */
    }
    if (d.moved > FAB_DRAG_THRESHOLD && fabPosRef.current) {
      localStorage.setItem(FAB_POS_KEY, JSON.stringify(fabPosRef.current));
    }
  };

  const handleFabClick = () => {
    // Suppress the click that fires right after a drag; only a *tap* (small
    // movement) toggles the dock. Stays visible while open, so it doubles as
    // a close control.
    if (fabDrag.current.moved > FAB_DRAG_THRESHOLD) {
      fabDrag.current.moved = 0;
      return;
    }
    setOpen(!isDockOpenRef.current);
  };

  // Ref mirror of `open` for callbacks registered once (auto-open rules +
  // keyboard toggle below), so they read the current value without re-binding.
  const isDockOpenRef = useRef(false);
  useEffect(() => {
    isDockOpenRef.current = open;
  }, [open]);

  // ---- Keyboard shortcut: Ctrl+Space toggles the dock ----
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.ctrlKey && !e.shiftKey && !e.altKey && e.code === "Space") {
        e.preventDefault();
        setOpen(!isDockOpenRef.current);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [setOpen]);

  // ---- Resize (left edge, mouse-drag; mirrors the Wiki sidebar pattern) ----
  const handleDockDrag = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      const startX = e.clientX;
      const startWidth = dockWidth;
      const onMove = (ev: MouseEvent) =>
        setDockWidth(
          Math.min(MAX_DOCK_WIDTH, Math.max(MIN_DOCK_WIDTH, startWidth + startX - ev.clientX))
        );
      // The live width lives in `dockWidth` (stale here), so re-read it from
      // state at mouseup via the functional updater and persist that value.
      const onUp = () => {
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
        setDockWidth((w) => {
          localStorage.setItem(DOCK_WIDTH_KEY, String(w));
          return w;
        });
      };
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
    },
    [dockWidth]
  );

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

  // The agent produced content while the dock was hidden: surface it once per
  // turn (auto-open), and dot the toggle button otherwise.
  const markAgentOutput = useCallback(() => {
    if (isDockOpenRef.current) return;
    setUnread(true);
    if (!reopenedThisTurn.current) {
      reopenedThisTurn.current = true;
      setOpen(true, false);
    }
  }, [setOpen]);

  // ---- Load settings + connection status ----
  const refreshStatus = useCallback(async () => {
    if (!isAgentHost()) return;
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
    if (!isAgentHost() || connecting) return;
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
    if (!isAgentHost()) return;
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

  // Auto-connect as soon as the app is up and the agent is enabled — the dock
  // is always alive, so the agent can act from any page even if never opened.
  useEffect(() => {
    if (!enabled || status.connected || connectAttempted.current) return;
    connectAttempted.current = true;
    void doConnect();
  }, [enabled, status.connected, doConnect]);

  // Re-arm the auto-connect attempt once a connection drops.
  useEffect(() => {
    if (!status.connected) connectAttempted.current = false;
  }, [status.connected]);

  // ---- Report UI state so app_get_ui_state can answer the agent ----
  // Ungated by dock visibility: the shell's real active tab is what the agent
  // needs to know, wherever the user currently is.
  useEffect(() => {
    if (!isAgentHost()) return;
    invoke("agent_report_ui_state", { activeTab, activeDatasetUuid: null }).catch(() => {});
  }, [activeTab]);

  // ---- Event listeners (registered once) ----
  useEffect(() => {
    if (!isAgentHost()) return;
    const unlisteners: (() => void)[] = [];
    let cancelled = false;
    const reg = <T,>(event: string, handler: (payload: T) => void) => {
      listen<T>(event, (e) => handler(e.payload)).then((fn) => {
        if (cancelled) fn();
        else unlisteners.push(fn);
      });
    };

    reg<AgentStatus>("agent-status", (p) => setStatus(p));
    // Another page (e.g. the Workflow page) hands us a reviewed prompt: load it
    // into the input and bring the dock forward, but let the user do the send.
    reg<{ text: string }>("agent-prefill", (p) => {
      if (p?.text) setInput(p.text);
      setOpen(true, false);
    });
    reg<AgentMessageChunk>("agent-message-chunk", (p) => {
      appendStream("agent", p.text);
      markAgentOutput();
    });
    reg<AgentThought>("agent-thought", (p) => appendStream("thought", p.text));
    reg<AgentToolCall>("agent-tool-call", (p) => {
      markAgentOutput();
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
      });
    });
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
    reg<AgentPlan>("agent-plan", (p) => {
      markAgentOutput();
      setBlocks((prev) => {
        const idx = prev.map((b) => b.kind).lastIndexOf("plan");
        if (idx >= 0) {
          const copy = [...prev];
          copy[idx] = { kind: "plan", entries: p.entries ?? [] };
          return copy;
        }
        return [...prev, { kind: "plan", entries: p.entries ?? [] }];
      });
    });
    // A pending permission blocks the turn — always bring the dock forward.
    reg<AgentPermissionRequest>("agent-permission-request", (p) => {
      setPermission(p);
      setOpen(true, false);
    });
    reg<AgentDone>("agent-done", () => {
      setBusy(false);
      reopenedThisTurn.current = false;
    });
    reg<AgentError>("agent-error", (p) => {
      setBlocks((prev) => [...prev, { kind: "error", text: p.message }]);
      setBusy(false);
      reopenedThisTurn.current = false;
    });

    return () => {
      cancelled = true;
      unlisteners.forEach((fn) => fn());
    };
  }, [appendStream, markAgentOutput, setOpen]);

  // ---- Send prompt ----
  const handleSend = useCallback(async () => {
    const text = input.trim();
    if (!text || busy || !isAgentHost() || !status.connected) return;
    reopenedThisTurn.current = false;
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
    if (!isAgentHost()) return;
    try {
      await invoke("agent_cancel");
    } catch (e) {
      console.error("agent: cancel failed", e);
    }
  }, []);

  const respondPermission = useCallback(
    async (optionId: string | null) => {
      if (!permission || !isAgentHost()) return;
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
  if (mobile) return null;

  return (
    <>
      {/* Floating toggle button — always present, whether the dock is open or
          closed, so it doubles as a close control and stays draggable. Dots
          when the agent is waiting, working, or produced output the user
          hasn't seen. Drag it anywhere on-screen to move it out of the way
          when it would cover content; position persists. z above the dock so
          it never gets buried when parked over the panel. */}
      <button
        type="button"
        onClick={handleFabClick}
        onPointerDown={handleFabPointerDown}
        onPointerMove={handleFabPointerMove}
        onPointerUp={handleFabPointerUp}
        onPointerCancel={handleFabPointerUp}
        title="Agent — drag to move, click to open/close (Ctrl+Space)"
        aria-label="Toggle agent panel"
        className="fixed z-[101] rounded-full bg-accent-bg text-white shadow-lg flex items-center justify-center hover:opacity-90 cursor-grab active:cursor-grabbing select-none touch-none"
        style={fabStyle}
      >
        <Bot size={22} />
        {(unread || busy || permission) && (
          <span className="absolute top-0.5 right-0.5 w-3 h-3 rounded-full bg-yellow-500 border-2 border-bg-card" />
        )}
      </button>

      {/* Dock panel — overlays the content area on the right. */}
      {open && (
        <aside
          className="fixed top-0 right-0 h-full z-[100] flex flex-col bg-bg-card border-l border-border-default shadow-xl"
          style={{ width: dockWidth }}
        >
          {/* Resize handle on the left edge (mouse-drag only; not useful on touch) */}
          <div
            className="absolute top-0 left-0 h-full w-3 cursor-col-resize flex items-center justify-center group z-10 -ml-1.5"
            onMouseDown={handleDockDrag}
          >
            <div className="w-0.5 h-12 rounded-full bg-border-default group-hover:bg-accent transition-colors" />
          </div>

          {/* Header / connection banner */}
          <div className="p-4 border-b border-border-default flex items-center gap-3">
            <h1 className="text-sm font-semibold flex items-center gap-2 truncate">
              <Bot size={18} /> Agent
            </h1>
            <span
              className={`text-[10px] font-medium uppercase tracking-wide px-1.5 py-0.5 rounded border shrink-0 ${
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
                className="inline-flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg bg-bg-muted border border-border-default text-xs font-medium text-text-secondary hover:bg-bg-hover transition-colors"
              >
                <PlugZap size={14} /> Disconnect
              </button>
            ) : (
              <button
                type="button"
                onClick={doConnect}
                disabled={connecting}
                className="inline-flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg bg-accent-bg text-white text-xs font-medium hover:opacity-90 transition-opacity disabled:opacity-50"
              >
                {connecting ? <Loader2 size={14} className="animate-spin" /> : <Plug size={14} />}
                Connect
              </button>
            )}
            <button
              type="button"
              onClick={() => setOpen(false)}
              aria-label="Close agent panel"
              title="Close (Ctrl+Space)"
              className="p-1.5 rounded-md text-text-secondary hover:text-text-primary hover:bg-bg-hover transition-colors"
            >
              <X size={16} />
            </button>
          </div>

          {!enabled && !status.connected && (
            <div className="px-4 py-2 text-xs text-text-tertiary bg-bg-muted border-b border-border-default">
              The Agent is disabled. Enable it and set the goose ACP URL/secret in
              Settings → Agent (Goose ACP), then start <code>goose serve</code>.
            </div>
          )}

          {/* Message list */}
          <div className="flex-1 overflow-y-auto p-4 space-y-4 min-h-0">
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
                className="flex-1 min-w-0 px-3 py-2.5 rounded-lg bg-bg-muted border border-border-default text-text-primary text-sm resize-none focus:outline-none focus:border-accent"
                placeholder={
                  status.connected
                    ? "Message the agent… (Enter to send)"
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
                  className="inline-flex items-center justify-center p-2.5 rounded-lg border border-border-default bg-bg-muted text-text-secondary hover:bg-bg-hover transition-colors"
                  onClick={handleCancel}
                  title="Cancel the current turn"
                >
                  <Square size={18} />
                </button>
              ) : (
                <button
                  type="button"
                  className="inline-flex items-center justify-center p-2.5 rounded-lg border border-transparent bg-accent-bg text-white hover:opacity-90 transition-opacity disabled:opacity-50 disabled:cursor-not-allowed"
                  onClick={handleSend}
                  disabled={!input.trim() || !status.connected || busy}
                >
                  <Send size={18} />
                </button>
              )}
            </div>
          </div>
        </aside>
      )}
    </>
  );
}

// Small helpers so the drag handler stays readable.
function clampFabPos(x: number, y: number) {
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  return {
    x: Math.max(FAB_EDGE_PAD, Math.min(vw - FAB_SIZE - FAB_EDGE_PAD, x)),
    y: Math.max(FAB_EDGE_PAD, Math.min(vh - FAB_SIZE - FAB_EDGE_PAD, y)),
  };
}

// ---------------------------------------------------------------------------
// Block rendering
// ---------------------------------------------------------------------------

function BlockView({ block, streaming }: { block: Block; streaming: boolean }) {
  switch (block.kind) {
    case "user":
      return (
        <div className="flex gap-3 justify-end">
          <div className="max-w-[80%] px-4 py-2.5 rounded-2xl whitespace-pre-wrap break-words bg-accent-bg text-white text-sm">
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
          <div className="max-w-[80%] px-4 py-2.5 rounded-2xl whitespace-pre-wrap break-words bg-bg-muted text-text-primary text-sm">
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
          <details className="max-w-[80%] text-xs text-text-tertiary">
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
          <div className="max-w-[85%] px-3 py-2 rounded-lg bg-bg-card border border-border-default text-xs space-y-1">
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
          <div className="max-w-[85%] px-3 py-2 rounded-lg bg-bg-card border border-border-default text-xs space-y-1">
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
          <div className="max-w-[85%] px-3 py-2 rounded-lg bg-error-bg text-error-text text-xs break-words">
            {block.text}
          </div>
        </div>
      );
  }
}
