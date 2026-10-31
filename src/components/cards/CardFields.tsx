"use client";

// ---------------------------------------------------------------------------
// The four fields of a card, as one leaf.
//
// `EditableField` is the presentational half of a single field; this is the
// quartet — question, answer, suggestion, note, in reading order, with the rule
// under the question that separates the prompt from the prose. It exists
// because the overlay is not the only place a card is read: the Cards page opens
// one inside its own breadcrumb level (`CardLevel`), with the page's toolbar
// rather than the overlay's.
//
// It owns geometry, not state. Values, which fields are open, dirtiness, the
// draft, save and the exit guard all stay with the host — a field that owned its
// own state could not tell the host "unsaved", and per-field edit and whole-card
// edit are the same mechanism seen from two ends.
//
// The seam is `CardFieldBinding`: everything the leaf reads and every turn it
// hands back, with nothing about how the host keeps any of it. The overlay
// produces one from its own state in a dozen lines (`CardEditorContext`); the
// card level produces a read-only one, and neither copy can drift from the
// other's rendering because there is only one rendering.
// ---------------------------------------------------------------------------

import { Fragment } from "react";
import type { CardMedia, CardMediaField } from "@/lib/types";
import { isTauri } from "@/lib/platform";
import EditableField from "./EditableField";

/** The four editable values of a card. */
export type CardFieldValues = Record<CardMediaField, string>;

/** The attachment kinds a field accepts, as the toolbar offers them. */
export type CardMediaKind = "image" | "audio" | "video";

/** Everything the quartet needs, and nothing about where it comes from. */
export type CardFieldBinding = {
  values: CardFieldValues;
  /** Per field: is its textarea open. */
  editing: Record<CardMediaField, boolean>;
  /** Per field: does its text differ from what was read or last saved. */
  dirty: Record<CardMediaField, boolean>;
  /** All of the card's attachments; each field shows its own. */
  media: CardMedia[];
  /** Absolute dataset directory, for resolving attachment thumbnails. */
  datasetPath: string;
  /** Where a new attachment or a Library "Place" lands. `null` when no field
   *  is open — a read-only host has no landing place, and the field it would
   *  otherwise mark would claim "attachments land here" while offering none. */
  attachField: CardMediaField | null;
  /** A file is being dragged over the window and would land in `attachField`. */
  dragOver: boolean;
  /** A media operation is in flight: a blur must not close the field under it. */
  mediaBusy: boolean;
  /** Does clicking a field's text open it? Default true. A read-only host says
   *  false — there, a click selects text and the pencil is the way in. */
  clickToEdit?: boolean;
  onFieldChange: (field: CardMediaField, value: string) => void;
  /** Open one field for editing. The host focuses it. */
  onStartEdit: (field: CardMediaField) => void;
  /** Close one field — committed or cancelled is the host's distinction. */
  onEndEdit: (field: CardMediaField) => void;
  onAttach: (kind: CardMediaKind, field: CardMediaField) => void;
  /** Hand the mounted editor element back, so the host can insert a token. */
  onRegisterElement: (
    field: CardMediaField,
    el: HTMLTextAreaElement | HTMLInputElement | null
  ) => void;
};

/** The reading order, which is also the order the vector and the API use. */
export const CARD_FIELD_ORDER: CardMediaField[] = [
  "question",
  "answer",
  "suggestion",
  "note",
];

/** Per-field copy and typography: text, not state, so it lives here. */
const CHROME: Record<
  CardMediaField,
  {
    label: string;
    mode: "plain" | "markdown";
    boxed?: boolean;
    singleLine?: boolean;
    placeholder: string;
    textClassName: string;
  }
> = {
  question: {
    label: "Question",
    mode: "plain",
    placeholder: "What do you want to learn?",
    textClassName: "text-3xl font-semibold",
  },
  answer: {
    label: "Answer",
    mode: "markdown",
    boxed: true,
    placeholder: "The answer or explanation (Markdown supported)",
    textClassName: "text-base",
  },
  suggestion: {
    label: "Suggestion",
    mode: "plain",
    singleLine: true,
    placeholder: "AI suggestion or hint",
    textClassName: "text-lg",
  },
  note: {
    label: "Note",
    mode: "markdown",
    boxed: true,
    placeholder: "Additional notes (Markdown supported)",
    textClassName: "text-base",
  },
};

export default function CardFields({ binding }: { binding: CardFieldBinding }) {
  // Desktop only: the file pickers need the Tauri dialog plugin, and a phone
  // has no way to hand a field a local file.
  const canAttach = isTauri();

  return (
    <div className="space-y-6">
      {CARD_FIELD_ORDER.map((field, index) => (
        <Fragment key={field}>
          {/* The prompt and the prose are different things: one rule does that
              job, and it belongs to the order, not to either field. */}
          {index === 1 && <div className="border-t border-border-default" />}
          <EditableField
            {...CHROME[field]}
            field={field}
            value={binding.values[field]}
            editing={binding.editing[field]}
            dirty={binding.dirty[field]}
            media={binding.media}
            datasetPath={binding.datasetPath}
            attachTarget={binding.attachField === field}
            dropTarget={binding.dragOver && binding.attachField === field}
            canAttach={canAttach}
            clickToEdit={binding.clickToEdit ?? true}
            keepEditing={binding.mediaBusy}
            onChange={(value) => binding.onFieldChange(field, value)}
            onStartEdit={() => binding.onStartEdit(field)}
            onEndEdit={() => binding.onEndEdit(field)}
            onAttach={(kind) => binding.onAttach(kind, field)}
            registerElement={(el) => binding.onRegisterElement(field, el)}
          />
        </Fragment>
      ))}
    </div>
  );
}
