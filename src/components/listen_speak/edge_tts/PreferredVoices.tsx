"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Loader2, Play, Plus, Trash2 } from "lucide-react";
import { isTauri } from "@/lib/tauri";
import type { TtsVoice } from "@/lib/edge_tts/types";
import { localeLabel, voiceName } from "@/lib/edge_tts/voices";

type Props = {
  voices: TtsVoice[];
  /** The page's text, so auditioning a voice speaks what is typed. Empty falls
   *  back to a line the voice can say in its own language. */
  text: string;
  /** A preview is already in flight; the page owns that flag. */
  previewing: boolean;
  onPreview: (voice: string, text: string) => void;
};

/**
 * What a preview says when the text box is empty. A German voice auditioned
 * with an English sentence hides exactly what the preference is for.
 */
const SAMPLE_BY_LANGUAGE: Record<string, string> = {
  en: "This is how I sound.",
  de: "So klinge ich.",
  fr: "Voici ma voix.",
  es: "Así suena mi voz.",
  it: "Questa è la mia voce.",
};

function sampleFor(locale: string): string {
  return SAMPLE_BY_LANGUAGE[locale.split("-")[0]] ?? "Hello — this is how I sound.";
}

/**
 * One Edge TTS voice per language, saved to the workspace settings.
 *
 * This is not read-aloud's preference: it belongs on the page where voices are
 * listed and auditioned, because deciding "my German voice" happens while
 * listening. Read-aloud then resolves a dataset's language through this map
 * (`VoiceResolution.source == "preference"`), falling back to the
 * language-less read-aloud voice and finally to the built-in default.
 */
