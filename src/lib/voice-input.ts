/**
 * Voice input utility: records microphone audio, converts to 16kHz mono 16-bit PCM WAV,
 * and base64-encodes for the Rust `model_transcribe` command.
 *
 * Supports global usage: tracks the currently focused input element and provides
 * a subscription-based state system for toolbar buttons, plus a global Ctrl+C shortcut.
 *
 * Capture runs on one of two backends (see `openMicCapture`): the MediaRecorder
 * pipeline the desktop app has always used, and a raw Web Audio PCM path for
 * webviews — i.e. the Android one — that cannot record or decode a compressed
 * container. Which microphone is opened follows Settings → Microphone; empty
 * means the system default.
 */

import { isMobileApp } from "@/lib/platform";
import { logError, logInfo } from "@/lib/logger";

/** Module tag used on the Log page for every step of the voice-input pipeline. */
const LOG_MODULE = "voice-input";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type VoiceState = "idle" | "recording" | "processing";

export type VoiceCallbacks = {
  onStateChange: (state: VoiceState) => void;
  onResult: (text: string) => void;
  onError: (error: string) => void;
};

// ---------------------------------------------------------------------------
// WAV encoder (16kHz, mono, 16-bit PCM)
// ---------------------------------------------------------------------------

export function encodeWav(samples: Float32Array, sampleRate: number): ArrayBuffer {
  const numChannels = 1;
  const bitsPerSample = 16;
  const byteRate = sampleRate * numChannels * (bitsPerSample / 8);
  const blockAlign = numChannels * (bitsPerSample / 8);
  const dataSize = samples.length * blockAlign;
  const bufferSize = 44 + dataSize;
  const buffer = new ArrayBuffer(bufferSize);
  const view = new DataView(buffer);

  writeString(view, 0, "RIFF");
  view.setUint32(4, bufferSize - 8, true);
  writeString(view, 8, "WAVE");
  writeString(view, 12, "fmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, numChannels, true);
  view.setUint32(24, sampleRate, true);
  view.setUint32(28, byteRate, true);
  view.setUint16(32, blockAlign, true);
  view.setUint16(34, bitsPerSample, true);
  writeString(view, 36, "data");
  view.setUint32(40, dataSize, true);

  let offset = 44;
  for (let i = 0; i < samples.length; i++) {
    const s = Math.max(-1, Math.min(1, samples[i]));
    view.setInt16(offset, s < 0 ? s * 0x8000 : s * 0x7fff, true);
    offset += 2;
  }

  return buffer;
}

function writeString(view: DataView, offset: number, str: string) {
  for (let i = 0; i < str.length; i++) {
    view.setUint8(offset + i, str.charCodeAt(i));
  }
}

// ---------------------------------------------------------------------------
// Resample to 16kHz mono using OfflineAudioContext
// ---------------------------------------------------------------------------

export async function resampleTo16k(audioBuffer: AudioBuffer): Promise<Float32Array> {
  const targetRate = 16000;
  if (audioBuffer.sampleRate === targetRate && audioBuffer.numberOfChannels === 1) {
    return audioBuffer.getChannelData(0);
  }
  const offline = new OfflineAudioContext(
    1,
    Math.ceil(audioBuffer.duration * targetRate),
    targetRate
  );
  const source = offline.createBufferSource();
  source.buffer = audioBuffer;
  source.connect(offline.destination);
  source.start();
  const rendered = await offline.startRendering();
  return rendered.getChannelData(0);
}

// ---------------------------------------------------------------------------
// Blob → Float32Array (16kHz mono) via AudioContext
// ---------------------------------------------------------------------------

export async function blobToSamples(blob: Blob): Promise<Float32Array> {
  const arrayBuffer = await blob.arrayBuffer();
  const audioContext = new AudioContext();
  const audioBuffer = await audioContext.decodeAudioData(arrayBuffer);
  await audioContext.close();
  return resampleTo16k(audioBuffer);
}

// ---------------------------------------------------------------------------
// ArrayBuffer → base64
// ---------------------------------------------------------------------------

export function arrayBufferToBase64(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  let binary = "";
  for (let i = 0; i < bytes.byteLength; i++) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary);
}

// ---------------------------------------------------------------------------
// Capture backends
// ---------------------------------------------------------------------------

