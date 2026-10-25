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
  /** The language the passages are written in (`"de"`, `"de-DE"`), empty when
   *  unset. It is what a per-language voice preference is looked up by, so an
   *  empty value means no preference can apply. */
  language: string;
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
  /**
   * The stored three-axis assessment, when one was computed. Absent for attempts
   * recorded before assessment existed and for attempts whose recording is gone.
   * It is the compact form: worst words only, at most 24 pauses.
   */
  metrics?: Assessment | null;
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
  /** Accuracy (the same number as `score`), fluency and pronunciation. */
  assessment: Assessment;
  xp_awarded: number;
  lifetime_xp: number;
  level: number;
}

/**
 * Why an axis is missing. First-class states, never a substituted proxy —
 * `gop.rs`/`assess.rs` report an honest gap instead of a fabricated score.
 */
export type PronUnavailableReason =
  /** The text has no standard audio yet: make one (Manage → Standard audio). */
  | "no_reference"
  /** The standard audio was made from an earlier version of the text. */
  | "stale_reference"
  | "model_missing"
  | "model_error"
  /** Legacy: the reference used to be fetched from Edge TTS while scoring. */
  | "no_network"
  /** Legacy, as above. */
  | "tts_failed"
  | "too_short";

/** What this attempt's pipeline could actually measure. */
export interface AssessmentCapabilities {
  /** The STT model returned per-word timings, so fluency exists. */
  word_timings: boolean;
  /** The phoneme GOP pass ran. */
  gop: boolean;
  /** A dataset-owned standard clip supplied the reference phone timeline. */
  tts_reference: boolean;
}

/** Per-word outcome, a refinement of the alignment op. */
export type WordVerdictKind =
  | "correct"
  | "minor"
  | "substituted"
  | "omitted"
  | "inserted"
  | "repeated";

export interface AccuracyCounts {
  ref: number;
  correct: number;
  /** Near-miss substitutions: an error for `score`, a hit for `lenient_score`. */
  minor: number;
  substituted: number;
  omitted: number;
  inserted: number;
  repeated: number;
}

/** The accuracy axis — `score` is the unchanged WIP number. */
export interface Accuracy {
  score: number;
  lenient_score: number;
  wer: number;
  counts: AccuracyCounts;
}

/** Reading-speed band, from words per minute. */
export type RateBand = "slow" | "target" | "fast";

/** Where a silence fell, relative to the reference text. */
export type PauseKind =
  | "leading"
  | "at_punctuation"
  | "within_phrase"
  | "trailing";

export interface Pause {
  start: number;
  end: number;
  kind: PauseKind;
  /** Index of the reference word this pause followed. */
  after_ref?: number | null;
}

/** The aggregate fluency picture. Absent when the STT gave no word timings. */
export interface FluencyMetrics {
  /** Length of the whole recording, in seconds. */
  duration: number;
  /** First word to last word, minus the pauses in between. */
  speaking_time: number;
  /** Correct words per minute. */
  wcpm: number;
  /** Spoken words per minute over the reading span (includes pauses). */
  speech_rate: number;
  /** Spoken words per minute excluding pauses. */
  articulation_rate: number;
  /** Pauses while reading (at punctuation or mid-phrase). */
  pause_count: number;
  long_pause_count: number;
  /** Share of reading pauses that fell mid-phrase — the hesitancy proxy. */
  within_phrase_pause_ratio: number;
  /** Mean words between pauses. */
  mean_length_of_run: number;
  repetition_count: number;
  restart_count: number;
  band: RateBand;
}

/**
 * The pronunciation axis. `score` is null exactly when
 * `unavailable_reason` is set: nothing is ever substituted for a real GOP.
 */
export interface Pronunciation {
  score: number | null;
  /** `"ctc_gop"` when measured. */
  method: string | null;
  /** Share of scored words below the weak-word threshold. */
  weak_ratio: number | null;
  unavailable_reason: PronUnavailableReason | null;
}

/** One reference word with everything known about it. */
export interface WordAssessment {
  ref_index?: number | null;
  ref: string;
  hyp: string;
  verdict: WordVerdictKind;
  start?: number | null;
  end?: number | null;
  /** GOP score of this word in (0, 1], when pronunciation was measured. */
  pron?: number | null;
}

