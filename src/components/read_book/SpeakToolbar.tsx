"use client";

/**
 * SpeakToolbar — a one-line bar that reads a block of text aloud with Edge TTS.
 * A language selector (only the languages Edge actually has voices for) picks
 * which preferred voice speaks; the server resolves the exact voice, so this
 * stays a single dropdown. The Listen button toggles into Stop while its own
 * utterance plays, and starting another utterance anywhere stops this one.
 *
 * It sits above generated text (explanation / simplification) and above the
 * Notes textarea — wherever there is prose worth hearing.
 */

import { useEffect, useMemo, useState } from "react";
import { Volume2, Square, Loader2 } from "lucide-react";
import { speakText, stopSpeech, onSpeechStop, listLanguages } from "@/lib/edge_tts/speak";

type Props = {
  /** The text to read (Markdown is stripped before synthesis). */
  text: string;
  /** Current language tag; "" = Auto (book / app preferred voice). */
  language: string;
  onLanguageChange: (lang: string) => void;
  disabled?: boolean;
};

/** Human name for a language subtag, degrading to the raw code if Intl balks. */
function languageLabel(code: string): string {
  if (!code) return "Auto (preferred)";
  try {
    const name = new Intl.DisplayNames(["en"], { type: "language" }).of(code);
    return name || code;
  } catch {
    return code;
  }
}

export default function SpeakToolbar({ text, language, onLanguageChange, disabled }: Props) {
  const [langs, setLangs] = useState<string[]>([]);
  const [playing, setPlaying] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    let alive = true;
    listLanguages().then((l) => alive && setLangs(l));
    return () => {
      alive = false;
    };
  }, []);

  // Any global stop (this utterance ended, or another replaced it) resets us.
  useEffect(() => onSpeechStop(() => setPlaying(false)), []);

  const options = useMemo(() => {
    const set = new Set(langs);
    // Keep a language that has no matching voice group visible (e.g. a book's
    // "de-DE" when only "de" is offered), so the select never silently resets.
    const current = language.split("-")[0].toLowerCase();
    if (current) set.add(current);
    return ["", ...[...set].sort()];
  }, [langs, language]);

  const canSpeak = !!text.trim() && !disabled;

  const handleToggle = async () => {
    if (playing) {
      stopSpeech();
      setPlaying(false);
      return;
    }
    if (!canSpeak || loading) return;
    setLoading(true);
    setError("");
    try {
      await speakText(text, language);
      setPlaying(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="flex flex-wrap items-center gap-2">
      <select
        className="px-2 py-1 rounded-md bg-bg-input border border-border-default text-xs text-text-primary focus:outline-none focus:border-accent cursor-pointer"
        value={options.includes(language) ? language : language.split("-")[0].toLowerCase()}
        onChange={(e) => onLanguageChange(e.target.value)}
        disabled={disabled}
        title="Language of the voice"
        aria-label="Read-aloud language"
      >
        {options.map((code) => (
          <option key={code || "auto"} value={code}>
            {languageLabel(code)}
          </option>
        ))}
      </select>
      <button
        onClick={handleToggle}
        disabled={!playing && (!canSpeak || loading)}
        title={playing ? "Stop" : "Listen"}
        className="flex items-center gap-1 px-2.5 py-1 rounded-md text-xs font-medium border border-border-default text-text-secondary hover:bg-bg-hover disabled:opacity-40 cursor-pointer"
      >
        {loading ? (
          <Loader2 size={13} className="animate-spin" />
        ) : playing ? (
          <Square size={13} />
        ) : (
          <Volume2 size={13} />
        )}
        {playing ? "Stop" : "Listen"}
      </button>
      {error && (
        <span className="text-[11px] text-red-500 truncate" title={error}>
          {error}
        </span>
      )}
    </div>
  );
}
