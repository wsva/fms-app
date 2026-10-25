"use client";

/**
 * Agent dock — the agent's outbox, reachable from Ctrl+Space or from the floating
 * Bot button.
 *
 * FmS does not dial an agent, so there is no session here to stream. An agent
 * attaches to this app's MCP server and runs in its own window; what it can
 * push back while the user is on some other page is a notification
 * (`app_notify` → `agent-action` notify) and a task (`todo_add`). This panel is
 * where the first one is kept: the shell's toast lasts four seconds, the dock
 * holds the trail, and clicking an entry copies it for pasting into the agent's
 * own window. The task queue keeps its page (Tools → Tasks) — the footer only
 * counts what is waiting so it is not silently missed.
 *
 * The button is opt-in: `agent_fab_visible` is off by default (Agent page →
 * Agent surface), because a FAB sits on top of every page. Only the button is
 * gated — Ctrl+Space always opens the panel, so the trail is never unreachable.
 *
 * Presentation is untouched from when this component spoke ACP: the FAB can be
 * dragged, the panel width is draggable, and both survive a reload.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Bell, Bot, Copy, ListTodo, X } from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import { copyAgentText, ensureAgentLoaded, ensureSurfaceLoaded, useAgentControl } from "@/lib/agent/control";
import type { AgentAction } from "@/lib/agent/types";

// ---------------------------------------------------------------------------
// View model — one row per thing the agent pushed at the user.
// ---------------------------------------------------------------------------

interface Note {
  id: number;
  at: number;
  text: string;
}

/** The subset of `todo::Todo` the footer counts. */
interface TodoRow {
  uuid: string;
  status: string;
}

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

// The trail is a convenience, not a journal: keep the last handful of notes.
const MAX_NOTES = 30;

