/**
 * Shared types for the "Read aloud" feature.
 *
 * These mirror the Rust structs in `src-tauri/src/read_aloud.rs` (serde keeps the
 * snake_case field names). A read-aloud dataset is a directory holding texts to
 * practise; each text can be read aloud, recorded, transcribed and scored.
 */

/** A read-aloud dataset (a collection of texts). */
export interface ReadAloudMeta {
  uuid: string;
  name: string;
  description: string;
  /** Absolute filesystem path of the dataset directory. */
  path: string;
  created_at: string;
  updated_at: string;
}

/** A single text to read aloud. */
export interface ReadText {
  uuid: string;
  dataset_uuid: string;
  order_num: number;
  title: string;
  /** Free-form note (source, location, etc.). */
  note: string;
  /** The reference text the user reads. */
  content: string;
  /**
   * LLM-normalized reference used for scoring (numbers/abbreviations spelled
   * out to match STT output). Derived and disposable: null until produced, and
   * editing `content` resets it to null on the backend.
   */
  normalized_content: string | null;
  /** Best score across attempts (null if never attempted). */
  best_score: number | null;
  /** Number of recorded attempts (computed on read). */
  attempt_count: number;
  created_at: string;
  updated_at: string;
}

/** One recorded attempt at a text. */
export interface ReadAttempt {
  uuid: string;
  text_uuid: string;
  user_id: string;
  /** Audio path relative to the dataset dir (e.g. `media/<uuid>.wav`). */
  audio_path: string | null;
  /** Resolved absolute audio path for playback (computed on read). */
  audio_url?: string | null;
  /** STT transcript of the recording. */
  recognized: string;
  /** WIP-based ASR score 0-100 against the reference text. */
  score: number;
  created_at: string;
}

/** The kind of one alignment run between reference and transcript. */
export type AsrOp = "equal" | "substitute" | "insert" | "delete";

/** A run of consecutive same-op alignment cells (words joined by spaces). */
export interface AsrChunk {
  op: AsrOp;
  /** The reference words of this run (empty for insertions). */
  ref_word: string;
  /** The transcript words of this run (empty for deletions). */
  hyp_word: string;
  /** Index of the run start in the reference words (null for insertions). */
  ref_index: number | null;
  /** Index of the run start in the transcript words (null for deletions). */
  hyp_index: number | null;
}

/** The full scoring picture returned by read_aloud_score / read_aloud_submit. */
export interface AsrScoreSummary {
  /** WIP × 100 — the number shown to the user and stored per attempt. */
  score: number;
  /** Word Error Rate (S + D + I) / N. */
  wer: number;
  /** Match Error Rate (S + D + I) / (H + S + D + I). */
  mer: number;
  hits: number;
  substitutions: number;
  deletions: number;
  insertions: number;
  ref_len: number;
  hyp_len: number;
  chunks: AsrChunk[];
}

/** Result of submitting a recorded attempt. */
export interface ReadAloudSubmitResult {
  score: number;
  /** WER/MER, H/S/D/I counts and the alignment chunks to highlight. */
  summary: AsrScoreSummary;
  attempt: ReadAttempt;
  xp_awarded: number;
  lifetime_xp: number;
  level: number;
}

/** Score threshold at/above which a reading passes and earns XP. */
export const PASS_SCORE = 60;
/** Score threshold for the bonus XP tier. */
export const GOOD_SCORE = 80;

/** Tailwind classes for a score badge, keyed by pass/good thresholds. */
export function scoreBadgeClasses(score: number | null | undefined): string {
  const base = "text-xs px-2 py-0.5 rounded-full font-semibold tabular-nums";
  if (score == null) return `${base} bg-bg-muted text-text-tertiary`;
  if (score >= GOOD_SCORE) return `${base} bg-success-bg text-success-text`;
  if (score >= PASS_SCORE) return `${base} bg-yellow-400/25 text-text-primary`;
  return `${base} bg-error-bg text-error-text`;
}
