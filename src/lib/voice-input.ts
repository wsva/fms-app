/**
 * Voice input utility: records microphone audio, converts to 16kHz mono 16-bit PCM WAV,
 * and base64-encodes for the Rust `model_transcribe` command.
 *
 * Supports global usage: tracks the currently focused input element and provides
 * a subscription-based state system for toolbar buttons, plus a global Ctrl+C shortcut.
 */

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

function encodeWav(samples: Float32Array, sampleRate: number): ArrayBuffer {
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

async function resampleTo16k(audioBuffer: AudioBuffer): Promise<Float32Array> {
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

async function blobToSamples(blob: Blob): Promise<Float32Array> {
  const arrayBuffer = await blob.arrayBuffer();
  const audioContext = new AudioContext();
  const audioBuffer = await audioContext.decodeAudioData(arrayBuffer);
  await audioContext.close();
  return resampleTo16k(audioBuffer);
}

// ---------------------------------------------------------------------------
// ArrayBuffer → base64
// ---------------------------------------------------------------------------

function arrayBufferToBase64(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  let binary = "";
  for (let i = 0; i < bytes.byteLength; i++) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary);
}

// ---------------------------------------------------------------------------
// Module-level state & subscription system
// ---------------------------------------------------------------------------

let mediaRecorder: MediaRecorder | null = null;
let audioChunks: Blob[] = [];
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
  document.addEventListener("focusout", () => {
    // Clear on next tick — if focus moved to another text input, focusin will set it
    setTimeout(() => {
      if (!isTextInput(document.activeElement)) {
        focusedElement = null;
      }
    }, 0);
  });
}

// ---------------------------------------------------------------------------
// Insert text at cursor of focused element
// ---------------------------------------------------------------------------

function insertTextAtCursor(text: string) {
  const el = focusedElement;
  if (!el) return;
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
}

// ---------------------------------------------------------------------------
// Global toggle handler (used by toolbar button & keyboard shortcut)
// ---------------------------------------------------------------------------

export async function handleToggle() {
  if (currentVoiceState === "processing") return;
  if (currentVoiceState === "recording") {
    stopRecording();
    return;
  }

  setError("");

  // Auto-load model if not running
  if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const status = await invoke<{ active_version: string | null }>("model_get_status");
      if (!status.active_version) {
        setVoiceState("processing");
        await invoke("model_start");
      }
    } catch {
      setError("Failed to load model");
      return;
    }
  }

  await startRecording({
    onStateChange: setVoiceState,
    onResult: (text) => {
      insertTextAtCursor(text.trim());
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
      const el = document.activeElement;
      if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
        if (el.selectionStart !== el.selectionEnd) return; // has selection → normal copy
      }
      if (focusedElement) {
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
  if (mediaRecorder && mediaRecorder.state === "recording") return;

  try {
    const stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        channelCount: 1,
        sampleRate: 16000,
        echoCancellation: true,
        noiseSuppression: true,
      },
    });

    audioChunks = [];
    mediaRecorder = new MediaRecorder(stream);

    mediaRecorder.ondataavailable = (e) => {
      if (e.data.size > 0) audioChunks.push(e.data);
    };

    mediaRecorder.onstop = async () => {
      stream.getTracks().forEach((t) => t.stop());
      callbacks.onStateChange("processing");

      try {
        const blob = new Blob(audioChunks, { type: audioChunks[0]?.type || "audio/webm" });
        const samples = await blobToSamples(blob);
        const wav = encodeWav(samples, 16000);
        const base64 = arrayBufferToBase64(wav);

        const { invoke } = await import("@tauri-apps/api/core");
        const text: string = await invoke("model_transcribe", { wavBase64: base64 });

        callbacks.onResult(text.trim());
        callbacks.onStateChange("idle");
      } catch (err) {
        callbacks.onError(err instanceof Error ? err.message : String(err));
        callbacks.onStateChange("idle");
      }
    };

    mediaRecorder.onerror = () => {
      callbacks.onError("Recording failed");
      callbacks.onStateChange("idle");
    };

    mediaRecorder.start();
    callbacks.onStateChange("recording");
  } catch (err) {
    callbacks.onError(
      err instanceof Error ? err.message : "Microphone access denied"
    );
  }
}

export function stopRecording(): void {
  if (mediaRecorder && mediaRecorder.state === "recording") {
    mediaRecorder.stop();
  }
}

export function isRecording(): boolean {
  return mediaRecorder?.state === "recording";
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