/** An open microphone. `stop()` finalises the recording and returns 16 kHz mono PCM. */
export type MicCapture = {
  stop: () => Promise<Float32Array>;
  backend: "mediarecorder" | "pcm";
  /** Human-readable name of the device actually capturing, for error messages. */
  device: string;
};

const MIC_CONSTRAINTS: MediaTrackConstraints = {
  channelCount: 1,
  sampleRate: 16000,
  echoCancellation: true,
  noiseSuppression: true,
};

// ---------------------------------------------------------------------------
// Input device selection
// ---------------------------------------------------------------------------

/**
 * Which microphone to open. Persisted in `localStorage` rather than in the
 * backend's settings file on purpose: the input device belongs to the machine
 * the microphone is plugged into, so a paired phone must not inherit the PC's
 * pick (and vice versa).
 */
const MIC_DEVICE_STORAGE_KEY = "micDeviceId";

export function getPreferredMicDeviceId(): string {
  if (typeof window === "undefined") return "";
  try {
    return localStorage.getItem(MIC_DEVICE_STORAGE_KEY) ?? "";
  } catch {
    return "";
  }
}

/** Empty string means "follow the system default". */
export function setPreferredMicDeviceId(deviceId: string): void {
  if (typeof window === "undefined") return;
  try {
    if (deviceId) localStorage.setItem(MIC_DEVICE_STORAGE_KEY, deviceId);
    else localStorage.removeItem(MIC_DEVICE_STORAGE_KEY);
  } catch {
    /* A webview with storage disabled keeps using the system default. */
  }
}

export type MicDevice = { deviceId: string; label: string };

/**
 * Audio input devices, in the order the browser reports them.
 *
 * `label` is empty until the page holds microphone permission (Chromium gates
 * device names behind it), which is what `requestAccess` unlocks by opening the
 * mic once and closing it again — a picker of anonymous ids would not help
 * telling a real microphone from a virtual one. Duplicate labels get their id
 * appended, because Chrome lists the same hardware again as the "Default" and
 * "Communications" endpoints.
 */
export async function listInputDevices(requestAccess = false): Promise<MicDevice[]> {
  const md = typeof navigator !== "undefined" ? navigator.mediaDevices : undefined;
  if (!md?.enumerateDevices) return [];
  const read = async (): Promise<MicDevice[]> =>
    (await md.enumerateDevices())
      .filter((d) => d.kind === "audioinput")
      .map((d) => ({ deviceId: d.deviceId, label: d.label }));

  let devices = await read();
  if (requestAccess && devices.some((d) => !d.label)) {
    try {
      const stream = await md.getUserMedia({ audio: true });
      stream.getTracks().forEach((t) => t.stop());
      devices = await read();
    } catch (err) {
      logError(
        `voice input: microphone access refused, device names stay hidden — ${describeMicError(err)}`,
        LOG_MODULE
      );
    }
  }

  const counts = new Map<string, number>();
  for (const d of devices) counts.set(d.label, (counts.get(d.label) ?? 0) + 1);
  return devices.map((d) =>
    d.label && (counts.get(d.label) ?? 0) > 1
      ? { ...d, label: `${d.label} (${d.deviceId.slice(0, 4)})` }
      : d
  );
}

/** Constraints for the configured device, or the bare defaults for "system default". */
function micConstraints(): MediaTrackConstraints {
  const preferred = getPreferredMicDeviceId();
  return preferred ? { ...MIC_CONSTRAINTS, deviceId: { exact: preferred } } : MIC_CONSTRAINTS;
}

// ---------------------------------------------------------------------------
// Signal level
// ---------------------------------------------------------------------------

export type LevelInfo = { peak: number; rms: number; dbfs: number; silent: boolean };

/**
 * Peak / RMS of a finished recording.
 *
 * A container that decodes to the right duration but carries no signal is the
 * signature of a virtual microphone (Steam, OBS, Voicemeeter) holding the
 * system default slot: Opus with DTX squeezes three seconds of silence into a
 * kilobyte, the WAV still measures the full length, and the model correctly
 * answers "" — which from the outside looks exactly like a broken STT
 * pipeline. Measuring the signal is what separates "nothing was captured"
 * from "nothing was recognized".
 */
