//! Frontend mirror of `datasets::cards::strip_media_anchors` and the anchor
//! split used at every render site.
//!
//! Grammar (one rule, both sides of the FFI): `{{handle}}` where a handle
//! matches `^[a-z0-9][a-z0-9-]{0,31}$`. A card field's raw text is the source
//! of truth about where its attachments sit; the row set (`CardMedia[]`) is
//! the source of truth about what each attachment *is*. Splitting on anchors
//! combines them: a token whose row exists becomes a media segment, a token
//! whose row was deleted (or whose bytes are missing on this device) becomes
//! a `missing` media segment so the reader still knows the file belongs there,
//! and everything else is prose.
//!
//! `stripAnchors` is the hash/search-side twin of the same rule: use it for
//! grid rows, search previews, and any export where the reader never sees a
//! media block. Keep the two grammars in step with the Rust function — a card
//! that reads differently here and there means search and the visible text
//! disagree about what a card says.

import type { CardMedia } from "@/lib/types";
import { convertFileSrc } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/tauri";

/** One piece of a split card field. */
export type AnchorSegment =
  | { kind: "text"; text: string }
  | { kind: "media"; media: CardMedia | null; handle: string; missing: boolean };

const HANDLE_ONLY = /^[a-z0-9][a-z0-9-]{0,31}$/;

/** Whether a candidate slug matches the handle grammar. Mirrors
 *  `datasets::cards::is_valid_handle` on the Rust side. */
export function isValidHandle(s: string): boolean {
  return HANDLE_ONLY.test(s);
}

/** Split `text` on `{{handle}}` tokens and pair them with `rows` by handle.
 *  Returns an ordered list of segments; empty text segments are dropped.
 *
 *  Handles that do not appear in `rows` become `missing: true` segments with
 *  `media: null` — the caller decides whether that means "deleted row" or
 *  "row exists, bytes gone on this device" (both render the placeholder). */
export function splitAnchors(text: string, rows: CardMedia[]): AnchorSegment[] {
  const out: AnchorSegment[] = [];
  const byHandle = new Map<string, CardMedia>();
  for (const r of rows) byHandle.set(r.handle, r);

  let cursor = 0;
  let i = 0;
  const pushText = (end: number) => {
    if (end > cursor) out.push({ kind: "text", text: text.slice(cursor, end) });
  };
  while (i < text.length) {
    if (text[i] === "{" && text[i + 1] === "{") {
      const close = text.indexOf("}}", i + 2);
      if (close >= 0) {
        const inner = text.slice(i + 2, close);
        // Grammar-guarded: only anchors we minted are split, so a template
        // literal `{{ other }}` stays inside its surrounding text segment.
        if (HANDLE_ONLY.test(inner)) {
          pushText(i);
          const row = byHandle.get(inner);
          const missing = !row || row.present === false;
          out.push({ kind: "media", media: row ?? null, handle: inner, missing });
          i = close + 2;
          cursor = i;
          continue;
        }
      }
    }
    i++;
  }
  pushText(text.length);
  return out;
}

/** The hash/search-side mirror of `strip_media_anchors` — remove every valid
 *  `{{handle}}` token and leave everything else, including non-anchor braces.
 *  Used by grid rows, search rows, and any export where the reader never sees
 *  a media block; not used at media render sites (those use [`splitAnchors`]). */
export function stripAnchors(text: string): string {
  let out = "";
  let i = 0;
  while (i < text.length) {
    if (text[i] === "{" && text[i + 1] === "{") {
      const close = text.indexOf("}}", i + 2);
      if (close >= 0) {
        const inner = text.slice(i + 2, close);
        if (HANDLE_ONLY.test(inner)) {
          i = close + 2;
          continue;
        }
      }
    }
    out += text[i];
    i++;
  }
  return out;
}

/** True when `text` carries at least one valid anchor token — used by the
 *  list-row heuristic to decide whether to show the "📎 N" chip. */
export function hasAnchor(text: string): boolean {
  const segs = splitAnchors(text, []);
  return segs.some((s) => s.kind === "media");
}

/** Turn a dataset-relative path (as stored in `card_media.rel_path`) plus the
 *  absolute dataset dir (as surfaced by `CardDatasetSummary.path`) into a URL
 *  the webview can stream. Returns `""` off desktop — every render site must
 *  handle that with a placeholder, not a broken image icon. */
export function resolveSrc(datasetPath: string, relPath: string): string {
  if (!datasetPath || !relPath || !isTauri()) return "";
  // Absolute on desktop; the trailing separator is normalised so `path` from
  // the summary works whether it ended with one or not.
  const joined = `${datasetPath.replace(/[/\\]+$/, "")}/${relPath.replace(/^[/\\]+/, "")}`;
  try {
    return convertFileSrc(joined);
  } catch {
    return "";
  }
}

