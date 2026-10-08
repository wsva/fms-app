"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  Check,
  CloudOff,
  Download,
  FileText,
  FolderOpen,
  Loader2,
  Paperclip,
  RefreshCw,
  Send,
  X,
} from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import { logError, logInfo } from "@/lib/logger";

// ── Types (mirror src-tauri/src/chat.rs) ────────────────────────────────────

interface ChatAttachment {
  uuid: string;
  filename: string;
  mime: string;
  size: number;
}

interface ChatMessage {
  uuid: string;
  sender_device: string;
  sender_name: string;
  text: string;
  created_at: string;
  attachments: ChatAttachment[];
  /** Hub-side rowid cursor; 0 while the message is still only in the outbox. */
  id?: number;
  /** Phone mirror: queued offline, not yet acknowledged by the hub (§4). */
  pending?: boolean;
}

interface ChatListResponse {
  messages: ChatMessage[];
  /** `"pc"` on the desktop, this device's paired id on the phone. */
  self_device: string;
}

/** A message whose send failed; kept so the user can retry with the same
 *  client uuid (the store is idempotent on it, so no duplicates). */
interface FailedSend {
  uuid: string;
  text: string;
  files: { path: string; name: string }[];
  error: string;
}

/** Message still travelling: shown as an outbox row until the PC answers. */
interface OutboxSend {
  uuid: string;
  text: string;
  files: { path: string; name: string }[];
}

interface GlobalSettings {
  pc_url?: string;
}

/** Progress of an attachment being copied (desktop) or downloaded (phone),
 *  keyed by attachment uuid. Mirrors `SaveProgress` in chat.rs. */
interface SaveState {
  pct: number;
  done?: boolean;
  error?: string;
}

interface SaveProgressEvent {
  uuid: string;
  received: number;
  total: number;
  done: boolean;
}

// ── Helpers ─────────────────────────────────────────────────────────────────

const POLL_INTERVAL_MS = 4000;