export function analyzeLevel(samples: Float32Array): LevelInfo {
  let peak = 0;
  let sumSq = 0;
  for (let i = 0; i < samples.length; i++) {
    const v = samples[i];
    const abs = v < 0 ? -v : v;
    if (abs > peak) peak = abs;
    sumSq += v * v;
  }
  const rms = samples.length > 0 ? Math.sqrt(sumSq / samples.length) : 0;
  const dbfs = peak > 0 ? Math.round(20 * Math.log10(peak)) : -120;
  // ≈ -50 dBFS peak: quieter than any microphone that is actually reproducing
  // speech, yet well above the all-zero floor of a device that is fully dead.
  return { peak, rms, dbfs, silent: peak < 0.0032 && rms < 0.001 };
}

/** One-line rendering for the Log page. */
export function describeLevel(level: LevelInfo): string {
  const dbfs = level.dbfs <= -120 ? "-inf" : String(level.dbfs);
  return `peak ${level.peak.toFixed(4)} / RMS ${level.rms.toFixed(4)} (${dbfs} dBFS)`;
}

/**
 * What to tell the user when a recording produced no text.
 *
 * Returns the diagnosis for the capture itself — a silent mic is not the
 * model's fault, and the actionable step is to change input device, not to
 * reload the model.
 */
export function diagnoseEmptyTranscript(device: string, level: LevelInfo): string {
  return level.silent
    ? `The microphone "${device}" captured only silence (${describeLevel(level)}), so there was nothing to recognize. ` +
      "Pick your real microphone under Settings → Microphone, or set it as the system input device — " +
      "virtual microphones from Steam, OBS or Voicemeeter often take that slot."
    : `The model returned no text for a recording that does carry signal (${describeLevel(level)}). ` +
      "Check that the loaded STT model matches the language you spoke.";
}

/**
 * Open the configured device, falling back to the system default.
 *
 * A pinned device that has been unplugged since it was chosen would otherwise
 * fail every dictation with an `OverconstrainedError` until the user found the
 * Settings row and cleared it — so a vanished device degrades to the default
 * (loudly, in the log) instead of wedging voice input.
 */
export async function openMicStream(preferred: string): Promise<MediaStream> {
  const md = navigator.mediaDevices;
  try {
    return await md.getUserMedia({ audio: micConstraints() });
  } catch (err) {
    const name = err instanceof DOMException ? err.name : (err as { name?: string })?.name ?? "";
    if (!preferred || (name !== "OverconstrainedError" && name !== "NotFoundError")) throw err;
    logError(
      `voice input: the microphone pinned in Settings is gone (${name}) — falling back to the system default device`,
      LOG_MODULE
    );
    return await md.getUserMedia({ audio: MIC_CONSTRAINTS });
  }
}

/**
 * Why voice input cannot run here, phrased so the user can act on it. Checked
 * before `getUserMedia` so a webview without the API yields a message rather
 * than "Cannot read properties of undefined (reading 'getUserMedia')".
 */
export function checkMicSupport(): { ok: boolean; reason?: string } {
  if (typeof navigator === "undefined" || !navigator.mediaDevices?.getUserMedia) {
    // Chromium only exposes `mediaDevices` inside a secure context, and the
    // Android WebView serves the app from the tauri.localhost origin — so this
    // is where a regression in that origin handling would surface.
    return {
      ok: false,
      reason: isMobileApp()
        ? "The Android webview is not exposing a microphone: navigator.mediaDevices is unavailable in this context."
        : "This webview does not expose a microphone: navigator.mediaDevices is unavailable.",
    };
  }
  return { ok: true };
}

/** Turn a `getUserMedia` rejection into something actionable. */
export function describeMicError(err: unknown): string {
  const name =
    err instanceof DOMException ? err.name : (err as { name?: string } | null)?.name ?? "";
  switch (name) {
    case "NotAllowedError":
    case "SecurityError":
      return isMobileApp()
        ? "Microphone access was denied. Allow it under Android Settings → Apps → fms-app → Permissions → Microphone, then restart the app."
        : "Microphone access was denied.";
    case "NotFoundError":
    case "OverconstrainedError":
      return "No usable microphone was found for the requested format.";
    case "NotReadableError":
      return "The microphone is busy or unreadable — close other recording apps and try again.";
  }
  return err instanceof Error ? err.message : String(err);
}

