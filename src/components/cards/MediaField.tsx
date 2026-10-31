"use client";

//! Renders one card field (question / answer / note / suggestion) with its
//! inline attachments.
//!
//! A field's raw text is prose punctuated by `{{handle}}` anchors; the card's
//! `CardMedia[]` says what each handle is. This splits the text on anchors and
//! interleaves the two: prose segments go through the site's existing renderer
//! (MarkdownViewer, or a plain `<span>` where the field already renders plain
//! text — `mode="plain"`), and media segments render as a [`MediaBlock`].
//!
//! The whole point of the segment approach is that it never touches the wiki's
//! `html.ts` / `sanitizeUrl` — media placement is block-level, so a token
//! inside a sentence becomes its own block at that point rather than a glyph
//! mid-line (a v1 limitation, accepted in the design).
//!
//! `reveal` gates playback: pass the review flip state so `play_on: 'reveal'`
//! clips fire the moment the answer side becomes visible. It defaults true for
//! the editor, where everything shown is already revealed.
//!
//! `tools` is the editor's offer, resolved per segment: the field it sits in
//! takes the upload, the row behind the handle takes the removal, the handle
//! itself takes the copy. Reading hosts pass nothing and get plain media.

import type { CardMedia } from "@/lib/types";
import { splitAnchors } from "@/lib/media/anchors";
import MarkdownViewer from "@/components/wiki/markdown/markdown";
import MediaBlock from "./MediaBlock";

/** What the editor offers on a field's attachments. The field takes the upload,
 *  the row behind a handle takes the removal, the handle takes the copy — which
 *  is why the two are named by argument rather than closed over here. */
export type MediaFieldTools = {
  /** Add another file to this field. */
  upload: () => void;
  /** Remove one attachment by row uuid. */
  remove: (uuid: string) => void;
  /** Put one handle on the clipboard. */
  copy: (handle: string) => void;
};

type Props = {
  /** Raw field text, anchors included. */
  text: string;
  /** The card's live attachments (from `card_media_list`). */
  media: CardMedia[];
  /** Absolute dataset directory (from `CardDatasetSummary.path`). */
  datasetPath: string;
  /** How prose segments render: markdown (answer/note) or plain (question/suggestion). */
  mode?: "markdown" | "plain";
  /** Whether the field is currently revealed; gates `play_on: reveal` clips. */
  reveal?: boolean;
  /** Typographic class applied to plain text segments (e.g. the big question). */
  textClassName?: string;
  /** Let a click on an image open it at screen size (reading hosts only). */
  zoomable?: boolean;
  /** Editor host only: what can be done to one of this field's attachments. */
  tools?: MediaFieldTools;
};

export default function MediaField({
  text,
  media,
  datasetPath,
  mode = "markdown",
  reveal = true,
  textClassName,
  zoomable = false,
  tools,
}: Props) {
  const segments = splitAnchors(text, media);
  if (segments.length === 0) return null;

  return (
    <div className="space-y-3">
      {segments.map((seg, i) => {
        if (seg.kind === "text") {
          return mode === "plain" ? (
            <div key={i} className={`whitespace-pre-wrap break-words ${textClassName ?? ""}`}>
              {seg.text}
            </div>
          ) : (
            <MarkdownViewer key={i} content={seg.text} />
          );
        }
        return (
          <MediaBlock
            key={i}
            media={seg.media}
            handle={seg.handle}
            missing={seg.missing}
            datasetPath={datasetPath}
            reveal={reveal}
            zoomable={zoomable}
            tools={
              tools && seg.media
                ? {
                    upload: tools.upload,
                    remove: () => tools.remove(seg.media!.uuid),
                    copy: () => tools.copy(seg.handle),
                  }
                : undefined
            }
          />
        );
      })}
    </div>
  );
}
