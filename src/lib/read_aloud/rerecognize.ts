import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { arrayBufferToBase64 } from "@/lib/voice-input";
import type { HubAssessResult, ReadAttempt } from "./types";
import { beginAssessment, finishAssessment, logStep } from "./assessmentLog";

/** Both routes need the take on this machine; a synced row may not have one. */
const NO_RECORDING = "This recording is not on this device, so it cannot be assessed again.";

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
function logTranscript(recognized: string, words: readonly { text: string }[] | null): void {
  const wordCount = recognized ? recognized.split(/\s+/).length : 0;
  const preview = recognized.length > 80 ? `${recognized.slice(0, 80)}…` : recognized;
  logStep(
    `Transcript ready: ${wordCount} word${wordCount === 1 ? "" : "s"}` +
      (words ? `, ${words.length} timed` : ", no word timings") +
      (preview ? ` — "${preview}"` : "")
  );
}

/**
 * Re-assess an attempt from its existing recording: run STT again, then store
 * the new transcript and the assessment derived from it. The recording itself
 * is untouched — only what was heard, and the score that follows from it,
 * change. Shared by the attempts list (per-row button) and the attempt page
 * (toolbar button) so the two entry points cannot drift apart.
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
): Promise<void> {
  if (!attempt.audio_url) {
    throw new Error(NO_RECORDING);
  }

  beginAssessment(`Assess again — attempt ${attempt.uuid.slice(0, 8)}`);
  const startedAt = Date.now();
  try {
    const wavBase64 = await readTakeBase64(attempt.audio_url);

    // Detailed (not plain) transcription: the attempt is re-scored, so it needs
    // the word timings that fluency is measured from as well as the text.
    logStep("Transcribing with the current STT model (the first run also loads it)…");
    const detailed = await invoke<{
      text: string;
      words: { text: string; start: number; end: number }[];
      capabilities: { word_timings: boolean };
    }>("model_transcribe_detailed", { wavBase64 });
    const recognized = (detailed.text || "").trim();
    const words = detailed.capabilities?.word_timings ? detailed.words : null;
    logTranscript(recognized, words);

    logStep("Scoring accuracy, fluency and pronunciation (the first run loads the phoneme model)…");
    await invoke("read_aloud_rerecognize", {
      datasetUuid,
      uuid: attempt.uuid,
      recognized,
      words,
      // No passed measurement: this device scored it, so nothing is stored as
      // measured and no provenance key is written.
      assessment: null,
      via: null,
    });

    finishAssessment(
      true,
      `Done — transcript and score saved (${((Date.now() - startedAt) / 1000).toFixed(1)}s)`
    );
  } catch (e) {
    finishAssessment(false, `Assess again failed: ${String(e)}`);
    throw e;
  }
}

/**
 * The same re-assessment with the hub's models doing the work: the recording is
 * posted to the hub's `read_aloud_assess_take`, which transcribes it and
 * measures the three axes, and this device then stores what came back as
 * measured (docs/design/hub-mcp-compute-offload.md). The hub persists nothing —
 * the attempt row, its journal and its provenance stay here — so a follower
 * without a pronunciation model of its own still gets the full picture, and
 * never loses history it cannot re-record.
 *
 * `content` is the text's own reference, sent as the fallback for a text the
 * hub has not received yet; the hub's `read_text` row wins when it has one.
 * Every failure simply comes back as an error for the caller to show: the take
 * is unchanged, so scoring it on this device stays one tap away.
 */
export async function rerecognizeAttemptOnHub(
  datasetUuid: string,
  attempt: ReadAttempt,
  content: string,
  milestones: {
    /** Called as soon as the hub's reply is in, before this device stores it —
     *  the caller's status dialog turns to complete there rather than after. */
    onReplied?: () => void;
    /** True once the learner has walked away from this call. The HTTP request
     *  cannot be aborted, so the reply is checked against it here and dropped:
     *  a cancelled re-assessment must not silently re-write the attempt row. */
    cancelled: () => boolean;
  }
): Promise<void> {
  if (!attempt.audio_url) {
    throw new Error(NO_RECORDING);
  }

  beginAssessment(`Assessing again on the Hub — attempt ${attempt.uuid.slice(0, 8)}`);
  const startedAt = Date.now();
  try {
    const wavBase64 = await readTakeBase64(attempt.audio_url);

    logStep("Sending the recording to the hub, then waiting for it to transcribe and score…");
    const reply = await invoke<HubAssessResult>("hub_mcp_call", {
      tool: "read_aloud_assess_take",
      args: {
        dataset_uuid: datasetUuid,
        text_uuid: attempt.text_uuid,
        wav_base64: wavBase64,
        content,
      },
    });
    if (milestones.cancelled()) {
      // Nothing was stored, so the attempt keeps the numbers it had.
      finishAssessment(false, "Cancelled — the hub's reply arrived after the learner walked away, so nothing was stored");
      return;
    }
    milestones.onReplied?.();
    const recognized = (reply.recognized || "").trim();
    const words = reply.words?.length ? reply.words : null;
    logTranscript(recognized, words);

    logStep("Storing the hub's measurement (nothing is re-scored here)…");
    await invoke("read_aloud_rerecognize", {
      datasetUuid,
      uuid: attempt.uuid,
      recognized,
      // The hub's word timings ride along for symmetry with the local route;
      // with an assessment passed, the backend takes no model pass at all.
      words,
      assessment: reply.assessment,
      via: "hub",
    });

    finishAssessment(
      true,
      `Done — hub measurement saved (${((Date.now() - startedAt) / 1000).toFixed(1)}s)`
    );
  } catch (e) {
    finishAssessment(false, `Assess again failed: ${String(e)}`);
    throw e;
  }
}
