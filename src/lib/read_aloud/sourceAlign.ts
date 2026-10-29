import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/tauri";

/**
 * Wrappers for the `read_aloud_align_*` commands — guessing every text's
 * clip from a directory of recordings.
 *
 * The pipeline is `register` → `transcribe` (once per file) → `guess` →
 * `save`/`delete`, and it exists because the per-text "Guess from speech"
 * decodes and transcribes the whole file *per text*. Here one recording is
 * transcribed once and every text is placed along it, so the cost is
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
  /**
   * sha256 of the bytes. This — not the path and not the mtime — is what says
   * a cached transcript describes this file, which is why a folder copied from
   * another machine keeps every transcript it brought with it.
   */
  content_hash: string;
  /** Seconds of audio; `null` until this file is transcribed. */
  duration: number | null;
  /** Spoken words in the cached transcript; `null` when there is no cache. */
  words: number | null;
}

/**
 * What a folder pick did to the cache.
 *
 * Every file's bytes either matched a transcript this dataset already had
 * (`reused`), matched one under an older name (`adopted`), or need listening
 * (`new`). Nothing is deleted: `orphaned` counts cache files no recording in
 * this folder claims, and they are kept so switching folders back is free.
 */
export interface AlignReconcile {
  directory: string;
  total: number;
  reused: number;
  adopted: number;
  /** Adopted by file name alone, because the align dir carried no size to check. */
  adopted_unverified: number;
  new: number;
  /** Recordings the previous binding had that this folder does not. */
  missing: number;
  /** Cache files no recording here claims — kept, not deleted. */
  orphaned: number;
  /** Recordings still here whose position in the playing order moved. */
  reordered: number;
}

