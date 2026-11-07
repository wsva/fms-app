import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { arrayBufferToBase64 } from "@/lib/voice-input";
import type {
  Assessment,
  HubAssessResult,
  ReadAloudSubmitResult,
  ReadAttempt,
} from "./types";
import { beginAssessment, finishAssessment, logStep } from "./assessmentLog";

/** Both routes need the take on this machine; a synced row may not have one. */
const NO_RECORDING = "This recording is not on this device, so it cannot be assessed again.";

/** One word with the seconds it occupied — what `model_transcribe_detailed`
 *  reports when the model places words, and the finer of the two kinds of
 *  fluency evidence. */
export type WordTiming = { text: string; start: number; end: number };

/** One *phrase* with the seconds it occupied — what the same call reports when
 *  the model's timestamp ceiling is a segment (whisper). Pauses are detected on
 *  the waveform, so these spans carry the fluency axis; they only say less about
 *  where inside a phrase a silence fell. */
export type SegmentTiming = { text: string; start: number; end: number };

/**
 * Make an ended take durable *before* anything measures it: store the recording
 * and its attempt row, with the transcript this device already has (empty when
 * it has not transcribed yet). The returned attempt's `uuid` is what
 * [`measureAttempt`] writes the numbers onto.
 *
 * Saving comes first so a take survives whatever can go wrong in the measuring:
 * a hub call that takes minutes and gets cancelled, a model that fails to load,
 * an app that is closed mid-flight. What is left behind is a saved but
 * unmeasured attempt — one "Assess again" press away from its numbers, never a
 * lost recording.
 */
export function saveTake(
  datasetUuid: string,
  textUuid: string,
  wavBase64: string,
  recognized: string
): Promise<ReadAttempt> {
  return invoke<ReadAttempt>("read_aloud_save_take", {
    datasetUuid,
    textUuid,
    wavBase64,
    recognized,
  });
}

/**
 * The measure step both flows end in: write one transcript and the numbers
 * derived from it onto a saved attempt, in place. Pass `assessment` (the
 * `assessment` of a hub's `read_aloud_assess_take` reply) with `via` to store a
 * measurement taken elsewhere as taken, and this device runs no model at all.
 *
 * This is the backend's scoring seam for a fresh take and for "Assess again"
 * alike, so it carries the whole result — score, summary, axes, attempt and the
 * XP the measured number earned (XP is awarded here, not at save time).
 */
export function measureAttempt({
  datasetUuid,
  uuid,
  recognized,
  words,
  segments,
  assessment,
  via,
}: {
  datasetUuid: string;
  /** The saved attempt being measured. */
  uuid: string;
  recognized: string;
  words?: WordTiming[] | null;
  segments?: SegmentTiming[] | null;
  assessment?: Assessment | null;
  via?: string | null;
}): Promise<ReadAloudSubmitResult> {
  return invoke<ReadAloudSubmitResult>("read_aloud_rerecognize", {
    datasetUuid,
    uuid,
    recognized,
    words: words ?? null,
    segments: segments ?? null,
    // No passed measurement means this device scored it, so nothing is stored as
    // measured and no provenance key is written.
    assessment: assessment ?? null,
    via: via ?? null,
  });
}

/** Read the attempt's own recording off this disk, as the base64 both routes send. */
async function readTakeBase64(audioUrl: string): Promise<string> {
  logStep("Reading the recording from disk…");
  const res = await fetch(convertFileSrc(audioUrl));
  if (!res.ok) throw new Error(`Could not read the recording (HTTP ${res.status}).`);
  const bytes = await res.arrayBuffer();
  logStep(`Recording loaded (${(bytes.byteLength / (1024 * 1024)).toFixed(2)} MB)`);
  return arrayBufferToBase64(bytes);
}

/** What the transcript came out as, for the live log — the same line either way. */
function logTranscript(
  recognized: string,
  words: readonly { text: string }[] | null,
  segments?: readonly { text: string }[] | null
): void {
  const wordCount = recognized ? recognized.split(/\s+/).length : 0;
  const preview = recognized.length > 80 ? `${recognized.slice(0, 80)}…` : recognized;
  const placement = words
    ? `, ${words.length} timed word${words.length === 1 ? "" : "s"}`
    : segments?.length
      ? `, ${segments.length} timed phrase${segments.length === 1 ? "" : "s"} (the model places no words)`
      : ", no timings";
  logStep(`Transcript ready: ${wordCount} word${wordCount === 1 ? "" : "s"}${placement}${preview ? ` — "${preview}"` : ""}`);
}

/**
 * Re-assess an attempt from its existing recording: run STT again, then store
 * the new transcript and the assessment derived from it. The recording itself
 * is untouched — only what was heard, and the score that follows from it,
 * change. Shared by the attempts list (per-row button) and the attempt page
 * (toolbar button) so the two entry points cannot drift apart, and the way an
 * attempt that was saved unmeasured finally gets its numbers.
 *
 * Each stage is reported to the live assessment log (see `assessmentLog`), so
 * the otherwise silent wait is visible while it runs.
 *
 * Deliberately no `model_start` warm-up: that command loads the model currently
 * *selected* in the Models list, and after an idle unload that can differ from
 * the user's default. A re-assessment must not swap the active model underneath
 * itself — `model_transcribe_detailed` resolves the default when nothing is
 * loaded, and leaves a loaded model alone.
 */
