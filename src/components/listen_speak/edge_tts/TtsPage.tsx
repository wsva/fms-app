"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { Volume2, Loader2, Play } from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { type TtsVoice, type TtsSynthesizeResult, type TtsPreviewResult } from "@/lib/edge_tts/types";

// Popular European languages with their locale prefixes
const EUROPEAN_LANGUAGES: { code: string; label: string; prefix: string }[] = [
  { code: "en", label: "English", prefix: "en-" },
  { code: "de", label: "Deutsch", prefix: "de-" },
  { code: "fr", label: "Fran\u00e7ais", prefix: "fr-" },
  { code: "es", label: "Espa\u00f1ol", prefix: "es-" },
  { code: "it", label: "Italiano", prefix: "it-" },
];

export default function TtsPage() {
  const [voices, setVoices] = useState<TtsVoice[]>([]);
  const [selectedLang, setSelectedLang] = useState("en");
  const [selectedVoice, setSelectedVoice] = useState("");
  const [text, setText] = useState("");
  const [rate, setRate] = useState("+0%");
  const [volume, setVolume] = useState("+0%");
  const [pitch, setPitch] = useState("+0Hz");
  const [loading, setLoading] = useState(false);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [loadingVoices, setLoadingVoices] = useState(false);
  const [error, setError] = useState("");
  const [audioUrl, setAudioUrl] = useState<string | null>(null);
  const [previewUrl, setPreviewUrl] = useState<string | null>(null);
  const [lastOutputPath, setLastOutputPath] = useState<string | null>(null);
  const audioRef = useRef<HTMLAudioElement>(null);

  // ---- Load voices ----

  const fetchVoices = useCallback(async () => {
    if (!isTauri()) return;
    setLoadingVoices(true);
    setError("");
    try {
      const result = await invoke<TtsVoice[]>("edge_tts_list_voices");
      setVoices(result);
    } catch (e) {
      setError(`Failed to load voices: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setLoadingVoices(false);
    }
  }, []);

  useEffect(() => {
    fetchVoices();
  }, [fetchVoices]);

  // ---- Filter voices by selected language ----

  const langPrefix = EUROPEAN_LANGUAGES.find((l) => l.code === selectedLang)?.prefix ?? "en-";
  const langVoices = voices.filter((v) => v.locale.startsWith(langPrefix));

  // Auto-select first voice when language changes
  useEffect(() => {
    if (langVoices.length > 0) {
      setSelectedVoice(langVoices[0].short_name);
    } else {
      setSelectedVoice("");
    }
  }, [selectedLang, voices]);

  // ---- Preview audio (fetch and play without saving) ----

  async function handlePreview() {
    if (!text.trim() || !selectedVoice || previewLoading) return;
    setError("");
    setPreviewLoading(true);
    try {
      const result = await invoke<TtsPreviewResult>("edge_tts_preview", {
        args: {
          text: text.trim(),
          voice: selectedVoice,
          rate,
          volume,
          pitch,
          output_path: "", // not used for preview
        },
      });
      // Convert base64 to blob URL
      const binaryStr = atob(result.audio_base64);
      const bytes = new Uint8Array(binaryStr.length);
      for (let i = 0; i < binaryStr.length; i++) {
        bytes[i] = binaryStr.charCodeAt(i);
      }
      const blob = new Blob([bytes], { type: "audio/mpeg" });
      // Revoke previous preview URL
      if (previewUrl) URL.revokeObjectURL(previewUrl);
      const url = URL.createObjectURL(blob);
      setPreviewUrl(url);
      // Play the audio
      const audio = new Audio(url);
      audio.play();
    } catch (e) {
      setError(`Preview failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setPreviewLoading(false);
    }
  }

  // ---- Generate audio ----

  async function handleGenerate() {
    if (!text.trim() || !selectedVoice || loading) return;
    setError("");

    // Open save dialog
    const outputPath = await save({
      filters: [{ name: "Audio", extensions: ["mp3"] }],
      defaultPath: "tts_output.mp3",
    });

    if (!outputPath) return; // User cancelled

    setLoading(true);
    try {
      const result = await invoke<TtsSynthesizeResult>("edge_tts_synthesize", {
        args: {
          text: text.trim(),
          voice: selectedVoice,
          rate,
          volume,
          pitch,
          output_path: outputPath,
        },
      });

      setLastOutputPath(result.output_path);
      // Create a file:// URL for the audio player
      // On Windows, convert backslashes to forward slashes
      const normalizedPath = result.output_path.replace(/\\/g, "/");
      const url = `file:///${normalizedPath}`;
      setAudioUrl(url);
    } catch (e) {
      setError(`Synthesis failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setLoading(false);
    }
  }

  // ---- Render ----

  return (
    <main className="flex-1 flex flex-col h-full">
      {/* Header */}
      <div className="p-4 border-b border-border-default flex items-center gap-4">
        <h1 className="text-lg font-semibold flex-1">Text to Speech</h1>
        {loadingVoices && <Loader2 size={18} className="animate-spin text-text-tertiary" />}
      </div>

      <div className="flex-1 overflow-y-auto p-4">
        <div className="max-w-3xl mx-auto space-y-6">
          {/* Error banner */}
          {error && (
            <div className="px-4 py-3 rounded-lg bg-error-bg text-error-text text-sm">
              {error}
            </div>
          )}

          {/* Language + Voice selectors */}
          <div className="grid grid-cols-2 gap-4">
            <div className="space-y-2">
              <label className="text-sm font-medium text-text-secondary">Language</label>
              <select
                className="w-full px-3 py-2 rounded-lg bg-bg-muted border border-border-default text-sm text-text-primary cursor-pointer focus:outline-none focus:border-accent"
                value={selectedLang}
                onChange={(e) => setSelectedLang(e.target.value)}
              >
                {EUROPEAN_LANGUAGES.map((lang) => (
                  <option key={lang.code} value={lang.code}>
                    {lang.label}
                  </option>
                ))}
              </select>
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium text-text-secondary">Voice</label>
              <select
                className="w-full px-3 py-2 rounded-lg bg-bg-muted border border-border-default text-sm text-text-primary cursor-pointer focus:outline-none focus:border-accent"
                value={selectedVoice}
                onChange={(e) => setSelectedVoice(e.target.value)}
              >
                {langVoices.map((v) => (
                  <option key={v.short_name} value={v.short_name}>
                    {v.short_name.replace(/^[^-]+-/, "")} ({v.gender})
                  </option>
                ))}
              </select>
            </div>
          </div>

          {/* Controls */}
          <div className="grid grid-cols-3 gap-4">
            <div className="space-y-1">
              <label className="text-xs font-medium text-text-secondary">Rate</label>
              <select
                className="w-full px-2 py-1.5 rounded-lg bg-bg-muted border border-border-default text-sm text-text-primary cursor-pointer"
                value={rate}
                onChange={(e) => setRate(e.target.value)}
              >
                <option value="-50%">-50%</option>
                <option value="-25%">-25%</option>
                <option value="+0%">Normal</option>
                <option value="+25%">+25%</option>
                <option value="+50%">+50%</option>
                <option value="+100%">+100%</option>
              </select>
            </div>
            <div className="space-y-1">
              <label className="text-xs font-medium text-text-secondary">Volume</label>
              <select
                className="w-full px-2 py-1.5 rounded-lg bg-bg-muted border border-border-default text-sm text-text-primary cursor-pointer"
                value={volume}
                onChange={(e) => setVolume(e.target.value)}
              >
                <option value="-50%">-50%</option>
                <option value="-25%">-25%</option>
                <option value="+0%">Normal</option>
                <option value="+25%">+25%</option>
                <option value="+50%">+50%</option>
                <option value="+100%">+100%</option>
              </select>
            </div>
            <div className="space-y-1">
              <label className="text-xs font-medium text-text-secondary">Pitch</label>
              <select
                className="w-full px-2 py-1.5 rounded-lg bg-bg-muted border border-border-default text-sm text-text-primary cursor-pointer"
                value={pitch}
                onChange={(e) => setPitch(e.target.value)}
              >
                <option value="-50Hz">-50Hz</option>
                <option value="-25Hz">-25Hz</option>
                <option value="+0Hz">Normal</option>
                <option value="+25Hz">+25Hz</option>
                <option value="+50Hz">+50Hz</option>
              </select>
            </div>
          </div>

          {/* Text input */}
          <div className="space-y-2">
            <label className="text-sm font-medium text-text-secondary">Text</label>
            <textarea
              className="w-full px-4 py-3 rounded-lg bg-bg-muted border border-border-default text-text-primary text-sm resize-y min-h-[160px] focus:outline-none focus:border-accent"
              placeholder="Enter text to synthesize..."
              value={text}
              onChange={(e) => setText(e.target.value)}
              disabled={loading}
            />
            <p className="text-xs text-text-tertiary">{text.length} characters</p>
          </div>

          {/* Preview + Generate buttons */}
          <div className="flex items-center gap-3">
            <button
              className="flex items-center gap-2 px-4 py-2.5 rounded-lg bg-bg-muted border border-border-default text-sm font-medium hover:bg-bg-hover cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
              onClick={handlePreview}
              disabled={!text.trim() || !selectedVoice || previewLoading}
            >
              {previewLoading ? (
                <>
                  <Loader2 size={16} className="animate-spin" />
                  Loading...
                </>
              ) : (
                <>
                  <Play size={16} />
                  Preview
                </>
              )}
            </button>
            <button
              className="flex items-center gap-2 px-5 py-2.5 rounded-lg bg-accent-bg text-white text-sm font-medium hover:opacity-90 cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-opacity"
              onClick={handleGenerate}
              disabled={!text.trim() || !selectedVoice || loading}
            >
              {loading ? (
                <>
                  <Loader2 size={16} className="animate-spin" />
                  Generating...
                </>
              ) : (
                <>
                  <Volume2 size={16} />
                  Save
                </>
              )}
            </button>
            {lastOutputPath && !loading && (
              <span className="text-xs text-text-tertiary truncate">
                Saved to: {lastOutputPath}
              </span>
            )}
          </div>

          {/* Audio player */}
          {audioUrl && (
            <div className="space-y-2">
              <label className="text-sm font-medium text-text-secondary">Preview</label>
              <div className="flex items-center gap-3 p-3 rounded-lg bg-bg-muted border border-border-default">
                <audio ref={audioRef} src={audioUrl} controls className="flex-1" />
              </div>
            </div>
          )}

          {/* Empty state */}
          {!loadingVoices && voices.length === 0 && !error && (
            <div className="flex flex-col items-center justify-center py-12 text-center text-text-tertiary">
              <Volume2 size={48} className="mb-4 opacity-50" />
              <p className="text-sm">Unable to load voices. Check your internet connection.</p>
              <button
                className="mt-3 px-4 py-2 rounded-lg bg-bg-muted border border-border-default text-sm cursor-pointer hover:bg-bg-hover transition-colors"
                onClick={fetchVoices}
              >
                Retry
              </button>
            </div>
          )}
        </div>
      </div>
    </main>
  );
}