export default function AgentDock({ onNavigate }: { onNavigate?: (tab: string) => void }) {
  // ---- Dock presentation state ----
  const [mobile, setMobile] = useState(false);
  const [open, setOpenState] = useState(false);
  const [unread, setUnread] = useState(false);
  const [dockWidth, setDockWidth] = useState(DEFAULT_DOCK_WIDTH);

  // ---- The outbox ----
  const [notes, setNotes] = useState<Note[]>([]);
  const [openTodos, setOpenTodos] = useState<number | null>(null);
  const noteSeq = useRef(0);

  // ---- MCP surface state, read from the same store the Agent page uses ----
  const { manual, surface } = useAgentControl();
  const listening = Boolean(manual?.endpoints.running);
  // Missing settings read as "no button": the switch is opt-in, so until the
  // store has the real value the safe default is the one a fresh install gets.
  const fabVisible = surface?.agent_fab_visible === true;

  // ---- Floating toggle button: position + drag state ----
  const [fabPos, setFabPosState] = useState<{ x: number; y: number } | null>(null);
  // Mirror the latest pos so pointerup can persist without re-reading state.
  const fabPosRef = useRef<{ x: number; y: number } | null>(null);
  const setFabPos = (p: { x: number; y: number } | null) => {
    fabPosRef.current = p;
    setFabPosState(p);
  };
  // Live drag session; kept in a ref so pointermove doesn't force a rerender.
  // `moved` gates the click handler.
  const fabDrag = useRef({
    active: false,
    startX: 0,
    startY: 0,
    startLeft: 0,
    startTop: 0,
    moved: 0,
  });

  const isDockOpenRef = useRef(false);

  // setOpen — single funnel for every open/close so the user's explicit choice
  // is persisted; auto-opens pass persist=false.
  const setOpen = useCallback((v: boolean, persist: boolean = true) => {
    setOpenState(v);
    if (v) setUnread(false);
    if (persist) localStorage.setItem(DOCK_OPEN_KEY, v ? "1" : "0");
  }, []);

  useEffect(() => {
    isDockOpenRef.current = open;
  }, [open]);

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
    // The header badge reads the MCP endpoint state, so the manual is needed
    // here too, not only on the Agent page. The surface snapshot carries
    // `agent_fab_visible`, which decides whether the FAB below is drawn at all.
    void ensureAgentLoaded();
    void ensureSurfaceLoaded();
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

  // ---- Inbox: the notify half of `agent-action` ----
  useEffect(() => {
    if (!isTauri()) return;
    const unlisten = listen<AgentAction>("agent-action", (event) => {
      const action = event.payload;
      if (action.type !== "notify") return;
      const text = String(action.message ?? "").trim();
      if (!text) return;
      // Allocate the id outside the updater: a state updater must be pure, and
      // React may run it twice (StrictMode) which would burn ids.
      const id = ++noteSeq.current;
      setNotes((prev) => [{ id, at: Date.now(), text }, ...prev].slice(0, MAX_NOTES));
      // The shell raises a toast for the same event; the dot says the trail is
      // still here after it fades. No auto-open: a notification should not
      // steal the panel over the page the user is working on.
      if (!isDockOpenRef.current) setUnread(true);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // ---- Footer count: open agent tasks ----
  const refreshTodos = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const rows = await invoke<TodoRow[]>("todo_list", { status: "open" });
      setOpenTodos(rows.length);
    } catch {
      // A count is decoration; failing to read it must not break the panel.
      setOpenTodos(null);
    }
  }, []);

  useEffect(() => {
    if (!open) return;
    void refreshTodos();
  }, [open, refreshTodos]);

  if (mobile) return null;

  const goTo = (tab: string) => {
    onNavigate?.(tab);
    setOpen(false);
  };

  return (
    <>
      {fabVisible && (
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
          {unread && (
            <span className="absolute top-0.5 right-0.5 w-3 h-3 rounded-full bg-yellow-500 border-2 border-bg-card" />
          )}
        </button>
      )}

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

          {/* Header. The badge is the MCP surface, not a session: it says whether
              an agent *could* be attached right now. */}
          <div className="p-4 border-b border-border-default flex items-center gap-3">
            <h1 className="text-sm font-semibold flex items-center gap-2 truncate">
              <Bot size={18} /> Agent
            </h1>
            <span
              className={`text-[10px] font-medium uppercase tracking-wide px-1.5 py-0.5 rounded border shrink-0 ${
                listening
                  ? "bg-accent-bg/15 border-accent-bg/30 text-accent-bg"
                  : "bg-bg-muted border-border-default text-text-tertiary"
              }`}
              title={manual?.endpoints.mcp ?? "The MCP server is not answering"}
            >
              {listening ? "listening" : "offline"}
            </span>
            <div className="flex-1" />
            <button
              type="button"
              onClick={() => goTo("agent")}
              className="inline-flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg bg-bg-muted border border-border-default text-xs font-medium text-text-secondary hover:bg-bg-hover transition-colors"
            >
              Setup
            </button>
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

          {/* The one line that keeps the panel from reading as a chat box. */}
          <div className="px-4 py-2 text-xs text-text-tertiary bg-bg-muted border-b border-border-default">
            {listening ? (
              <>
                An agent attaches at <code>{manual?.endpoints.mcp ?? "/mcp"}</code> and runs in its
                own window. What it sends you lands here.
              </>
            ) : (
              <>
                The MCP server is not answering, so no agent can attach.{" "}
                <button type="button" className="underline hover:text-text-secondary" onClick={() => goTo("agent")}>
                  Open Setup
                </button>
                .
              </>
            )}
          </div>

          {/* The trail. */}
          <div className="flex-1 overflow-y-auto p-4 space-y-3 min-h-0">
            {notes.length === 0 ? (
              <div className="flex flex-col items-center justify-center h-full text-center text-text-tertiary">
                <Bell size={48} className="mb-4 opacity-50" />
                <p className="text-sm max-w-xs">
                  Nothing yet. When an agent needs your attention it calls{" "}
                  <code>app_notify</code>, and the note stays here after the toast fades.
                </p>
              </div>
            ) : (
              notes.map((n) => (
                <div
                  key={n.id}
                  className="rounded-lg border border-border-default bg-bg-muted px-3 py-2 space-y-1"
                >
                  <div className="flex items-center gap-2 text-[10px] uppercase tracking-wide text-text-tertiary">
                    <Bell size={11} />
                    <span>{clock(n.at)}</span>
                    <div className="flex-1" />
                    <button
                      type="button"
                      onClick={() => void copyAgentText(`note-${n.id}`, n.text)}
                      title="Copy this note"
                      className="p-1 rounded text-text-tertiary hover:text-text-primary hover:bg-bg-hover transition-colors"
                    >
                      <Copy size={12} />
                    </button>
                  </div>
                  <p className="text-sm text-text-primary whitespace-pre-wrap break-words">{n.text}</p>
                </div>
              ))
            )}
          </div>

          {/* Footer: the task queue keeps its own page; this is only the count,
              so a waiting `human` step is not missed. */}
          <button
            type="button"
            onClick={() => goTo("tasks")}
            className="p-3 border-t border-border-default flex items-center gap-2 text-xs text-text-secondary hover:bg-bg-hover transition-colors"
          >
            <ListTodo size={14} />
            <span>
              {openTodos === null
                ? "Agent tasks"
                : openTodos === 0
                ? "No open agent tasks"
                : `${openTodos} open agent task${openTodos === 1 ? "" : "s"}`}
            </span>
            <div className="flex-1" />
            <span className="text-text-tertiary">Tasks →</span>
          </button>
        </aside>
      )}
    </>
  );
}

// Small helper so the drag handler stays readable.
function clampFabPos(x: number, y: number) {
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  return {
    x: Math.max(FAB_EDGE_PAD, Math.min(vw - FAB_SIZE - FAB_EDGE_PAD, x)),
    y: Math.max(FAB_EDGE_PAD, Math.min(vh - FAB_SIZE - FAB_EDGE_PAD, y)),
  };
}

/** HH:MM in the user's locale. Only ever called after mount (notes arrive from
 *  events), so this cannot desync the prerendered HTML. */
function clock(at: number) {
  return new Date(at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}
