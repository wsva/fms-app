/**
 * Shared bits of the normalized-reference generator: the localStorage key for
 * the model choice, and the shapes the backend's two commands return.
 *
 * The prompt itself is *not* here. It lives in the backend
 * (`src-tauri/src/datasets/read_aloud/normalize.rs`) so the in-app model call,
 * the "copy the prompt" route and the MCP tool an agent drives all send exactly
 * the same words — a candidate's shape has to be comparable however it was made.
 */

export const LS_NORMALIZE_MODEL = "read_aloud_normalize_model";

/** Response of `read_aloud_normalize`. `candidate` is absent when no model was named. */
export interface NormalizeResult {
  prompt: string;
  candidate?: string | null;
}

/** Response of `read_aloud_normalize_measure`: the accept/reject gate. */
export interface NormalizeMeasure {
  /** Transcripts compared. 0 means the numbers prove nothing. */
  attempts: number;
  /** Best score of the reference in use today. */
  baseline: number | null;
  /** Best score of the candidate against the same transcripts. */
  candidate: number | null;
}