function humanSize(bytes: number): string {
  if (!bytes || bytes < 0) return "";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function isImage(att: ChatAttachment): boolean {
  return att.mime.startsWith("image/");
}

/** Human label for a picked attachment. Android SAF hands back a `content://`
 *  URI whose tail is percent-encoded (`…/primary%3ADCIM%2FCamera%2FIMG.jpg`), so
 *  decode it and keep the last path segment; real filesystem paths pass through. */
function attachmentDisplayName(p: string): string {
  if (p.startsWith("content://") || p.startsWith("file://")) {
    try {
      const last = decodeURIComponent(p).split(/[\\/]/).pop();
      if (last) return last;
    } catch {
      /* malformed escapes — fall through to the raw basename */
    }
  }
  return p.split(/[\\/]/).pop() || p;
}

function formatTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function dayKey(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString([], { year: "numeric", month: "long", day: "numeric" });
}

/** Resolve an attachment uuid to a locally openable path and memoize it, so
 *  re-renders never re-download a phone-side attachment. */
const pathCache = new Map<string, string>();

async function resolveAttachmentPath(att: ChatAttachment): Promise<string> {
  const cached = pathCache.get(att.uuid);
  if (cached) return cached;
  const path = await invoke<string>("chat_resolve_attachment", {
    uuid: att.uuid,
    filename: att.filename,
  });
  pathCache.set(att.uuid, path);
  return path;
}

// ── Attachment renderers ────────────────────────────────────────────────────

function ImageAttachment({ att, mine }: { att: ChatAttachment; mine: boolean }) {
  const [src, setSrc] = useState<string>("");
  const [error, setError] = useState<string>("");

  useEffect(() => {
    let cancelled = false;
    resolveAttachmentPath(att)
      .then((p) => !cancelled && setSrc(convertFileSrc(p)))
      .catch((e) => !cancelled && setError(String(e)));
    return () => {
      cancelled = true;
    };
  }, [att.uuid, att.filename]);

  if (error) {
    return (
      <div className="px-3 py-2 rounded-lg bg-mid-gray/10 text-xs text-red-400 break-all">
        Image unavailable: {error}
      </div>
    );
  }
  if (!src) {
    return (
      <div className="flex items-center gap-2 px-3 py-3 rounded-lg bg-mid-gray/10 text-xs text-text-tertiary">
        <Loader2 size={14} className="animate-spin" /> loading image…
      </div>
    );
  }
  return (
    <img
      src={src}
      alt={att.filename}
      loading="lazy"
      className={`max-w-[240px] max-h-[240px] rounded-lg object-contain cursor-zoom-in ${
        mine ? "ml-auto" : ""
      }`}
      onClick={() => resolveAttachmentPath(att).then((p) => openPath(p)).catch(() => {})}
      title={att.filename}
    />
  );
}

function FileChip({ att }: { att: ChatAttachment }) {
  const [busy, setBusy] = useState(false);

  const openAttachment = useCallback(async () => {
    setBusy(true);
    try {
      const path = await resolveAttachmentPath(att);
      await openPath(path);
    } catch (e) {
      logError(`chat: cannot open attachment ${att.filename}: ${String(e)}`, "chat");
    } finally {
      setBusy(false);
    }
  }, [att]);

  return (
    <button
      onClick={openAttachment}
      disabled={busy}
      className="inline-flex items-center gap-1.5 min-w-0 max-w-full px-2.5 py-1.5 rounded-lg bg-bg-body border border-border-light hover:bg-bg-hover text-xs text-text-primary cursor-pointer"
      title={`Open ${att.filename}`}
    >
      {busy ? <Loader2 size={13} className="animate-spin shrink-0" /> : <FileText size={13} className="shrink-0" />}
      <span className="truncate">{att.filename}</span>
      {att.size > 0 && <span className="text-text-tertiary shrink-0">{humanSize(att.size)}</span>}
    </button>
  );
}

/**
 * An attachment with its action row below it: a save/download button that
 * turns into a percentage readout while the file is travelling, plus (desktop
 * only) a reveal-in-folder shortcut.
 */
function AttachmentBlock({
  att,
  mine,
  desktop,
  saveState,
  onSave,
  onReveal,
}: {
  att: ChatAttachment;
  mine: boolean;
  desktop: boolean;
  saveState?: SaveState;
  onSave: (att: ChatAttachment) => void;
  onReveal: (att: ChatAttachment) => void;
}) {
  const busy = !!saveState && !saveState.done && !saveState.error;
  const label = saveState?.error
    ? "failed"
    : saveState?.done
      ? "saved"
      : busy
        ? `${saveState?.pct ?? 0}%`
        : null;
  return (
    <div className={`flex flex-col gap-1 ${mine ? "items-end" : "items-start"}`}>
      {isImage(att) ? <ImageAttachment att={att} mine={mine} /> : <FileChip att={att} />}
      <div className="flex items-center gap-0.5">
        <button
          onClick={() => onSave(att)}
          disabled={busy}
          className="inline-flex items-center gap-1 px-1.5 py-1 rounded-md text-[11px] text-text-tertiary hover:text-text-primary hover:bg-bg-hover cursor-pointer disabled:opacity-70 disabled:cursor-default"
          title={saveState?.error ?? (desktop ? "Save as…" : "Save to this device")}
        >
          {saveState?.error ? (
            <CloudOff size={13} className="shrink-0 text-red-400" />
          ) : saveState?.done ? (
            <Check size={13} className="shrink-0 text-green-500" />
          ) : busy ? (
            <Loader2 size={13} className="shrink-0 animate-spin" />
          ) : (
            <Download size={13} className="shrink-0" />
          )}
          <span className="tabular-nums">{label}</span>
        </button>
        {desktop && (
          <button
            onClick={() => onReveal(att)}
            className="p-1.5 rounded text-text-tertiary hover:text-text-primary hover:bg-bg-hover cursor-pointer"
            title="Reveal in folder"
          >
            <FolderOpen size={13} />
          </button>
        )}
      </div>
    </div>
  );
}

// ── Page ────────────────────────────────────────────────────────────────────

/**
 * Device Chat: one shared thread between the PC and the paired Android client.
 * The PC owns the store; the phone relays through its `chat_*` commands to the
 * PC's REST API, so both platforms talk to the same three command names.
 */
export default function DeviceChatPage({ active }: { active: boolean }) {
  const [mounted, setMounted] = useState(false);
  const [mobile, setMobile] = useState(false);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [selfDevice, setSelfDevice] = useState("pc");
  const [draft, setDraft] = useState("");
  const [pendingFiles, setPendingFiles] = useState<{ path: string; name: string }[]>([]);
  const [outbox, setOutbox] = useState<OutboxSend[]>([]);
  const [failed, setFailed] = useState<FailedSend[]>([]);
  const [loadError, setLoadError] = useState("");
  const [pcUrl, setPcUrl] = useState("");
  const [loading, setLoading] = useState(false);
  // Attachment uuid -> save/download progress, fed by `chat-save-progress`.
  const [saves, setSaves] = useState<Record<string, SaveState>>({});
  const scrollRef = useRef<HTMLDivElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const atBottomRef = useRef(true);
  // Pages stay mounted (display toggling), so the poll must know whether this
  // tab is actually on screen.
  const activeRef = useRef(active);
  activeRef.current = active;
  // Newest `created_at` seen so far — the poll cursor.
  const cursorRef = useRef("");

  useEffect(() => {
    setMobile(isMobileApp());
    setMounted(true);
  }, []);

  const desktop = mounted && !mobile;

  // ── Loading ──────────────────────────────────────────────────────────────

  const mergeMessages = useCallback((incoming: ChatMessage[]) => {
    setMessages((prev) => {
      const byUuid = new Map(prev.map((m) => [m.uuid, m]));
      for (const m of incoming) byUuid.set(m.uuid, m);
      return [...byUuid.values()].sort((a, b) => a.created_at.localeCompare(b.created_at));
    });
    for (const m of incoming) {
      if (m.created_at > cursorRef.current) cursorRef.current = m.created_at;
    }
  }, []);

  /** Full reload (mount, retry, desktop event) — the thread is cheap to refetch. */
  const reload = useCallback(async () => {
    if (!isTauri()) return;
    setLoading(true);
    try {
      const res = await invoke<ChatListResponse>("chat_list_messages", { after: null, limit: 200 });
      setMessages(res.messages);
      setSelfDevice(res.self_device);
      setLoadError("");
      const last = res.messages[res.messages.length - 1];
      cursorRef.current = last ? last.created_at : "";
    } catch (e) {
      setLoadError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  /** Incremental poll (mobile only): ask for everything after the cursor. */
  const poll = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const res = await invoke<ChatListResponse>("chat_list_messages", {
        after: cursorRef.current || null,
        limit: 200,
      });
      setSelfDevice(res.self_device);
      setLoadError("");
      if (res.messages.length > 0) mergeMessages(res.messages);
    } catch (e) {
      setLoadError(String(e));
    }
  }, [mergeMessages]);

  useEffect(() => {
    if (!isTauri()) return;
    void reload();
  }, [reload]);

  // Phone: poll while this tab is on screen. Desktop gets pushed events instead.
  useEffect(() => {
    if (!isTauri() || !mobile) return;
    const id = window.setInterval(() => {
      if (activeRef.current && document.visibilityState === "visible") void poll();
    }, POLL_INTERVAL_MS);
    const onFocus = () => {
      if (activeRef.current) void poll();
    };
    window.addEventListener("focus", onFocus);
    return () => {
      window.clearInterval(id);
      window.removeEventListener("focus", onFocus);
    };
  }, [mobile, poll]);

  // Catch up on messages that arrived while the tab was in the background.
  useEffect(() => {
    if (active && mobile) void poll();
  }, [active, mobile, poll]);

  // PC: every message stored on this machine — locally or relayed from a phone
  // — is broadcast, so open chat views refresh without waiting for a poll.
  useEffect(() => {
    if (!isTauri() || mobile) return;
    let unlisten: (() => void) | undefined;
    listen<ChatMessage>("chat-message", (event) => {
      const msg = event.payload;
      setFailed((prev) => prev.filter((f) => f.uuid !== msg.uuid));
      mergeMessages([msg]);
    })
      .then((fn) => {
        unlisten = fn;
      })
      .catch((e) => logError(`chat: event listener failed: ${String(e)}`, "chat"));
    return () => {
      if (unlisten) unlisten();
    };
  }, [mobile, mergeMessages]);

  // Phone: the precondition for everything is a reachable, paired PC.
  useEffect(() => {
    if (!isTauri() || !mobile) return;
    invoke<GlobalSettings>("settings_get_global")
      .then((g) => setPcUrl((g.pc_url ?? "").trim()))
      .catch(() => {});
  }, [mobile]);

  // Android soft keyboard: the WebView often does not resize when the IME
  // opens, so the composer would sit behind it. Track the visual viewport and
  // pad the page bottom by the keyboard height, lifting the input forward into
  // view while keeping the newest messages pinned above it.
  useEffect(() => {
    if (!isTauri() || !mobile) return;
    const viewport = window.visualViewport;
    const root = rootRef.current;
    if (!viewport || !root) return;
    const onViewportChange = () => {
      const inset = Math.max(0, window.innerHeight - viewport.height - viewport.offsetTop);
      root.style.paddingBottom = inset > 0 ? `${inset}px` : "";
      if (inset > 0 && atBottomRef.current && scrollRef.current) {
        scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
      }
    };
    viewport.addEventListener("resize", onViewportChange);
    viewport.addEventListener("scroll", onViewportChange);
    onViewportChange();
    return () => {
      viewport.removeEventListener("resize", onViewportChange);
      viewport.removeEventListener("scroll", onViewportChange);
      root.style.paddingBottom = "";
    };
  }, [mobile]);

  const clearSave = useCallback((uuid: string) => {
    setSaves((prev) => {
      if (!(uuid in prev)) return prev;
      const next = { ...prev };
      delete next[uuid];
      return next;
    });
  }, []);

  // Byte-level progress: emitted by the desktop copy and the phone download.
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    listen<SaveProgressEvent>("chat-save-progress", (event) => {
      const { uuid, received, total, done } = event.payload;
      const pct = total > 0 ? Math.min(100, Math.floor((received * 100) / total)) : 0;
      setSaves((prev) => ({ ...prev, [uuid]: done ? { pct: 100, done: true } : { pct } }));
      if (done) window.setTimeout(() => clearSave(uuid), 2500);
    })
      .then((fn) => {
        unlisten = fn;
      })
      .catch((e) => logError(`chat: progress listener failed: ${String(e)}`, "chat"));
    return () => {
      if (unlisten) unlisten();
    };
  }, [clearSave]);

  // Keep the view pinned to the newest message unless the user scrolled up.
  const messageCount = messages.length;
  useEffect(() => {
    const el = scrollRef.current;
    if (el && atBottomRef.current) el.scrollTop = el.scrollHeight;
  }, [messageCount, outbox, failed]);

  // ── Composing ────────────────────────────────────────────────────────────

  const pickFiles = useCallback(async () => {
    try {
      const selected = await open({ multiple: true, title: "Attach files" });
      if (!selected) return;
      const paths = Array.isArray(selected) ? selected : [selected];
      // Log the raw picker result: on Android SAF often returns `content://`
      // URIs instead of filesystem paths, which the backend copy cannot read.
      logInfo(`chat: picked ${paths.length} file(s): ${paths.join(" | ")}`, "chat");
      setPendingFiles((prev) => {
        const seen = new Set(prev.map((p) => p.path));
        return [
          ...prev,
          ...paths
            .filter((p): p is string => !!p && !seen.has(p))
            .map((p) => ({ path: p, name: attachmentDisplayName(p) })),
        ];
      });
    } catch (e) {
      logError(`chat: file picker failed: ${String(e)}`, "chat");
    }
  }, []);

  const send = useCallback(
    async (text: string, files: { path: string; name: string }[], uuid?: string) => {
      const id = uuid ?? crypto.randomUUID?.() ?? `${Date.now()}-${Math.random()}`;
      const inflight: OutboxSend = { uuid: id, text, files };
      setOutbox((prev) => [...prev, inflight]);
      logInfo(
        `chat: sending ${id} text_len=${text.trim().length} files=${files.length} [${files
          .map((f) => f.path)
          .join(" | ")}]`,
        "chat",
      );
      try {
        const msg = await invoke<ChatMessage>("chat_send_message", {
          text,
          filePaths: files.map((f) => f.path),
          uuid: id,
        });
        mergeMessages([msg]);
        setFailed((prev) => prev.filter((f) => f.uuid !== id));
      } catch (e) {
        logError(`chat: send ${id} failed: ${String(e)}`, "chat");
        setFailed((prev) => [...prev.filter((f) => f.uuid !== id), { uuid: id, text, files, error: String(e) }]);
      } finally {
        setOutbox((prev) => prev.filter((o) => o.uuid !== id));
      }
      return id;
    },
    [mergeMessages],
  );

  const handleSend = useCallback(async () => {
    const text = draft.trim();
    if ((!text && pendingFiles.length === 0) || outbox.length > 0) return;
    const files = pendingFiles;
    setDraft("");
    setPendingFiles([]);
    await send(text, files);
  }, [draft, pendingFiles, outbox.length, send]);

  const canSend = (draft.trim().length > 0 || pendingFiles.length > 0) && outbox.length === 0;

  // ── Saving attachments ───────────────────────────────────────────────────

  /** Desktop picks a destination; the phone keeps its own copy in the cache. */
  const saveAttachment = useCallback(
    async (att: ChatAttachment) => {
      let dest: string | null = null;
      if (!mobile) {
        try {
          const picked = await save({ title: "Save attachment", defaultPath: att.filename });
          if (!picked) return; // user cancelled
          dest = picked;
        } catch (e) {
          logError(`chat: save dialog failed: ${String(e)}`, "chat");
          return;
        }
      }
      setSaves((prev) => ({ ...prev, [att.uuid]: { pct: 0 } }));
      try {
        await invoke<string>("chat_save_attachment", {
          uuid: att.uuid,
          filename: att.filename,
          dest,
        });
      } catch (e) {
        const error = String(e);
        setSaves((prev) => ({ ...prev, [att.uuid]: { pct: 0, error } }));
        logError(`chat: cannot save attachment ${att.filename}: ${error}`, "chat");
        window.setTimeout(() => clearSave(att.uuid), 5000);
      }
    },
    [mobile, clearSave],
  );

  const revealAttachment = useCallback((att: ChatAttachment) => {
    resolveAttachmentPath(att)
      .then((p) => revealItemInDir(p))
      .catch(() => {});
  }, []);

  const grouped = useMemo(() => {
    // A send in flight renders once as an ephemeral "sending…" outbox row; the
    // durable mirror echoes the same uuid back as `pending`, so drop it here to
    // avoid a double bubble until the hub confirms and it becomes a normal row.
    const inflight = new Set(outbox.map((o) => o.uuid));
    const out: { day: string; items: ChatMessage[] }[] = [];
    for (const m of messages) {
      if (inflight.has(m.uuid)) continue;
      const day = dayKey(m.created_at);
      const last = out[out.length - 1];
      if (last && last.day === day) last.items.push(m);
      else out.push({ day, items: [m] });
    }
    return out;
  }, [messages, outbox]);

  // Platform facts (`isTauri()`, `mobile`) only exist inside the WebView, so
  // nothing platform-dependent may be rendered before mount — otherwise the
  // prerendered HTML and the first client render disagree and React has to
  // regenerate the tree (hydration error). Same discipline as StudioPage and
  // DatasetsSyncPage, which both branch behind a `mounted` guard.
  if (!mounted) return null;

  if (!isTauri()) {
    return (
      <div className="flex flex-col w-full h-full bg-bg-base p-4">
        <h1 className="text-[1.3em] font-bold text-text-primary">Device Chat</h1>
        <p className="text-text-secondary text-sm mt-2">
          The message store lives on the PC — open the chat inside the app.
        </p>
      </div>
    );
  }

  return (
    <div ref={rootRef} className="flex flex-col w-full h-full min-h-0 bg-bg-base">
      {/* Header */}
      <div className="flex items-center gap-2 px-4 py-2.5 border-b border-border-default shrink-0">
        <h1 className="text-[1.1em] font-bold text-text-primary">Device Chat</h1>
        <span className="text-xs text-text-tertiary">
          {mobile ? "synced with the PC" : "this PC · shared with paired devices"}
        </span>
        <button
          onClick={() => void reload()}
          disabled={loading}
          className="ml-auto p-1.5 rounded text-text-tertiary hover:text-text-primary hover:bg-bg-hover cursor-pointer disabled:opacity-50"
          title="Reload"
        >
          <RefreshCw size={15} className={loading ? "animate-spin" : undefined} />
        </button>
      </div>

      {/* Phone precondition / connection errors */}
      {mobile && !pcUrl && (
        <div className="px-4 py-2 text-xs text-yellow-500 border-b border-border-default shrink-0">
          Not connected to a PC — pick one under Datasets to send or receive messages.
        </div>
      )}
      {loadError && (
        <div className="px-4 py-2 text-xs text-red-400 border-b border-border-default shrink-0 flex items-center gap-2">
          <CloudOff size={13} className="shrink-0" />
          <span className="break-all">{loadError}</span>
        </div>
      )}

      {/* Thread */}
      <div
        ref={scrollRef}
        onScroll={(e) => {
          const el = e.currentTarget;
          atBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
        className="flex-1 min-h-0 overflow-y-auto px-4 py-3 flex flex-col gap-2"
      >
        {mounted && messages.length === 0 && outbox.length === 0 && failed.length === 0 && (
          <div className="flex-1 flex flex-col items-center justify-center gap-1 text-text-tertiary text-sm">
            <p>No messages yet.</p>
            <p>Write something here and it appears on your other devices.</p>
          </div>
        )}
        {grouped.map((group) => (
          <div key={group.day} className="flex flex-col gap-2">
            <div className="text-[11px] text-text-tertiary text-center py-1 select-none">
              {group.day}
            </div>
            {group.items.map((m) => {
              const mine = m.sender_device === selfDevice;
              return (
                <div
                  key={m.uuid}
                  className={`flex flex-col gap-1 max-w-[85%] ${mine ? "self-end items-end" : "self-start items-start"}`}
                >
                  <span className="text-[11px] text-text-tertiary px-1 select-none">
                    {m.sender_name} · {formatTime(m.created_at)}
                    {m.pending && (
                      <span className="ml-1 inline-flex items-center gap-0.5">
                        <Loader2 size={10} className="animate-spin inline" />pending
                      </span>
                    )}
                  </span>
                  {m.text && (
                    <div
                      className={`px-3 py-2 rounded-xl text-sm whitespace-pre-wrap break-words ${
                        mine
                          ? m.pending
                            ? "bg-accent-bg/40 text-text-primary"
                            : "bg-accent-bg text-white"
                          : "bg-bg-card border border-border-light text-text-primary"
                      }`}
                    >
                      {m.text}
                    </div>
                  )}
                  {m.attachments.length > 0 && (
                    <div className={`flex flex-wrap gap-2 ${mine ? "justify-end" : ""}`}>
                      {m.attachments.map((att) => (
                        <AttachmentBlock
                          key={att.uuid}
                          att={att}
                          mine={mine}
                          desktop={desktop}
                          saveState={saves[att.uuid]}
                          onSave={(a) => void saveAttachment(a)}
                          onReveal={revealAttachment}
                        />
                      ))}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        ))}

        {outbox.map((o) => (
          <div key={o.uuid} className="self-end flex flex-col gap-1 items-end max-w-[85%]">
            <span className="text-[11px] text-text-tertiary">sending…</span>
            <div className="px-3 py-2 rounded-xl text-sm bg-accent-bg/40 text-text-primary whitespace-pre-wrap break-words flex items-center gap-2">
              <Loader2 size={14} className="animate-spin shrink-0" />
              {o.text || `${o.files.length} file(s)`}
            </div>
          </div>
        ))}

        {failed.map((f) => (
          <div key={f.uuid} className="self-end flex flex-col gap-1 items-end max-w-[85%]">
            <span className="text-[11px] text-red-400">not delivered</span>
            <div className="px-3 py-2 rounded-xl text-sm bg-red-500/10 border border-red-500/30 text-text-primary whitespace-pre-wrap break-words">
              {f.text || `${f.files.length} file(s)`}
            </div>
            <div className="flex items-center gap-2">
              <button
                onClick={() => {
                  void send(f.text, f.files, f.uuid);
                }}
                className="inline-flex items-center gap-1 px-2.5 py-1 rounded-md text-xs bg-bg-body border border-border-light hover:bg-bg-hover text-text-primary cursor-pointer"
                title={f.error}
              >
                <RefreshCw size={12} /> Retry
              </button>
              <button
                onClick={() => setFailed((prev) => prev.filter((x) => x.uuid !== f.uuid))}
                className="text-xs text-text-tertiary hover:text-text-primary cursor-pointer"
              >
                Discard
              </button>
            </div>
          </div>
        ))}
      </div>

      {/* Composer */}
      <div className="shrink-0 border-t border-border-default p-3 flex flex-col gap-2">
        {pendingFiles.length > 0 && (
          <div className="flex flex-wrap gap-1.5">
            {pendingFiles.map((f) => (
              <span
                key={f.path}
                className="inline-flex items-center gap-1 max-w-[220px] px-2 py-1 rounded-md bg-bg-card border border-border-light text-xs text-text-primary"
              >
                <FileText size={12} className="shrink-0" />
                <span className="truncate">{f.name}</span>
                <button
                  onClick={() => setPendingFiles((prev) => prev.filter((p) => p.path !== f.path))}
                  className="shrink-0 text-text-tertiary hover:text-text-primary cursor-pointer"
                  title="Remove attachment"
                >
                  <X size={12} />
                </button>
              </span>
            ))}
          </div>
        )}
        <div className="flex items-end gap-2">
          <button
            onClick={() => void pickFiles()}
            className="p-2 rounded-md text-text-tertiary hover:text-text-primary hover:bg-bg-hover cursor-pointer shrink-0"
            title="Attach files"
          >
            <Paperclip size={17} />
          </button>
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !mobile) {
                e.preventDefault();
                void handleSend();
              }
            }}
            rows={1}
            placeholder="Message…"
            className="flex-1 min-w-0 max-h-32 resize-none px-3 py-2 rounded-lg bg-bg-input border border-border-light text-sm text-text-primary placeholder:text-text-tertiary focus:outline-none focus:border-accent"
          />
          <button
            onClick={() => void handleSend()}
            disabled={!canSend}
            className="inline-flex items-center gap-1.5 px-3 py-2 rounded-lg text-sm font-medium bg-accent-bg text-white hover:bg-accent-bg-hover disabled:opacity-40 cursor-pointer shrink-0"
          >
            {outbox.length > 0 ? <Loader2 size={15} className="animate-spin" /> : <Send size={15} />}
            {desktop ? "Send" : ""}
          </button>
        </div>
      </div>
    </div>
  );
}