/**
 * How long to wait for a `MediaRecorder` to finish. A webview that never fires
 * `stop` would otherwise leave the recording hanging with no message anywhere —
 * which is exactly the "I recorded something and nothing happened" symptom.
 */
const RECORDER_STOP_TIMEOUT_MS = 5000;

function withTimeout<T>(promise: Promise<T>, ms: number, message: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(
      () =>
        reject(
          new Error(`${message} — timed out after ${ms} ms. The next recording will capture raw PCM instead.`)
        ),
      ms
    );
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (err) => {
        clearTimeout(timer);
        reject(err);
      }
    );
  });
}

/**
 * Open the microphone and start capturing.
 *
 * Two backends with identical output (16 kHz mono Float32 PCM):
 *  - MediaRecorder → container blob → decodeAudioData → PCM. What the desktop
 *    app has always used.
 *  - Web Audio ScriptProcessor → PCM taken straight out of the graph, with no
 *    encoder and no decoder involved.
 *
 * Android's WebView is why the second one exists: `MediaRecorder` may be absent
 * outright, and even when it records there is no guarantee the container can be
 * decoded back. A decode failure flips `preferPcm`, so the very next tap
 * captures through Web Audio without any user intervention.
 */
export async function openMicCapture(): Promise<MicCapture> {
  const openedAt = Date.now();
  const preferred = getPreferredMicDeviceId();
  const stream = await openMicStream(preferred);
  const release = () => stream.getTracks().forEach((t) => t.stop());

  const track = stream.getAudioTracks()[0];
  const settings = track?.getSettings() ?? {};
  const recorderAvailable = typeof MediaRecorder !== "undefined";
  const useRecorder = !preferPcm && recorderAvailable;
  logInfo(
    `voice input: microphone "${track?.label || "unnamed"}" granted after ${
      Date.now() - openedAt
    } ms (${settings.sampleRate ?? "?"} Hz, ${settings.channelCount ?? "?"} ch, ${
      track?.readyState ?? "no track"
    }), capture backend = ${useRecorder ? "MediaRecorder" : "raw PCM"}` +
      (preferred ? " [pinned in Settings → Microphone]" : " [system default device]") +
      (preferPcm
        ? " — MediaRecorder output could not be decoded earlier"
        : recorderAvailable
          ? ""
          : " — MediaRecorder is unavailable here"),
    LOG_MODULE
  );
  const device = track?.label || (preferred ? "selected microphone" : "system default microphone");

  if (useRecorder) {
    const recorder = new MediaRecorder(stream);
    const blobs: Blob[] = [];
    const finished = new Promise<Blob>((resolve) => {
      recorder.ondataavailable = (e) => {
        if (e.data.size > 0) blobs.push(e.data);
      };
      recorder.onstop = () => resolve(new Blob(blobs, { type: blobs[0]?.type || "audio/webm" }));
    });
    recorder.start();
    return {
      backend: "mediarecorder",
      device,
      async stop() {
        logInfo("voice input: stopping MediaRecorder", LOG_MODULE);
        recorder.stop();
        let blob: Blob;
        try {
          blob = await withTimeout(
            finished,
            RECORDER_STOP_TIMEOUT_MS,
            "The webview's MediaRecorder never delivered its audio"
          );
        } catch (err) {
          // A recorder that hangs is as good as absent: latch the PCM fallback.
          preferPcm = true;
          release();
          throw err;
        }
        release();
        logInfo(
          `voice input: recorded ${blob.size} bytes of ${blob.type || "unknown-type"} audio`,
          LOG_MODULE
        );
        try {
          return await blobToSamples(blob);
        } catch (err) {
          preferPcm = true;
          const detail = err instanceof Error ? err.message : String(err);
          logError(
            `voice input: this webview cannot decode its own recording ${
              blob.type || "(unknown container)"
            }: ${detail}`,
            LOG_MODULE
          );
          throw new Error(
            `This webview cannot decode its own recording (${blob.type || "unknown container"}: ${detail}). ` +
              "The next recording will capture raw PCM instead."
          );
        }
      },
    };
  }

  // Raw PCM backend.
  let ctx: AudioContext;
  try {
    ctx = new AudioContext({ sampleRate: 16000 });
  } catch {
    ctx = new AudioContext();
  }
  const inRate = ctx.sampleRate;
  logInfo(
    `voice input: raw PCM capture running (AudioContext ${inRate} Hz, state ${ctx.state})`,
    LOG_MODULE
  );
  const source = ctx.createMediaStreamSource(stream);
  // ScriptProcessorNode is deprecated, but AudioWorklet wants a module URL and
  // the asset-protocol CSP makes shipping one awkward. 4096 frames is a
  // callback every ~85–256 ms — far quicker than anything a button-paced
  // recording can produce.
  const processor = ctx.createScriptProcessor(4096, 1, 1);
  const chunks: Float32Array[] = [];
  processor.onaudioprocess = (e) => {
    chunks.push(new Float32Array(e.inputBuffer.getChannelData(0)));
  };
  // Chromium only pulls a graph that reaches the destination; the muted gain
  // node keeps the monitored audio from coming back out of the speaker.
  const sink = ctx.createGain();
  sink.gain.value = 0;
  source.connect(processor);
  processor.connect(sink);
  sink.connect(ctx.destination);

  return {
    backend: "pcm",
    device,
    async stop() {
      processor.onaudioprocess = null;
      processor.disconnect();
      source.disconnect();
      sink.disconnect();
      release();
      const total = chunks.reduce((n, c) => n + c.length, 0);
      const merged = new Float32Array(total);
      let offset = 0;
      for (const c of chunks) {
        merged.set(c, offset);
        offset += c.length;
      }
      await ctx.close();
      const out = resampleLinear(merged, inRate, 16000);
      logInfo(
        `voice input: collected ${merged.length} frames at ${inRate} Hz, resampled to ${
          out.length
        } samples at 16 kHz (${(out.length / 16000).toFixed(2)} s)`,
        LOG_MODULE
      );
      return out;
    },
  };
}

