"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { Volume2, Loader2, Play } from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { type TtsVoice, type TtsSynthesizeResult, type TtsPreviewResult } from "@/lib/edge_tts/types";
import { PageHeader } from "@/components/ui/toolbar";
import { reqStr, optStr, useAgentActions } from "@/lib/agent/ui-actions";

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

  // Overrides let the agent action speak text/voice it names instead of whatever
  // the form currently holds; the button passes none and reads the inputs as before.
  // Returns an outcome (rather than only setting `error`) so a caller can report
  // success or the failure reason back instead of guessing.
  async function handlePreview(overrides?: {
    text?: string;
    voice?: string;
    rate?: string;
    volume?: string;
    pitch?: string;
  }): Promise<{ ok: boolean; voice: string; chars: number; error?: string }> {
    const sayText = (overrides?.text ?? text).trim();
    const sayVoice = overrides?.voice ?? selectedVoice;
    const sayRate = overrides?.rate ?? rate;
    const sayVolume = overrides?.volume ?? volume;
    const sayPitch = overrides?.pitch ?? pitch;
    if (!sayText || !sayVoice || previewLoading) {
      return {
        ok: false,
        voice: sayVoice,
        chars: sayText.length,
        error: !sayText
          ? "empty text"
          : !sayVoice
            ? "no voice selected"
            : "a preview is already loading",
      };
    }
    setError("");
    setPreviewLoading(true);
    try {
      const result = await invoke<TtsPreviewResult>("edge_tts_preview", {
        args: {
          text: sayText,
          voice: sayVoice,
          rate: sayRate,
          volume: sayVolume,
          pitch: sayPitch,
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
      return { ok: true, voice: sayVoice, chars: sayText.length };
    } catch (e) {
      const detail = `Preview failed: ${e instanceof Error ? e.message : String(e)}`;
      setError(detail);
      return { ok: false, voice: sayVoice, chars: sayText.length, error: detail };
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

  // ── Agent-drivable actions ──
  // Speaking through the app is the one thing here an agent cannot do with a data
  // tool: the sound comes out of the webview. Saving to a file stays in the UI
  // (handleGenerate opens a save dialog), and there is no MCP tts domain, so the
  // agent path is play-aloud + discover voices.
  useAgentActions("edge-tts", [
    {
      id: "edge-tts.list-voices",
      label: "List the Edge TTS voices loaded on this page, optionally filtered to one language.",
      kind: "read",
      params_schema: {
        language: {
          type: "string",
          description:
            "Optional language code (en, de, fr, es, it) or locale prefix (e.g. 'en-'). Omit for every voice.",
        },
      },
      run: (params) => {
        const lang = optStr("edge-tts.list-voices", params, "language");
        let list = voices;
        if (lang) {
          const meta = EUROPEAN_LANGUAGES.find(
            (l) => l.code === lang || l.label.toLowerCase() === lang.toLowerCase()
          );
          const prefix = meta ? meta.prefix : lang.endsWith("-") ? lang : `${lang}-`;
          list = voices.filter((v) => v.locale.startsWith(prefix));
        }
        const shown = 40;
        return {
          count: list.length,
          truncated: list.length > shown,
          voices: list.slice(0, shown).map((v) => ({
            short_name: v.short_name,
            locale: v.locale,
            gender: v.gender,
          })),
        };
      },
    },
    {
      id: "edge-tts.speak",
      label:
        "Synthesize text with an Edge TTS voice and play it aloud through the app. No file is written.",
      kind: "read",
      params_schema: {
        text: { type: "string", description: "The text to speak.", required: true },
        voice: {
          type: "string",
          description:
            "A voice short_name from edge-tts.list-voices. Omit to use the voice currently selected on the page.",
        },
        rate: { type: "string", description: "Speed adjust, e.g. '+0%', '-25%', '+50%'. Defaults to the page's setting." },
        volume: { type: "string", description: "Volume adjust, e.g. '+0%', '+50%'. Defaults to the page's setting." },
        pitch: { type: "string", description: "Pitch adjust, e.g. '+0Hz', '+25Hz'. Defaults to the page's setting." },
      },
      run: async (params) => {
        const id = "edge-tts.speak";
        const sayText = reqStr(id, params, "text");
        const voice = optStr(id, params, "voice");
        if (voice && !voices.some((v) => v.short_name === voice)) {
          throw new Error(
            `no voice '${voice}' is loaded. Use edge-tts.list-voices for the valid short_names (or omit voice to use the page's current selection).`
          );
        }
        const res = await handlePreview({
          text: sayText,
          voice,
          rate: optStr(id, params, "rate"),
          volume: optStr(id, params, "volume"),
          pitch: optStr(id, params, "pitch"),
        });
        if (!res.ok) throw new Error(res.error ?? "preview failed");
        return res;
      },
    },
  ]);

  return (
    <main className="flex-1 flex flex-col h-full">
      <PageHeader
        title="Text to Speech"
        badge={loadingVoices ? <Loader2 size={18} className="animate-spin text-text-tertiary" /> : undefined}
      />

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
              onClick={() => handlePreview()}
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
