import { useCallback, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  arrayBufferToBase64,
  checkMicSupport,
  describeMicError,
  encodeWav,
  openMicCapture,
} from "@/lib/voice-input";
import { isTauri } from "@/lib/tauri";
import { logError, logInfo } from "@/lib/logger";

/** Log-page tag for the reading drawer's recording flow. */
const LOG_MODULE = "read-book-recorder";

export type RecorderResult = { wavBase64: string; text: string };

/**
 * Microphone recorder for the reading drawer.
 *
 * Records audio, converts it to 16kHz mono WAV, runs STT via `model_transcribe`,
 * and hands back BOTH the WAV (base64) and the recognized text so the caller can
 * persist the audio and show a diff.
 *
 * Capture is delegated to `openMicCapture`, which is what lets this work on the
 * Android WebView as well as on the desktop app.
 */
export function useRecorder(
  onResult: (r: RecorderResult) => void,
  onError: (msg: string) => void
) {
  const [recording, setRecording] = useState(false);
  const [processing, setProcessing] = useState(false);
  // Resolves the `stopped` gate created in `start`; `stop` triggers it.
  const requestStopRef = useRef<(() => void) | null>(null);

  // Keep the latest callbacks without re-creating `start`.
  const cbRef = useRef({ onResult, onError });
  cbRef.current = { onResult, onError };

  const start = useCallback(async () => {
    if (!isTauri()) {
      cbRef.current.onError("Recording needs the app; it is not available in the browser.");
      return;
    }
    const support = checkMicSupport();
    if (!support.ok) {
      logError(`recording: unsupported environment — ${support.reason}`, LOG_MODULE);
      cbRef.current.onError(support.reason ?? "Microphone unavailable");
      return;
    }

    try {
      // Ensure the STT model is loaded before recording.
      setProcessing(true);
      const status = await invoke<{ active_version: string | null }>("model_get_status");
      logInfo(`recording: model status = ${status.active_version ?? "nothing loaded"}`, LOG_MODULE);
      if (!status.active_version) {
        const loadedAt = Date.now();
        await invoke("model_start");
        logInfo(`recording: STT model loaded in ${Date.now() - loadedAt} ms`, LOG_MODULE);
      }
      setProcessing(false);

      const capture = await openMicCapture();
      logInfo(`recording: started (${capture.backend} backend)`, LOG_MODULE);
      const stopped = new Promise<void>((resolve) => {
        requestStopRef.current = resolve;
      });
      setRecording(true);

      // Everything after the press of "Stop" runs off that gate.
      void (async () => {
        try {
          await stopped;
          setProcessing(true);
          const samples = await capture.stop();
          if (samples.length === 0) {
            throw new Error("The microphone produced no audio at all — nothing to transcribe.");
          }
          const wav = encodeWav(samples, 16000);
          const wavBase64 = arrayBufferToBase64(wav);
          logInfo(
            `recording: ${(samples.length / 16000).toFixed(2)} s captured, sending ${(
              wav.byteLength / 1024
            ).toFixed(0)} KB WAV to model_transcribe`,
            LOG_MODULE
          );
          const transcribedAt = Date.now();
          const text = await invoke<string>("model_transcribe", { wavBase64 });
          logInfo(
            `recording: model_transcribe answered after ${Date.now() - transcribedAt} ms: "${text}"`,
            LOG_MODULE
          );
          cbRef.current.onResult({ wavBase64, text: (text || "").trim() });
        } catch (err) {
          const msg = describeMicError(err);
          logError(`recording: failed — ${msg}`, LOG_MODULE);
          cbRef.current.onError(msg);
        } finally {
          requestStopRef.current = null;
          setProcessing(false);
          setRecording(false);
        }
      })();
    } catch (err) {
      setProcessing(false);
      const msg = describeMicError(err);
      logError(`recording: could not start — ${msg}`, LOG_MODULE);
      cbRef.current.onError(msg);
    }
  }, []);

  const stop = useCallback(() => {
    requestStopRef.current?.();
  }, []);

  const toggle = useCallback(() => {
    if (processing) return;
    if (recording) stop();
    else start();
  }, [recording, processing, start, stop]);

  return { recording, processing, toggle };
}
