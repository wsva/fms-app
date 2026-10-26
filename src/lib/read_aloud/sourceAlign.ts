import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/tauri";

/**
 * Wrappers for the `read_aloud_align_*` commands — guessing every passage's
 * clip from a directory of recordings.
 *
 * The pipeline is `register` → `transcribe` (once per file) → `guess` →
 * `decide` → `apply`, and it exists because the per-text "Guess from speech"
 * decodes and transcribes the whole file *per passage*. Here one recording is
 * transcribed once and every passage is placed along it, so the cost is
 * O(files) instead of O(texts × files).
 *
 * Every call is guarded so the page still renders in a plain browser: a read
 * returns nothing, a write throws a message the panel can show.
 *
 * The word timings, peaks and proposals live in `<dataset>/align/` — derived
 * files that sync never ships and that this device can rebuild. The recordings
 * themselves stay where the user keeps them, which is why `source_path` is
 * stored per row and re-checked rather than copied into the dataset.
 */

function requireTauri(): void {
  if (!isTauri()) {
    throw new Error("Source alignment needs the desktop app.");
  }
}

/** One registered recording, in playing order. */
export interface AlignSource {
  uuid: string;
  file_name: string;
  /** Absolute path as it was when the folder was picked. */
  path: string;
  size: number;
  modified_ms: number;
  /** Seconds of audio; `null` until this file is transcribed. */
  duration: number | null;
  /** Spoken words in the cached transcript; `null` when there is no cache. */
  words: number | null;
}

export interface AlignSourceIndex {
  status: string;
  directory: string;
  sources: AlignSource[];
  source_count: number;
  transcribed: number;
  pending_transcription: number;
}

export interface AlignTranscribeResult extends AlignSourceIndex {
  /** True once every registered file has a transcript. */
  done: boolean;
  /** Files still missing a transcript. */
  remaining: number;
  source_uuid?: string;
  file_name?: string;
  duration?: number;
  words?: number;
}

export interface AlignGuessResult {
  status: string;
  passages: number;
  proposed: number;
  refused: number;
  /** Mean confidence of the proposed spans, 0-1, two decimals. */
  mean_confidence: number;
  /** Words after the last placed passage — a sign of a recording left out. */
  unused_words: number;
  sources: number;
  generated_at: string;
}

/** The peak envelope around one span: the thumbnail beside its row. */
export interface AlignPreview {
  /** Absolute seconds of the first pair. */
  start: number;
  /** Seconds each pair covers. */
  peak_seconds: number;
  /** Interleaved min/max signed 8-bit pairs — the `WaveformPeaks` layout. */
  peaks: number[];
}

export type AlignProposalStatus = "proposed" | "refused";
export type AlignProposalState = "pending" | "accepted" | "rejected";

export interface AlignProposal {
  text_uuid: string;
  title: string;
  content: string;
  status: AlignProposalStatus;
  state: AlignProposalState;
  /** Why it was refused, or what to look at when it is unsure. */
  note: string;
  source_uuid: string | null;
  source_name: string;
  /** Absolute path of the recording, for playback. */
  source_path: string;
  start: number;
  end: number;
  confidence: number;
  matched_words: number;
  reference_words: number;
  /** Set by `apply`: the standard-audio uuid now holding this span. */
  applied_uuid: string | null;
  /** `null` for a refusal, or when this device no longer holds the cache. */
  preview: AlignPreview | null;
}

export interface AlignCounts {
  proposed: number;
  refused: number;
  accepted: number;
  rejected: number;
  applied: number;
  /** Accepted but not yet cut. */
  open: number;
}

export interface AlignProposals {
  status: string;
  generated_at: string;
  proposals: AlignProposal[];
  passages_total: number;
  counts: AlignCounts;
}

export interface AlignDecideResult {
  status: string;
  changed: number;
  state: AlignProposalState;
  counts: AlignCounts;
}

