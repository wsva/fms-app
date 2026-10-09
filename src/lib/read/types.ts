/**
 * Shared types for the "Read a book" feature.
 *
 * These mirror the Rust structs in `src-tauri/src/book.rs` (serde snake_case).
 * Books are stored dataset-style: each book is a directory containing
 * `info.json`, `data.sqlite3` (chapters / sentences / words) and `media/`.
 */

// ---------------------------------------------------------------------------
// Persisted (DB) types
// ---------------------------------------------------------------------------

/** A book in the reading library. Mirrors the `name` field of its info.json. */
export interface BookMeta {
  uuid: string;
  name: string;
  /** Absolute filesystem path of the book directory. */
  path: string;
  created_at: string;
  updated_at: string;
}

/** A (hierarchical) chapter belonging to a book. */
export interface BookChapter {
  uuid: string;
  book_uuid: string;
  parent_uuid: string | null;
  order_num: number;
  title: string;
  /** e.g. "completed" or null. */
  status: string | null;
  created_at: string;
  updated_at: string;
}

/** A sentence (or paragraph break) belonging to a chapter. */
export interface BookSentence {
  uuid: string;
  chapter_uuid: string;
  user_id: string;
  order_num: number;
  content: string;
  /** "text" | "paragraph_break". */
  sentence_type: string;
  /** Audio path relative to the book dir (e.g. `media/<uuid>.wav`) — persisted. */
  audio_path: string | null;
  /** Resolved absolute audio path for playback — computed on read, not persisted. */
  audio_url?: string | null;
  recognized: string | null;
  bg_color: string | null;
  created_at: string;
  updated_at: string;
}

/** A vocabulary word attached to a sentence. */
export interface BookSentenceWord {
  uuid: string;
  sentence_uuid: string;
  word: string;
  word_type: string;
  note: string;
}

/** Result of writing/importing an audio file into a book. */
export interface AudioWriteResult {
  name: string;
  /** Relative path (store this in `audio_path`). */
  rel_path: string;
  /** Absolute path (use for playback). */
  abs_path: string;
}

// ---------------------------------------------------------------------------
// Client-side types
// ---------------------------------------------------------------------------

/** A sentence plus transient client-only flags. */
export type SentenceClient = BookSentence & {
  /** order_num changed locally, needs a "Save Order" flush. */
  modified: boolean;
  /** An audio blob recorded in this session but not yet persisted. */
  hasLocalAudio: boolean;
};

/** A group of sentences terminated by an optional paragraph_break row. */
export type Paragraph = {
  sentences: SentenceClient[];
  breakSentence?: SentenceClient;
};

export type DrawerState =
  | { mode: "edit"; sentence: SentenceClient }
  | { mode: "add"; insertBeforeUUID: string | null } // null = append to end
  | null;

export type FlatChapter = BookChapter & { depth: number };

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Flatten a chapter forest into a depth-annotated, ordered list. */
export function flattenChapters(
  all: BookChapter[],
  parentUuid: string | null = null,
  depth = 0
): FlatChapter[] {
  return all
    .filter((c) => (c.parent_uuid ?? null) === parentUuid)
    .sort((a, b) => (a.order_num ?? 0) - (b.order_num ?? 0))
    .flatMap((c) => [
      { ...c, depth },
      ...flattenChapters(all, c.uuid, depth + 1),
    ]);
}

/** Group a flat, ordered sentence list into paragraphs split on paragraph_break rows. */
export function groupIntoParagraphs(sentences: SentenceClient[]): Paragraph[] {
  const result: Paragraph[] = [];
  let current: SentenceClient[] = [];
  for (const s of sentences) {
    if (s.sentence_type === "paragraph_break") {
      result.push({ sentences: current, breakSentence: s });
      current = [];
    } else {
      current.push(s);
    }
  }
  result.push({ sentences: current });
  return result;
}

/** Strip client-only flags, yielding the persisted DB shape. */
export function toDbSentence(s: SentenceClient): BookSentence {
  const { modified: _m, hasLocalAudio: _h, ...db } = s;
  return db;
}