export interface AlignSourceIndex {
  status: string;
  directory: string;
  sources: AlignSource[];
  source_count: number;
  transcribed: number;
  pending_transcription: number;
  /** What the pick found; `null` when this call only listed. */
  reconcile: AlignReconcile | null;
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

/** One word of a cached transcript, at millisecond precision. */
export interface AlignTranscriptWord {
  start: number;
  end: number;
  /**
   * The word after scoring's normalization (lowercased, numbers spelled out).
   * That is the shape the aligner matched, so it is the shape shown: the
   * model's original casing is not kept.
   */
  text: string;
}

/** One recording's cache: what the model heard, and when it heard it. */
export interface AlignTranscript {
  status: string;
  source_uuid: string;
  file_name: string;
  path: string;
  duration: number;
  /** Seconds one peak pair covers in the cache. */
  peak_seconds: number;
  word_count: number;
  words: AlignTranscriptWord[];
}

export interface AlignGuessResult {
  status: string;
  texts: number;
  proposed: number;
  refused: number;
  /** Mean confidence of the proposed spans, 0-1, two decimals. */
  mean_confidence: number;
  /** Words after the last placed text — a sign of a recording left out. */
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

/**
 * The standard audio a text already owns.
 *
 * A text that has one is skipped by a save rather than given a second clip: the
 * clip on disk still holds the seconds its wav was cut from, so changing a span
 * means deleting that clip first (`deleteStandardAudio`).
 */
export interface AlignClip {
  uuid: string;
  /** `tts`, `upload` or `align`. */
  kind: "tts" | "upload" | "align";
  /** The clip the scorer reads. */
  is_default: boolean;
  /** The seconds it covers, when it was cut from a recording. */
  start_secs: number | null;
  end_secs: number | null;
  /** Provenance as written: `Align · chapter1.mp3 · 12.30–18.90 s`. */
  note: string;
}

export interface AlignProposal {
  text_uuid: string;
  title: string;
  content: string;
  status: AlignProposalStatus;
  /** Why it was refused. Empty for a placed span — its match is the confidence. */
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
  /** The standard audio the text already has, if any — the reason Save is off. */
  clip: AlignClip | null;
  /** `null` for a refusal, or when this device no longer holds the cache. */
  preview: AlignPreview | null;
}

export interface AlignCounts {
  /** Spans the model placed — `status === "proposed"`. */
  placed: number;
  refused: number;
  /** Placed with no clip yet — what a save would still store. */
  unsaved: number;
  /** Texts that own a clip, whatever the proposal says. */
  saved: number;
}

export interface AlignProposals {
  status: string;
  generated_at: string;
  proposals: AlignProposal[];
  texts_total: number;
  counts: AlignCounts;
}

export interface AlignMoveSpanResult {
  status: string;
  /** `0` when the span did not move (a nudge that rounded to the same ms). */
  changed: number;
  counts: AlignCounts;
}

/** One text re-placed, with the row it ended up with. */
export interface AlignGuessOneResult {
  status: string;
  text_uuid: string;
  /** `false` when the guess refused: the row is now a refusal. */
  placed: boolean;
  source_uuid: string | null;
  source_name: string;
  start: number;
  end: number;
  confidence: number;
  matched_words: number;
  reference_words: number;
  /** A refusal's reason; empty when the span was placed. */
  note: string;
  counts: AlignCounts;
  generated_at: string;
}

export interface AlignApplyFailure {
  text_uuid: string;
  error: string;
}

/** A placed span that was not saved because its text already has a clip. */
export interface AlignApplySkip {
  text_uuid: string;
  /** The clip in the way — delete it to save this span. */
  uuid: string;
  kind: "tts" | "upload" | "align";
  reason: string;
}

export interface AlignApplyResult {
  status: string;
  applied: number;
  failed: AlignApplyFailure[];
  failed_count: number;
  /** Texts that already had a clip: nothing was written for them. */
  skipped: AlignApplySkip[];
  skipped_count: number;
  /** Spans still waiting to be saved; repeat until 0. */
  remaining: number;
  counts?: AlignCounts;
}

/**
 * Scan a folder for recordings and bind them to this dataset in playing order.
 *
 * Registration connects rather than rebuilds: a recording is identified by the
 * sha256 of its bytes, so a folder copied to another machine — or renamed, or
 * moved — keeps the transcript of every file whose content did not change, and
 * a cache file this dataset already owns is reused however the mtime moved.
 * The returned `reconcile` block says what the connection found.
 *
 * Omitting `directory` lists what is already registered, which is what the
 * panel calls on mount, and hashes nothing.
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
 * One recording's cached transcript, word by word.
 *
 * Read-only and free — the cache is already on disk. This is what the guess
 * actually compared, so it answers "why is this text at that second?"
 * better than any number in the queue does.
 */
export async function readAlignTranscript(
  datasetUuid: string,
  sourceUuid: string
): Promise<AlignTranscript> {
  requireTauri();
  return invoke<AlignTranscript>("read_aloud_align_transcript", { datasetUuid, sourceUuid });
}

/**
 * Place every text along the recordings, in order, and write the proposals.
 *
 * Read-only with respect to `media/`: it only writes `align/proposals.json`.
 * Texts are walked in `order_num` with a cursor that never moves backwards,
 * so a re-run after fixing the folder replaces the proposal set rather than
 * appending to it. A row whose span did not move keeps its saved state; a row
 * that moved is a new answer and starts unsaved.
 */
export async function guessAlignSpans(datasetUuid: string): Promise<AlignGuessResult> {
  requireTauri();
  return invoke<AlignGuessResult>("read_aloud_align_guess", { datasetUuid });
}

/**
 * Place one text again, leaving every other span in the file alone.
 *
 * The whole-set guess rewrites the file, so re-running it to fix one text
 * would also overwrite the edges you had just nudged elsewhere. This one reads
 * the span of the text before it out of the file, starts where that span ends,
 * and replaces only its own row.
 *
 * A text that has never been placed is an error rather than a new row at the
 * end of the list: the order of the file is the reading order, and appending
 * would break it.
 */
export async function guessAlignSpanOne(
  datasetUuid: string,
  textUuid: string
): Promise<AlignGuessOneResult> {
  requireTauri();
  return invoke<AlignGuessOneResult>("read_aloud_align_guess_one", { datasetUuid, textUuid });
}

/**
 * The proposals joined with the texts they are for, plus a thumbnail of the
 * audio around each span. Read-only, and the only call the panel needs to draw.
 */
export async function listAlignProposals(datasetUuid: string): Promise<AlignProposals | null> {
  if (!isTauri()) return null;
  return invoke<AlignProposals>("read_aloud_align_proposals", { datasetUuid });
}

/**
 * Move one placed span by hand (the 10 ms nudges).
 *
 * A nudge only rewrites the span in `proposals.json`; `applyAlignProposals` is
 * what stores a clip. A text that already has a clip keeps it until that clip is
 * deleted — nothing here touches it.
 */
export async function moveAlignSpan(
  datasetUuid: string,
  textUuid: string,
  start: number,
  end: number
): Promise<AlignMoveSpanResult> {
  requireTauri();
  return invoke<AlignMoveSpanResult>("read_aloud_align_move_span", {
    datasetUuid,
    textUuid,
    start,
    end,
  });
}

/**
 * Store placed spans as their text's standard audio.
 *
 * Each recording is decoded **once** however many spans come out of it — and
 * not at all here: the wav is cut on first use. `limit` bounds the call, so the
 * page repeats until `remaining` is 0 rather than holding one request open for
 * an hour. `textUuid` saves that one text (a row's Save button); omit it to save
 * every text that still has no clip.
 *
 * A text that already has standard audio is never overwritten — it comes back
 * in `skipped`, and its span can only be changed by deleting that clip first.
 */
export async function applyAlignProposals(
  datasetUuid: string,
  limit?: number,
  textUuid?: string
): Promise<AlignApplyResult> {
  requireTauri();
  return invoke<AlignApplyResult>("read_aloud_align_apply", {
    datasetUuid,
    limit: limit ?? null,
    textUuid: textUuid ?? null,
  });
}
