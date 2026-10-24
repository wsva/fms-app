import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { arrayBufferToBase64 } from "@/lib/voice-input";
import type { ReadAttempt } from "./types";

/**
 * Re-run STT over an attempt's existing recording and store the new transcript
 * and score. The recording itself is untouched — only what was heard, and the
 * assessment derived from it, change. Shared by the attempts list (per-row
 * button) and the attempt detail page (toolbar button) so the two entry points
 * cannot drift apart.
 *
 * Deliberately no `model_start` warm-up: that command loads the model currently
 * *selected* in the Models list, and after an idle unload that can differ from
 * the user's default. A re-recognition must not swap the active model
 * underneath itself — `model_transcribe_detailed` resolves the default when
 * nothing is loaded, and leaves a loaded model alone.
 */
export async function rerecognizeAttempt(
  datasetUuid: string,
  attempt: ReadAttempt
): Promise<void> {
  if (!attempt.audio_url) {
    throw new Error("This recording is not on this device, so it cannot be recognized again.");
  }

  const res = await fetch(convertFileSrc(attempt.audio_url));
  if (!res.ok) throw new Error(`Could not read the recording (HTTP ${res.status}).`);
  const wavBase64 = arrayBufferToBase64(await res.arrayBuffer());

  // Detailed (not plain) transcription: the attempt is re-scored, so it needs
  // the word timings that fluency is measured from as well as the text.
  const detailed = await invoke<{
    text: string;
    words: { text: string; start: number; end: number }[];
    capabilities: { word_timings: boolean };
  }>("model_transcribe_detailed", { wavBase64 });

  await invoke("read_aloud_rerecognize", {
    datasetUuid,
    uuid: attempt.uuid,
    recognized: (detailed.text || "").trim(),
    words: detailed.capabilities?.word_timings ? detailed.words : null,
  });
}
