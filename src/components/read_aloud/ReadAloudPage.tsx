"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import {
  Plus,
  Pencil,
  Trash2,
  FilePlus,
  FileText,
  Wand2,
  Save,
  X,
  Square,
  Loader2,
  MoreVertical,
  Check,
  History,
  Play,
} from "lucide-react";
import { MicGlyph } from "@/components/voice/MicGlyph";
import MdEditor from "@/components/wiki/MdEditor";
import type { ReadAloudMeta, ReadAttempt, ReadText } from "@/lib/read_aloud/types";
import { scoreBadgeClasses } from "@/lib/read_aloud/types";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import { logError } from "@/lib/logger";
import { tbGhost, tbToggle, tbIconToggleSoft, tbDanger } from "@/lib/ui/toolbar";
import PracticePanel, { type RecorderCtl } from "./PracticePanel";
import AttemptsPanel from "./AttemptsPanel";
import DatasetDialog from "./DatasetDialog";
import ConfirmDialog, { type ConfirmRequest } from "@/components/read_book/ConfirmDialog";

const LOG_MODULE = "read-aloud";

type DatasetDialogState = { mode: "create" } | { mode: "edit"; meta: ReadAloudMeta } | null;

function getUUID(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) return crypto.randomUUID();
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    const v = c === "x" ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
}

function nowIso(): string {
  return new Date().toISOString();
}

/** Format an ISO timestamp as `yyyy-mm-dd hh24:mi:ss` (local time) — the label
 *  used for an attempt's breadcrumb crumb. */
function formatStamp(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(
    d.getHours()
  )}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

