"use client";

/**
 * Zoom-style microphone test, inline in Settings → Microphone.
 *
 * "Start test" opens the configured microphone and shows a live segmented dB
 * bar while you speak; "Stop" finalises the recording, reports the measured
 * peak level (silence = the wrong device), and offers a player so you can
 * replay the take and judge the clarity yourself.
 *
 * Two capture backends keep it usable on every webview, mirroring
 * `voice-input.ts`: MediaRecorder when available (desktop), raw Web Audio PCM
 * encoded to WAV when the container path is unreliable (Android WebView).
 * The level meter itself always runs on an AnalyserNode, so the bar you see
 * while speaking is independent of which backend produced the playback file.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { Mic, Square, RotateCcw } from "lucide-react";
import { logError, logInfo } from "@/lib/logger";
import {
  analyzeLevel,
  blobToSamples,
  checkMicSupport,
  describeLevel,
  describeMicError,
  encodeWav,
  getPreferredMicDeviceId,
  isRecording,
  openMicStream,
  type LevelInfo,
} from "@/lib/voice-input";

const LOG_MODULE = "mic-test";

// ---------------------------------------------------------------------------
// Meter geometry: 24 segments spanning -60..0 dBFS, green → amber → red.
// ---------------------------------------------------------------------------

const SEGMENT_COUNT = 24;
const DB_FLOOR = -60;
const DB_CEILING = 0;
/** ≈ -50 dBFS peak: the same "digital silence" cutoff the dictation path uses. */
const SILENT_PEAK = 0.0032;