/**
 * Linear-interpolation resample, used only when the AudioContext ignored the
 * 16 kHz request and ran at the hardware rate instead.
 */
function resampleLinear(input: Float32Array, fromRate: number, toRate: number): Float32Array {
  if (fromRate === toRate || input.length === 0) return input;
  const ratio = toRate / fromRate;
  const out = new Float32Array(Math.floor(input.length * ratio));
  for (let i = 0; i < out.length; i++) {
    const pos = i / ratio;
    const i0 = Math.floor(pos);
    const i1 = Math.min(i0 + 1, input.length - 1);
    out[i] = input[i0] + (input[i1] - input[i0]) * (pos - i0);
  }
  return out;
}

// ---------------------------------------------------------------------------
// Module-level state & subscription system
// ---------------------------------------------------------------------------

let activeCapture: MicCapture | null = null;
let requestStop: (() => void) | null = null;
// Flipped after a MediaRecorder/decode failure so the next tap goes straight to
// the PCM backend instead of failing the same way twice.
let preferPcm = false;
let currentVoiceState: VoiceState = "idle";
let currentError = "";
let focusedElement: HTMLInputElement | HTMLTextAreaElement | null = null;

const subscribers = new Set<() => void>();

function notifySubscribers() {
  subscribers.forEach((fn) => fn());
}

function setVoiceState(state: VoiceState) {
  currentVoiceState = state;
  notifySubscribers();
}

function setError(msg: string) {
  currentError = msg;
  notifySubscribers();
}

// ---------------------------------------------------------------------------
// Focused element tracking
// ---------------------------------------------------------------------------

function isTextInput(el: EventTarget | null): el is HTMLInputElement | HTMLTextAreaElement {
  if (!el) return false;
  if (el instanceof HTMLTextAreaElement) return true;
  if (el instanceof HTMLInputElement) {
    const type = (el.type || "text").toLowerCase();
    return ["text", "search", "url", "email", "password"].includes(type);
  }
  return false;
}

if (typeof document !== "undefined") {
  document.addEventListener("focusin", (e) => {
    if (isTextInput(e.target)) {
      focusedElement = e.target;
    }
  });
  // Deliberately no `focusout` handler. Chrome on Android does not move focus to
  // a <button> when you tap it: the dictation field blurs straight to
  // `document.body`. Forgetting the target on blur therefore erased it on every
  // single mic tap on a phone — and on a touch screen the mic button is the only
  // way to start a dictation. The last text input stays the dictation target
  // until it disappears from the DOM (`insertTextAtCursor` checks that), and the
  // Ctrl+C shortcut below keys off the *live* focus instead of this memory.
}

