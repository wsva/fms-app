import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  analyzeLevel,
  arrayBufferToBase64,
  checkMicSupport,
  describeLevel,
  describeMicError,
  encodeWav,
  openMicCapture,
  type MicCapture,
} from "@/lib/voice-input";
import { isTauri } from "@/lib/tauri";
import { logError, logInfo } from "@/lib/logger";

/** Log-page tag for the reading drawer's recording flow. */
const LOG_MODULE = "read-book-recorder";

export type RecorderResult = { wavBase64: string; text: string };

// ---------------------------------------------------------------------------
// Live preview segmentation
// ---------------------------------------------------------------------------

/** How often the live loop looks for a finished stretch of audio. */
const LIVE_TICK_MS = 1000;
/** A stretch is never cut before this much new audio (1.8 s). */
const LIVE_MIN_FRAMES = 1.8 * 16000;
/** Past this much new audio (3 s), cut at the quietest gap that has been found. */
const LIVE_SOFT_FRAMES = 3 * 16000;
/** Cut at 6 s regardless, so a run of speech never stalls the preview. */
const LIVE_HARD_FRAMES = 6 * 16000;
/** Width of the window a cut point is measured over (30 ms at 16 kHz). */
const CUT_WINDOW = 480;
/** Mean square below which a 30 ms window is a gap between words, not speech. */
const LIVE_GAP_RMS = 0.02 * 0.02;

/**
 * Where this live stretch should end, in frames; 0 means "wait for more".
 *
 * The loaded model is a batch recognizer, so a live preview has to cut the take
 * into stretches and recognize them one by one. Where the cuts fall matters: a
 * cut inside a word hands the model two half-words and the preview shows them
 * as such. So the cut is taken at the quietest 30 ms window in the range, and
 * an early cut (before `LIVE_SOFT_FRAMES`) only when that window really is a gap
 * between words.
 */
function chooseLiveCut(fresh: Float32Array): number {
  if (fresh.length < LIVE_MIN_FRAMES) return 0;
  const to = Math.min(fresh.length, LIVE_HARD_FRAMES);
  let cut = 0;
  let quietest = Infinity;
  for (let start = LIVE_MIN_FRAMES; start + CUT_WINDOW <= to; start += CUT_WINDOW) {
    let sum = 0;
    for (let i = start; i < start + CUT_WINDOW; i++) sum += fresh[i] * fresh[i];
    const power = sum / CUT_WINDOW;
    if (power < quietest) {
      quietest = power;
      cut = start + CUT_WINDOW / 2;
    }
  }
  if (cut <= 0) return 0;
  if (fresh.length >= LIVE_HARD_FRAMES) return cut;
  if (fresh.length >= LIVE_SOFT_FRAMES || quietest <= LIVE_GAP_RMS) return cut;
  return 0;
}

/**
 * Microphone recorder for the reading drawer.
 *
 * Records audio, converts it to 16kHz mono WAV, runs STT via `model_transcribe`,
 * and hands back BOTH the WAV (base64) and the recognized text so the caller can
 * persist the audio and show a diff.
 *
 * Capture is delegated to `openMicCapture`, which is what lets this work on the
 * Android WebView as well as on the desktop app.
 *
 * With `{ live: true }` the take is also recognized while it is being recorded:
 * the model is re-run on each finished stretch of audio and the transcript
 * accumulates in `liveText`, so the reader can see what was heard before the
 * take ends. The take itself still ends with `stop`, which recognizes the whole
 * recording in one pass and is what the caller is handed.
 */