export async function rerecognizeAttempt(
  datasetUuid: string,
  attempt: ReadAttempt
): Promise<ReadAloudSubmitResult> {
  if (!attempt.audio_url) {
    throw new Error(NO_RECORDING);
  }

  beginAssessment(`Assess again — attempt ${attempt.uuid.slice(0, 8)}`);
  const startedAt = Date.now();
  try {
    const wavBase64 = await readTakeBase64(attempt.audio_url);

    // Detailed (not plain) transcription: the attempt is re-scored, so it needs
    // the model's placement as well as the text — word timings when it places
    // words, phrase spans when it places phrases, either way enough for fluency.
    logStep("Transcribing with the current STT model (the first run also loads it)…");
    const detailed = await invoke<{
      text: string;
      words: WordTiming[];
      segments: SegmentTiming[];
      capabilities: { word_timings: boolean };
    }>("model_transcribe_detailed", { wavBase64 });
    const recognized = (detailed.text || "").trim();
    const words = detailed.capabilities?.word_timings && detailed.words?.length ? detailed.words : null;
    const segments = words?.length ? null : detailed.segments?.length ? detailed.segments : null;
    logTranscript(recognized, words, segments);

    logStep("Scoring accuracy, fluency and pronunciation (the first run loads the phoneme model)…");
    const result = await measureAttempt({ datasetUuid, uuid: attempt.uuid, recognized, words, segments });

    finishAssessment(
      true,
      `Done — transcript and score saved (${((Date.now() - startedAt) / 1000).toFixed(1)}s)`
    );
    return result;
  } catch (e) {
    finishAssessment(false, `Assess again failed: ${String(e)}`);
    throw e;
  }
}

/**
 * Measure a saved attempt with the hub's models: the take is posted to the hub's
 * `read_aloud_assess_take`, which transcribes it and measures the three axes,
 * and this device then stores what came back as measured
 * (docs/design/hub-mcp-compute-offload.md). The hub persists nothing — the
 * attempt row, its journal and its provenance stay here — so a follower without
 * a pronunciation model of its own still gets the full picture, and never loses
 * history it cannot re-record.
 *
 * `content` is the text's reference, sent as the fallback for a text the hub has
 * not received yet; the hub's `read_text` row wins when it has one.
 *
 * Returns `null` when the learner walked away mid-call: the HTTP request cannot
 * be aborted, so the reply is checked against `cancelled` and dropped rather
 * than written over whatever they chose to do instead. Every failure comes back
 * as an error for the caller to show — the take is already saved either way, so
 * nothing is lost by a failed hub call.
 */
export async function measureAttemptOnHub({
  datasetUuid,
  uuid,
  textUuid,
  wavBase64,
  content,
  milestones,
}: {
  datasetUuid: string;
  /** The saved attempt being measured. */
  uuid: string;
  textUuid: string;
  wavBase64: string;
  content: string;
  milestones: {
    /** Called as soon as the hub's reply is in, before this device stores it —
     *  the caller's status dialog turns to complete there rather than after. */
    onReplied?: () => void;
    /** True once the learner has walked away from this call. */
    cancelled: () => boolean;
  };
}): Promise<ReadAloudSubmitResult | null> {
  beginAssessment(`Measuring on the Hub — attempt ${uuid.slice(0, 8)}`);
  const startedAt = Date.now();
  try {
    logStep("Sending the recording to the hub, then waiting for it to transcribe and score…");
    const reply = await invoke<HubAssessResult>("hub_mcp_call", {
      tool: "read_aloud_assess_take",
      args: {
        dataset_uuid: datasetUuid,
        text_uuid: textUuid,
        wav_base64: wavBase64,
        content,
      },
    });
    if (milestones.cancelled()) {
      // Nothing was stored, so the attempt keeps the numbers it had.
      finishAssessment(false, "Cancelled — the hub's reply arrived after the learner walked away, so nothing was stored");
      return null;
    }
    milestones.onReplied?.();
    const recognized = (reply.recognized || "").trim();
    const words = reply.words?.length ? reply.words : null;
    const segments = words ? null : reply.segments?.length ? reply.segments : null;
    logTranscript(recognized, words, segments);

    logStep("Storing the hub's measurement (nothing is re-scored here)…");
    // The hub's placement rides along for symmetry with the local route; with
    // an assessment passed, the backend takes no model pass at all.
    const result = await measureAttempt({
      datasetUuid,
      uuid,
      recognized,
      words,
      segments,
      assessment: reply.assessment,
      via: "hub",
    });

    finishAssessment(
      true,
      `Done — hub measurement saved (${((Date.now() - startedAt) / 1000).toFixed(1)}s)`
    );
    return result;
  } catch (e) {
    finishAssessment(false, `Assess again failed: ${String(e)}`);
    throw e;
  }
}

/**
 * The same measurement for an attempt already on disk: its own recording is read
 * back and sent, every step else identical. Used by "Assess again" on a
 * follower, which has no in-memory take to offer.
 */
export async function rerecognizeAttemptOnHub(
  datasetUuid: string,
  attempt: ReadAttempt,
  content: string,
  milestones: {
    onReplied?: () => void;
    cancelled: () => boolean;
  }
): Promise<ReadAloudSubmitResult | null> {
  if (!attempt.audio_url) {
    throw new Error(NO_RECORDING);
  }
  const wavBase64 = await readTakeBase64(attempt.audio_url);
  return measureAttemptOnHub({
    datasetUuid,
    uuid: attempt.uuid,
    textUuid: attempt.text_uuid,
    wavBase64,
    content,
    milestones,
  });
}
