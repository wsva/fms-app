"use client";

/**
 * Agent dock — the mailbox (§6 of `docs/design/agent/mailbox.md`).
 *
 * FmS does not dial an agent, so there is no session here to stream. An agent
 * attaches to this app's MCP server, runs in its own window, and finds work in
 * the mail tray. This panel is the other end of that tray: three lanes over one
 * ledger, so "what is my agent doing" and "what is waiting on me" are answered
 * by the same rows the agent claims from.
 *
 * The lanes are two folders, one of them split: YOUR AGENT OWES is the Outbox,
 * YOU OWE and INBOX are both the Inbox, split by `owed_by`. That is why the
 * header counts and the lane lengths cannot disagree — both are the backend's
 * fold (§4.2), never a rule re-implemented here.
 *
 * The shell is unchanged from when this was a notification trail: draggable
 * opt-in FAB (drag to move, tap to open), draggable width, Ctrl+Space, desktop
 * only. The dock's own transitions — take over, cancel, answer — are Tauri
 * commands published as UI actions, so an agent acting *as the user* reaches
 * them through `ui_invoke_action` like any other button.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Bot,
  Copy,
  CornerDownRight,
  Inbox,
  ListTodo,
  RefreshCw,
  Undo2,
  X,
} from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import {
  copyAgentText,
  ensureAgentLoaded,
  ensureSurfaceLoaded,
  useAgentControl,
} from "@/lib/agent/control";
import type { AgentAction } from "@/lib/agent/types";
import {
  dispatchDockState,
  DOCK_TOGGLE_EVENT,
} from "@/lib/agent/dock-events";
import { reqStr, useAgentActions } from "@/lib/agent/ui-actions";
import { pageZoomFactor, ZOOM_CHANGED_EVENT } from "@/hooks/usePageZoom";
import {
  badgeOf,
  channelLine,
  mailBrief,
  mailCancel,
  mailHead,
  mailList,
  mailLook,
  mailOpen,
  mailReply,
  mailTakeOver,
  promptOf,
  tabForRow,
  type MailHead,
  type MailSummary,
  type MailTone,
} from "@/lib/agent/mail";

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
const MIN_DOCK_WIDTH = 320;
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

// The mailbox lives in the app database, so polling it is cheap — but the
// panel is not the place to spend a core on. The badge polls slower than the
// open list because it only needs the counts.
const OPEN_POLL_MS = 8000;
const BADGE_POLL_MS = 30000;

const TONE: Record<MailTone, string> = {
  neutral: "text-text-tertiary",
  accent: "text-accent-bg",
  ok: "text-green-500",
  warn: "text-yellow-500",
  bad: "text-red-400",
};

const GHOST =
  "inline-flex items-center gap-1 rounded border border-border-default bg-bg-muted px-1.5 py-0.5 " +
  "text-[10px] text-text-secondary hover:bg-bg-hover hover:text-text-primary transition-colors " +
  "disabled:opacity-50 disabled:cursor-default";

type LaneId = "you" | "agent" | "inbox";

export default function AgentDock({ onNavigate }: { onNavigate?: (tab: string) => void }) {
  // ---- Dock presentation state ----
  const [mobile, setMobile] = useState(false);
  const [open, setOpenState] = useState(false);
  const [unread, setUnread] = useState(false);
  const [dockWidth, setDockWidth] = useState(DEFAULT_DOCK_WIDTH);

  // ---- The mailbox ----
  const [head, setHead] = useState<MailHead | null>(null);
  const [lanes, setLanes] = useState<{ you: MailSummary[]; agent: MailSummary[]; inbox: MailSummary[] }>({
    you: [],
    agent: [],
    inbox: [],
  });
  // Ages are rendered from one stamped instant, refreshed with the poll, so a
  // row list does not re-read the clock for every row.
  const [now, setNow] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [armed, setArmed] = useState<string | null>(null);
  const [replyTo, setReplyTo] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [openTodos, setOpenTodos] = useState<number | null>(null);

  // ---- MCP surface state, read from the same store the Agent page uses ----
  const { manual, surface, copied } = useAgentControl();
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
  const setOpen = useCallback((v: boolean) => {
    setOpenState(v);
    if (v) setUnread(false);
    localStorage.setItem(DOCK_OPEN_KEY, v ? "1" : "0");
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
    // to the default bottom-right corner at render time (see `fabStyle`).
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
  // or the page zoom changes. Zoom needs its own event: CSS `zoom` leaves
  // `innerWidth/Height` at the window size while a `left/top`-placed element
  // renders at `left*zoom`, so a position that was in view at 100% is off the
  // edge at 130% and no `resize` fires to tell us.
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
    window.addEventListener(ZOOM_CHANGED_EVENT, onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      window.removeEventListener(ZOOM_CHANGED_EVENT, onResize);
    };
  }, []);

  // Effective FAB styling: until a position is stored or dragged, the button
  // keeps its default bottom-right corner expressed as `right`/`bottom` insets.
  // Those are viewport-independent, which matters: deriving the corner from
  // `window.innerWidth/innerHeight` makes the prerendered HTML (no window, so a
  // placeholder size) disagree with the client's real viewport on the style
  // attribute — a hydration mismatch React warns about and deliberately does not
  // patch up, leaving the FAB in the wrong corner. The bar's height therefore
  // goes in as a `calc()` over the shell's own var, not as a JS-read number.
  const fabStyle: React.CSSProperties = fabPos
    ? { left: fabPos.x, top: fabPos.y, width: FAB_SIZE, height: FAB_SIZE }
    : {
        right: FAB_MARGIN,
        bottom: `calc(var(--shell-statusbar-h) + ${FAB_MARGIN}px)`,
        width: FAB_SIZE,
        height: FAB_SIZE,
      };

  const handleFabPointerDown = (e: React.PointerEvent<HTMLButtonElement>) => {
    // Ignore right/middle mouse; let text-selection / context menu still work.
    if (e.pointerType === "mouse" && e.button !== 0) return;
    // Handlers run on the client only, so reading the viewport here is safe; it
    // mirrors the `right`/`bottom` default into `left`/`top` for the drag. The
    // viewport must be read in layout units (`/ zoom`): `left` is multiplied by
    // the page zoom on screen, so the window-px corner would land off it. The
    // status bar is subtracted because the default corner sits above it.
    const z = pageZoomFactor();
    const cur = fabPosRef.current ?? {
      x: window.innerWidth / z - FAB_MARGIN - FAB_SIZE,
      y: window.innerHeight / z - statusBarInset() - FAB_MARGIN - FAB_SIZE,
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
    // Pointer deltas are window px; the element's left/top are layout px.
    // Dividing by the zoom factor keeps the button pinned to the cursor.
    const z = pageZoomFactor();
    const dx = (e.clientX - d.startX) / z;
    const dy = (e.clientY - d.startY) / z;
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

  // ---- Status bar button: the shell's permanent entry point ----
  // Same toggle as the shortcut and the FAB, so the three never disagree about
  // what "open" means.
  useEffect(() => {
    const onToggle = () => setOpen(!isDockOpenRef.current);
    window.addEventListener(DOCK_TOGGLE_EVENT, onToggle);
    return () => window.removeEventListener(DOCK_TOGGLE_EVENT, onToggle);
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

  // ---- Reading the ledger ----

  /** Counts only: what the FAB badge needs, and all it needs. */
  const refreshBadge = useCallback(async () => {
    if (!isTauri()) return;
    try {
      setHead(await mailHead());
    } catch {
      // A count is decoration; a failed read must not break the panel.
    }
  }, []);

  /** The three lanes, folded by the backend rather than by this file. */
  const refresh = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const [nextHead, owedYou, owedAgent, folderInbox] = await Promise.all([
        mailHead(),
        mailList({ owed_by: "you" }),
        mailList({ owed_by: "agent" }),
        mailList({ folder: "inbox" }),
      ]);
      setHead(nextHead);
      setLanes({
        you: owedYou,
        agent: owedAgent,
        // The Inbox lane is the *rest* of the folder: a `waiting_user` request
        // sits in the same folder and is already under YOU OWE (§6).
        inbox: folderInbox.filter((row) => row.owed_by === "nobody"),
      });
      setNow(Date.now());
      setError(null);
    } catch (e) {
      setError(reason(e));
    }
  }, []);

  // ---- The user's transitions, in one place ----
  //
  // These throw rather than swallow: the same functions back the UI actions, so
  // an agent invoking them gets the failure verbatim. A button click attaches a
  // `.catch` that puts it on the panel's error strip.

  const goTo = useCallback(
    (tab: string) => {
      onNavigate?.(tab);
      setOpen(false);
    },
    [onNavigate, setOpen]
  );

  const copyBrief = useCallback(
    async (limit?: number) => {
      const brief = await mailBrief(limit);
      await copyAgentText("mail.brief", brief.text);
      void refreshBadge();
      return brief;
    },
    [refreshBadge]
  );

  const copyOne = useCallback(
    async (uuid: string) => {
      const row = await mailOpen(uuid);
      await copyAgentText(`mail.${uuid}`, promptOf(row));
      await refresh();
      return { mail_uuid: uuid, subject: row.subject };
    },
    [refresh]
  );

  const openItem = useCallback(
    async (uuid: string) => {
      const row = await mailOpen(uuid);
      await refresh();
      const tab = tabForRow(row);
      if (tab) goTo(tab);
      return { mail_uuid: uuid, subject: row.subject, tab };
    },
    [goTo, refresh]
  );

  /** T14 — end a **live** hold. Not a claim: nothing here has expired. */
  const takeOverClaim = useCallback(
    async (uuid: string) => {
      const row = await mailOpen(uuid);
      if (!row.claim_id) {
        throw new Error(
          "nobody is holding this item any more, so there is no claim to end — it is " +
            "already free. An agent picks it up with `mail_claim {next: true}`."
        );
      }
      await mailTakeOver(uuid, row.claim_id);
      await refresh();
      return { mail_uuid: uuid, released: row.claimed_by ?? "the previous holder" };
    },
    [refresh]
  );

  /** T15 — cancel an open request. A delivery for it is refused afterwards. */
  const cancelRequest = useCallback(
    async (uuid: string) => {
      await mailCancel(uuid);
      await refresh();
      return { mail_uuid: uuid, cancelled: true };
    },
    [refresh]
  );

  /** T10 — answer the question a request is blocked on; it returns to the tray. */
  const answerQuestion = useCallback(
    async (uuid: string, answer: string) => {
      await mailReply(uuid, answer);
      await refresh();
      return { mail_uuid: uuid, answered: true };
    },
    [refresh]
  );

  /** Run a row action with the busy marker and the panel's error strip. */
  const runRow = useCallback((uuid: string, work: () => Promise<unknown>) => {
    setBusy(uuid);
    setError(null);
    void work()
      .catch((e) => setError(reason(e)))
      .finally(() => setBusy(null));
  }, []);

  // ---- Publishing the dock's buttons to an agent ----
  //
  // The registry is how the agent-friendly rule is kept without a seventh
  // mailbox tool: an agent that wants to act *as the user* drives the same
  // buttons the person clicks. `cancel` is declared destructive and therefore
  // refused by `ui_invoke_action` — a cancelled request has no undo, and an
  // agent can end its own claim with `mail_fail` instead.
  useAgentActions("shell", [
    {
      id: "shell.mail-copy-all",
      label: "Copy every prompt your agent owes, as one brief, to the clipboard",
      kind: "read",
      params_schema: {
        limit: { type: "number", description: "How many items to include. Default 50." },
      },
      run: (params) => copyBrief(typeof params.limit === "number" ? params.limit : undefined),
    },
    {
      id: "shell.mail-copy",
      label: "Open one item and copy its prompt to the clipboard (marks it read)",
      kind: "mutate",
      params_schema: {
        mail_uuid: { type: "string", required: true, description: "The mail's uuid." },
      },
      run: (params) => copyOne(reqStr("shell.mail-copy", params, "mail_uuid")),
    },
    {
      id: "shell.mail-open",
      label: "Open one item, mark it read, and go to the page it is about",
      kind: "mutate",
      params_schema: {
        mail_uuid: { type: "string", required: true, description: "The mail's uuid." },
      },
      run: (params) => openItem(reqStr("shell.mail-open", params, "mail_uuid")),
    },
    {
      id: "shell.mail-take-over",
      label: "Release a live claim so the item can be claimed again, refunding the attempt",
      kind: "mutate",
      params_schema: {
        mail_uuid: { type: "string", required: true, description: "The mail's uuid." },
      },
      run: (params) => takeOverClaim(reqStr("shell.mail-take-over", params, "mail_uuid")),
    },
    {
      id: "shell.mail-answer",
      label: "Answer the question a request is waiting on, and return it to the tray",
      kind: "mutate",
      params_schema: {
        mail_uuid: { type: "string", required: true, description: "The waiting request's uuid." },
        answer: { type: "string", required: true, description: "The decision, in words." },
      },
      run: (params) =>
        answerQuestion(
          reqStr("shell.mail-answer", params, "mail_uuid"),
          reqStr("shell.mail-answer", params, "answer")
        ),
    },
    {
      id: "shell.mail-cancel",
      label:
        "Cancel an open request; a later delivery for it is refused and nothing is written. No undo.",
      kind: "destructive",
      params_schema: {
        mail_uuid: { type: "string", required: true, description: "The request's uuid." },
      },
      run: (params) => cancelRequest(reqStr("shell.mail-cancel", params, "mail_uuid")),
    },
  ]);

  // ---- Polling: the badge always, the lanes while the panel is open ----
  useEffect(() => {
    void refreshBadge();
  }, [refreshBadge]);

  useEffect(() => {
    if (!open) return;
    void refresh();
    const id = setInterval(() => void refresh(), OPEN_POLL_MS);
    return () => clearInterval(id);
  }, [open, refresh]);

  useEffect(() => {
    const id = setInterval(() => void refreshBadge(), BADGE_POLL_MS);
    return () => clearInterval(id);
  }, [refreshBadge]);

  // ---- `agent-action`: now a hint that the ledger changed ----
  //
  // `app_notify` writes a row, so the trail is the ledger itself; this handler
  // only makes the panel notice without waiting for the poll.
  useEffect(() => {
    if (!isTauri()) return;
    const unlisten = listen<AgentAction>("agent-action", (event) => {
      if (event.payload.type !== "notify") return;
      if (isDockOpenRef.current) {
        void refresh();
      } else {
        setUnread(true);
        void refreshBadge();
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [refresh, refreshBadge]);

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

  // ---- Publish presentation state for the status bar button ----
  // It mirrors what the FAB used to show — open/closed plus the same attention
  // marker — so relocating the entry point costs the user no information. Runs
  // on mount too, which is how a listener that mounted first converges.
  useEffect(() => {
    if (mobile) return;
    dispatchDockState({ open, unread, badge: head ? badgeOf(head) : null });
  }, [mobile, open, unread, head]);

  if (mobile) return null;

  const badge = head ? badgeOf(head) : null;
  const copyable = lanes.agent.filter((row) => row.status === "queued").length;
  const stamp = now || Date.now();

  const rowProps = (lane: LaneId) => (row: MailSummary) => ({
    row,
    lane,
    now: stamp,
    busy: busy === row.uuid,
    armed: armed === row.uuid,
    replyOpen: replyTo === row.uuid,
    draft,
    onOpen: () => runRow(row.uuid, () => openItem(row.uuid)),
    onCopy: () => runRow(row.uuid, () => copyOne(row.uuid)),
    onTakeOver: () => runRow(row.uuid, () => takeOverClaim(row.uuid)),
    onCancel: () => {
      // A cancelled request is terminal and has no undo, so the first click
      // only arms the button.
      if (armed !== row.uuid) {
        setArmed(row.uuid);
        return;
      }
      setArmed(null);
      runRow(row.uuid, () => cancelRequest(row.uuid));
    },
    onReply: () => {
      if (replyTo === row.uuid) {
        setReplyTo(null);
        setDraft("");
      } else {
        setReplyTo(row.uuid);
        setDraft("");
      }
    },
    onDraft: setDraft,
    onSubmit: () => {
      const answer = draft.trim();
      if (!answer) return;
      setBusy(row.uuid);
      setError(null);
      void answerQuestion(row.uuid, answer)
        .then(() => {
          setReplyTo(null);
          setDraft("");
        })
        .catch((e) => setError(reason(e)))
        .finally(() => setBusy(null));
    },
  });

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
          title="Mailbox — drag to move, click to open/close (Ctrl+Space)"
          aria-label="Toggle the mailbox"
          className="fixed z-[101] rounded-full bg-accent-bg text-white shadow-lg flex items-center justify-center hover:opacity-90 cursor-grab active:cursor-grabbing select-none touch-none"
          style={fabStyle}
        >
          <Bot size={22} />
          {badge ? (
            <span className="absolute -top-1 -right-1 min-w-[18px] h-[18px] px-1 rounded-full bg-yellow-500 text-[10px] font-semibold leading-[18px] text-black text-center">
              {badge}
            </span>
          ) : unread ? (
            <span className="absolute top-0.5 right-0.5 w-3 h-3 rounded-full bg-yellow-500 border-2 border-bg-card" />
          ) : null}
        </button>
      )}

      {/* Dock panel — overlays the content area on the right and stops at the
          status bar's top edge, so the bar stays fully readable and clickable
          while the mailbox is open. No `shadow-*`: a box shadow falls mainly
          downward, so it painted a smudge across the status bar — the left
          border alone separates the panel from the page. */}
      {open && (
        <aside
          className="fixed top-0 right-0 bottom-[var(--shell-statusbar-h)] z-[100] flex flex-col bg-bg-card border-l border-border-default"
          style={{ width: dockWidth }}
        >
          {/* Resize handle on the left edge (mouse-drag only; not useful on touch) */}
          <div
            className="absolute top-0 left-0 h-full w-3 cursor-col-resize flex items-center justify-center group z-10 -ml-1.5"
            onMouseDown={handleDockDrag}
          >
            <div className="w-0.5 h-12 rounded-full bg-border-default group-hover:bg-accent transition-colors" />
          </div>

          {/* Header: the title, the counts, whether an agent could attach right
              now, and the two exits. */}
          <div className="px-4 py-3 border-b border-border-default flex items-center gap-2">
            <h1 className="text-xs font-semibold uppercase tracking-[0.18em] flex items-center gap-2 shrink-0">
              <Inbox size={16} /> Mailbox
            </h1>
            <span
              className="text-[10px] text-text-tertiary tabular-nums truncate"
              title="held: your agent is working on it · waiting: queued or lapsed, nobody holds it"
            >
              {head ? `${head.held} held · ${head.waiting} waiting` : "reading the ledger…"}
            </span>
            <div className="flex-1" />
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
            <button
              type="button"
              onClick={() => goTo("agent")}
              className="px-2 py-1 rounded-md bg-bg-muted border border-border-default text-[11px] font-medium text-text-secondary hover:bg-bg-hover transition-colors"
            >
              Setup
            </button>
            <button
              type="button"
              onClick={() => setOpen(false)}
              aria-label="Close the mailbox"
              title="Close (Ctrl+Space)"
              className="p-1.5 rounded-md text-text-secondary hover:text-text-primary hover:bg-bg-hover transition-colors"
            >
              <X size={16} />
            </button>
          </div>

          {/* The one line that keeps the panel from reading as a chat box. */}
          <div className="px-4 py-1.5 text-[11px] text-text-tertiary bg-bg-muted border-b border-border-default">
            {listening ? (
              <>
                An agent attaches at <code>{manual?.endpoints.mcp ?? "/mcp"}</code> and runs in its
                own window. Copy a prompt into it, or let it claim straight from the tray.
              </>
            ) : (
              <>
                No agent can attach — the MCP server is not answering.{" "}
                <button
                  type="button"
                  className="underline hover:text-text-secondary"
                  onClick={() => goTo("agent")}
                >
                  Open Setup
                </button>
                .
              </>
            )}
          </div>

          {error && (
            <div className="px-3 py-2 text-[11px] text-red-400 bg-red-500/10 border-b border-border-default flex items-start gap-2">
              <span className="flex-1 break-words">{error}</span>
              <button
                type="button"
                onClick={() => setError(null)}
                aria-label="Dismiss"
                className="p-0.5 rounded hover:bg-bg-hover"
              >
                <X size={12} />
              </button>
            </div>
          )}

          {/* The lanes. Two folders, one of them split by `owed_by` (§6). */}
          <div className="flex-1 overflow-y-auto min-h-0 divide-y divide-border-default">
            <Lane
              title="You owe"
              hint="Requests blocked on a decision or an unread failure."
              rows={lanes.you}
              empty="Nothing is waiting on you."
              render={rowProps("you")}
            />
            <Lane
              title="Your agent owes"
              hint="The Outbox: what an attached agent can claim."
              rows={lanes.agent}
              empty="Your agent has nothing queued."
              render={rowProps("agent")}
            />
            <Lane
              title="Inbox"
              hint="Deliveries and notices. Opening one marks it read."
              rows={lanes.inbox}
              empty="No unread mail."
              render={rowProps("inbox")}
            />
          </div>

          {/* Footer: one manual action per session, not per item (§6). */}
          <div className="border-t border-border-default">
            {openTodos ? (
              <button
                type="button"
                onClick={() => goTo("tasks")}
                className="w-full px-3 pt-2 pb-0 flex items-center gap-2 text-[11px] text-text-tertiary hover:text-text-secondary transition-colors"
              >
                <ListTodo size={12} />
                <span>
                  {openTodos} open agent task{openTodos === 1 ? "" : "s"}
                </span>
                <div className="flex-1" />
                <span>Tasks →</span>
              </button>
            ) : null}
            <div className="p-3 flex items-center gap-2">
              <button
                type="button"
                onClick={() => runRow("", () => copyBrief())}
                disabled={copyable === 0}
                title={
                  copyable === 0
                    ? "Nothing is waiting to be handed over"
                    : "Copy every queued prompt as one brief"
                }
                className="inline-flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg bg-bg-muted border border-border-default text-xs font-medium text-text-secondary hover:bg-bg-hover transition-colors disabled:opacity-50"
              >
                <Copy size={13} />
                {copied === "mail.brief" ? "Copied" : `Copy all${copyable ? ` ${copyable}` : ""}`}
              </button>
              <div className="flex-1" />
              <button
                type="button"
                onClick={() => void refresh()}
                title="Reload the mailbox"
                className="inline-flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg bg-bg-muted border border-border-default text-xs font-medium text-text-secondary hover:bg-bg-hover transition-colors"
              >
                <RefreshCw size={13} /> Receive
              </button>
            </div>
          </div>
        </aside>
      )}
    </>
  );
}

