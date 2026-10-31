"use client";

//! Per-card media, shown in the editor's media view.
//!
//! This is the browse feature the design calls out: the card's own media,
//! listed from `card_media_list` (never the raw directory). Each row can be
//! reordered within its field's tail strip (the `sort` column), copied as a
//! `{{handle}}` to paste into any field, or removed. Nothing here re-uploads
//! bytes — Upload is the one action that adds a row, and it is the same
//! `card_media_add` the toolbar's Attach calls.
//!
//! Every row shows the media itself, not just its handle: a name cannot tell a
//! listener whether this is the right clip, and the whole reason to open this
//! view is to answer "what is this file". The preview is the same `MediaBlock`
//! the card uses, so a picture zooms and a clip plays exactly as it does in the
//! field — with `reveal` off, because a list of ten autoplaying rows is not a
//! preview.

import type { CardMedia } from "@/lib/types";
import { Clipboard, Trash2, ChevronUp, ChevronDown, Upload } from "lucide-react";
import MediaBlock from "./MediaBlock";

type Props = {
  media: CardMedia[];
  /** Absolute dataset directory, for resolving a row's bytes into a preview. */
  datasetPath: string;
  /** Rows whose bytes are missing on this device (present === false), by handle. */
  missingHandles: Set<string>;
  /**
   * Rows that are written to the table but whose `{{handle}}` token sits in no
   * field — attached, not placed. A media write is durable the moment it
   * happens; the text token that points at it only survives if the card is
   * saved. Naming the gap here is what keeps that drift from reading as a bug.
   */
  unplacedHandles: Set<string>;
  /** Desktop only: the file picker needs the Tauri dialog plugin. */
  canUpload: boolean;
  /** The field an upload lands in, by its UI name — "answer", "note"… Shown in
   *  the Upload tooltip so the one thing the button decides is not a secret. */
  uploadTarget: string;
  /** Add a file to `uploadTarget`. */
  onUpload: () => void;
  /** Put a row's `{{handle}}` on the clipboard, ready to paste into a field. */
  onCopy: (handle: string) => void;
  onMove: (uuid: string, dir: -1 | 1) => void;
  onDelete: (uuid: string) => void;
};

export default function MediaLibrary({
  media,
  datasetPath,
  missingHandles,
  unplacedHandles,
  canUpload,
  uploadTarget,
  onUpload,
  onCopy,
  onMove,
  onDelete,
}: Props) {
  // The one action that adds a row, kept at the head of the list rather than in
  // a row: an upload has no row yet, and the field it lands in is the placement
  // target chosen above.
  const upload = canUpload ? (
    <button
      type="button"
      onClick={onUpload}
      title={`Add an image, audio, or video to the ${uploadTarget} field`}
      className="flex items-center gap-1.5 rounded-md border border-border-default px-2.5 py-1 text-xs text-text-secondary hover:bg-bg-hover hover:text-text-primary"
    >
      <Upload size={13} />
      Upload
    </button>
  ) : null;

  if (media.length === 0) {
    return (
      <div className="space-y-3">
        {upload}
        {/* A panel in a pane rather than a line in a 288px sidebar: name the one
            action that fills it, and the second path a desktop reader has (a
            window drop lands in the field named above this list). */}
        <div className="rounded-xl border border-dashed border-border-default px-4 py-10 text-center text-sm text-text-tertiary">
          {canUpload
            ? `No media yet. Upload a file, or drop one onto the window — it lands in the ${uploadTarget} field.`
            : "No media yet. Files added on a desktop show up here."}
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-2">
      {upload}
      <div className="flex flex-col gap-1.5">
        {media.map((m, i) => {
          const missing = missingHandles.has(m.handle);
          const unplaced = unplacedHandles.has(m.handle);
          return (
            <div
              key={m.uuid}
              className="rounded-lg border border-border-default bg-bg-surface px-2.5 py-2 flex flex-col gap-1.5"
            >
              {/* The file itself, capped so a tall screenshot cannot push the
                  next row off the pane and a long video keeps its controls in
                  reach. A missing row falls back to MediaBlock's own "not on
                  this device" placeholder — the one thing a preview cannot
                  show is bytes that are not here. */}
              <div className="overflow-hidden rounded-md [&_img]:max-h-44 [&_video]:max-h-44">
                <MediaBlock
                  media={m}
                  handle={m.handle}
                  missing={missing}
                  datasetPath={datasetPath}
                  reveal={false}
                  zoomable
                />
              </div>
              {/* No handle, no field chip: the preview above already says what
                  the file is, and the slug is an implementation detail the writer
                  pastes rather than reads. What is left is the caption and the two
                  states a preview cannot show — bytes that are not here, and a row
                  no field points at. */}
              {(m.title || missing || unplaced) && (
                <div className="flex min-w-0 items-center gap-2">
                  {m.title && (
                    <span className="min-w-0 truncate text-xs text-text-secondary">{m.title}</span>
                  )}
                  {missing && (
                    <span className="shrink-0 text-[10px] text-error-text" title="bytes not on this device">
                      missing
                    </span>
                  )}
                  {unplaced && !missing && (
                    <span
                      className="shrink-0 text-[10px] text-warning-text"
                      title="saved with the card, but no field holds its {{handle}} yet"
                    >
                      not placed
                    </span>
                  )}
                </div>
              )}
              <div className="flex items-center gap-1">
                <button
                  type="button"
                  onClick={() => onMove(m.uuid, -1)}
                  disabled={i === 0}
                  title="Move up"
                  className="p-1.5 rounded hover:bg-bg-hover text-text-secondary disabled:opacity-30"
                >
                  <ChevronUp size={14} />
                </button>
                <button
                  type="button"
                  onClick={() => onMove(m.uuid, 1)}
                  disabled={i === media.length - 1}
                  title="Move down"
                  className="p-1.5 rounded hover:bg-bg-hover text-text-secondary disabled:opacity-30"
                >
                  <ChevronDown size={14} />
                </button>
                <div className="flex-1" />
                {/* The two actions that do something you cannot see: copy puts the
                    handle where a field can take it, remove takes the file away. Both
                    carry their verb — an icon pair at the far right of a wide row had
                    to be hovered to be identified. */}
                <button
                  type="button"
                  onClick={() => onCopy(m.handle)}
                  title={`Copy {{${m.handle}}} — paste it into any field`}
                  className="flex items-center gap-1 rounded-md border border-border-default px-2 py-1 text-xs text-text-secondary hover:bg-bg-hover hover:text-text-primary"
                >
                  <Clipboard size={13} />
                  Copy
                </button>
                <button
                  type="button"
                  onClick={() => onDelete(m.uuid)}
                  title="Remove this media"
                  className="flex items-center gap-1 rounded-md border border-border-default px-2 py-1 text-xs text-error-text hover:bg-bg-hover"
                >
                  <Trash2 size={13} />
                  Remove
                </button>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
