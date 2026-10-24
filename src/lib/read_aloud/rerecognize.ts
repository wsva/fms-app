import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { arrayBufferToBase64 } from "@/lib/voice-input";
import type { ReadAttempt } from "./types";
import { beginAssessment, finishAssessment, logStep } from "./assessmentLog";

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
    throw new Error("This recording is not on this device, so it cannot be assessed again.");
  }

  beginAssessment(`Assess again — attempt ${attempt.uuid.slice(0, 8)}`);
  const startedAt = Date.now();
  try {
    logStep("Reading the recording from disk…");
    const res = await fetch(convertFileSrc(attempt.audio_url));
    if (!res.ok) throw new Error(`Could not read the recording (HTTP ${res.status}).`);
    const bytes = await res.arrayBuffer();
    logStep(`Recording loaded (${(bytes.byteLength / (1024 * 1024)).toFixed(2)} MB)`);
    const wavBase64 = arrayBufferToBase64(bytes);

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
    const wordCount = recognized ? recognized.split(/\s+/).length : 0;
    const preview = recognized.length > 80 ? `${recognized.slice(0, 80)}…` : recognized;
    logStep(
      `Transcript ready: ${wordCount} word${wordCount === 1 ? "" : "s"}` +
        (words ? `, ${words.length} timed` : ", no word timings") +
        (preview ? ` — "${preview}"` : "")
    );

    logStep("Scoring accuracy, fluency and pronunciation (the first run loads the phoneme model)…");
    await invoke("read_aloud_rerecognize", {
      datasetUuid,
      uuid: attempt.uuid,
      recognized,
      words,
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
