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
const LOG_MODULE = "books-recorder";

export type RecorderResult = {
  wavBase64: string;
  text: string;
  /**
   * Word-level timings, when the loaded model reports them. Read-aloud scoring
   * uses them for fluency; a model that only places phrases omits them and
   * fills `segments` instead, and a model that stamps nothing at all omits both.
   */
  words?: RecorderWord[];
  /**
   * Phrase spans — the coarser fluency evidence of a model whose timestamp
   * ceiling is a segment (whisper). Absent whenever `words` are there: word
   * spans are strictly finer, so the phrase list would only muddy the label.
   */
  segments?: RecorderSegment[];
  /**
   * Set when the hook was built with `deferTranscribe`: the take ended and the
   * WAV is ready, but nothing was recognized — the caller routes the recording
   * (transcribe on-device via `transcribeTake`, or score it elsewhere).
   */
  captured?: boolean;
};

/** One word of the transcript with its position in the take, in seconds. */
export type RecorderWord = { text: string; start: number; end: number };

/** One phrase of it, with the seconds the model placed it over. */
export type RecorderSegment = { text: string; start: number; end: number };

/** What `model_transcribe_detailed` returns. */
type DetailedTranscript = {
  text: string;
  words: RecorderWord[];
  segments: RecorderSegment[];
  capabilities: { word_timings: boolean };
};

/** The placement a detailed run actually produced, as this hook reports it:
 *  the finer list when the model placed words, the phrase list when it only
 *  placed phrases, neither when it stamped nothing. */
function placement(detailed: DetailedTranscript): Pick<RecorderResult, "words" | "segments"> {
  return detailed.capabilities?.word_timings && detailed.words?.length
    ? { words: detailed.words }
    : detailed.segments?.length
      ? { segments: detailed.segments }
      : {};
}

/** Say what the model actually placed, so a log line never has to be decoded. */
function describePlacement(timings: Pick<RecorderResult, "words" | "segments">): string {
  return timings.words
    ? `${timings.words.length} word timings`
    : timings.segments
      ? `${timings.segments.length} phrase timings (the model places no words)`
      : "no timings";
}

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
 * Records audio, converts it to 16kHz mono WAV, runs STT via
 * `model_transcribe_detailed`, and hands back the WAV (base64), the recognized
 * text, and — when the model places the take in time at all — that placement:
 * per-word timings, or phrase spans for a model whose ceiling is a phrase. The
 * caller persists the audio and shows a diff; read-aloud scoring turns the
 * placement into fluency metrics.
 *
 * Capture is delegated to `openMicCapture`, which is what lets this work on the
 * Android WebView as well as on the desktop app.
 *
 * With `{ live: true }` the take is also recognized while it is being recorded:
 * the model is re-run on each finished stretch of audio and the transcript
 * accumulates in `liveText`, so the reader can see what was heard before the
 * take ends. The take itself still ends with `stop`, which recognizes the whole
 * recording in one pass and is what the caller is handed.
 *
 * Read-aloud routing (hub scoring) reshapes the two busy phases:
 * * `preparing` covers the press of Record until the microphone is open — the
 *   model preload included — so the overlay can say "Preparing…" instead of
 *   borrowing "Transcribing…" for a phase that has not transcribed anything.
 * * `preloadModel: false` skips the model check/load entirely (a follower that
 *   means to shed the STT work must not auto-load the model it is shedding);
 *   the live preview needs the on-device model, so it stays off in this mode
 *   while `liveSeconds` still ticks.
 * * `deferTranscribe: true` ends the take at the WAV: the caller receives
 *   `{ wavBase64, captured: true }` and runs `transcribeTake` itself when the
 *   recording is to be recognized on this device.
 */
