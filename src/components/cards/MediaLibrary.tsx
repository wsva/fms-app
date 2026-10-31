"use client";

//! Per-card attachment library, shown in the editor.
//!
//! This is the browse feature the design calls out: the card's own media,
//! listed from `card_media_list` (never the raw directory). Each row can be
//! placed into a field, reordered within its field's tail strip, or removed.
//! Reorder writes the `sort` column; placement moves the `{{handle}}` token to
//! the caret of the field the user last focused. Nothing here re-uploads bytes
//! — it only rearranges the index the anchors point at.

import type { CardMedia, CardMediaKind } from "@/lib/types";
import { FileQuestion, Trash2, ChevronUp, ChevronDown, CornerDownLeft, Music2, Video, ImageIcon } from "lucide-react";

const KIND_ICON: Record<CardMediaKind, typeof ImageIcon> = {
  image: ImageIcon,
  audio: Music2,
  video: Video,
};

type Props = {
  media: CardMedia[];
  /** Rows whose bytes are missing on this device (present === false), by handle. */
  missingHandles: Set<string>;
  /**
   * Rows that are written to the table but whose `{{handle}}` token sits in no
   * field — attached, not placed. A media write is durable the moment it
   * happens; the text token that points at it only survives if the card is
   * saved. Naming the gap here is what keeps that drift from reading as a bug.
   */
  unplacedHandles: Set<string>;
  onPlace: (handle: string) => void;
  onMove: (uuid: string, dir: -1 | 1) => void;
  onDelete: (uuid: string) => void;
};

export default function MediaLibrary({ media, missingHandles, unplacedHandles, onPlace, onMove, onDelete }: Props) {
  if (media.length === 0) {
    return (
      <div className="px-3 py-4 text-sm text-text-tertiary">
        No attachments yet. Use Attach to add an image, audio, or video, or drop a file onto the editor.
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-1.5 p-2">
      {media.map((m, i) => {
        const Icon = KIND_ICON[m.kind] ?? FileQuestion;
        const missing = missingHandles.has(m.handle);
        const unplaced = unplacedHandles.has(m.handle);
        return (
          <div
            key={m.uuid}
            className="rounded-lg border border-border-default bg-bg-surface px-2.5 py-2 flex flex-col gap-1.5"
          >
            <div className="flex items-center gap-2 min-w-0">
              <Icon size={16} className="shrink-0 text-text-secondary" />
              <span className="font-mono text-xs text-text-primary truncate" title={m.handle}>
                {`{{${m.handle}}}`}
              </span>
              <span className="text-[10px] px-1.5 py-0.5 rounded bg-mid-gray/20 text-text-tertiary shrink-0">
                {m.field}
              </span>
              {missing && (
                <span className="text-[10px] text-error-text shrink-0" title="bytes not on this device">
                  missing
                </span>
              )}
              {unplaced && !missing && (
                <span
                  className="text-[10px] text-warning-text shrink-0"
                  title="saved with the card, but no field holds its {{handle}} yet"
                >
                  not placed
                </span>
              )}
            </div>
            {m.title && <div className="text-xs text-text-secondary truncate">{m.title}</div>}
            <div className="flex items-center gap-1">
              <button
                type="button"
                onClick={() => onPlace(m.handle)}
                title={unplaced ? "Place this attachment in the target field" : "Move this attachment to the target field"}
                className="p-1.5 rounded hover:bg-bg-hover text-text-secondary"
              >
                <CornerDownLeft size={14} />
              </button>
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
              <button
                type="button"
                onClick={() => onDelete(m.uuid)}
                title="Remove attachment"
                className="p-1.5 rounded hover:bg-bg-hover text-error-text"
              >
                <Trash2 size={14} />
              </button>
            </div>
          </div>
        );
      })}
    </div>
  );
}