export default function ReadAloudPage() {
  const [datasets, setDatasets] = useState<ReadAloudMeta[]>([]);
  const [selectedDataset, setSelectedDataset] = useState<string>("");
  const [texts, setTexts] = useState<ReadText[]>([]);
  const [selectedText, setSelectedText] = useState<string | null>(null);

  // `isMobileApp()` is false during prerender (no navigator) and only turns true
  // inside the webview, so resolve it in an effect to avoid a hydration mismatch.
  const [mobile, setMobile] = useState(false);
  useEffect(() => {
    setMobile(isMobileApp());
  }, []);

  // ── Inline text editor (edit/view mode, mirroring the wiki page) ──
  // `editing` swaps the content column for the text form and the sub-toolbar for
  // Save / Cancel. `editText` is the text being edited, or null for a brand-new
  // one.
  const [editing, setEditing] = useState(false);
  const [editText, setEditText] = useState<ReadText | null>(null);
  const [title, setTitle] = useState("");
  const [note, setNote] = useState("");
  const [content, setContent] = useState("");
  const [normalized, setNormalized] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // A dedicated inline editor for the LLM-normalized scoring reference, opened
  // from the practice view's overflow menu (mirrors the main edit mode). Its
  // draft is separate from `normalized` (used by the main text editor) so the
  // two flows never interfere.
  const [editingNormalized, setEditingNormalized] = useState(false);
  const [normalizedDraft, setNormalizedDraft] = useState("");

  const [datasetDialog, setDatasetDialog] = useState<DatasetDialogState>(null);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  // The attempt history lives on its own page, opened from the practice view's
  // toolbar ("Attempts").
  const [attemptsView, setAttemptsView] = useState(false);
  // The attempt opened in the detail panel is lifted here so the breadcrumb can
  // show it as a level. `selectedAttempt` is reported up by AttemptsPanel and is
  // used only to label the crumb.
  const [selectedAttemptUuid, setSelectedAttemptUuid] = useState<string | null>(null);
  const [selectedAttempt, setSelectedAttempt] = useState<ReadAttempt | null>(null);
  const handleSelectAttempt = useCallback((a: ReadAttempt | null) => setSelectedAttempt(a), []);
  // Bumped after the toolbar deletes the open attempt so AttemptsPanel reloads.
  const [attemptsRefreshKey, setAttemptsRefreshKey] = useState(0);

  // Playback of the open attempt's recording, driven by the toolbar Play/Stop
  // button. The element is kept in a ref so playback can be stopped — on toggle,
  // when navigating away from the attempt, and on unmount — instead of running
  // fire-and-forget.
  const attemptAudioRef = useRef<HTMLAudioElement | null>(null);
  const [playingAttempt, setPlayingAttempt] = useState(false);

  const stopAttemptAudio = useCallback(() => {
    const el = attemptAudioRef.current;
    if (el) {
      el.onended = null;
      el.pause();
      el.currentTime = 0;
      attemptAudioRef.current = null;
    }
    setPlayingAttempt(false);
  }, []);

  const toggleAttemptPlay = useCallback(
    (url?: string | null) => {
      // Already playing → stop (toggle off), regardless of which attempt.
      if (attemptAudioRef.current) {
        stopAttemptAudio();
        return;
      }
      if (!url) return;
      const el = new Audio(convertFileSrc(url));
      attemptAudioRef.current = el;
      el.onended = () => {
        attemptAudioRef.current = null;
        setPlayingAttempt(false);
      };
      setPlayingAttempt(true);
      el.play().catch(() => {
        attemptAudioRef.current = null;
        setPlayingAttempt(false);
      });
    },
    [stopAttemptAudio]
  );

  // Stop playback when the open attempt changes or is cleared, and on unmount.
  useEffect(() => {
    return () => stopAttemptAudio();
  }, [selectedAttemptUuid, stopAttemptAudio]);

  // The Record button lives in the toolbar, but the recorder + submit logic stay
  // in PracticePanel; it publishes its control here (and clears it on unmount).
  const recToggleRef = useRef<() => void>(() => {});
  const [rec, setRec] = useState({ ready: false, recording: false, busy: false });
  const handleRecorder = useCallback((ctl: RecorderCtl | null) => {
    if (!ctl) {
      recToggleRef.current = () => {};
      setRec({ ready: false, recording: false, busy: false });
      return;
    }
    recToggleRef.current = ctl.toggle;
    setRec((prev) =>
      prev.ready && prev.recording === ctl.recording && prev.busy === ctl.busy
        ? prev
        : { ready: true, recording: ctl.recording, busy: ctl.busy }
    );
  }, []);

  const loadDatasets = useCallback(async (preferUuid?: string) => {
    if (!isTauri()) return;
    try {
      const list = await invoke<ReadAloudMeta[]>("read_aloud_list");
      setDatasets(list);
      setSelectedDataset((prev) => {
        const target = preferUuid ?? prev;
        if (target && list.some((d) => d.uuid === target)) return target;
        // No explicit target: fall back to the dataset list view (wiki-style),
        // letting the user pick a dataset rather than auto-opening the first.
        return "";
      });
    } catch (e) {
      logError(`load datasets failed: ${String(e)}`, LOG_MODULE);
    }
  }, []);

  const loadTexts = useCallback(async (datasetUuid: string) => {
    if (!isTauri() || !datasetUuid) {
      setTexts([]);
      return;
    }
    try {
      const list = await invoke<ReadText[]>("read_aloud_list_texts", { datasetUuid });
      setTexts(list);
      // Keep a still-valid selection, otherwise show the text list (no auto-open).
      setSelectedText((prev) => (prev && list.some((t) => t.uuid === prev) ? prev : null));
    } catch (e) {
      logError(`load texts failed: ${String(e)}`, LOG_MODULE);
    }
  }, []);

  useEffect(() => {
    loadDatasets();
  }, [loadDatasets]);

  useEffect(() => {
    loadTexts(selectedDataset);
  }, [selectedDataset, loadTexts]);

  // The normalized editor and the attempts page are per-text concerns; close
  // them whenever the selected text changes so they never leak across texts.
  useEffect(() => {
    setEditingNormalized(false);
    setAttemptsView(false);
    setSelectedAttemptUuid(null);
  }, [selectedText]);

  // Leaving the attempts page clears any open attempt detail.
  useEffect(() => {
    if (!attemptsView) {
      setSelectedAttemptUuid(null);
      setSelectedAttempt(null);
    }
  }, [attemptsView]);

  const activeText = texts.find((t) => t.uuid === selectedText) ?? null;
  const activeDataset = datasets.find((d) => d.uuid === selectedDataset) ?? null;

  // Wiki-style view modes derived from the selection: no dataset → the dataset
  // list, a dataset but no text → its text list, a text → the practice pane.
  const viewMode: "datasets" | "texts" | "practice" = !activeDataset
    ? "datasets"
    : activeText
      ? "practice"
      : "texts";

  // Draft differs from the stored text (or, for a new one, from empty fields).
  const dirty =
    title !== (editText?.title ?? "") ||
    note !== (editText?.note ?? "") ||
    content !== (editText?.content ?? "") ||
    normalized !== (editText?.normalized_content ?? "");

  const goDatasets = () => {
    setSelectedDataset("");
    setSelectedText(null);
  };
  const goTexts = () => setSelectedText(null);

  // ── Editor lifecycle ──────────────────────────────────────────────
  const startEdit = (t: ReadText | null) => {
    setEditText(t);
    setTitle(t?.title ?? "");
    setNote(t?.note ?? "");
    setContent(t?.content ?? "");
    setNormalized(t?.normalized_content ?? "");
    setMenuOpen(false);
    setError(null);
    setEditing(true);
  };

  const cancelEdit = () => {
    setEditing(false);
    setEditText(null);
    setError(null);
  };

  const saveText = async () => {
    if (!isTauri() || !selectedDataset) return;
    if (!content.trim()) {
      setError("The text content must not be empty.");
      return;
    }
    setSaving(true);
    setError(null);
    const now = nowIso();
    const trimmedContent = content.trim();
    const contentChanged = editText != null && trimmedContent !== editText.content.trim();
    // Mirrors the backend staleness rule: editing the content invalidates a
    // normalization that was derived from the old one.
    const normalizedValue = contentChanged ? null : normalized.trim() || null;
    const payload: ReadText = {
      uuid: editText?.uuid ?? getUUID(),
      dataset_uuid: selectedDataset,
      order_num: editText?.order_num ?? texts.length + 1,
      title: title.trim(),
      note: note.trim(),
      content: trimmedContent,
      normalized_content: normalizedValue,
      best_score: editText?.best_score ?? null,
      attempt_count: editText?.attempt_count ?? 0,
      created_at: editText?.created_at ?? now,
      updated_at: now,
    };
    try {
      await invoke("read_aloud_save_text", { datasetUuid: selectedDataset, text: payload });
      setEditing(false);
      setEditText(null);
      await loadTexts(selectedDataset);
      // Open the saved text in the practice (view) pane.
      setSelectedText(payload.uuid);
    } catch (e) {
      const msg = String(e);
      logError(`save text failed: ${msg}`, LOG_MODULE);
      setError(msg);
    } finally {
      setSaving(false);
    }
  };

  // ── Normalized-reference editor lifecycle ──────────────────────────
  const startNormalizeEdit = () => {
    if (!activeText) return;
    setNormalizedDraft(activeText.normalized_content ?? "");
    setMenuOpen(false);
    setError(null);
    setEditingNormalized(true);
  };

  const cancelNormalizeEdit = () => {
    setEditingNormalized(false);
    setError(null);
  };

  const saveNormalized = async () => {
    if (!isTauri() || !selectedDataset || !activeText) return;
    setSaving(true);
    setError(null);
    const t = activeText;
    // Only the normalized reference changes here; content is passed through
    // untouched so the backend staleness rule keeps the value we set.
    const payload: ReadText = {
      ...t,
      normalized_content: normalizedDraft.trim() || null,
      updated_at: nowIso(),
    };
    try {
      await invoke("read_aloud_save_text", { datasetUuid: selectedDataset, text: payload });
      setEditingNormalized(false);
      await loadTexts(selectedDataset);
      setSelectedText(t.uuid);
    } catch (e) {
      const msg = String(e);
      logError(`save normalized failed: ${msg}`, LOG_MODULE);
      setError(msg);
    } finally {
      setSaving(false);
    }
  };

  const deleteText = (t: ReadText) => {
    setConfirm({
      title: "Delete text",
      message: `Delete "${t.title || "Untitled"}" and all its recordings?`,
      confirmLabel: "Delete",
      onConfirm: async () => {
        try {
          await invoke("read_aloud_delete_text", { datasetUuid: selectedDataset, uuid: t.uuid });
          if (selectedText === t.uuid) setSelectedText(null);
          await loadTexts(selectedDataset);
        } catch (e) {
          logError(`delete text failed: ${String(e)}`, LOG_MODULE);
        }
      },
    });
  };

  // Delete the attempt currently open in the detail panel. The button lives in
  // the toolbar; AttemptsPanel reloads via the bumped `attemptsRefreshKey`.
  const deleteSelectedAttempt = () => {
    if (!selectedAttempt || !selectedDataset) return;
    const a = selectedAttempt;
    setConfirm({
      title: "Delete attempt",
      message: "Delete this recording and its score? This cannot be undone.",
      confirmLabel: "Delete",
      onConfirm: async () => {
        try {
          await invoke("read_aloud_delete_attempt", { datasetUuid: selectedDataset, uuid: a.uuid });
          setSelectedAttemptUuid(null);
          setAttemptsRefreshKey((k) => k + 1);
          await loadTexts(selectedDataset);
        } catch (e) {
          logError(`delete attempt failed: ${String(e)}`, LOG_MODULE);
        }
      },
    });
  };

  const deleteDataset = (meta: ReadAloudMeta) => {
    setConfirm({
      title: "Delete dataset",
      message: `Delete the read-aloud dataset "${meta.name}" and everything inside it?\n\nThe folder is moved to trash, not permanently deleted.`,
      confirmLabel: "Delete",
      onConfirm: async () => {
        try {
          await invoke("read_aloud_delete", { uuid: meta.uuid });
          if (selectedDataset === meta.uuid) setSelectedDataset("");
          await loadDatasets();
        } catch (e) {
          logError(`delete dataset failed: ${String(e)}`, LOG_MODULE);
        }
      },
    });
  };

  const fieldCls =
    "w-full p-2 rounded-md bg-bg-muted border border-border-default text-text-primary text-sm focus:outline-none focus:border-accent-bg";
  const labelCls = "text-xs font-medium text-text-secondary mb-1 block";
  const menuItem =
    "w-full text-left px-3 py-2 text-sm hover:bg-bg-hover cursor-pointer flex items-center gap-2 text-text-primary disabled:opacity-50 disabled:cursor-not-allowed";
  const menuItemDanger =
    "w-full text-left px-3 py-2 text-sm hover:bg-bg-hover cursor-pointer flex items-center gap-2 text-error-text disabled:opacity-50 disabled:cursor-not-allowed";

  // Either inline editor (the main text form or the normalized-reference form)
  // swaps the content column and the sub-toolbar into Save / Cancel.
  const inEditor = editing || editingNormalized;
  const editorDirty = editing
    ? dirty
    : normalizedDraft !== (activeText?.normalized_content ?? "");

  return (
    <main className="flex-1 flex flex-col h-full min-h-0 min-w-0 bg-bg-body">
      {/* Breadcrumb navigation — Read Aloud › dataset › text, mirroring the wiki
          page's edge-to-edge trail. Crumbs are inert while editing so a stray
          click can't discard the draft. */}
      <nav className="shrink-0 flex items-center justify-start gap-2 px-4 py-2 text-sm select-none min-w-0 overflow-x-auto border-b border-border-default bg-bg-card">
        {inEditor ? (
          <span className="shrink-0 text-accent">Read Aloud</span>
        ) : (
          <button
            className={`shrink-0 cursor-pointer hover:underline ${
              viewMode === "datasets" ? "text-text-primary font-medium" : "text-accent"
            }`}
            onClick={goDatasets}
          >
            Read Aloud
          </button>
        )}
        {activeDataset && (
          <>
            <span className="shrink-0 text-text-tertiary">&rsaquo;</span>
            {inEditor ? (
              <span className="truncate min-w-0 max-w-[240px] text-accent" title={activeDataset.name}>
                {activeDataset.name}
              </span>
            ) : (
              <button
                className={`cursor-pointer hover:underline truncate min-w-0 max-w-[240px] ${
                  viewMode === "practice" ? "text-accent" : "text-text-primary font-medium"
                }`}
                title={activeDataset.name}
                onClick={goTexts}
              >
                {activeDataset.name}
              </button>
            )}
          </>
        )}
        {editing ? (
          <>
            <span className="shrink-0 text-text-tertiary">&rsaquo;</span>
            <span className="text-text-primary font-medium truncate min-w-0 max-w-[320px]">
              {editText ? editText.title || "Untitled" : "New text"}
            </span>
          </>
        ) : (
          viewMode === "practice" &&
          activeText && (
            <>
              <span className="shrink-0 text-text-tertiary">&rsaquo;</span>
              {attemptsView ? (
                <button
                  className="cursor-pointer hover:underline text-accent truncate min-w-0 max-w-[320px]"
                  title={activeText.title || "Untitled"}
                  onClick={() => setAttemptsView(false)}
                >
                  {activeText.title || "Untitled"}
                </button>
              ) : (
                <span
                  className="text-text-primary font-medium truncate min-w-0 max-w-[320px]"
                  title={activeText.title || "Untitled"}
                >
                  {activeText.title || "Untitled"}
                </span>
              )}
              {attemptsView && (
                <>
                  <span className="shrink-0 text-text-tertiary">&rsaquo;</span>
                  {selectedAttempt ? (
                    <button
                      className="cursor-pointer hover:underline text-accent shrink-0"
                      onClick={() => setSelectedAttemptUuid(null)}
                    >
                      Attempts
                    </button>
                  ) : (
                    <span className="text-text-primary font-medium shrink-0">Attempts</span>
                  )}
                  {selectedAttempt && (
                    <>
                      <span className="shrink-0 text-text-tertiary">&rsaquo;</span>
                      <span className="text-text-primary font-medium shrink-0 tabular-nums">
                        {formatStamp(selectedAttempt.created_at)}
                      </span>
                    </>
                  )}
                </>
              )}
            </>
          )
        )}
      </nav>

      {/* Sub-toolbar: contextual actions for the current view — the same stacked
          bar treatment the wiki page uses. While an inline editor is open it hosts
          the Save / Cancel controls instead. */}
      <div className="shrink-0 flex items-center gap-2 px-4 py-2 border-b border-border-default bg-bg-card">
        <div className="flex-1 min-w-0" />

        {inEditor ? (
          <>
            <button
              type="button"
              className={tbToggle(editorDirty)}
              onClick={() => void (editing ? saveText() : saveNormalized())}
              disabled={saving || !editorDirty}
              title="Save"
            >
              <Save size={14} /> {saving ? "Saving…" : "Save"}
            </button>
            <button
              type="button"
              className={tbGhost}
              onClick={editing ? cancelEdit : cancelNormalizeEdit}
              title="Cancel editing"
            >
              <X size={14} />{!mobile && " Cancel"}
            </button>
          </>
        ) : (
          <>
            {viewMode === "datasets" && (
              <button
                type="button"
                className={tbGhost}
                title="New dataset"
                onClick={() => setDatasetDialog({ mode: "create" })}
              >
                <Plus size={14} />{!mobile && " New Dataset"}
              </button>
            )}

            {viewMode === "texts" && (
              <>
                <button
                  type="button"
                  className={tbGhost}
                  title="Add text"
                  onClick={() => startEdit(null)}
                >
                  <FilePlus size={14} />{!mobile && " New Text"}
                </button>
                <button
                  type="button"
                  className={tbGhost}
                  title="Edit dataset"
                  disabled={!activeDataset}
                  onClick={() => activeDataset && setDatasetDialog({ mode: "edit", meta: activeDataset })}
                >
                  <Pencil size={14} />{!mobile && " Edit"}
                </button>
              </>
            )}

            {viewMode === "practice" && activeText && !attemptsView && (
              <>
                <button
                  type="button"
                  onClick={() => recToggleRef.current()}
                  disabled={!rec.ready || rec.busy}
                  title={rec.recording ? "Stop recording" : "Record yourself reading"}
                  className={`flex items-center gap-1.5 px-3 py-1 text-sm rounded-md transition-colors disabled:opacity-50 disabled:cursor-not-allowed ${
                    rec.recording
                      ? "bg-red-500 text-white animate-pulse"
                      : "bg-accent text-white hover:bg-accent/90"
                  }`}
                >
                  {rec.busy ? (
                    <Loader2 size={14} className="animate-spin" />
                  ) : rec.recording ? (
                    <Square size={14} />
                  ) : (
                    <MicGlyph size={14} />
                  )}
                  {!mobile && (rec.recording ? " Stop" : rec.busy ? " Working…" : " Record")}
                </button>
                <button
                  type="button"
                  className={tbGhost}
                  title="Show attempts"
                  onClick={() => setAttemptsView(true)}
                >
                  <History size={14} />{!mobile && " Attempts"}
                </button>
              </>
            )}

            {/* Attempt detail — its Play / Delete actions live here rather than
                inside the panel, mirroring how Record lives in the toolbar. */}
            {viewMode === "practice" && activeText && attemptsView && selectedAttempt && (
              <>
                {selectedAttempt.audio_url && (
                  <button
                    type="button"
                    className={tbGhost}
                    title={playingAttempt ? "Stop playback" : "Play recording"}
                    onClick={() => toggleAttemptPlay(selectedAttempt.audio_url)}
                  >
                    {playingAttempt ? <Square size={14} /> : <Play size={14} />}
                    {!mobile && (playingAttempt ? " Stop" : " Play")}
                  </button>
                )}
                <button
                  type="button"
                  className={tbDanger}
                  title="Delete attempt"
                  onClick={deleteSelectedAttempt}
                >
                  <Trash2 size={14} />{!mobile && " Delete"}
                </button>
              </>
            )}
          </>
        )}

        {/* Overflow menu — Delete and the Normalized view toggle live here to
            keep the toolbar compact. Items are contextual to the current view;
            the attempts view has no overflow actions (its Play / Delete are in
            the toolbar above), so the menu is hidden there. */}
        {!inEditor && (viewMode === "texts" || (viewMode === "practice" && !attemptsView)) && (
          <div className="relative shrink-0">
            <button
              type="button"
              className={tbIconToggleSoft(menuOpen)}
              title="More actions"
              aria-label="More actions"
              onClick={() => setMenuOpen((v) => !v)}
            >
              <MoreVertical size={16} />
            </button>
            {menuOpen && (
              <div
                className="absolute right-0 top-full mt-1 w-56 rounded-lg border border-border-default bg-bg-card shadow-xl z-20 py-1"
                onMouseLeave={() => setMenuOpen(false)}
              >
                {viewMode === "texts" && (
                  <button
                    type="button"
                    className={menuItemDanger}
                    disabled={!activeDataset}
                    onClick={() => {
                      setMenuOpen(false);
                      if (activeDataset) deleteDataset(activeDataset);
                    }}
                  >
                    <Trash2 size={15} /> Delete dataset
                  </button>
                )}
                {viewMode === "practice" && activeText && (
                  <>
                    <button
                      type="button"
                      className={menuItem}
                      onClick={() => startEdit(activeText)}
                    >
                      <Pencil size={15} /> Edit text
                    </button>
                    <button
                      type="button"
                      className={menuItem}
                      onClick={startNormalizeEdit}
                    >
                      <Wand2 size={15} /> Normalized text
                      {activeText.normalized_content && <Check size={14} className="ml-auto" />}
                    </button>
                    <button
                      type="button"
                      className={menuItemDanger}
                      onClick={() => {
                        setMenuOpen(false);
                        deleteText(activeText);
                      }}
                    >
                      <Trash2 size={15} /> Delete text
                    </button>
                  </>
                )}
              </div>
            )}
          </div>
        )}
      </div>

      {/* Content — while editing this is the inline text form; otherwise the
          practice pane (own scroll) or the dataset/text lists. */}
      {editing ? (
        <div className="flex-1 overflow-y-auto min-h-0 min-w-0 p-4 flex flex-col gap-3">
          <div>
            <label className={labelCls}>Title</label>
            <input
              className={fieldCls}
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="e.g. Der Erlkönig — first stanza"
            />
          </div>

          <div>
            <label className={labelCls}>Note (source, location, …)</label>
            <input
              className={fieldCls}
              value={note}
              onChange={(e) => setNote(e.target.value)}
              placeholder="e.g. Goethe, from a textbook p.42"
            />
          </div>

          <div className="flex flex-col min-h-0">
            <label className={labelCls}>Text to read</label>
            <MdEditor
              value={content}
              onChange={setContent}
              spellCheck={false}
              shortcutsOpen={false}
              textareaBgClass="bg-bg-muted"
            />
          </div>

          {error && (
            <div className="p-2 rounded-md bg-error-bg text-error-text text-xs break-words">{error}</div>
          )}
        </div>
      ) : editingNormalized && activeText ? (
        <div className="flex-1 overflow-y-auto min-h-0 min-w-0 p-4 flex flex-col gap-3">
          <div className="flex flex-col min-h-0">
            <label className={labelCls}>Normalized text for scoring</label>
            <p className="text-[11px] text-text-tertiary mb-1">
              LLM-normalized reference — numbers and abbreviations spelled out so they match the
              transcription. Scoring uses this when set; leave it empty to score against the text to
              read.
            </p>
            <MdEditor
              value={normalizedDraft}
              onChange={setNormalizedDraft}
              spellCheck={false}
              shortcutsOpen={false}
              textareaBgClass="bg-bg-muted"
            />
          </div>
          {error && (
            <div className="p-2 rounded-md bg-error-bg text-error-text text-xs break-words">{error}</div>
          )}
        </div>
      ) : attemptsView && activeDataset && activeText ? (
        <AttemptsPanel
          datasetUuid={activeDataset.uuid}
          text={activeText}
          onChanged={() => loadTexts(selectedDataset)}
          selectedUuid={selectedAttemptUuid}
          onSelectUuid={setSelectedAttemptUuid}
          onSelectAttempt={handleSelectAttempt}
          refreshKey={attemptsRefreshKey}
        />
      ) : viewMode === "practice" && activeDataset && activeText ? (
        <PracticePanel
          datasetUuid={activeDataset.uuid}
          text={activeText}
          onScored={() => loadTexts(selectedDataset)}
          onRecorder={handleRecorder}
        />
      ) : viewMode === "texts" ? (
        <div className="flex-1 overflow-y-auto min-h-0 min-w-0 p-4 flex flex-col gap-1">
          {texts.length === 0 ? (
            <p className="text-text-secondary">
              No texts yet. Add one with the New Text button.
            </p>
          ) : (
            texts.map((t) => (
              <div
                key={t.uuid}
                className="flex items-center gap-2 px-3 py-2 rounded-md text-left transition-colors hover:bg-bg-hover cursor-pointer"
                onClick={() => setSelectedText(t.uuid)}
              >
                <FileText size={16} className="shrink-0 text-text-tertiary" />
                <div className="flex flex-col min-w-0 flex-1">
                  <span className="text-sm text-text-primary truncate flex items-center gap-1">
                    {t.title || "Untitled"}
                    {t.normalized_content && (
                      <Wand2
                        size={12}
                        className="shrink-0 text-accent-bg"
                        aria-label="Normalized reference set (used for scoring)"
                      />
                    )}
                  </span>
                  {t.note && (
                    <span className="text-[11px] text-text-tertiary truncate">{t.note}</span>
                  )}
                </div>
                <span className={scoreBadgeClasses(t.best_score)} title="Best score">
                  {t.best_score != null ? `${Math.round(t.best_score)}%` : "—"}
                </span>
                <span className="text-[10px] text-text-tertiary tabular-nums shrink-0" title="Attempts">
                  ×{t.attempt_count}
                </span>
              </div>
            ))
          )}
        </div>
      ) : (
        <div className="flex-1 overflow-y-auto min-h-0 min-w-0 p-4 flex flex-col gap-2">
          {datasets.length === 0 ? (
            <p className="text-text-secondary">
              No read-aloud datasets yet. Create one with the New Dataset button.
            </p>
          ) : (
            datasets.map((d) => (
              <button
                key={d.uuid}
                className="text-left p-4 border border-border-default rounded-lg transition-colors hover:bg-bg-hover cursor-pointer"
                onClick={() => {
                  setSelectedDataset(d.uuid);
                  setSelectedText(null);
                }}
                title={d.path}
              >
                <div className="flex items-center gap-2">
                  <MicGlyph size={16} />
                  <span className="font-medium text-text-primary truncate">{d.name}</span>
                </div>
                {d.description && (
                  <div className="text-xs text-text-tertiary truncate mt-1">{d.description}</div>
                )}
              </button>
            ))
          )}
        </div>
      )}

      {datasetDialog && (
        <DatasetDialog
          state={datasetDialog}
          onClose={() => setDatasetDialog(null)}
          onSaved={(uuid) => loadDatasets(uuid)}
        />
      )}

      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
    </main>
  );
}