export default function PreferredVoices({ voices, text, previewing, onPreview }: Props) {
  // Language tag → voice short name. Mirrors `tts_voice_by_language`.
  const [preferred, setPreferred] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  const languages = useMemo(() => {
    const seen: string[] = [];
    for (const v of voices) if (!seen.includes(v.locale)) seen.push(v.locale);
    return seen.sort();
  }, [voices]);

  const voicesOf = useCallback(
    (locale: string) =>
      voices
        .filter((v) => v.locale === locale)
        .slice()
        .sort((a, b) => a.short_name.localeCompare(b.short_name)),
    [voices]
  );

  useEffect(() => {
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    void (async () => {
      try {
        const workspace = await invoke<Record<string, unknown>>("settings_get_workspace");
        const map = workspace.tts_voice_by_language;
        setPreferred(map && typeof map === "object" ? (map as Record<string, string>) : {});
      } catch (e) {
        setError(`Failed to load preferred voices: ${e instanceof Error ? e.message : String(e)}`);
      } finally {
        setLoading(false);
      }
    })();
  }, []);

  /**
   * Save the whole map.
   *
   * `settings_set_workspace` replaces the entire workspace object, so the write
   * re-reads it and swaps only this field — a payload that knew only the map
   * would reset the directories and the role with it. The backend refuses a
   * language/voice pair that do not match, so a refusal puts the rows back
   * rather than leaving a pair on screen that no synthesis would ever use.
   */
  const save = useCallback(
    async (next: Record<string, string>) => {
      if (!isTauri()) return;
      const before = preferred;
      setPreferred(next);
      setSaving(true);
      setError("");
      try {
        const workspace = await invoke<Record<string, unknown>>("settings_get_workspace");
        await invoke("settings_set_workspace", {
          workspace: { ...workspace, tts_voice_by_language: next },
        });
      } catch (e) {
        setPreferred(before);
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setSaving(false);
      }
    },
    [preferred]
  );

  const rows = useMemo(
    () => Object.keys(preferred).sort().map((locale) => ({ locale, voice: preferred[locale] })),
    [preferred]
  );

  const addLanguage = () => {
    const free = languages.find((l) => !(l in preferred));
    if (!free) {
      setError("Every language with voices already has a preferred voice.");
      return;
    }
    const voice = voicesOf(free)[0];
    if (voice) void save({ ...preferred, [free]: voice.short_name });
  };

  const changeLanguage = (from: string, to: string) => {
    if (to === from) return;
    const voice = voicesOf(to)[0];
    if (!voice) return;
    const next = { ...preferred };
    delete next[from];
    // The old voice does not speak the new language and the backend would
    // refuse the pair, so moving a row also moves its voice.
    next[to] = voice.short_name;
    void save(next);
  };

  const remove = (locale: string) => {
    const next = { ...preferred };
    delete next[locale];
    void save(next);
  };

  const field =
    "px-3 py-1.5 rounded-lg bg-bg-muted border border-border-default text-sm text-text-primary cursor-pointer focus:outline-none focus:border-accent disabled:opacity-50 disabled:cursor-not-allowed";
  const iconButton =
    "p-1.5 rounded-lg bg-bg-muted border border-border-default text-text-secondary hover:bg-bg-hover cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed";

  return (
    <section className="rounded-xl border border-border-default bg-bg-card p-4 space-y-3">
      <div className="flex items-start justify-between gap-3">
        <div className="space-y-1">
          <h2 className="text-sm font-medium text-text-primary">Preferred voices</h2>
          <p className="text-xs text-text-secondary">
            One voice per language. Read-aloud uses it for a dataset written in that language, so
            you get the voice you chose instead of whichever one sorts first.
          </p>
        </div>
        <button
          className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-bg-muted border border-border-default text-xs font-medium hover:bg-bg-hover cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors shrink-0"
          onClick={addLanguage}
          disabled={loading || saving || languages.length === 0}
        >
          <Plus size={14} />
          Add language
        </button>
      </div>

      {error && (
        <div className="px-3 py-2 rounded-lg bg-error-bg text-error-text text-xs">{error}</div>
      )}

      {loading ? (
        <p className="text-xs text-text-tertiary flex items-center gap-2">
          <Loader2 size={12} className="animate-spin" /> Loading…
        </p>
      ) : rows.length === 0 ? (
        <p className="text-xs text-text-tertiary">
          No preference set — a generation falls back to the voice configured for read-aloud, then
          to the built-in default.
        </p>
      ) : (
        <ul className="space-y-2">
          {rows.map(({ locale, voice }) => {
            const list = voicesOf(locale);
            // A saved voice the fetched catalogue no longer lists (offline, or
            // retired) still has to be visible: silently blanking the row would
            // look like "no preference".
            const missing = !list.some((v) => v.short_name === voice);
            const languageMissing = !languages.includes(locale);
            return (
              <li key={locale} className="flex flex-wrap items-center gap-2">
                <select
                  className={`${field} w-[170px]`}
                  value={locale}
                  onChange={(e) => changeLanguage(locale, e.target.value)}
                  disabled={saving}
                >
                  {languageMissing && <option value={locale}>{localeLabel(locale)}</option>}
                  {languages.map((l) => (
                    <option key={l} value={l}>
                      {localeLabel(l)}
                    </option>
                  ))}
                </select>
                <select
                  className={`${field} w-[200px]`}
                  value={voice}
                  onChange={(e) => void save({ ...preferred, [locale]: e.target.value })}
                  disabled={saving}
                >
                  {missing && <option value={voice}>{voiceName(voice)} (not listed)</option>}
                  {list.map((v) => (
                    <option key={v.short_name} value={v.short_name}>
                      {voiceName(v.short_name)} ({v.gender})
                    </option>
                  ))}
                </select>
                <button
                  className={iconButton}
                  onClick={() => onPreview(voice, text.trim() || sampleFor(locale))}
                  disabled={previewing || saving}
                  title="Hear this voice"
                >
                  {previewing ? (
                    <Loader2 size={14} className="animate-spin" />
                  ) : (
                    <Play size={14} />
                  )}
                </button>
                <button
                  className={iconButton}
                  onClick={() => remove(locale)}
                  disabled={saving}
                  title="Remove this preference — the language falls back to the configured voice"
                >
                  <Trash2 size={14} />
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
