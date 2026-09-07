"use client";

import { useState, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/tauri";
import { btnBase } from "@/lib/models/types";

export default function TestSection({ isModelRunning }: { isModelRunning: boolean }) {
  const [recording, setRecording] = useState(false);
  const [transcription, setTranscription] = useState<string | null>(null);
  const [transcribing, setTranscribing] = useState(false);
  const mediaRecorderRef = useRef<MediaRecorder | null>(null);
  const chunksRef = useRef<Blob[]>([]);

  async function startRecording() {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const mediaRecorder = new MediaRecorder(stream);
      mediaRecorderRef.current = mediaRecorder;
      chunksRef.current = [];

      mediaRecorder.ondataavailable = (e) => {
        if (e.data.size > 0) chunksRef.current.push(e.data);
      };

      mediaRecorder.onstop = async () => {
        stream.getTracks().forEach((t) => t.stop());
        const blob = new Blob(chunksRef.current, { type: "audio/webm" });
        await processAudio(blob);
      };

      mediaRecorder.start();
      setRecording(true);
      setTranscription(null);
    } catch (e) {
      console.error("Microphone access denied:", e);
    }
  }

  function stopRecording() {
    mediaRecorderRef.current?.stop();
    setRecording(false);
  }

  async function processAudio(blob: Blob) {
    if (!isTauri()) return;
    setTranscribing(true);
    try {
      const arrayBuffer = await blob.arrayBuffer();
      const audioCtx = new AudioContext({ sampleRate: 16000 });
      const audioBuffer = await audioCtx.decodeAudioData(arrayBuffer);
      const mono = audioBuffer.getChannelData(0);

      // Encode as 16-bit PCM WAV
      const wavBuf = new ArrayBuffer(mono.length * 2 + 44);
      const view = new DataView(wavBuf);
      const writeStr = (offset: number, str: string) => {
        for (let i = 0; i < str.length; i++)
          view.setUint8(offset + i, str.charCodeAt(i));
      };
      writeStr(0, "RIFF");
      view.setUint32(4, 36 + mono.length * 2, true);
      writeStr(8, "WAVE");
      writeStr(12, "fmt ");
      view.setUint32(16, 16, true);
      view.setUint16(20, 1, true);
      view.setUint16(22, 1, true);
      view.setUint32(24, 16000, true);
      view.setUint32(28, 32000, true);
      view.setUint16(32, 2, true);
      view.setUint16(34, 16, true);
      writeStr(36, "data");
      view.setUint32(40, mono.length * 2, true);
      let offset = 44;
      for (let i = 0; i < mono.length; i++) {
        const s = Math.max(-1, Math.min(1, mono[i]));
        view.setInt16(offset, s < 0 ? s * 0x8000 : s * 0x7fff, true);
        offset += 2;
      }

      const bytes = new Uint8Array(wavBuf);
      let binary = "";
      for (let i = 0; i < bytes.length; i++)
        binary += String.fromCharCode(bytes[i]);
      const base64 = btoa(binary);

      const result = await invoke<string>("model_transcribe", { wavBase64: base64 });
      setTranscription(result);
    } catch (e) {
      console.error("Transcription failed:", e);
      setTranscription(`Error: ${e}`);
    } finally {
      setTranscribing(false);
    }
  }

  return (
    <section className="mb-8">
      <h2 className="text-[1.3em] font-semibold mb-2">Test</h2>
      <p className="text-text-secondary text-sm mb-4">
        Record a short voice clip and transcribe it using the active model.
      </p>

      <div className="flex items-center gap-3 mb-4">
        <button
          className={`${btnBase} ${recording ? "bg-error-text text-white hover:bg-error-hover" : "bg-accent-bg text-white hover:bg-accent-bg-hover"}`}
          onClick={recording ? stopRecording : startRecording}
          disabled={!isModelRunning || transcribing}
        >
          {recording ? "Stop Recording" : "Record"}
        </button>
        {recording && (
          <span className="flex items-center gap-2 text-sm text-error-text">
            <span className="inline-block w-2 h-2 rounded-full bg-error-text animate-pulse" />
            Recording...
          </span>
        )}
        {transcribing && (
          <span className="text-sm text-text-secondary italic">Transcribing...</span>
        )}
      </div>

      {transcription !== null && (
        <div className="p-4 bg-bg-card border border-border-default rounded-lg">
          <h4 className="font-medium mb-1">Result</h4>
          <p>{transcription}</p>
        </div>
      )}
    </section>
  );
}
