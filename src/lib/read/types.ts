/**
 * Shared types for the "Books" feature.
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
  /** The info.json `description`, empty when unset — shown on the book card. */
  description: string;
  /** The info.json language (`"de"`, `"de-DE"`), empty when unset. Seeds the
   *  read-aloud voice resolution so generated text is narrated in the book's
   *  own language. */
  language: string;
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
// Understanding layer (artifacts, glossary, quiz)
// ---------------------------------------------------------------------------

/** The `kind` of an understanding artifact. */
export type ArtifactKind =
  | "explanation"
  | "simplification"
  | "digest"
  | "argument_map"
  | "questions"
  | "note"
  | "assessment";

/** All artifact kinds, for iteration. */
export const ARTIFACT_KINDS: ArtifactKind[] = [
  "explanation",
  "simplification",
  "digest",
  "argument_map",
  "questions",
  "note",
  "assessment",
];

/** Kinds the LLM generates (regenerated in place, hash-tracked). */
export const GENERATED_KINDS: ArtifactKind[] = [
  "explanation",
  "simplification",
  "digest",
  "argument_map",
  "questions",
];

/** What an artifact is attached to. */
export type ArtifactScopeType = "paragraph" | "chapter" | "sentence";

/** A persisted understanding artifact. Mirrors the Rust `BookArtifact`. */
export interface BookArtifact {
  uuid: string;
  book_uuid: string;
  scope_type: ArtifactScopeType | string;
  /** break-row uuid / chapter uuid / sentence uuid. */
  scope_uuid: string;
  kind: ArtifactKind | string;
  /** Markdown; for `questions`, a JSON array of `QuizQuestion`. */
  content: string;
  /** "ollama" | "agent" | "user". */
  source: string;
  model?: string | null;
  /** assessment -> the note it assesses. */
  parent_uuid?: string | null;
  /** FNV-1a hex of the source text at generation time ("" for notes). */
  content_hash: string;
  created_at: string;
  updated_at: string;
}

/** A `BookArtifact` plus the computed staleness flag returned to readers. */
export interface BookArtifactView extends BookArtifact {
  /** True when a generated artifact's source text changed since it was made. */
  stale: boolean;
}

/** An outstanding request for a third-party agent to generate an artifact.
 *  Mirrors the Rust `ArtifactJob` (the `book_artifact_job` mailbox). The agent
 *  discovers jobs over MCP and delivers through `book_save_artifact`, which
 *  clears the job; the panel refreshes on the `book-artifact-changed` event. */
export interface ArtifactJob {
  uuid: string;
  book_uuid: string;
  scope_type: ArtifactScopeType | string;
  scope_uuid: string;
  kind: ArtifactKind | string;
  /** Free-form guidance captured at request time (e.g. output language). */
  instructions: string;
  /** Model hint from the requester; the fulfilling agent may ignore it. */
  model?: string | null;
  /** `queued` | `claimed` | `delivering` | `waiting_user`. */
  status: string;
  /** Who holds the lease, when an agent does. */
  claimed_by?: string | null;
  /** Claims charged so far: above zero while still `queued` means one lapsed. */
  attempts: number;
  /** Unix ms the lease runs out; null when nobody holds it. */
  expires_at?: number | null;
  /** Why it last came back, when the ledger recorded one. */
  reason?: string | null;
  created_at: string;
  updated_at: string;
  /** The scope's text at read time, so an agent need not fetch it separately. */
  source_text: string;
  /** True when the scope's text changed since the job was requested. */
  stale: boolean;
}

/** A glossary entry. Mirrors the Rust `BookTerm`. */
export interface BookTerm {
  uuid: string;
  book_uuid: string;
  term: string;
  definition: string;
  /** "ollama" | "agent" | "user". */
  source: string;
  model?: string | null;
  created_at: string;
  updated_at: string;
}

/** A multiple-choice quiz question. */
export interface QuizMcq {
  type: "mcq";
  stem: string;
  choices: string[];
  /** Index into `choices` of the correct answer. */
  correct: number;
  explanation: string;
}

/** A Socratic open question, self-graded against `model_answer`. */
export interface QuizOpen {
  type: "open";
  stem: string;
  model_answer: string;
  explanation: string;
}

export type QuizQuestion = QuizMcq | QuizOpen;

/** Response of `book_record_quiz_result`. */
export interface QuizResultResponse {
  uuid: string;
  best_correct: number;
  best_total: number;
  xp: {
    xp_awarded: number;
    lifetime_xp: number;
    level: number;
    is_new: boolean;
  };
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
