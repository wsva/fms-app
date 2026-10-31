"use client";

// ---------------------------------------------------------------------------
// The four fields of a card, as one leaf.
//
// `EditableField` is the presentational half of a single field; this is the
// quartet — question, answer, suggestion, note, in reading order. It is the form
// half of a card: labels, a surface per field, an attach menu in each header.
// The other half is the reader's — the Cards page's breadcrumb level and the
// review panel both draw a card without a single label, at display size, and
// neither goes through this file. That split is deliberate: a card being edited
// and a card being read are two different objects, and a form is not a page.
// What they share is the composition — one panel per field, same border, same
// background as the reading level — so pressing Edit changes what the page does,
// not what it is.
//
// It owns geometry, not state. Values, which fields are open, dirtiness, the
// draft, save and the exit guard all stay with the host — a field that owned its
// own state could not tell the host "unsaved", and per-field edit and whole-card
// edit are the same mechanism seen from two ends.
//
// The seam is `CardFieldBinding`: everything the leaf reads and every turn it
// hands back, with nothing about how the host keeps any of it. `CardEditorContext`
// produces one from the overlay's own state in a dozen lines.
// ---------------------------------------------------------------------------

import { type CSSProperties } from "react";
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
  /** The font step, as inline style: the host owns the zoom, the leaf applies it
   *  to the value text only — labels and controls never scale with the type. */
  textStyle?: CSSProperties;
  /** A media operation is in flight: a blur must not close the field under it. */
  mediaBusy: boolean;
  onFieldChange: (field: CardMediaField, value: string) => void;
  /** Open one field for editing. The host focuses it. */
  onStartEdit: (field: CardMediaField) => void;
  /** Close one field — committed or cancelled is the host's distinction. */
  onEndEdit: (field: CardMediaField) => void;
  onAttach: (kind: CardMediaKind, field: CardMediaField) => void;
  /** Put a file into `field` through the picker (a preview's Upload). */
  onMediaUpload: (field: CardMediaField) => void;
  /** Remove one attachment, and the `{{handle}}` token that points at it. */
  onMediaRemove: (uuid: string) => void;
  /** Put one handle on the clipboard. */
  onMediaCopy: (handle: string) => void;
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

/** The field names on their own — the media view names the field an upload lands
 *  in, and the strip and this file must spell the four one way. */
export const CARD_FIELD_LABEL: Record<CardMediaField, string> = {
  question: "Question",
  answer: "Answer",
  suggestion: "Suggestion",
  note: "Note",
};

/** Per-field copy and typography: text, not state, so it lives here. */
const CHROME: Record<
  CardMediaField,
  {
    label: string;
    mode: "plain" | "markdown";
    singleLine?: boolean;
    placeholder: string;
    textClassName: string;
  }
> = {
  question: {
    label: CARD_FIELD_LABEL.question,
    mode: "plain",
    placeholder: "What do you want to learn?",
    // Same size as the answer: the question is the prompt, not a headline, and a
    // 3xl question in a form set every other field at half its weight.
    textClassName: "text-base font-semibold",
  },
  answer: {
    label: CARD_FIELD_LABEL.answer,
    mode: "markdown",
    placeholder: "The answer or explanation (Markdown supported)",
    textClassName: "text-base",
  },
  suggestion: {
    label: CARD_FIELD_LABEL.suggestion,
    mode: "plain",
    singleLine: true,
    placeholder: "AI suggestion or hint",
    textClassName: "text-lg",
  },
  note: {
    label: CARD_FIELD_LABEL.note,
    mode: "markdown",
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
      {CARD_FIELD_ORDER.map((field) => (
        <EditableField
          {...CHROME[field]}
          key={field}
          field={field}
          value={binding.values[field]}
          editing={binding.editing[field]}
          dirty={binding.dirty[field]}
          media={binding.media}
          datasetPath={binding.datasetPath}
          attachTarget={binding.attachField === field}
          dropTarget={binding.dragOver && binding.attachField === field}
          canAttach={canAttach}
          keepEditing={binding.mediaBusy}
          textStyle={binding.textStyle}
          onChange={(value) => binding.onFieldChange(field, value)}
          onStartEdit={() => binding.onStartEdit(field)}
          onEndEdit={() => binding.onEndEdit(field)}
          onAttach={(kind) => binding.onAttach(kind, field)}
          mediaTools={{
            upload: () => binding.onMediaUpload(field),
            remove: binding.onMediaRemove,
            copy: binding.onMediaCopy,
          }}
          registerElement={(el) => binding.onRegisterElement(field, el)}
        />
      ))}
    </div>
  );
}