export interface AlignApplyFailure {
  text_uuid: string;
  error: string;
}

export interface AlignApplyResult {
  status: string;
  applied: number;
  failed: AlignApplyFailure[];
  failed_count: number;
  /** Accepted spans still uncut; repeat until 0. */
  remaining: number;
  counts?: AlignCounts;
}

/**
 * Scan a folder for recordings and register them in playing order.
 *
 * Omitting `directory` lists what is already registered, which is what the
 * panel calls on mount. Re-picking a folder keeps the transcript of every file
 * that kept its name *and* its size and mtime; anything else is re-transcribed.
 */
export async function registerAlignSources(
  datasetUuid: string,
  directory?: string
): Promise<AlignSourceIndex> {
  requireTauri();
  return invoke<AlignSourceIndex>("read_aloud_align_sources", {
    datasetUuid,
    directory: directory ?? null,
  });
}

/**
 * Cache one recording's word timings and peak envelope.
 *
 * One file per call on purpose: a chapter is minutes of model time, so the
 * panel loops on this until `remaining` is 0 (and can stop between files) and
 * an agent repeats it just the same. Omitting `sourceUuid` takes the first file
 * still missing a transcript, which makes the loop a one-liner.
 */
export async function transcribeAlignSource(
  datasetUuid: string,
  sourceUuid?: string
): Promise<AlignTranscribeResult> {
  requireTauri();
  return invoke<AlignTranscribeResult>("read_aloud_align_transcribe", {
    datasetUuid,
    sourceUuid: sourceUuid ?? null,
  });
}

/**
 * Place every passage along the recordings, in order, and write the proposals.
 *
 * Read-only with respect to `media/`: it only writes `align/proposals.json`.
 * Passages are walked in `order_num` with a cursor that never moves backwards,
 * so a re-run after fixing the folder replaces the proposal set rather than
 * appending to it. Review state (accepted/rejected) survives a re-run only for
 * spans that did not move.
 */
export async function guessAlignSpans(datasetUuid: string): Promise<AlignGuessResult> {
  requireTauri();
  return invoke<AlignGuessResult>("read_aloud_align_guess", { datasetUuid });
}

/**
 * The proposals joined with the passages they are for, plus a thumbnail of the
 * audio around each span. Read-only, and the only call the panel needs to draw.
 */
export async function listAlignProposals(datasetUuid: string): Promise<AlignProposals | null> {
  if (!isTauri()) return null;
  return invoke<AlignProposals>("read_aloud_align_proposals", { datasetUuid });
}

/**
 * Accept or reject one proposal, or every proposal at or above a confidence.
 *
 * `start`/`end` correct one passage's span (the panel's 10 ms nudges) and need
 * a `textUuid`; the two must be given together. Nudging is a judgement, not a
 * write — `apply` is what cuts audio.
 */
export async function decideAlignProposals(params: {
  datasetUuid: string;
  state: AlignProposalState;
  textUuid?: string;
  minConfidence?: number;
  start?: number;
  end?: number;
}): Promise<AlignDecideResult> {
  requireTauri();
  return invoke<AlignDecideResult>("read_aloud_align_decide", {
    datasetUuid: params.datasetUuid,
    textUuid: params.textUuid ?? null,
    state: params.state,
    minConfidence: params.minConfidence ?? null,
    start: params.start ?? null,
    end: params.end ?? null,
  });
}

/**
 * Cut every accepted span out of its recording and store it as the text's
 * standard audio.
 *
 * Each recording is decoded **once** however many spans come out of it. `limit`
 * bounds the call, so the panel repeats until `remaining` is 0 rather than
 * holding one request open for an hour.
 */
export async function applyAlignProposals(
  datasetUuid: string,
  limit?: number
): Promise<AlignApplyResult> {
  requireTauri();
  return invoke<AlignApplyResult>("read_aloud_align_apply", {
    datasetUuid,
    limit: limit ?? null,
  });
}
