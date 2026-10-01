/**
 * Example: Using the global Card Editor from a context menu
 * 
 * This file demonstrates how to use the global card editor
 * from anywhere in the app, such as a context menu.
 */

"use client";

import { useCardEditor } from "@/components/cards/CardEditorContext";

export function ExampleContextMenu() {
  const { openCardEditor } = useCardEditor();

  // Example 1: Add selected text as a new card
  function handleAddToCard(selectedText: string, datasetUuid: string) {
    openCardEditor(
      datasetUuid,
      null, // null = new card
      (savedCard) => {
        console.log("Card saved:", savedCard);
        // Optionally refresh your data here
      }
    );
  }

  // Example 2: Pre-fill question with selected text
  function handleAddSelectedTextAsQuestion(
    selectedText: string,
    datasetUuid: string
  ) {
    openCardEditor(
      datasetUuid,
      {
        uuid: "",
        question: selectedText, // Pre-fill question
        answer: "",
        suggestion: "",
        note: "",
        familiarity: 0,
        question_hash: null,
        source_card_uuid: null,
        source_dataset_uuid: null,
        deleted_at: null,
        created_at: "",
        updated_at: "",
      },
      (savedCard) => {
        console.log("Card saved with pre-filled question:", savedCard);
      }
    );
  }

  // Example 3: Edit existing card
  function handleEditCard(cardUuid: string, datasetUuid: string) {
    // You would fetch the card first, then open editor
    // For this example, assume you have the card data
    const existingCard = {
      uuid: cardUuid,
      question: "Existing question",
      answer: "Existing answer",
      suggestion: "",
      note: "",
      familiarity: 3,
      question_hash: null,
      source_card_uuid: null,
      source_dataset_uuid: null,
      deleted_at: null,
      created_at: "",
      updated_at: "",
    };

    openCardEditor(datasetUuid, existingCard, (savedCard) => {
      console.log("Card updated:", savedCard);
    });
  }

  return (
    <div>
      <button
        onClick={() => handleAddToCard("Sample text", "your-dataset-uuid")}
        className="px-4 py-2 border rounded"
      >
        Add to Cards
      </button>
    </div>
  );
}