// ---------------------------------------------------------------------------
// Insert text at cursor of focused element
// ---------------------------------------------------------------------------

/** Short description of where the transcript would land, for the log. */
function describeInsertTarget(): string {
  const el = focusedElement;
  if (!el) return "none (no text input was focused)";
  const label = el.id || el.getAttribute("aria-label") || el.name || "(unlabelled)";
  return `${el.tagName.toLowerCase()}#${label}`;
}

/** Insert at the cursor. Returns false when there is no usable target. */
function insertTextAtCursor(text: string): boolean {
  const el = focusedElement;
  // The cue list re-renders freely, so the remembered node may be detached;
  // writing into it would silently go nowhere.
  if (!el || !el.isConnected) {
    logError(
      `voice input: no text input to write into (target ${describeInsertTarget()}), dropping ${text.length} characters of transcript`,
      LOG_MODULE
    );
    focusedElement = null;
    return false;
  }
  const start = el.selectionStart ?? el.value.length;
  const end = el.selectionEnd ?? el.value.length;
  const newValue = el.value.slice(0, start) + text + el.value.slice(end);
  const proto =
    el instanceof HTMLTextAreaElement
      ? HTMLTextAreaElement.prototype
      : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
  setter?.call(el, newValue);
  const caret = start + text.length;
  requestAnimationFrame(() => {
    el.selectionStart = caret;
    el.selectionEnd = caret;
  });
  el.dispatchEvent(new Event("input", { bubbles: true }));
  el.dispatchEvent(new Event("change", { bubbles: true }));
  logInfo(
    `voice input: inserted ${text.length} characters into ${describeInsertTarget()}`,
    LOG_MODULE
  );
  return true;
}

// ---------------------------------------------------------------------------
// Global toggle handler (used by toolbar button & keyboard shortcut)
// ---------------------------------------------------------------------------

export async function handleToggle() {
  if (currentVoiceState === "processing") {
    logInfo("voice input: toggle ignored — a transcription is still running", LOG_MODULE);
    return;
  }
  if (currentVoiceState === "recording") {
    logInfo("voice input: toggle stopped the recording", LOG_MODULE);
    stopRecording();
    return;
  }

  setError("");
  logInfo(
    `voice input: toggle pressed on ${isMobileApp() ? "Android" : "desktop"}, insert target = ${describeInsertTarget()}`,
    LOG_MODULE
  );

  // Auto-load model if not running
  if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const status = await invoke<{ active_version: string | null }>("model_get_status");
      logInfo(
        `voice input: model status = ${status.active_version ?? "nothing loaded"}`,
        LOG_MODULE
      );
      if (!status.active_version) {
        logInfo("voice input: loading the STT model (first load can take a while)", LOG_MODULE);
        setVoiceState("processing");
        const loadedAt = Date.now();
        await invoke("model_start");
        logInfo(`voice input: STT model loaded in ${Date.now() - loadedAt} ms`, LOG_MODULE);
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      logError(`voice input: could not load the STT model — ${msg}`, LOG_MODULE);
      setError(`Failed to load the STT model: ${msg}`);
      setVoiceState("idle");
      return;
    }
  }

  await startRecording({
    onStateChange: setVoiceState,
    onResult: (text) => {
      // Dictation lands in a single-line field, and the model happily emits line
      // breaks — collapse them before inserting, or the value gets a stray newline.
      const cleaned = text.trim().replace(/\s+/g, " ");
      if (!insertTextAtCursor(cleaned)) {
        // The dictation itself succeeded, so do not throw it away: show it and
        // explain how to land it in a field next time.
        setError(
          `No text field was focused, so "${cleaned}" could not be inserted. ` +
            "Tap the field you want to fill, then dictate again."
        );
      }
      setVoiceState("idle");
    },
    onError: (err) => {
      setError(err);
      setVoiceState("idle");
    },
  });
}

// ---------------------------------------------------------------------------
// Global Ctrl+C shortcut (copy if selection exists, else voice input)
// ---------------------------------------------------------------------------