function segmentClass(index: number): string {
  if (index >= SEGMENT_COUNT - 2) return "bg-red-500";
  if (index >= SEGMENT_COUNT - 6) return "bg-amber-400";
  return "bg-green-500";
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type TestPhase = "idle" | "testing" | "finished";

type TestResult = {
  level: LevelInfo;
  device: string;
  backend: "mediarecorder" | "pcm";
  durationSec: number;
  /** Object URL of the recorded take, revoked on restart/unmount. */
  playbackUrl: string;
};

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function MicTestPanel() {
  const [phase, setPhase] = useState<TestPhase>("idle");
  const [error, setError] = useState("");
  const [result, setResult] = useState<TestResult | null>(null);

  // Everything the rAF loop and the stop path touch without a re-render.
  const streamRef = useRef<MediaStream | null>(null);
  const ctxRef = useRef<AudioContext | null>(null);
  const rafRef = useRef<number | null>(null);
  const recorderRef = useRef<MediaRecorder | null>(null);
  // Raw PCM fallback chunks (Android WebView without a decodable MediaRecorder).
  const pcmChunksRef = useRef<Float32Array[]>([]);
  const pcmRateRef = useRef(48000);
  // Blob list for the MediaRecorder path, kept out of React state.
  const recorderBlobsRef = useRef<Blob[]>([]);
  const maxPeakRef = useRef(0);
  const startedAtRef = useRef(0);

  const segmentsRef = useRef<(HTMLDivElement | null)[]>([]);
  const liveLabelRef = useRef<HTMLSpanElement | null>(null);
  const urlRef = useRef<string>("");

  const revokeUrl = useCallback(() => {
    if (urlRef.current) {
      URL.revokeObjectURL(urlRef.current);
      urlRef.current = "";
    }
  }, []);

  const releaseMic = useCallback(() => {
    if (rafRef.current !== null) {
      cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
    }
    streamRef.current?.getTracks().forEach((t) => t.stop());
    streamRef.current = null;
    if (ctxRef.current && ctxRef.current.state !== "closed") void ctxRef.current.close();
    ctxRef.current = null;
  }, []);

  // Abandon a forgotten test cleanly; the mic LED must not outlive the page.
  useEffect(() => {
    return () => {
      if (recorderRef.current && recorderRef.current.state !== "inactive") {
        recorderRef.current.stop();
      }
      releaseMic();
      revokeUrl();
    };
  }, [releaseMic, revokeUrl]);

  // ---- Live meter ----------------------------------------------------------

  function paintMeter(dbfs: number) {
    for (let i = 0; i < SEGMENT_COUNT; i++) {
      const el = segmentsRef.current[i];
      if (!el) continue;
      const threshold = DB_FLOOR + (i / SEGMENT_COUNT) * (DB_CEILING - DB_FLOOR);
      el.style.opacity = dbfs >= threshold ? "1" : "0.15";
    }
    if (liveLabelRef.current) {
      liveLabelRef.current.textContent =
        dbfs < DB_FLOOR ? "—" : `${Math.max(-99, Math.round(dbfs))} dB`;
    }
  }

  function startMeterLoop(analyser: AnalyserNode) {
    const buffer = new Float32Array(analyser.fftSize);
    const tick = () => {
      analyser.getFloatTimeDomainData(buffer);
      let peak = 0;
      for (let i = 0; i < buffer.length; i++) {
        const abs = buffer[i] < 0 ? -buffer[i] : buffer[i];
        if (abs > peak) peak = abs;
      }
      if (peak > maxPeakRef.current) maxPeakRef.current = peak;
      paintMeter(peak > 0 ? 20 * Math.log10(peak) : DB_FLOOR - 1);
      rafRef.current = requestAnimationFrame(tick);
    };
    rafRef.current = requestAnimationFrame(tick);
  }

  // ---- Start / stop --------------------------------------------------------

  const handleStart = useCallback(async () => {
    setError("");
    if (phase === "testing") return;
    revokeUrl();
    setResult(null);

    const support = checkMicSupport();
    if (!support.ok) {
      setError(support.reason ?? "Microphone unavailable.");
      return;
    }
    if (isRecording()) {
      setError("Voice input is recording right now — finish that first, then test.");
      return;
    }

    let stream: MediaStream;
    try {
      stream = await openMicStream(getPreferredMicDeviceId());
    } catch (err) {
      const msg = describeMicError(err);
      setError(msg);
      logError(`mic test: could not open the microphone — ${msg}`, LOG_MODULE);
      return;
    }
    streamRef.current = stream;

    const ctx = new AudioContext();
    try {
      await ctx.resume();
    } catch {
      /* Autoplay policy variations: the meter still runs once the graph is pulled. */
    }
    ctxRef.current = ctx;

    const analyser = ctx.createAnalyser();
    analyser.fftSize = 2048;
    const source = ctx.createMediaStreamSource(stream);
    // Muted gain to the destination: Chromium only pulls graphs that reach it,
    // and the zero gain keeps the mic out of the speakers (no feedback loop).
    const sink = ctx.createGain();
    sink.gain.value = 0;
    source.connect(analyser);
    analyser.connect(sink);
    sink.connect(ctx.destination);

    maxPeakRef.current = 0;
    startedAtRef.current = Date.now();
    pcmChunksRef.current = [];
    pcmRateRef.current = ctx.sampleRate;

    if (typeof MediaRecorder !== "undefined") {
      const recorder = new MediaRecorder(stream);
      const blobs: Blob[] = [];
      recorderBlobsRef.current = blobs;
      recorder.ondataavailable = (e) => {
        if (e.data.size > 0) blobs.push(e.data);
      };
      recorderRef.current = recorder;
      recorder.start();
    } else {
      const processor = ctx.createScriptProcessor(4096, 1, 1);
      processor.onaudioprocess = (e) => {
        pcmChunksRef.current.push(new Float32Array(e.inputBuffer.getChannelData(0)));
      };
      source.connect(processor);
      processor.connect(sink);
    }

    paintMeter(DB_FLOOR - 1);
    startMeterLoop(analyser);
    setPhase("testing");
    logInfo(
      `mic test: started on "${stream.getAudioTracks()[0]?.label ?? "unnamed"}" (${ctx.sampleRate} Hz, ${
        recorderRef.current ? "MediaRecorder" : "raw PCM"
      } backend)`,
      LOG_MODULE
    );
  }, [phase, revokeUrl]);

  const handleStop = useCallback(async () => {
    if (rafRef.current !== null) {
      cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
    }

    const device = streamRef.current?.getAudioTracks()[0]?.label || "selected microphone";
    const backend: "mediarecorder" | "pcm" = recorderRef.current ? "mediarecorder" : "pcm";
    const elapsedSec = (Date.now() - startedAtRef.current) / 1000;

    // Finalise the recording before releasing the mic so the last bytes land.
    let blob: Blob | null = null;
    let samples: Float32Array | null = null;
    let sampleRate = pcmRateRef.current;

    if (recorderRef.current) {
      const recorder = recorderRef.current;
      recorderRef.current = null;
      const blobs = recorderBlobsRef.current;
      recorderBlobsRef.current = [];
      const collect = new Promise<Blob>((resolve) => {
        recorder.onstop = () => resolve(new Blob(blobs, { type: blobs[0]?.type || "audio/webm" }));
        if (recorder.state === "inactive") {
          resolve(new Blob(blobs, { type: blobs[0]?.type || "audio/webm" }));
        } else {
          recorder.stop();
        }
      });
      // A webview whose MediaRecorder never fires `stop` must not hang the test;
      // after 5 s the chunks collected so far are good enough to listen back.
      blob = await Promise.race([
        collect,
        new Promise<Blob>((resolve) =>
          setTimeout(
            () =>
              resolve(
                new Blob(blobs, {
                  type: blobs[0]?.type || "audio/webm",
                })
              ),
            5000
          )
        ),
      ]);
    } else {
      const chunks = pcmChunksRef.current.filter((c) => c.length > 0);
      const total = chunks.reduce((n, c) => n + c.length, 0);
      const merged = new Float32Array(total);
      let offset = 0;
      for (const c of chunks) {
        merged.set(c, offset);
        offset += c.length;
      }
      samples = merged;
    }

    releaseMic();

    // Prefer the finished audio for the verdict (exactly what playback contains);
    // a container this webview cannot decode falls back to the live meter peak.
    let level: LevelInfo;
    let durationSec = elapsedSec;
    if (samples) {
      level = analyzeLevel(samples);
      durationSec = samples.length / sampleRate;
    } else {
      try {
        samples = await blobToSamples(blob!);
        sampleRate = 16000;
        level = analyzeLevel(samples);
        durationSec = samples.length / 16000;
      } catch (err) {
        const peak = maxPeakRef.current;
        level = {
          peak,
          rms: 0,
          dbfs: peak > 0 ? Math.round(20 * Math.log10(peak)) : -120,
          silent: peak < SILENT_PEAK,
        };
        logError(
          `mic test: could not decode its own recording (${err instanceof Error ? err.message : String(err)}) — judging from the live meter peak instead`,
          LOG_MODULE
        );
      }
    }

    let playbackUrl = "";
    if (blob) {
      playbackUrl = URL.createObjectURL(blob);
    } else if (samples && samples.length > 0) {
      playbackUrl = URL.createObjectURL(
        new Blob([encodeWav(samples, sampleRate)], { type: "audio/wav" })
      );
    }
    if (playbackUrl) urlRef.current = playbackUrl;

    logInfo(
      `mic test: finished — ${durationSec.toFixed(1)} s, ${describeLevel(level)}, ${backend} backend, "${device}"`,
      LOG_MODULE
    );
    setResult({ level, device, backend, durationSec, playbackUrl });
    setPhase("finished");
  }, [releaseMic]);

  const handleReset = useCallback(() => {
    revokeUrl();
    setResult(null);
    setError("");
    setPhase("idle");
    paintMeter(DB_FLOOR - 1);
  }, [revokeUrl]);

  // ---- Verdict wording -----------------------------------------------------

  function verdict(): { tone: "ok" | "bad" | "warn"; text: string } {
    if (!result) return { tone: "ok", text: "" };
    if (result.level.silent) {
      return {
        tone: "bad",
        text:
          `No signal: "${result.device}" delivered silence (${describeLevel(result.level)}). ` +
          "Pick your real microphone above — virtual devices (Steam, OBS, Voicemeeter) often sit in the default slot.",
      };
    }
    if (result.level.peak > 0.98) {
      return {
        tone: "warn",
        text:
          `Very loud — the peak hits full scale (${describeLevel(result.level)}), so the recording may be clipping. ` +
          "Lower the input volume or move a bit away from the microphone.",
      };
    }
    return {
      tone: "ok",
      text: `Signal detected (${describeLevel(result.level)}). Play the recording below to check whether it sounds clear.`,
    };
  }

  const v = result ? verdict() : null;

  // ---- Render ---------------------------------------------------------------

  return (
    <div className="mt-6 pt-6 border-t border-border-light">
      <h3 className="text-base font-semibold mb-1 flex items-center gap-2">
        <Mic size={16} /> Test microphone
      </h3>
      <p className="text-text-secondary text-sm mb-3">
        Like the audio test in a meeting app: press start, speak normally for a few seconds, and
        watch the level bar react. After stopping you can replay the recording to judge its clarity.
      </p>

      {error && <p className="text-sm text-red-600 mb-3">{error}</p>}

      {/* Live dB meter */}
      <div
        className={`flex items-center gap-3 mb-3 ${phase === "testing" ? "" : "opacity-60"}`}
        aria-label="Microphone level"
      >
        <div className="flex-1 flex gap-[3px] h-5 items-stretch max-w-md">
          {Array.from({ length: SEGMENT_COUNT }, (_, i) => (
            <div
              key={i}
              ref={(el) => {
                segmentsRef.current[i] = el;
              }}
              className={`flex-1 rounded-[2px] transition-opacity duration-75 ${segmentClass(i)}`}
              style={{ opacity: 0.15 }}
            />
          ))}
        </div>
        <span
          ref={liveLabelRef}
          className="text-xs font-mono text-text-secondary w-14 text-right shrink-0"
        >
          —
        </span>
      </div>

      {/* Controls */}
      <div className="flex items-center gap-2">
        {phase !== "testing" && (
          <button
            type="button"
            className="px-4 py-2 rounded-md font-medium cursor-pointer transition-colors bg-accent-bg text-white hover:bg-accent-bg-hover flex items-center gap-2"
            onClick={handleStart}
          >
            <Mic size={15} /> {phase === "finished" ? "Test again" : "Start test"}
          </button>
        )}
        {phase === "testing" && (
          <button
            type="button"
            className="px-4 py-2 rounded-md font-medium cursor-pointer transition-colors bg-red-600 text-white hover:bg-red-700 flex items-center gap-2"
            onClick={handleStop}
          >
            <Square size={14} /> Stop &amp; listen back
          </button>
        )}
        {phase === "finished" && (
          <button
            type="button"
            className="px-3 py-1.5 text-xs rounded-md border border-border-light hover:bg-bg-hover cursor-pointer flex items-center gap-1.5"
            onClick={handleReset}
          >
            <RotateCcw size={13} /> Clear
          </button>
        )}
      </div>

      {/* Result: verdict + playback */}
      {result && v && (
        <div className="mt-4 max-w-md">
          <p
            className={`text-sm mb-2 ${
              v.tone === "ok"
                ? "text-green-600"
                : v.tone === "warn"
                  ? "text-amber-600"
                  : "text-red-600"
            }`}
          >
            {v.text}
          </p>
          <p className="text-xs text-text-tertiary mb-2">
            {result.durationSec.toFixed(1)} s recorded from &quot;{result.device}&quot; via{" "}
            {result.backend === "mediarecorder" ? "MediaRecorder" : "raw PCM"}
          </p>
          {result.playbackUrl ? (
            <audio controls preload="auto" className="w-full" src={result.playbackUrl} />
          ) : (
            <p className="text-sm text-text-secondary">
              The recording could not be prepared for playback on this webview — the level verdict
              above still stands.
            </p>
          )}
        </div>
      )}
    </div>
  );
}
