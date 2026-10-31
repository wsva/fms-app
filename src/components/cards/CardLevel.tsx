"use client";

// ---------------------------------------------------------------------------
// A card as a place in the Cards page, not a thing on top of it.
//
// Clicking a row used to raise the editor overlay: a card had no address in the
// page, so the trail could only say `Cards › der Lernende` and the card itself
// was a modal you had to dismiss to get back to the list you came from. This
// level puts the body in the content area behind a third crumb, so a card is a
// step in the trail like a dataset is.
//
// It is the reading half only, on purpose. The overlay keeps the machinery it
// owns alone — the draft, the save, the exit guard, attachment, swap, delete —
// and `Edit` hands the same card back to it. That is what makes this level
// cheap to try: nothing was removed to build it, and a reader who wants to
// change something lands where the change has always been made.
//
// The body is `CardFields`, the same leaf the overlay renders, so the two hosts
// cannot drift: one rendering, one field order, and every open field is closed
// here because a field belongs to whatever owns the text.
// ---------------------------------------------------------------------------

import type { Card, CardMedia, CardMediaField } from "@/lib/types";
import CardFields, {
  type CardFieldBinding,
  type CardFieldValues,
} from "./CardFields";

/** No field is open, and none is dirty: this host renders, it does not edit. */
const CLOSED: Record<CardMediaField, boolean> = {
  question: false,
  answer: false,
  suggestion: false,
  note: false,
};

type Props = {
  card: Card;
  media: CardMedia[];
  /** Absolute dataset directory, for resolving attachment thumbnails. */
  datasetPath: string;
  /** A pencil on any field is the same door: the editor, on this card. The
   *  overlay has no field-level entry, so the field the pencil sat on is not
   *  passed on — every field opens there. */
  onEdit: () => void;
};

export default function CardLevel({ card, media, datasetPath, onEdit }: Props) {
  const values: CardFieldValues = {
    question: card.question,
    answer: card.answer,
    suggestion: card.suggestion,
    note: card.note,
  };

  // Every callback below is a no-op for a reason: an unfocused field cannot be
  // typed into, and a leaf that wants to change text, attach a file or register
  // an element is telling us it is open — which this level never allows.
  const binding: CardFieldBinding = {
    values,
    editing: CLOSED,
    dirty: CLOSED,
    media,
    datasetPath,
    attachField: null,
    dragOver: false,
    mediaBusy: false,
    // A click here selects a word, not a field: selecting text while reading is
    // most of what a reading half is for. The pencil opens the editor instead.
    clickToEdit: false,
    onFieldChange: () => {},
    onStartEdit: () => onEdit(),
    onEndEdit: () => {},
    onAttach: () => {},
    onRegisterElement: () => {},
  };

  return (
    <div className="flex-1 min-h-0 overflow-y-auto">
      <div className="mx-auto w-full max-w-3xl px-4 py-5 sm:px-6 sm:py-8">
        <CardFields binding={binding} />

        {/* What a list row can only imply: the deck's own familiarity score and
            when the card last changed, so a reader can tell a practised card from
            a fresh one without leaving it. */}
        <p className="mt-8 border-t border-border-default pt-3 text-[11px] text-text-tertiary select-none">
          familiarity {card.familiarity} · updated{" "}
          {card.updated_at.slice(0, 16).replace("T", " ")}
        </p>
      </div>
    </div>
  );
}
