"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
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
} from "lucide-react";
import { MicGlyph } from "@/components/voice/MicGlyph";
import MdEditor from "@/components/wiki/MdEditor";
import type { ReadAloudMeta, ReadText } from "@/lib/read_aloud/types";
import { scoreBadgeClasses } from "@/lib/read_aloud/types";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import { logError } from "@/lib/logger";
import { tbGhost, tbDanger, tbToggle, tbToggleSoft } from "@/lib/ui/toolbar";
import PracticePanel, { type RecorderCtl } from "./PracticePanel";
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
  // Save / Cancel / ABC / Help. `editText` is the text being edited, or null for
  // a brand-new one.
  const [editing, setEditing] = useState(false);
  const [editText, setEditText] = useState<ReadText | null>(null);
  const [title, setTitle] = useState("");
  const [note, setNote] = useState("");
  const [content, setContent] = useState("");
  const [normalized, setNormalized] = useState("");
  // Which field the MdEditor shows while editing: the text to read, or the
  // LLM-normalized scoring reference (toggled from the sub-toolbar).
  const [showNormalized, setShowNormalized] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [datasetDialog, setDatasetDialog] = useState<DatasetDialogState>(null);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);

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

  const toolbarLabel = editing
    ? editText
      ? "Edit text"
      : "New text"
    : viewMode === "datasets"
      ? `${datasets.length} dataset${datasets.length === 1 ? "" : "s"}`
      : viewMode === "texts"
        ? `${texts.length} text${texts.length === 1 ? "" : "s"}`
        : activeText?.title || "Untitled";

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
    setShowNormalized(false);
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

  return (
    <main className="flex-1 flex flex-col h-full min-h-0 min-w-0 bg-bg-body">
      {/* Breadcrumb navigation — Read Aloud › dataset › text, mirroring the wiki
          page's edge-to-edge trail. Crumbs are inert while editing so a stray
          click can't discard the draft. */}
      <nav className="shrink-0 flex items-center justify-start gap-2 px-4 py-2 text-sm select-none min-w-0 overflow-x-auto border-b border-border-default bg-bg-card">
        {editing ? (
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
            {editing ? (
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
              <span
                className="text-text-primary font-medium truncate min-w-0 max-w-[320px]"
                title={activeText.title || "Untitled"}
              >
                {activeText.title || "Untitled"}
              </span>
            </>
          )
        )}
      </nav>

      {/* Sub-toolbar: contextual actions for the current view — the same stacked
          bar treatment the wiki page uses. While editing it hosts the editor
          controls (Save / Cancel / ABC / Help) instead. */}
      <div className="shrink-0 flex items-center gap-2 px-4 py-2 border-b border-border-default bg-bg-card">
        <div className="text-xs text-text-tertiary truncate flex-1 min-w-0">{toolbarLabel}</div>

        {editing ? (
          <>
            <button
              type="button"
              className={tbToggle(dirty)}
              onClick={() => void saveText()}
              disabled={saving || !dirty}
              title="Save this text"
            >
              <Save size={14} /> {saving ? "Saving…" : "Save"}
            </button>
            <button type="button" className={tbGhost} onClick={cancelEdit} title="Cancel editing">
              <X size={14} />{!mobile && " Cancel"}
            </button>
            <button
              type="button"
              className={tbToggleSoft(showNormalized)}
              onClick={() => setShowNormalized((v) => !v)}
              title={
                showNormalized
                  ? "Edit the text to read"
                  : "Edit the LLM-normalized text used for scoring"
              }
            >
              <Wand2 size={14} />{!mobile && " Normalized"}
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
                <button
                  type="button"
                  className={tbDanger}
                  title="Delete dataset"
                  disabled={!activeDataset}
                  onClick={() => activeDataset && deleteDataset(activeDataset)}
                >
                  <Trash2 size={14} />{!mobile && " Delete"}
                </button>
              </>
            )}

            {viewMode === "practice" && activeText && (
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
                  title="Edit text"
                  onClick={() => startEdit(activeText)}
                >
                  <Pencil size={14} />{!mobile && " Edit"}
                </button>
                <button
                  type="button"
                  className={tbDanger}
                  title="Delete text"
                  onClick={() => deleteText(activeText)}
                >
                  <Trash2 size={14} />{!mobile && " Delete"}
                </button>
              </>
            )}
          </>
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
            <label className={labelCls}>
              {showNormalized ? "Normalized text for scoring (optional)" : "Text to read"}
            </label>
            {showNormalized && (
              <p className="text-[11px] text-text-tertiary mb-1">
                LLM-normalized reference — numbers and abbreviations spelled out so they match the
                transcription. Scoring uses this when set; editing the text to read clears it.
              </p>
            )}
            <MdEditor
              value={showNormalized ? normalized : content}
              onChange={showNormalized ? setNormalized : setContent}
              spellCheck={false}
              shortcutsOpen={false}
            />
          </div>

          {error && (
            <div className="p-2 rounded-md bg-error-bg text-error-text text-xs break-words">{error}</div>
          )}
        </div>
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
