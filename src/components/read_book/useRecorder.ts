import { useCallback, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { blobToSamples, encodeWav, arrayBufferToBase64 } from "@/lib/voice-input";
import { isTauri } from "@/lib/tauri";

export type RecorderResult = { wavBase64: string; text: string };

/**
 * Microphone recorder for the reading drawer.
 *
 * Records audio, converts it to 16kHz mono WAV, runs STT via `model_transcribe`,
 * and hands back BOTH the WAV (base64) and the recognized text so the caller can
 * persist the audio and show a diff.
 */
export function useRecorder(
  onResult: (r: RecorderResult) => void,
  onError: (msg: string) => void
) {
  const [recording, setRecording] = useState(false);
  const [processing, setProcessing] = useState(false);
  const recorderRef = useRef<MediaRecorder | null>(null);
  const chunksRef = useRef<Blob[]>([]);

  // Keep the latest callbacks without re-creating `start`.
  const cbRef = useRef({ onResult, onError });
  cbRef.current = { onResult, onError };

  const start = useCallback(async () => {
    if (!isTauri()) {
      cbRef.current.onError("Recording requires the desktop app");
      return;
    }
    try {
      // Ensure the STT model is loaded before recording.
      setProcessing(true);
      const status = await invoke<{ active_version: string | null }>("model_get_status");
      if (!status.active_version) {
        await invoke("model_start");
      }
      setProcessing(false);

      const stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          channelCount: 1,
          sampleRate: 16000,
          echoCancellation: true,
          noiseSuppression: true,
        },
      });

      chunksRef.current = [];
      const rec = new MediaRecorder(stream);
      recorderRef.current = rec;

      rec.ondataavailable = (e) => {
        if (e.data.size > 0) chunksRef.current.push(e.data);
      };

      rec.onstop = async () => {
        stream.getTracks().forEach((t) => t.stop());
        setProcessing(true);
        try {
          const blob = new Blob(chunksRef.current, {
            type: chunksRef.current[0]?.type || "audio/webm",
          });
          const samples = await blobToSamples(blob);
          const wav = encodeWav(samples, 16000);
          const wavBase64 = arrayBufferToBase64(wav);
          const text = await invoke<string>("model_transcribe", { wavBase64 });
          cbRef.current.onResult({ wavBase64, text: (text || "").trim() });
        } catch (err) {
          cbRef.current.onError(err instanceof Error ? err.message : String(err));
        } finally {
          setProcessing(false);
          setRecording(false);
        }
      };

      rec.onerror = () => {
        cbRef.current.onError("Recording failed");
        setRecording(false);
      };

      rec.start();
      setRecording(true);
    } catch (err) {
      setProcessing(false);
      cbRef.current.onError(
        err instanceof Error ? err.message : "Microphone access denied"
      );
    }
  }, []);

  const stop = useCallback(() => {
    if (recorderRef.current && recorderRef.current.state === "recording") {
      recorderRef.current.stop();
    }
  }, []);

  const toggle = useCallback(() => {
    if (processing) return;
    if (recording) stop();
    else start();
  }, [recording, processing, start, stop]);

  return { recording, processing, toggle };
}
