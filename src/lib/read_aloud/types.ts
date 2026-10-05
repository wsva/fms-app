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
  /** Similarity score 0-100 against the reference text. */
  score: number;
  created_at: string;
}

/** Result of submitting a recorded attempt. */
export interface ReadAloudSubmitResult {
  score: number;
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
