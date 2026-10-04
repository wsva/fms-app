"use client";

import { useState, useEffect, useLayoutEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, Eye, Pencil, Trash2, Type, X } from "lucide-react";
import type { Card, CardDatasetSummary } from "@/lib/types";
import { isMobileApp } from "@/lib/platform";
import MarkdownViewer from "@/components/wiki/markdown/markdown";
import "./CardEditor.css";

type FontScaleKey = "normal" | "large" | "x2";

// Font size options: each step renders the content bigger and bolder.
const FONT_SCALE_MAP: Record<FontScaleKey, { zoom: number; weight: number }> = {
  normal: { zoom: 1, weight: 400 },
  large: { zoom: 1.3, weight: 500 },
  x2: { zoom: 1.6, weight: 600 },
};

function CardEditModal({
  card,
  datasetUuid,
  datasets,
  onSave,
  onDelete,
  onClose,
  initialViewMode,
}: {
  card: Card | null;
  datasetUuid: string;
  datasets: CardDatasetSummary[];
  onSave: (datasetUuid: string, card: Partial<Card>) => void;
  onDelete?: () => void;
  onClose: () => void;
  initialViewMode?: boolean;
}) {
  const [selectedDataset, setSelectedDataset] = useState(datasetUuid);
  const [question, setQuestion] = useState(card?.question || "");
  const [answer, setAnswer] = useState(card?.answer || "");
  const [note, setNote] = useState(card?.note || "");
  const [suggestion, setSuggestion] = useState(card?.suggestion || "");
  // Editing an existing card defaults to view mode; new cards start in edit mode.
  // If initialViewMode is explicitly provided, use that instead.
  const [viewMode, setViewMode] = useState(
    initialViewMode !== undefined ? initialViewMode : !!card?.uuid
  );
  const [fontScale, setFontScale] = useState<FontScaleKey>("normal");
  // Deferred platform detection (SSR-safe): isMobileApp() must not run during render.
  const [mobile, setMobile] = useState(false);
  const answerRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    setMobile(isMobileApp());
  }, []);

  // Callback ref that resizes the textarea when it's mounted
  const setAnswerRef = useCallback((el: HTMLTextAreaElement | null) => {
    answerRef.current = el;
    if (el) {
      el.style.height = "0px";
      el.style.height = `${el.scrollHeight}px`;
    }
  }, []);

  // Applied to an inner content wrapper (not the scroll container) so text
  // renders bigger and bolder per level without overflowing the modal.
  const scaleStyle: React.CSSProperties = {
    zoom: FONT_SCALE_MAP[fontScale].zoom,
    fontWeight: FONT_SCALE_MAP[fontScale].weight,
  };

  // Auto-resize the answer textarea to fit its content.
  // useLayoutEffect ensures it runs on initial mount before paint.
  useLayoutEffect(() => {
    const el = answerRef.current;
    if (!el) return;
    el.style.height = "0px"; // Reset to force recalculation of actual content height
    el.style.height = `${el.scrollHeight}px`;
  }, [answer]);

  const selectedDatasetName =
    datasets.find((ds) => ds.info.uuid === selectedDataset)?.info.name || "";

  // Compact square icon button used for the header actions on narrow screens.
  const iconBtn = "p-1.5 rounded-lg border flex items-center justify-center";
  const iconSize = 16;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div
        className={`relative bg-bg-card border border-border-default flex flex-col ${
          mobile
            ? "rounded-none w-full h-full px-4 pt-[max(1rem,env(safe-area-inset-top))] pb-[max(1rem,env(safe-area-inset-bottom))]"
            : "rounded-xl p-8 w-[80vw] h-[80vh]"
        }`}
      >
        {/* Close button (top-right corner, desktop only — on mobile it joins the header row) */}
        {!mobile && (
          <button
            onClick={onClose}
            className="absolute top-4 right-4 p-1.5 rounded-lg text-text-tertiary hover:text-text-primary hover:bg-mid-gray/20 transition-colors"
            title="Close"
          >
            <X size={20} />
          </button>
        )}
        {/* Header with title and buttons */}
        <div className={`flex items-center justify-between ${mobile ? "flex-wrap gap-y-2" : ""} mb-4`}>
          <h2 className={`${mobile ? "text-lg" : "text-xl"} font-semibold min-w-0 truncate`}>
            {viewMode ? "View Card" : card ? "Edit Card" : "New Card"}
          </h2>
          <div className={`flex items-center gap-2 ${mobile ? "" : "pr-10"}`}>
            <div
              className="flex items-center gap-1.5 px-2 rounded-lg border border-border-default bg-bg-surface"
              title="Font size"
            >
              <Type size={15} className="text-text-tertiary shrink-0" />
              <select
                value={fontScale}
                onChange={(e) => setFontScale(e.target.value as FontScaleKey)}
                className={`${mobile ? "py-1" : "py-2"} pr-1 bg-transparent text-sm cursor-pointer outline-none text-text-primary`}
              >
                <option value="normal">Normal</option>
                <option value="large">Large</option>
                <option value="x2">Large x2</option>
              </select>
            </div>
            <button
              onClick={() => setViewMode((v) => !v)}
              className={mobile ? `${iconBtn} border-border-default hover:bg-mid-gray/20` : "px-4 py-2 rounded-lg border border-border-default text-sm hover:bg-mid-gray/20 flex items-center gap-2"}
              title={viewMode ? "Switch to edit mode" : "Switch to view mode"}
              aria-label={viewMode ? "Edit" : "View"}
            >
              {viewMode ? (
                <Pencil size={mobile ? iconSize : 14} />
              ) : (
                <Eye size={mobile ? iconSize : 14} />
              )}
              {!mobile && (viewMode ? "Edit" : "View")}
            </button>
            {onDelete && (
              <button
                onClick={onDelete}
                className={
                  mobile
                    ? `${iconBtn} border-red-500/20 bg-red-500/10 text-red-500 hover:bg-red-500/20`
                    : "px-4 py-2 rounded-lg bg-red-500/10 text-red-500 text-sm hover:bg-red-500/20"
                }
                title="Delete"
                aria-label="Delete"
              >
                {mobile ? <Trash2 size={iconSize} /> : "Delete"}
              </button>
            )}
            {!viewMode && (
              <button
                onClick={() =>
                  onSave(selectedDataset, {
                    uuid: card?.uuid || "",
                    question,
                    answer,
                    note,
                    suggestion,
                    familiarity: card?.familiarity || 0,
                  })
                }
                disabled={!question.trim()}
                className={`${
                  mobile
                    ? `${iconBtn} border-accent/20 bg-accent-bg/20 text-accent hover:bg-accent-bg/30`
                    : "px-4 py-2 rounded-lg border border-border-default text-sm hover:bg-mid-gray/20"
                } disabled:opacity-50`}
                title="Save"
                aria-label="Save"
              >
                {mobile ? <Check size={iconSize} /> : "Save"}
              </button>
            )}
            {mobile && (
              <button
                onClick={onClose}
                className={`${iconBtn} border-border-default text-text-tertiary hover:text-text-primary hover:bg-mid-gray/20`}
                title="Close"
                aria-label="Close"
              >
                <X size={iconSize} />
              </button>
            )}
          </div>
        </div>

        {viewMode ? (
          <div className="flex-1 overflow-y-auto">
            <div className="space-y-6" style={scaleStyle}>
            <div>
              <div className="text-sm text-text-tertiary mb-2">
                Dataset: {selectedDatasetName || "(unknown)"}
              </div>
            </div>
            <div>
              <div className="text-3xl font-semibold">
                {question || "(empty question)"}
              </div>
            </div>
            <div>
              {answer.trim() ? (
                <div className="card-md relative px-4 py-3 rounded-lg border border-border-default bg-bg-surface">
                  <MarkdownViewer content={answer} withTOC={!mobile} />
                </div>
              ) : (
                <div className="text-text-tertiary italic">(no answer)</div>
              )}
            </div>
            {suggestion.trim() && (
              <div>
                <div className="text-sm text-text-tertiary mb-2">Suggestion</div>
                <div className="text-lg">{suggestion}</div>
              </div>
            )}
            {note.trim() && (
              <div>
                <div className="text-sm text-text-tertiary mb-2">Note</div>
                <div className="px-4 py-3 rounded-lg border border-border-default bg-bg-surface">
                  <MarkdownViewer content={note} />
                </div>
              </div>
            )}
            </div>
          </div>
        ) : (
          <div className="flex-1 overflow-y-auto">
            <div className="space-y-6" style={scaleStyle}>
            <div>
              <label className="block text-lg font-medium mb-2">Dataset</label>
              <select
                value={selectedDataset}
                onChange={(e) => setSelectedDataset(e.target.value)}
                className="w-full px-4 py-3 rounded-lg border border-border-default bg-bg-surface text-lg"
              >
                {datasets.map((ds) => (
                  <option key={ds.info.uuid} value={ds.info.uuid}>
                    {ds.info.name} ({ds.card_count} cards)
                  </option>
                ))}
              </select>
            </div>
            <div>
              <label className="block text-lg font-medium mb-2">Question</label>
              <textarea
                value={question}
                onChange={(e) => setQuestion(e.target.value)}
                rows={1}
                className="w-full px-4 py-3 rounded-lg border border-border-default bg-bg-surface text-2xl"
                placeholder="What do you want to learn?"
              />
            </div>
            <div>
              <label className="block text-lg font-medium mb-2">Answer</label>
              <textarea
                ref={setAnswerRef}
                value={answer}
                onChange={(e) => setAnswer(e.target.value)}
                className="w-full px-4 py-3 rounded-lg border border-border-default bg-bg-surface text-2xl resize-none overflow-hidden min-h-[15rem]"
                placeholder="The answer or explanation (Markdown supported)"
              />
            </div>
            <div>
              <label className="block text-lg font-medium mb-2">
                Suggestion (optional)
              </label>
              <input
                type="text"
                value={suggestion}
                onChange={(e) => setSuggestion(e.target.value)}
                className="w-full px-4 py-3 rounded-lg border border-border-default bg-bg-surface text-2xl"
                placeholder="AI suggestion or hint"
              />
            </div>
            <div>
              <label className="block text-lg font-medium mb-2">
                Note (optional)
              </label>
              <textarea
                value={note}
                onChange={(e) => setNote(e.target.value)}
                rows={3}
                className="w-full px-4 py-3 rounded-lg border border-border-default bg-bg-surface text-2xl"
                placeholder="Additional notes (Markdown supported)"
              />
            </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

export type CardEditorState = {
  isOpen: boolean;
  card: Card | null;
  datasetUuid: string;
  datasetName?: string;
  datasets: CardDatasetSummary[];
  onSave?: (card: Card) => void;
  onDelete?: () => void;
  initialViewMode?: boolean;
};

export type CardEditorContextType = {
  openCardEditor: (
    datasetUuid: string,
    card?: Card | null,
    onSave?: (card: Card) => void,
    onDelete?: () => void,
    datasetName?: string,
    initialViewMode?: boolean
  ) => void;
  closeCardEditor: () => void;
};

import { createContext, useContext } from "react";

const CardEditorContext = createContext<CardEditorContextType | null>(null);

export function useCardEditor() {
  const context = useContext(CardEditorContext);
  if (!context) {
    throw new Error("useCardEditor must be used within CardEditorProvider");
  }
  return context;
}

export function CardEditorProvider({
  children,
}: {
  children: React.ReactNode;
}) {
  const [state, setState] = useState<CardEditorState>({
    isOpen: false,
    card: null,
    datasetUuid: "",
    datasets: [],
  });

  // Load datasets on mount
  useEffect(() => {
    loadDatasets();
  }, []);

  async function loadDatasets() {
    try {
      const result = await invoke<CardDatasetSummary[]>("card_dataset_list");
      setState((prev) => ({ ...prev, datasets: result }));
    } catch (e) {
      console.error("Failed to load datasets:", e);
    }
  }

  function openCardEditor(
    datasetUuid: string,
    card?: Card | null,
    onSave?: (card: Card) => void,
    onDelete?: () => void,
    datasetName?: string,
    initialViewMode?: boolean
  ) {
    // Reload datasets to ensure we have the latest card counts
    loadDatasets().then(() => {
      setState((prev) => {
        // Check if the dataset is already in the list
        const datasetExists = prev.datasets.some((ds) => ds.info.uuid === datasetUuid);
        
        // If dataset doesn't exist but we have a name, add a placeholder
        let datasets = prev.datasets;
        if (!datasetExists && datasetName && datasetUuid) {
          datasets = [
            { info: { uuid: datasetUuid, name: datasetName }, card_count: 0 } as CardDatasetSummary,
            ...prev.datasets,
          ];
        }
        
        return {
          ...prev,
          isOpen: true,
          card: card || null,
          datasetUuid,
          datasetName,
          datasets,
          onSave,
          onDelete,
          initialViewMode,
        };
      });
    });
  }

  function closeCardEditor() {
    setState((prev) => ({
      ...prev,
      isOpen: false,
      card: null,
      datasetUuid: "",
    }));
  }

  async function handleSave(datasetUuid: string, cardData: Partial<Card>) {
    if (!datasetUuid) return;

    try {
      const fullCard: Card = {
        uuid: cardData.uuid || "",
        question: cardData.question || "",
        suggestion: cardData.suggestion || "",
        answer: cardData.answer || "",
        note: cardData.note || "",
        familiarity: cardData.familiarity || 0,
        question_hash: null,
        source_card_uuid: cardData.source_card_uuid || null,
        source_dataset_uuid: cardData.source_dataset_uuid || null,
        deleted_at: null,
        created_at: "",
        updated_at: "",
      };

      const savedCard = await invoke<Card>("card_save", {
        datasetUuid,
        card: fullCard,
      });

      if (state.onSave) {
        state.onSave(savedCard);
      }

      closeCardEditor();
    } catch (e) {
      console.error("Failed to save card:", e);
      alert(`Failed to save card: ${e}`);
    }
  }

  async function handleDelete() {
    if (!state.card || !state.datasetUuid) return;

    try {
      await invoke("card_delete", {
        datasetUuid: state.datasetUuid,
        cardUuid: state.card.uuid,
      });

      if (state.onDelete) {
        state.onDelete();
      }

      closeCardEditor();
    } catch (e) {
      console.error("Failed to delete card:", e);
      alert(`Failed to delete card: ${e}`);
    }
  }

  return (
    <CardEditorContext.Provider value={{ openCardEditor, closeCardEditor }}>
      {children}
      {state.isOpen && (
        <CardEditModal
          card={state.card}
          datasetUuid={state.datasetUuid}
          datasets={state.datasets}
          onSave={handleSave}
          onDelete={state.card?.uuid ? handleDelete : undefined}
          onClose={closeCardEditor}
          initialViewMode={state.initialViewMode}
        />
      )}
    </CardEditorContext.Provider>
  );
}
