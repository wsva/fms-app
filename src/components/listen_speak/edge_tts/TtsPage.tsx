"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { Volume2, Loader2, Search, Play } from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { type TtsVoice, type TtsSynthesizeResult } from "@/lib/edge_tts/types";

export default function TtsPage() {
  const [voices, setVoices] = useState<TtsVoice[]>([]);
  const [selectedVoice, setSelectedVoice] = useState("");
  const [searchQuery, setSearchQuery] = useState("");
  const [text, setText] = useState("");
  const [rate, setRate] = useState("+0%");
  const [volume, setVolume] = useState("+0%");
  const [pitch, setPitch] = useState("+0Hz");
  const [loading, setLoading] = useState(false);
  const [loadingVoices, setLoadingVoices] = useState(false);
  const [error, setError] = useState("");
  const [audioUrl, setAudioUrl] = useState<string | null>(null);
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
      // Auto-select first English voice if none selected
      if (result.length > 0 && !selectedVoice) {
        const enVoice = result.find((v) => v.locale.startsWith("en-"));
        setSelectedVoice(enVoice?.short_name ?? result[0].short_name);
      }
    } catch (e) {
      setError(`Failed to load voices: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setLoadingVoices(false);
    }
  }, [selectedVoice]);

  useEffect(() => {
    fetchVoices();
  }, [fetchVoices]);

  // ---- Filter voices ----

  const filteredVoices = voices.filter((v) => {
    if (!searchQuery) return true;
    const q = searchQuery.toLowerCase();
    return (
      v.short_name.toLowerCase().includes(q) ||
      v.locale.toLowerCase().includes(q) ||
      v.name.toLowerCase().includes(q) ||
      v.gender.toLowerCase().includes(q)
    );
  });

  // Group filtered voices by locale
  const groupedVoices: Record<string, TtsVoice[]> = {};
  for (const v of filteredVoices) {
    const locale = v.locale;
    if (!groupedVoices[locale]) groupedVoices[locale] = [];
    groupedVoices[locale].push(v);
  }
  const sortedLocales = Object.keys(groupedVoices).sort();

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

          {/* Voice selector */}
          <div className="space-y-2">
            <label className="text-sm font-medium text-text-secondary">Voice</label>
            <div className="relative">
              <Search size={16} className="absolute left-3 top-1/2 -translate-y-1/2 text-text-tertiary" />
              <input
                type="text"
                className="w-full pl-9 pr-3 py-2 rounded-lg bg-bg-muted border border-border-default text-sm text-text-primary focus:outline-none focus:border-accent"
                placeholder="Search voices by name, locale, or gender..."
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
              />
            </div>
            <select
              className="w-full px-3 py-2 rounded-lg bg-bg-muted border border-border-default text-sm text-text-primary cursor-pointer focus:outline-none focus:border-accent"
              value={selectedVoice}
              onChange={(e) => setSelectedVoice(e.target.value)}
              size={8}
            >
              {sortedLocales.map((locale) => (
                <optgroup key={locale} label={locale}>
                  {groupedVoices[locale].map((v) => (
                    <option key={v.short_name} value={v.short_name}>
                      {v.short_name.replace(/^[^-]+-/, "")} ({v.gender})
                    </option>
                  ))}
                </optgroup>
              ))}
            </select>
            {selectedVoice && (
              <p className="text-xs text-text-tertiary">
                Selected: {voices.find((v) => v.short_name === selectedVoice)?.name ?? selectedVoice}
              </p>
            )}
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

          {/* Generate button */}
          <div className="flex items-center gap-3">
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
                  Generate Audio
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
