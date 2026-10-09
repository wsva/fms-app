"use client";

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Plus,
  Pencil,
  Trash2,
  FolderPlus,
  BookText,
} from "lucide-react";
import { MicGlyph } from "@/components/voice/MicGlyph";
import type { ReadAloudMeta, ReadText } from "@/lib/read_aloud/types";
import { scoreBadgeClasses } from "@/lib/read_aloud/types";
import { isTauri } from "@/lib/tauri";
import { logError } from "@/lib/logger";
import {
  useCollapsibleSidebar,
  CollapsibleSidebar,
  SidebarToggleButton,
} from "@/components/layout/CollapsibleSidebar";
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
  // Shared slide-in sidebar: text list overlays the practice pane on desktop,
  // slides in as a drawer on mobile.
  const sidebar = useCollapsibleSidebar({ storageKey: "read-aloud-sidebar-width", defaultWidth: 288 });
  const mobile = sidebar.mobile;

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
        return list[0]?.uuid ?? "";
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
      setSelectedText((prev) => (prev && list.some((t) => t.uuid === prev) ? prev : list[0]?.uuid ?? null));
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

  const btnIcon =
    "p-1.5 rounded-md text-text-secondary hover:bg-bg-hover hover:text-text-primary cursor-pointer";

  // ── Text list (left pane / mobile list) ──────────────────────────
  const textList = (
    <div className="flex flex-col h-full min-h-0 min-w-0">
      <div className="flex items-center gap-2 p-3 border-b border-border-default shrink-0">
        <h2 className="text-sm font-semibold text-text-primary flex items-center gap-1.5 truncate">
          <BookText size={16} /> Texts
        </h2>
        <button
          className={`${btnIcon} ml-auto`}
          title="Add text"
          aria-label="Add text"
          disabled={!selectedDataset}
          onClick={() => setTextEditor({ open: true, text: null })}
        >
          <Plus size={18} />
        </button>
      </div>
      <div className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden p-2 flex flex-col gap-1">
        {!selectedDataset ? (
          <p className="text-xs text-text-tertiary p-2">Create or select a dataset to begin.</p>
        ) : texts.length === 0 ? (
          <p className="text-xs text-text-tertiary p-2">
            No texts yet. Tap <Plus size={12} className="inline" /> to add one.
          </p>
        ) : (
          texts.map((t) => (
            <div
              key={t.uuid}
              className={`group flex items-center gap-2 p-2 rounded-lg cursor-pointer transition-colors ${
                selectedText === t.uuid ? "bg-accent-bg/15" : "hover:bg-bg-hover"
              }`}
              onClick={() => {
                setSelectedText(t.uuid);
                sidebar.closeOnSelect();
              }}
            >
              <div className="flex flex-col min-w-0 flex-1">
                <span className="text-sm text-text-primary truncate">
                  {t.title || "Untitled"}
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
    </div>
  );

  // ── Practice pane (right) ────────────────────────────────────────
  const practice = activeDataset && activeText ? (
    <PracticePanel
      datasetUuid={activeDataset.uuid}
      text={activeText}
      onScored={() => loadTexts(selectedDataset)}
    />
  ) : (
    <div className="flex-1 flex items-center justify-center p-6 text-center">
      <p className="text-sm text-text-tertiary max-w-xs">
        {selectedDataset
          ? "Select a text on the left, then record yourself reading it. You'll be scored against the reference and earn XP at 60% or better."
          : "Create a read-aloud dataset to get started."}
      </p>
    </div>
  );

  return (
    <main className="flex-1 flex flex-col h-full min-h-0 min-w-0">
      {/* Header */}
      <div className="p-3 border-b border-border-default flex items-center gap-2 shrink-0 min-w-0">
        <h1 className="text-lg font-semibold flex items-center gap-2 text-text-primary shrink-0">
          <MicGlyph size={20} />{!mobile && " Read Aloud"}
        </h1>
        <SidebarToggleButton sidebar={sidebar} title="Show/hide text list" />

        <select
          className="ml-2 min-w-0 flex-1 max-w-xs p-1.5 rounded-md bg-bg-muted border border-border-default text-text-primary text-sm cursor-pointer"
          value={selectedDataset}
          onChange={(e) => {
            setSelectedDataset(e.target.value);
            setSelectedText(null);
          }}
        >
          {datasets.length === 0 && <option value="">No datasets</option>}
          {datasets.map((d) => (
            <option key={d.uuid} value={d.uuid}>
              {d.name}
            </option>
          ))}
        </select>

        <div className="flex items-center gap-1 shrink-0">
          <button className={btnIcon} title="New dataset" aria-label="New dataset" onClick={() => setDatasetDialog({ mode: "create" })}>
            <FolderPlus size={18} />
          </button>
          <button
            className={btnIcon}
            title="Edit dataset"
            aria-label="Edit dataset"
            disabled={!activeDataset}
            onClick={() => activeDataset && setDatasetDialog({ mode: "edit", meta: activeDataset })}
          >
            <Pencil size={18} />
          </button>
          <button
            className={`${btnIcon} hover:text-error-text`}
            title="Delete dataset"
            aria-label="Delete dataset"
            disabled={!activeDataset}
            onClick={() => activeDataset && deleteDataset(activeDataset)}
          >
            <Trash2 size={18} />
          </button>
        </div>
      </div>

      {/* Body — text list is a split column on desktop / slide-in drawer on
          mobile; the practice pane takes the remaining width */}
      <div className="relative flex-1 min-h-0 flex overflow-hidden min-w-0">
        <CollapsibleSidebar sidebar={sidebar}>{textList}</CollapsibleSidebar>
        {practice}
      </div>

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