if (typeof document !== "undefined") {
  document.addEventListener("keydown", (e) => {
    if (e.ctrlKey && (e.key === "c" || e.key === "C")) {
      // The shortcut only applies while a text field actually has focus; the
      // sticky `focusedElement` is for the button path, where touch taps blur the
      // field. Otherwise Ctrl+C with no selection would start a recording from
      // anywhere on the page once an input had been focused earlier.
      const el = document.activeElement;
      if (isTextInput(el)) {
        if ((el.selectionStart ?? 0) !== (el.selectionEnd ?? 0)) return; // has selection → normal copy
        focusedElement = el;
        e.preventDefault();
        handleToggle();
      }
    }
  });
}

// ---------------------------------------------------------------------------
// Public API — recording
// ---------------------------------------------------------------------------

export async function startRecording(callbacks: VoiceCallbacks): Promise<void> {
  if (activeCapture) {
    logInfo("voice input: start ignored — already recording", LOG_MODULE);
    return;
  }

  const support = checkMicSupport();
  if (!support.ok) {
    logError(`voice input: unsupported environment — ${support.reason}`, LOG_MODULE);
    callbacks.onError(support.reason ?? "Microphone unavailable");
    return;
  }

  let capture: MicCapture;
  try {
    capture = await openMicCapture();
  } catch (err) {
    const msg = describeMicError(err);
    logError(`voice input: the microphone could not be opened — ${msg}`, LOG_MODULE);
    callbacks.onError(msg);
    return;
  }

  // stopRecording() resolves this gate; the task below then drains the capture
  // and runs transcription, so the two backends stay interchangeable.
  const stopped = new Promise<void>((resolve) => {
    requestStop = resolve;
  });
  activeCapture = capture;
  callbacks.onStateChange("recording");
  logInfo(`voice input: recording started (${capture.backend} backend)`, LOG_MODULE);

  void (async () => {
    const capturedAt = Date.now();
    try {
      await stopped;
      const samples = await capture.stop();
      if (samples.length === 0) {
        throw new Error("The microphone produced no audio at all — nothing to transcribe.");
      }
      const level = analyzeLevel(samples);
      logInfo(
        `voice input: signal level ${describeLevel(level)} — ${
          level.silent ? "no signal: this microphone delivered silence" : "signal present"
        }`,
        LOG_MODULE
      );
      callbacks.onStateChange("processing");

      const wav = encodeWav(samples, 16000);
      const base64 = arrayBufferToBase64(wav);
      logInfo(
        `voice input: ${(samples.length / 16000).toFixed(2)} s of audio captured in ${
          Date.now() - capturedAt
        } ms, sending ${(wav.byteLength / 1024).toFixed(0)} KB WAV to model_transcribe via "${capture.device}"`,
        LOG_MODULE
      );

      const { invoke } = await import("@tauri-apps/api/core");
      const transcribedAt = Date.now();
      const text: string = await invoke("model_transcribe", { wavBase64: base64 });
      logInfo(
        `voice input: model_transcribe answered after ${Date.now() - transcribedAt} ms: "${text}"`,
        LOG_MODULE
      );

      // An empty transcript used to be indistinguishable from a broken model.
      // The level decides who to blame, and either way the user gets a step to act on.
      if (!text.trim()) {
        throw new Error(diagnoseEmptyTranscript(capture.device, level));
      }

      callbacks.onResult(text.trim());
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      logError(`voice input: recording failed — ${msg}`, LOG_MODULE);
      callbacks.onError(msg);
    } finally {
      activeCapture = null;
      requestStop = null;
      callbacks.onStateChange("idle");
    }
  })();
}

export function stopRecording(): void {
  if (requestStop) logInfo("voice input: stop requested", LOG_MODULE);
  requestStop?.();
}

export function isRecording(): boolean {
  return activeCapture !== null;
}

// ---------------------------------------------------------------------------
// Public API — subscription (for React components)
// ---------------------------------------------------------------------------

export function subscribe(fn: () => void): () => void {
  subscribers.add(fn);
  return () => subscribers.delete(fn);
}

export function getVoiceState(): VoiceState {
  return currentVoiceState;
}

export function getVoiceError(): string {
  return currentError;
}

export function clearVoiceError(): void {
  currentError = "";
  notifySubscribers();
}