export function useRecorder(
  onResult: (r: RecorderResult) => void,
  onError: (msg: string) => void,
  options?: { live?: boolean; deferTranscribe?: boolean; preloadModel?: boolean }
) {
  const live = options?.live === true;
  const deferTranscribe = options?.deferTranscribe === true;
  const preloadModel = options?.preloadModel !== false;
  const [recording, setRecording] = useState(false);
  const [processing, setProcessing] = useState(false);
  // Record was pressed; the microphone (and, unless shed, the model) is opening.
  const [preparing, setPreparing] = useState(false);
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
   * The final pass: same model, but answering with the take's placement as well
   * as the text. Runs once per take (the live preview keeps using the cheap
   * text-only call), because timings are only meaningful for the whole
   * recording.
   *
   * Exported as `transcribeTake` for a deferred caller: the local option of
   * the read-aloud scoring choice runs exactly this on-device path.
   */
  const transcribeDetailed = useCallback(
    (wavBase64: string) =>
      invoke<DetailedTranscript>("model_transcribe_detailed", { wavBase64 }),
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
      // Prepare: open the microphone, and — unless the caller sheds the STT
      // work — make sure the model is loaded before recording. This phase is
      // `preparing`, not `processing`: nothing has been transcribed yet.
      setPreparing(true);
      if (preloadModel) {
        const status = await invoke<{ active_version: string | null }>("model_get_status");
        logInfo(`recording: model status = ${status.active_version ?? "nothing loaded"}`, LOG_MODULE);
        if (!status.active_version) {
          const loadedAt = Date.now();
          await invoke("model_start");
          logInfo(`recording: STT model loaded in ${Date.now() - loadedAt} ms`, LOG_MODULE);
        }
      }

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
      setPreparing(false);
      setRecording(true);

      // Everything after the press of Stop runs off that gate.
      void (async () => {
        const startedAt = Date.now();
        const elapsed = setInterval(
          () => setLiveSeconds(Math.floor((Date.now() - startedAt) / 1000)),
          500
        );
        // The live loop recognizes on-device; a caller shedding the model work
        // cannot run it (it would auto-load exactly what it is shedding).
        if (live && preloadModel) void runLiveLoop(capture);
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
          if (deferTranscribe) {
            // Hand the recording to the caller unrouted-by-us: the silent-take
            // check below needs the model, so the caller's own transcribe or
            // hub score is where a silent take surfaces.
            logInfo(
              `recording: ${(samples.length / 16000).toFixed(2)} s captured as ${(
                wav.byteLength / 1024
              ).toFixed(0)} KB of WAV — transcription deferred to the caller`,
              LOG_MODULE
            );
            cbRef.current.onResult({ wavBase64, text: "", captured: true });
            return;
          }
          setProcessing(true);
          logInfo(
            `recording: ${(samples.length / 16000).toFixed(2)} s captured as ${(
              wav.byteLength / 1024
            ).toFixed(0)} KB of WAV — sending it to model_transcribe_detailed`,
            LOG_MODULE
          );
          const transcribedAt = Date.now();
          const detailed = await transcribeDetailed(wavBase64);
          const text = detailed.text ?? "";
          const timings = placement(detailed);
          logInfo(
            `recording: model_transcribe_detailed answered after ${
              Date.now() - transcribedAt
            } ms: "${text}" (${describePlacement(timings)})`,
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
          cbRef.current.onResult({ wavBase64, text: (text || "").trim(), ...timings });
        } catch (err) {
          const msg = describeMicError(err);
          logError(`recording: failed — ${msg}`, LOG_MODULE);
          cbRef.current.onError(msg);
        } finally {
          clearInterval(elapsed);
          requestStopRef.current = null;
          setProcessing(false);
          setPreparing(false);
          setRecording(false);
          setLiveBusy(false);
        }
      })();
    } catch (err) {
      setPreparing(false);
      setProcessing(false);
      const msg = describeMicError(err);
      logError(`recording: could not start — ${msg}`, LOG_MODULE);
      cbRef.current.onError(msg);
    }
  }, [live, preloadModel, deferTranscribe, runLiveLoop, transcribe, transcribeDetailed]);

  const stop = useCallback(() => {
    requestStopRef.current?.();
  }, []);

  /** The deferred local path: recognize a handed-back WAV on this device. */
  const transcribeTake = useCallback(
    async (wavBase64: string) => {
      const detailed = await transcribeDetailed(wavBase64);
      return {
        text: detailed.text ?? "",
        ...placement(detailed),
      };
    },
    [transcribeDetailed]
  );

  const toggle = useCallback(() => {
    if (preparing || processing) return;
    if (recording) stop();
    else start();
  }, [recording, preparing, processing, start, stop]);

  return { recording, processing, preparing, toggle, transcribeTake, liveText, liveBusy, liveSeconds };
}
