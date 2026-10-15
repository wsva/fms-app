"use client";

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Plus,
  Pencil,
  Trash2,
  FilePlus,
  FileText,
  Wand2,
} from "lucide-react";
import { MicGlyph } from "@/components/voice/MicGlyph";
import type { ReadAloudMeta, ReadText } from "@/lib/read_aloud/types";
import { scoreBadgeClasses } from "@/lib/read_aloud/types";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import { logError } from "@/lib/logger";
import { tbGhost, tbDanger } from "@/lib/ui/toolbar";
import PracticePanel from "./PracticePanel";
import TextEditor from "./TextEditor";
import DatasetDialog from "./DatasetDialog";
import ConfirmDialog, { type ConfirmRequest } from "@/components/read_book/ConfirmDialog";

const LOG_MODULE = "read-aloud";

type DatasetDialogState = { mode: "create" } | { mode: "edit"; meta: ReadAloudMeta } | null;

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

  const [textEditor, setTextEditor] = useState<{ open: boolean; text: ReadText | null }>({
    open: false,
    text: null,
  });
  const [datasetDialog, setDatasetDialog] = useState<DatasetDialogState>(null);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);

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

  const toolbarLabel =
    viewMode === "datasets"
      ? `${datasets.length} dataset${datasets.length === 1 ? "" : "s"}`
      : viewMode === "texts"
        ? `${texts.length} text${texts.length === 1 ? "" : "s"}`
        : activeText?.title || "Untitled";

  const goDatasets = () => {
    setSelectedDataset("");
    setSelectedText(null);
  };
  const goTexts = () => setSelectedText(null);

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

  return (
    <main className="flex-1 flex flex-col h-full min-h-0 min-w-0 bg-bg-body">
      {/* Breadcrumb navigation — Read Aloud › dataset › text, mirroring the wiki
          page's edge-to-edge trail. Each level steps back out of the drilldown. */}
      <nav className="shrink-0 flex items-center justify-start gap-2 px-4 py-2 text-sm select-none min-w-0 overflow-x-auto border-b border-border-default bg-bg-card">
        <button
          className={`shrink-0 cursor-pointer hover:underline ${
            viewMode === "datasets" ? "text-text-primary font-medium" : "text-accent"
          }`}
          onClick={goDatasets}
        >
          Read Aloud
        </button>
        {activeDataset && (
          <>
            <span className="shrink-0 text-text-tertiary">&rsaquo;</span>
            <button
              className={`cursor-pointer hover:underline truncate min-w-0 max-w-[240px] ${
                viewMode === "practice" ? "text-accent" : "text-text-primary font-medium"
              }`}
              title={activeDataset.name}
              onClick={goTexts}
            >
              {activeDataset.name}
            </button>
          </>
        )}
        {viewMode === "practice" && activeText && (
          <>
            <span className="shrink-0 text-text-tertiary">&rsaquo;</span>
            <span
              className="text-text-primary font-medium truncate min-w-0 max-w-[320px]"
              title={activeText.title || "Untitled"}
            >
              {activeText.title || "Untitled"}
            </span>
          </>
        )}
      </nav>

      {/* Sub-toolbar: contextual actions for the current view — the same stacked
          bar treatment the wiki page uses. */}
      <div className="shrink-0 flex items-center gap-2 px-4 py-2 border-b border-border-default bg-bg-card">
        <div className="text-xs text-text-tertiary truncate flex-1 min-w-0">{toolbarLabel}</div>

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
              onClick={() => setTextEditor({ open: true, text: null })}
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
              className={tbGhost}
              title="Edit text"
              onClick={() => setTextEditor({ open: true, text: activeText })}
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
      </div>

      {/* Content — the practice pane manages its own scroll and fills the column,
          while the dataset/text lists scroll in a plain container. */}
      {viewMode === "practice" && activeDataset && activeText ? (
        <PracticePanel
          datasetUuid={activeDataset.uuid}
          text={activeText}
          onScored={() => loadTexts(selectedDataset)}
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
                className="group flex items-center gap-2 px-3 py-2 rounded-md text-left transition-colors hover:bg-bg-hover cursor-pointer"
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
                <div className="hidden group-hover:flex items-center gap-0.5 shrink-0">
                  <button
                    className="p-1 rounded text-text-tertiary hover:text-text-primary cursor-pointer"
                    title="Edit text"
                    onClick={(e) => {
                      e.stopPropagation();
                      setTextEditor({ open: true, text: t });
                    }}
                  >
                    <Pencil size={14} />
                  </button>
                  <button
                    className="p-1 rounded text-text-tertiary hover:text-error-text cursor-pointer"
                    title="Delete text"
                    onClick={(e) => {
                      e.stopPropagation();
                      deleteText(t);
                    }}
                  >
                    <Trash2 size={14} />
                  </button>
                </div>
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

      {textEditor.open && selectedDataset && (
        <TextEditor
          datasetUuid={selectedDataset}
          text={textEditor.text}
          newOrderNum={texts.length + 1}
          onClose={() => setTextEditor({ open: false, text: null })}
          onSaved={() => loadTexts(selectedDataset)}
        />
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