export function useRecorder(
  onResult: (r: RecorderResult) => void,
  onError: (msg: string) => void,
  options?: { live?: boolean }
) {
  const live = options?.live === true;
  const [recording, setRecording] = useState(false);
  const [processing, setProcessing] = useState(false);
  // Live preview: what the model has recognized of the take so far.
  const [liveText, setLiveText] = useState("");
  const [liveBusy, setLiveBusy] = useState(false);
  const [liveSeconds, setLiveSeconds] = useState(0);
  // Resolves the `stopped` gate created in `start`; `stop` triggers it.
  const requestStopRef = useRef<(() => void) | null>(null);
  // Live-preview bookkeeping — refs, because the loop reads them across awaits.
  const liveTextRef = useRef("");
  const committedRef = useRef(0);
  const liveStopRef = useRef(true);
  // Set when the pane goes away mid-take: the microphone is released and the
  // take is dropped instead of being scored for a text the user has left.
  const abandonRef = useRef(false);

  // Keep the latest callbacks without re-creating `start`.
  const cbRef = useRef({ onResult, onError });
  cbRef.current = { onResult, onError };

  const transcribe = useCallback(
    (samples: Float32Array) =>
      invoke<string>("model_transcribe", {
        wavBase64: arrayBufferToBase64(encodeWav(samples, 16000)),
      }),
    []
  );

  /**
   * Recognize the take while it is still being recorded.
   *
   * Each stretch the segmenter hands back is transcribed on its own and appended
   * to the preview; the running offset means no stretch is recognized twice. A
   * failed stretch is logged and skipped — a preview that loses one sentence is
   * still useful, and the take itself is untouched.
   */
  const runLiveLoop = useCallback(
    async (capture: MicCapture) => {
      liveStopRef.current = false;
      while (!liveStopRef.current) {
        await new Promise((resolve) => setTimeout(resolve, LIVE_TICK_MS));
        if (liveStopRef.current) break;
        // `samples()` returns the take from its start every time, so the offset
        // committed so far is an index into the recording as a whole.
        const fresh = capture.samples().subarray(committedRef.current);
        const cut = chooseLiveCut(fresh);
        if (cut <= 0) continue;
        setLiveBusy(true);
        try {
          const piece = (await transcribe(fresh.slice(0, cut))).trim();
          committedRef.current += cut;
          if (piece) {
            liveTextRef.current = liveTextRef.current
              ? `${liveTextRef.current} ${piece}`
              : piece;
            setLiveText(liveTextRef.current);
            logInfo(
              `live: "${piece}" — ${(committedRef.current / 16000).toFixed(1)} s recognized so far`,
              LOG_MODULE
            );
          }
        } catch (err) {
          logError(
            `live: this stretch could not be recognized — ${describeMicError(err)}`,
            LOG_MODULE
          );
        } finally {
          setLiveBusy(false);
        }
      }
    },
    [transcribe]
  );

  // Leaving the pane mid-take used to keep the microphone open (and, now, the
  // live loop running against it). Release the device and drop the take instead
  // of recording into nothing.
  useEffect(() => {
    return () => {
      liveStopRef.current = true;
      abandonRef.current = true;
      requestStopRef.current?.();
    };
  }, []);

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

      const capture = await openMicCapture({ live });
      logInfo(
        `recording: started (${capture.backend} backend${live ? ", live preview on" : ""})`,
        LOG_MODULE
      );
      // Fresh bookkeeping for this take.
      liveTextRef.current = "";
      committedRef.current = 0;
      abandonRef.current = false;
      setLiveText("");
      setLiveSeconds(0);

      const stopped = new Promise<void>((resolve) => {
        requestStopRef.current = resolve;
      });
      setRecording(true);

      // Everything after the press of Stop runs off that gate.
      void (async () => {
        const startedAt = Date.now();
        const elapsed = setInterval(
          () => setLiveSeconds(Math.floor((Date.now() - startedAt) / 1000)),
          500
        );
        if (live) void runLiveLoop(capture);
        try {
          await stopped;
          liveStopRef.current = true;
          setProcessing(true);
          const samples = await capture.stop();
          if (abandonRef.current) {
            logInfo("recording: the pane went away — the take was released", LOG_MODULE);
            return;
          }
          if (samples.length === 0) {
            throw new Error("The microphone produced no audio at all — nothing to transcribe.");
          }
          const level = analyzeLevel(samples);
          logInfo(
            `recording: signal level ${describeLevel(level)} from "${capture.device}" — ${
              level.silent ? "no signal: this microphone delivered silence" : "signal present"
            }`,
            LOG_MODULE
          );
          const wav = encodeWav(samples, 16000);
          const wavBase64 = arrayBufferToBase64(wav);
          logInfo(
            `recording: ${(samples.length / 16000).toFixed(2)} s captured as ${(
              wav.byteLength / 1024
            ).toFixed(0)} KB of WAV — sending it to model_transcribe`,
            LOG_MODULE
          );
          const transcribedAt = Date.now();
          const text = await transcribe(samples);
          logInfo(
            `recording: model_transcribe answered after ${Date.now() - transcribedAt} ms: "${text}"`,
            LOG_MODULE
          );
          // A silent take is worthless as a recording, so report it. A take with
          // signal is kept even when the model found no words in it — the audio
          // is the point here, the transcript is only a diff helper.
          if (!text.trim() && level.silent) {
            throw new Error(
              `The microphone "${capture.device}" captured only silence (${describeLevel(level)}), ` +
                "so there is nothing to save. Pick your real microphone under Settings \u2192 Microphone \u2014 " +
                "virtual microphones from Steam, OBS or Voicemeeter often hold the system default slot."
            );
          }
          cbRef.current.onResult({ wavBase64, text: (text || "").trim() });
        } catch (err) {
          const msg = describeMicError(err);
          logError(`recording: failed — ${msg}`, LOG_MODULE);
          cbRef.current.onError(msg);
        } finally {
          clearInterval(elapsed);
          requestStopRef.current = null;
          setProcessing(false);
          setRecording(false);
          setLiveBusy(false);
        }
      })();
    } catch (err) {
      setProcessing(false);
      const msg = describeMicError(err);
      logError(`recording: could not start — ${msg}`, LOG_MODULE);
      cbRef.current.onError(msg);
    }
  }, [live, runLiveLoop, transcribe]);

  const stop = useCallback(() => {
    requestStopRef.current?.();
  }, []);

  const toggle = useCallback(() => {
    if (processing) return;
    if (recording) stop();
    else start();
  }, [recording, processing, start, stop]);

  return { recording, processing, toggle, liveText, liveBusy, liveSeconds };
}
