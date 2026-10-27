/**
 * Shared bits of the attempt feedback: the report shape, the localStorage key
 * for the model choice, and the calls the panel makes.
 *
 * The prompt itself is *not* here. It lives in the backend
 * (`src-tauri/src/datasets/read_aloud/feedback.rs`) so the "copy the prompt"
 * route and the MCP tool an agent drives all send exactly the same words — and
 * because the evidence it quotes is assembled from the stored attempt, which
 * only the backend can read.
 *
 * A report about a reading needs the reading, and the app's text-only LLM seam
 * cannot carry audio. So feedback is delegated: `requestFeedback` posts the
 * evidence, the prompt and the recording's location to the agent mailbox, an
 * attached agent that can hear the take answers, and `read_aloud_save_feedback`
 * writes the report back into this card. The panel reloads on the
 * `read-aloud-feedback-saved` event.
 *
 * A report is derived output: it never affects a score, and it is device-local
 * (see `feedback_json` on the attempt row).
 */

import { invoke } from "@tauri-apps/api/core";

/**
 * The model choice, under the same key the Chat, OCR and Understanding pickers
 * use: this is one preference ("the model I run locally"), not one per page.
 * Empty means the app default from Settings.
 */
export const LS_FEEDBACK_MODEL = "llm_selected_model";

/** One thing to fix, with the evidence it came from. */
export interface FeedbackIssue {
  /** `accuracy` | `fluency` | `pronunciation`. */
  area: string;
  /** The measurement being explained, quoted from the evidence. */
  evidence: string;
  detail: string;
  fix: string;
  drill?: string | null;
}

/** One word to work on, with the sound that went wrong when it is known. */
export interface FeedbackFocusWord {
  word: string;
  /** Target IPA for the word, when the pronunciation pass measured it. */
  ipa?: string | null;
  hint: string;
  score?: number | null;
}

/**
 * The report stored on an attempt.
 *
 * `format` is `"report"` for the structured answer and `"text"` when the model
 * answered in prose and the answer was kept rather than thrown away — then only
 * `summary` carries anything.
 */
export interface FeedbackReport {
  schema_version: number;
  summary: string;
  diagnosis?: string | null;
  issues: FeedbackIssue[];
  focus_words: FeedbackFocusWord[];
  drill?: string | null;
  next_try: string;
  /** `"agent"` for a report written back via MCP; `"llm"` on a legacy report
   *  the removed in-app route once produced. */
  source: string;
  model?: string | null;
  created_at: string;
  format: string;
}

/** One weak phone inside a weak word. */
export interface PhoneNote {
  /** The phone the reference asked for. */
  ipa: string;
  /** Its GOP score in (0, 1] — higher is clearer. */
  score: number;
  /** What the acoustic model heard instead, when it heard a single phone. */
  heard?: string | null;
}

/** The phone-level detail of one word, recomputed on demand. */
export interface WordPhones {
  word: string;
  /** The whole word's target IPA, phones joined by a space. */
  ipa: string;
  score: number;
  /** Its weakest phones, worst first. */
  weak: PhoneNote[];
}

/** The recording behind an attempt, as facts. */
export interface FeedbackAudioFacts {
  /** Absolute path on this device, when the wav is here. */
  path?: string | null;
  available: boolean;
  bytes?: number | null;
  duration?: number | null;
  format: string;
}

/** Identity of the attempt a context describes. */
export interface FeedbackAttemptFacts {
  uuid: string;
  text_uuid: string;
  score: number;
  recognized: string;
  audio_path?: string | null;
  created_at: string;
}

/** Response of `read_aloud_feedback_context` — everything an agent needs. */
export interface FeedbackContext {
  attempt: FeedbackAttemptFacts;
  passage: string;
  /** The dataset's language tag, empty when it has none. */
  language: string;
  /** The language the report was asked to be written in. */
  feedback_language: string;
  audio: FeedbackAudioFacts;
  /** The stored assessment, verbatim, when the take has one. */
  assessment?: unknown | null;
  /** Weak words with their weakest phones, when they could be recomputed. */
  phone_detail?: WordPhones[] | null;
  /** The rendered evidence block the prompt quotes. */
  evidence: string;
  prompt: string;
}

/** Args shared by the two routes. */
export interface FeedbackArgs {
  datasetUuid: string;
  attemptUuid: string;
  /** Checked against the attempt server-side; omitting it is always correct. */
  textUuid?: string | null;
  /** Empty means the app default configured in Settings → LLM. */
  model?: string | null;
  /** BCP-47 tag for the answer; the backend defaults to `en`. */
  language?: string | null;
}

/**
 * Receipt of a delegated feedback request: the mailbox row an attached agent
 * will claim, listen to the recording for, and answer. The report itself lands
 * later (via `read_aloud_save_feedback`) and reaches the card through the
 * `read-aloud-feedback-saved` event — this only confirms the ask was posted.
 */
export interface FeedbackRequestReceipt {
  mail_uuid: string;
  status: string;
  subject: string;
  recording?: string | null;
}

/**
 * Delegate this take to an agent that can hear it (T1): post the evidence, the
 * exact prompt and the recording's location to the agent mailbox. Resolves with
 * the mail receipt; throws the backend's message (already phrased for a person —
 * e.g. "This attempt's recording is not on this device…") when it cannot.
 */
export async function requestFeedback(args: FeedbackArgs): Promise<FeedbackRequestReceipt> {
  return invoke<FeedbackRequestReceipt>("read_aloud_request_feedback", {
    datasetUuid: args.datasetUuid,
    attemptUuid: args.attemptUuid,
    textUuid: args.textUuid ?? null,
    model: args.model?.trim() ? args.model : null,
    language: args.language ?? null,
  });
}

/** The evidence and the exact prompt, without running anything. */
export async function feedbackContext(args: FeedbackArgs): Promise<FeedbackContext> {
  return invoke<FeedbackContext>("read_aloud_feedback_context", {
    datasetUuid: args.datasetUuid,
    attemptUuid: args.attemptUuid,
    textUuid: args.textUuid ?? null,
    language: args.language ?? null,
  });
}

/**
 * Copy the exact prompt the app would send. The escape hatch for a provider the
 * app cannot reach: the answer comes back by hand, and a learner who pastes it
 * into an agent that can also hear the recording gets the same shape back.
 */
export async function copyFeedbackPrompt(args: FeedbackArgs): Promise<string> {
  const ctx = await feedbackContext(args);
  await navigator.clipboard.writeText(ctx.prompt);
  return ctx.prompt;
}

/**
 * A one-line provenance label: who wrote it and when.
 *
 * `source` is a machine value, and "llm" says nothing to a learner, so the label
 * names the actual origin: the model, or the agent that answered.
 */
export function feedbackProvenance(report: FeedbackReport): string {
  const who =
    report.source === "agent"
      ? report.model?.trim() || "an external agent"
      : report.model?.trim() || "the app's model";
  const when = formatStoredAt(report.created_at);
  return when ? `${who} · ${when}` : who;
}

/** `2025-06-01T12:34:56Z` → `1 June 2025, 12:34` — or empty when unparseable. */
function formatStoredAt(stamp: string): string {
  const trimmed = (stamp ?? "").trim();
  if (!trimmed) return "";
  // Stored stamps are SQLite's `YYYY-MM-DD HH:MM:SS` or RFC 3339. Only the
  // first form needs a nudge into something `Date` parses consistently.
  const iso = /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/.test(trimmed)
    ? `${trimmed.replace(" ", "T")}Z`
    : trimmed;
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleString(undefined, {
    day: "numeric",
    month: "long",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}
