"use client";

import { useState, useEffect, useLayoutEffect, useMemo, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, Eye, List, Pencil, Trash2, Type, X } from "lucide-react";
import type { Card, CardDatasetSummary } from "@/lib/types";
import { isMobileApp, isTauri } from "@/lib/platform";
import MarkdownViewer from "@/components/wiki/markdown/markdown";
import { toHTML } from "@/components/wiki/markdown/html";
import { CollapsibleSidebar, useCollapsibleSidebar } from "@/components/layout/CollapsibleSidebar";
import {
  PageHeader,
  Toolbar,
  ToolbarButton,
  ToolbarMenuItem,
  ToolbarSelect,
} from "@/components/ui/toolbar";

type FontScaleKey = "normal" | "large" | "x2";

// Font size options: each step renders the content bigger and bolder.
const FONT_SCALE_MAP: Record<FontScaleKey, { zoom: number; weight: number }> = {
  normal: { zoom: 1, weight: 400 },
  large: { zoom: 1.3, weight: 500 },
  x2: { zoom: 1.6, weight: 600 },
};

const FONT_SCALE_LABELS: Record<FontScaleKey, string> = {
  normal: "Normal",
  large: "Large",
  x2: "Large x2",
};

// Familiarity tint ramp (theme-aware --fam-* tokens, see globals.css), shared
// with the Cards list rows. Literal class names keep Tailwind's scanner happy.
const FAM_CLASSES = [
  "bg-fam-0",
  "bg-fam-1",
  "bg-fam-2",
  "bg-fam-3",
  "bg-fam-4",
  "bg-fam-5",
  "bg-fam-6",
];

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

  // TOC drawer, mirroring the wiki page: the toolbar's TOC button toggles a
  // left drawer hosting the answer's heading tree (drag-resizable split column
  // on desktop, slide-in drawer on mobile). Starts closed; width persists.
  const tocSidebar = useCollapsibleSidebar({
    storageKey: "card-toc-width",
    defaultWidth: 240,
    minWidth: 180,
    initialVisible: false,
  });

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
  // renders bigger and bolder per level without overflowing the overlay.
  const scaleStyle: React.CSSProperties = {
    zoom: FONT_SCALE_MAP[fontScale].zoom,
    fontWeight: FONT_SCALE_MAP[fontScale].weight,
  };

  // Escape closes the overlay, like the Close toolbar button does. No dirty
  // guard: the overlay never had one, and discards stay as silent as before.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

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

  // Build the TOC node from the same content the body renders, so the
  // `#sec-...` anchors stay in sync (wiki pattern). Null when the answer has
  // no headings — that gates both the toolbar button and the drawer column.
  const tocNode = useMemo(
    () => (viewMode && answer.trim() ? toHTML(answer).toc : null),
    [viewMode, answer],
  );

  const inputCls =
    "w-full rounded-lg border border-border-default bg-bg-surface px-3 py-2 text-base";
  const labelCls = "block text-sm font-medium text-text-secondary mb-1.5";

  return (
    // Full-screen overlay with the unified page top-area pattern: a PageHeader
    // bar, a Toolbar action row directly beneath it, and a scrolling body.
    // On a phone the safe-area insets go on the root (same idiom as the mobile
    // app shell in (app)/home) so the bars sit below the status bar.
    <div
      className="fixed inset-0 z-50 flex flex-col bg-bg-body"
      style={
        mobile
          ? {
              paddingTop: "env(safe-area-inset-top)",
              paddingBottom: "env(safe-area-inset-bottom)",
            }
          : undefined
      }
    >
      <PageHeader
        title={viewMode ? "Card" : card?.uuid ? "Edit Card" : "New Card"}
        badge={
          <div className="flex items-center gap-1.5 shrink-0">
            <span
              className="text-[10px] px-1.5 py-0.5 rounded bg-mid-gray/20 text-text-tertiary max-w-[180px] truncate"
              title={selectedDatasetName || undefined}
            >
              {selectedDatasetName || "(unknown dataset)"}
            </span>
            {viewMode && card && (
              <span
                className={`text-[10px] px-1.5 py-0.5 rounded font-medium ${
                  FAM_CLASSES[Math.min(card.familiarity, 6)]
                }`}
                title={`Familiarity ${card.familiarity}`}
              >
                f={card.familiarity}
              </span>
            )}
          </div>
        }
      />

      {/* Toolbar — the viewer's actions live here, right-aligned, through the
          shared kit (TOC toggle, Font size picker, View/Edit mode toggle, Save,
          Delete, Close). */}
      <Toolbar mobile={mobile}>
        <div className="flex-1 min-w-0" />
        {viewMode && !!tocNode && (
          <ToolbarButton
            icon={List}
            variant="toggleSoft"
            active={tocSidebar.visible}
            label="TOC"
            onClick={tocSidebar.toggle}
            title={tocSidebar.visible ? "Hide table of contents" : "Show table of contents"}
          />
        )}
        <ToolbarSelect icon={Type} label="Font size" value={FONT_SCALE_LABELS[fontScale]}>
          {(Object.keys(FONT_SCALE_MAP) as FontScaleKey[]).map((k) => (
            <ToolbarMenuItem key={k} checked={fontScale === k} onClick={() => setFontScale(k)}>
              {FONT_SCALE_LABELS[k]}
            </ToolbarMenuItem>
          ))}
        </ToolbarSelect>
        {/* Mode switch as an action button: in view mode it offers Edit, in
            edit mode it offers View — the label names what the click does, not
            the mode you are in. */}
        <ToolbarButton
          icon={viewMode ? Pencil : Eye}
          label={viewMode ? "Edit" : "View"}
          title={viewMode ? "Switch to edit mode" : "Switch to view mode"}
          onClick={() => setViewMode((v) => !v)}
        />
        {!viewMode && (
          <ToolbarButton
            variant="solid"
            icon={Check}
            label="Save"
            disabled={!question.trim()}
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
          />
        )}
        {onDelete && (
          <ToolbarButton variant="danger" icon={Trash2} label="Delete" onClick={onDelete} />
        )}
        <ToolbarButton icon={X} label="Close" onClick={onClose} />
      </Toolbar>

      {viewMode ? (
        // Body row: the TOC drawer occupies the left (split column on desktop,
        // slide-in on mobile) and the card fills the rest — anchored under the
        // bars so it never covers the chrome that opens it (wiki layout).
        <div className="relative flex flex-1 min-h-0 overflow-hidden">
          {!!tocNode && (
            <CollapsibleSidebar sidebar={tocSidebar}>
              <div
                className="wiki-toc flex-1 min-h-0 overflow-y-auto px-3 py-2"
                onClick={(e) => {
                  // Mobile: picking a section closes the drawer so the anchor
                  // jump is visible; the desktop column stays put.
                  if ((e.target as HTMLElement).closest("a")) tocSidebar.closeOnSelect();
                }}
              >
                {tocNode}
              </div>
            </CollapsibleSidebar>
          )}
          <div className="flex-1 min-h-0 overflow-y-auto">
            <div className="mx-auto max-w-3xl px-4 py-6 sm:px-8" style={scaleStyle}>
              <div className="space-y-6">
                <h2 className="text-3xl font-semibold break-words">
                  {question || "(empty question)"}
                </h2>
                <div className="border-t border-border-default" />
                {answer.trim() ? (
                  <div className="px-4 py-3 rounded-lg border border-border-default bg-bg-surface">
                    <MarkdownViewer content={answer} />
                  </div>
                ) : (
                  <div className="text-text-tertiary italic">(no answer)</div>
                )}
                {suggestion.trim() && (
                  <div>
                    <div className="text-xs uppercase tracking-wide text-text-tertiary mb-2">
                      Suggestion
                    </div>
                    <div className="text-lg break-words">{suggestion}</div>
                  </div>
                )}
                {note.trim() && (
                  <div>
                    <div className="text-xs uppercase tracking-wide text-text-tertiary mb-2">
                      Note
                    </div>
                    <div className="px-4 py-3 rounded-lg border border-border-default bg-bg-surface">
                      <MarkdownViewer content={note} />
                    </div>
                  </div>
                )}
              </div>
            </div>
          </div>
        </div>
      ) : (
        <div className="flex-1 min-h-0 overflow-y-auto">
          <div className="mx-auto max-w-3xl px-4 py-6 sm:px-8" style={scaleStyle}>
            <div className="space-y-6">
              <div>
                <label className={labelCls}>Dataset</label>
                <select
                  value={selectedDataset}
                  onChange={(e) => setSelectedDataset(e.target.value)}
                  className={`${inputCls} cursor-pointer`}
                >
                  {datasets.map((ds) => (
                    <option key={ds.info.uuid} value={ds.info.uuid}>
                      {ds.info.name} ({ds.card_count} cards)
                    </option>
                  ))}
                </select>
              </div>
              <div>
                <label className={labelCls}>Question</label>
                <textarea
                  value={question}
                  onChange={(e) => setQuestion(e.target.value)}
                  rows={1}
                  className={`${inputCls} resize-y`}
                  placeholder="What do you want to learn?"
                />
              </div>
              <div>
                <label className={labelCls}>Answer</label>
                <textarea
                  ref={setAnswerRef}
                  value={answer}
                  onChange={(e) => setAnswer(e.target.value)}
                  className={`${inputCls} resize-none overflow-hidden min-h-[15rem]`}
                  placeholder="The answer or explanation (Markdown supported)"
                />
              </div>
              <div>
                <label className={labelCls}>Suggestion (optional)</label>
                <input
                  type="text"
                  value={suggestion}
                  onChange={(e) => setSuggestion(e.target.value)}
                  className={inputCls}
                  placeholder="AI suggestion or hint"
                />
              </div>
              <div>
                <label className={labelCls}>Note (optional)</label>
                <textarea
                  value={note}
                  onChange={(e) => setNote(e.target.value)}
                  rows={3}
                  className={`${inputCls} resize-y`}
                  placeholder="Additional notes (Markdown supported)"
                />
              </div>
            </div>
          </div>
        </div>
      )}
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
  isOpen: boolean;
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
    // Card datasets only exist inside the Tauri runtime. This provider also
    // wraps the public marketing site (`/`), so skip the invoke there instead of
    // logging a failed call on every web visit.
    if (!isTauri()) return;
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
    <CardEditorContext.Provider value={{ isOpen: state.isOpen, openCardEditor, closeCardEditor }}>
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