// ---------------------------------------------------------------------------
// One lane, one row
// ---------------------------------------------------------------------------

function Lane({
  title,
  hint,
  rows,
  empty,
  render,
}: {
  title: string;
  hint: string;
  rows: MailSummary[];
  empty: string;
  render: (row: MailSummary) => React.ComponentProps<typeof MailItem>;
}) {
  return (
    <section className="px-3 py-3">
      <div className="flex items-baseline gap-2 px-1 mb-1.5" title={hint}>
        <h2 className="text-[10px] font-semibold uppercase tracking-wider text-text-tertiary">
          {title}
        </h2>
        {rows.length > 0 && (
          <span className="text-[10px] text-text-tertiary tabular-nums">{rows.length}</span>
        )}
      </div>
      {rows.length === 0 ? (
        <p className="px-1 text-[11px] text-text-tertiary">{empty}</p>
      ) : (
        <ul className="space-y-1.5">
          {rows.map((row) => (
            <MailItem key={row.uuid} {...render(row)} />
          ))}
        </ul>
      )}
    </section>
  );
}

function MailItem({
  row,
  lane,
  now,
  busy,
  armed,
  replyOpen,
  draft,
  onOpen,
  onCopy,
  onTakeOver,
  onCancel,
  onReply,
  onDraft,
  onSubmit,
}: {
  row: MailSummary;
  lane: LaneId;
  now: number;
  busy: boolean;
  armed: boolean;
  replyOpen: boolean;
  draft: string;
  onOpen: () => void;
  onCopy: () => void;
  onTakeOver: () => void;
  onCancel: () => void;
  onReply: () => void;
  onDraft: (value: string) => void;
  onSubmit: () => void;
}) {
  const look = mailLook(row, now);
  const held = row.status === "claimed" || row.status === "delivering";
  const copyable = lane === "agent" && row.status === "queued";
  const answerable = lane === "you" && row.kind === "question" && row.owed_by === "you";
  const hasActions = copyable || held || answerable;

  return (
    <li className="rounded-lg border border-border-default bg-bg-muted/60 px-2.5 py-2">
      <button
        type="button"
        onClick={onOpen}
        disabled={busy}
        className="w-full text-left flex items-start gap-2 min-w-0 disabled:opacity-60"
        title="Open: mark it read and go to what it is about"
      >
        <span className={`text-sm leading-5 shrink-0 ${TONE[look.tone]}`} aria-hidden>
          {look.glyph}
        </span>
        <span className="min-w-0 flex-1">
          <span className="flex items-baseline gap-2">
            <span className="text-[13px] text-text-primary truncate">{row.subject}</span>
            <span className="ml-auto shrink-0 text-[10px] tabular-nums text-text-tertiary">
              {look.text}
            </span>
          </span>
          <span className="block text-[10px] text-text-tertiary truncate">
            # {channelLine(row)}
          </span>
        </span>
      </button>

      {hasActions && (
        <div className="mt-1.5 pl-6 flex flex-wrap items-center gap-1">
          {copyable && (
            <button type="button" onClick={onCopy} disabled={busy} className={GHOST}>
              <Copy size={10} /> Copy
            </button>
          )}
          {copyable && (
            <button
              type="button"
              onClick={onCancel}
              disabled={busy}
              aria-label="Cancel this request"
              title="Cancel this request — a later delivery for it is refused"
              className={`${GHOST} ${armed ? "border-red-500/50 text-red-400" : ""}`}
            >
              {armed ? "Cancel?" : <X size={10} />}
            </button>
          )}
          {held && (
            <button
              type="button"
              onClick={onTakeOver}
              disabled={busy}
              title="End this hold so the item can be claimed again (the attempt is refunded)"
              className={GHOST}
            >
              <Undo2 size={10} /> Take over
            </button>
          )}
          {answerable && (
            <button type="button" onClick={onReply} disabled={busy} className={GHOST}>
              <CornerDownRight size={10} /> Reply
            </button>
          )}
        </div>
      )}

      {replyOpen && (
        <div className="mt-1.5 pl-6 space-y-1">
          <textarea
            autoFocus
            rows={2}
            value={draft}
            onChange={(e) => onDraft(e.target.value)}
            placeholder="The decision the agent asked for…"
            className="w-full resize-none rounded-md bg-bg-card border border-border-default px-2 py-1 text-xs text-text-primary focus:outline-none focus:border-accent-bg"
          />
          <div className="flex items-center justify-end gap-1">
            <button type="button" onClick={onReply} className={GHOST}>
              Close
            </button>
            <button
              type="button"
              onClick={onSubmit}
              disabled={busy || draft.trim() === ""}
              className="inline-flex items-center gap-1 rounded border border-accent-bg/40 bg-accent-bg/15 px-2 py-0.5 text-[10px] text-accent-bg hover:bg-accent-bg/25 transition-colors disabled:opacity-50"
            >
              Answer
            </button>
          </div>
        </div>
      )}
    </li>
  );
}

// Small helper so the drag handler stays readable. Bounds are in layout px —
// the space `left`/`top` live in — which is the window divided by the page
// zoom factor; clamping against raw `innerWidth` let `left * zoom` overshoot
// the viewport and threw the button off-screen at zoom levels above 100%.
// The bottom edge is the viewport minus the status bar, which the button must
// not cover.
function clampFabPos(x: number, y: number) {
  const z = pageZoomFactor();
  const vw = window.innerWidth / z;
  const vh = window.innerHeight / z - statusBarInset();
  return {
    x: Math.max(FAB_EDGE_PAD, Math.min(vw - FAB_SIZE - FAB_EDGE_PAD, x)),
    y: Math.max(FAB_EDGE_PAD, Math.min(vh - FAB_SIZE - FAB_EDGE_PAD, y)),
  };
}

// The status bar's height, read from the var the shell declares in globals.css
// rather than repeated here. A client-only call: it runs from a pointer handler
// or the clamp on restore, never during the prerender.
function statusBarInset(): number {
  const px = parseFloat(
    getComputedStyle(document.documentElement).getPropertyValue("--shell-statusbar-h")
  );
  return Number.isFinite(px) && px > 0 ? px : 0;
}

function reason(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