/** The whole scored picture for one attempt. */
export interface Assessment {
  schema_version: number;
  capabilities: AssessmentCapabilities;
  accuracy: Accuracy;
  fluency?: FluencyMetrics | null;
  pronunciation: Pronunciation;
  /** Every reference word on submit; the worst 12 when read back from storage. */
  words: WordAssessment[];
  /** At most 24 when read back from storage. */
  pauses: Pause[];
}

/**
 * The result of scoring a text's own standard audio: the clip is treated as the
 * "recording" and scored against the text, so the reference can be checked
 * before a learner is measured against it. Nothing is stored.
 */
export interface StandardAudioAssessment {
  /** The clip that was scored (the text's default). */
  clip_uuid: string;
  /** Seconds of the clip. */
  duration: number;
  /** The reference the clip was scored against. */
  reference: string;
  /** What the STT model heard in the clip — the "recognized" side. */
  recognized: string;
  assessment: Assessment;
}

/** Below this a word's pronunciation is reported as weak. */
export const WEAK_WORD_SCORE = 0.4;

/**
 * One reference clip a text owns, read from `read_standard_audio`.
 *
 * Scoring measures pronunciation against the **default** clip only. `kind`,
 * `duration` and `stale` are derived on read (the row stores neither) so they
 * can never disagree with the wav they describe.
 */
export interface ReadStandardAudio {
  uuid: string;
  text_uuid: string;
  /** Free-form origin note: `Edge TTS · <voice>` or `Upload · <file>`. */
  note: string;
  /** `tts` when the clip carries word timings, `upload` otherwise. */
  kind: "tts" | "upload";
  /**
   * Seconds, decoded from the wav — or `null` when nothing decoded it (the
   * dataset-wide listing deliberately skips that, and the wav may not be on
   * this device at all). `null` means "not measured", never "zero seconds".
   */
  duration: number | null;
  /** Absolute path for playback, null when the wav is missing here. */
  audio_url: string | null;
  is_default: boolean;
  /** Made from an earlier version of the text: regenerate before scoring. */
  stale: boolean;
  /** Present and not stale — what scoring would read. */
  usable: boolean;
  created_at: string;
}

/**
 * Which voice a generation would speak with, and why — `read_aloud_voice_resolve`.
 *
 * The provenance is the point: the same clip can come from the caller naming a
 * voice, from the language's preferred voice, from the language-less setting or
 * from the built-in default, and only the backend knows which. `source` is one
 * of `param` | `preference` | `setting` | `default`.
 */
export interface VoiceResolution {
  voice: string;
  /** Parsed from the short name: `de-DE` for `de-DE-ConradNeural`. */
  locale: string;
  source: "param" | "preference" | "setting" | "default";
  /** The dataset's `info.json` language; empty when it has none. */
  dataset_language: string;
  /** Languages a preference is configured for, so the UI can say why not. */
  languages_with_preference: string[];
  /** The voice speaks a different language than the dataset. */
  warning: string | null;
  /** The dataset has no language, so no preference could apply. */
  hint: string | null;
}

/** A decoded upload, returned by the probe before anything is stored. */
export interface AudioProbe {
  /** Seconds of the whole source. */
  duration: number;
  /** Interleaved min/max signed 8-bit pairs, the `WaveformPeaks` layout. */
  peaks: number[];
  /** Seconds one min/max pair covers. */
  seconds_per_pair: number;
  file_name: string;
}

/**
 * Where a text is spoken inside a longer recording, as guessed by
 * `read_aloud_standard_audio_guess`. Nothing is stored: both edges are for the
 * crop, and every other way of moving them still works afterwards.
 */
export interface SpanGuess {
  /** Seconds, already padded and clamped to the file. */
  start: number;
  end: number;
  /** Words of the text the model heard, in order. */
  matched_words: number;
  reference_words: number;
  /** `matched_words / reference_words` (0–1); well under 1 is a loose match. */
  confidence: number;
  /** Everything the model heard in the file. */
  transcript: string;
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
