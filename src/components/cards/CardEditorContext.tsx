"use client";

import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Card } from "@/lib/types";

function CardEditModal({
  card,
  onSave,
  onDelete,
  onClose,
}: {
  card: Card | null;
  onSave: (card: Partial<Card>) => void;
  onDelete?: () => void;
  onClose: () => void;
}) {
  const [question, setQuestion] = useState(card?.question || "");
  const [answer, setAnswer] = useState(card?.answer || "");
  const [note, setNote] = useState(card?.note || "");
  const [suggestion, setSuggestion] = useState(card?.suggestion || "");

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="bg-bg-card rounded-xl border border-border-default p-8 w-[80vw] h-[80vh] flex flex-col">
        {/* Header with title and buttons */}
        <div className="flex items-center justify-between mb-4">
          <h2 className="text-xl font-semibold">
            {card ? "Edit Card" : "New Card"}
          </h2>
          <div className="flex items-center gap-2">
            {onDelete && (
              <button
                onClick={onDelete}
                className="px-4 py-2 rounded-lg bg-red-500/10 text-red-500 text-sm hover:bg-red-500/20"
              >
                Delete
              </button>
            )}
            <button
              onClick={onClose}
              className="px-4 py-2 rounded-lg border border-border-default text-sm"
            >
              Cancel
            </button>
            <button
              onClick={() =>
                onSave({
                  uuid: card?.uuid || "",
                  question,
                  answer,
                  note,
                  suggestion,
                  familiarity: card?.familiarity || 0,
                })
              }
              disabled={!question.trim()}
              className="px-4 py-2 rounded-lg border border-border-default text-sm hover:bg-mid-gray/20 disabled:opacity-50"
            >
              Save
            </button>
          </div>
        </div>

        <div className="flex-1 overflow-y-auto space-y-6">
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
              value={answer}
              onChange={(e) => setAnswer(e.target.value)}
              rows={6}
              className="w-full px-4 py-3 rounded-lg border border-border-default bg-bg-surface text-2xl"
              placeholder="The answer or explanation"
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
              placeholder="Additional notes"
            />
          </div>
        </div>
      </div>
    </div>
  );
}

export type CardEditorState = {
  isOpen: boolean;
  card: Card | null;
  datasetUuid: string;
  onSave?: (card: Card) => void;
  onDelete?: () => void;
};

export type CardEditorContextType = {
  openCardEditor: (
    datasetUuid: string,
    card?: Card | null,
    onSave?: (card: Card) => void,
    onDelete?: () => void
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
  });

  function openCardEditor(
    datasetUuid: string,
    card?: Card | null,
    onSave?: (card: Card) => void,
    onDelete?: () => void
  ) {
    setState({
      isOpen: true,
      card: card || null,
      datasetUuid,
      onSave,
      onDelete,
    });
  }

  function closeCardEditor() {
    setState({
      isOpen: false,
      card: null,
      datasetUuid: "",
    });
  }

  async function handleSave(cardData: Partial<Card>) {
    if (!state.datasetUuid) return;

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
        datasetUuid: state.datasetUuid,
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
          onSave={handleSave}
          onDelete={state.card?.uuid ? handleDelete : undefined}
          onClose={closeCardEditor}
        />
      )}
    </CardEditorContext.Provider>
  );
}
