"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, Check, Loader2, Play, Square, Trash2, Upload, Wand2 } from "lucide-react";
import type { AudioProbe, ReadStandardAudio, VoiceResolution } from "@/lib/read_aloud/types";
import ConfirmDialog from "@/components/books/ConfirmDialog";
import {
  deleteStandardAudio,
  generateStandardAudio,
  guessStandardAudioSpan,
  importStandardAudio,
  listStandardAudio,
  probeAudioSource,
  resolveVoice,
  setDefaultStandardAudio,
} from "@/lib/read_aloud/standardAudio";
import { isMobileApp } from "@/lib/platform";
import { localeLabel, voiceName } from "@/lib/edge_tts/voices";
import { isTauri } from "@/lib/tauri";
import { logError } from "@/lib/logger";
import AudioTrimTrack from "./AudioTrimTrack";

const LOG_MODULE = "read-aloud";

/** A picked file, in whichever form the platform could hand it over. */
type Source = {
  /** A real filesystem path (desktop) — also playable while cropping. */
  path?: string;
  /** The bytes, when a mobile picker handed over a `content://` URI instead. */
  base64?: string;
  fileName: string;
};

type Voice = { short_name: string; locale: string; gender: string };

type Props = {
  datasetUuid: string;
  textUuid: string;
  /** Called after any clip change, so the page can refresh its badges. */
  onChanged?: () => void;
  /** Persist the dataset's language. The language picker below is an override
   *  of the voice *and* a fact about the dataset: a per-language preference can
   *  only apply once the dataset says which language it holds. */
  onLanguageChange?: (language: string) => Promise<void> | void;
  /** The page's draft differs from the stored text. Clips describe the stored
   *  text, so every control that would add, replace or delete one is paused;
   *  playing and reading stay live. */
  disabled?: boolean;
  /** Bumped by the page after a save: a clip that has just gone stale has to
   *  say so without the section being remounted. */
  refreshKey?: number;
};

function seconds(value: number | null): string {
  // `null` is "nobody measured this", which is not the same as zero seconds —
  // a fabricated `0.0 s` reads as a real measurement.
  if (value == null || !(value > 0)) return "—";
  return value >= 60
    ? `${Math.floor(value / 60)}:${String(Math.round(value % 60)).padStart(2, "0")}`
    : `${value.toFixed(1)} s`;
}

function readFileAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const url = String(reader.result ?? "");
      // `data:audio/mpeg;base64,XXXX` → XXXX
      const comma = url.indexOf(",");
      resolve(comma >= 0 ? url.slice(comma + 1) : "");
    };
    reader.onerror = () => reject(new Error("Could not read that file."));
    reader.readAsDataURL(file);
  });
}

/** How the resolved voice was chosen, in the words the line under the button
 *  uses. Mirrors `VoiceResolution.source`. */
const SOURCE_LABELS: Record<VoiceResolution["source"], string> = {
  param: "this call",
  preference: "preferred",
  setting: "set on the Edge TTS page",
  default: "built-in default",
};

/** What made a clip, in the row's words. An aligned clip is a span of a longer
 *  recording, which the note spells out (`Align · file · a–b s`). */
const KIND_LABELS: Record<ReadStandardAudio["kind"], string> = {
  tts: "Edge TTS",
  upload: "Upload",
  align: "Aligned",
};

/**
 * The standard audio of one text: the reference clip its pronunciation is
 * measured against.
 *
 * A text with no clip cannot be scored for pronunciation at all, so this is
 * where the scorer's reference comes from — either synthesised (word timings
 * included, which is what makes per-word attribution exact) or uploaded and
 * cut. Unlike the normalized-text draft beside it, every action here is
 * immediate: there is nothing to Save or Cancel.
 */
export default function StandardAudioSection({
  datasetUuid,
  textUuid,
  onChanged,
  onLanguageChange,
  disabled = false,
  refreshKey = 0,
}: Props) {
  const [clips, setClips] = useState<ReadStandardAudio[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Generate
  const [voices, setVoices] = useState<Voice[]>([]);
  const [lang, setLang] = useState("");
  const [voice, setVoice] = useState("");
  const [voicesFailed, setVoicesFailed] = useState(false);
  // What a generation with no voice named would speak with, and why. The
  // pickers open on it; the line under them says where it came from.
  const [resolved, setResolved] = useState<VoiceResolution | null>(null);
  // A picker has been touched, so the next generation sends its voice instead
  // of asking the backend to resolve one.
  const [override, setOverride] = useState(false);
  const [langSaving, setLangSaving] = useState(false);

  // Upload + crop
  const [source, setSource] = useState<Source | null>(null);
  const [probe, setProbe] = useState<AudioProbe | null>(null);
  const [cropStart, setCropStart] = useState(0);
  const [cropEnd, setCropEnd] = useState(0);
  const [uploadNote, setUploadNote] = useState("");
  // What the last span guess found (or why it refused). Cleared whenever a new
  // file is picked, because it describes that file's text placement.
  const [guessNote, setGuessNote] = useState<{ kind: "ok" | "warn"; text: string } | null>(null);
  // Bumped whenever the picked file changes, so a guess that answers late
  // cannot set the edges of a file it never heard.
  const guessTokenRef = useRef(0);
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  // Playback of a stored clip
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const [playingUuid, setPlayingUuid] = useState<string | null>(null);

  // Deleting a clip removes its audio file for good, and deleting the one in
  // use silently stops pronunciation scoring — both deserve a question first.
  const [confirmDelete, setConfirmDelete] = useState<ReadStandardAudio | null>(null);

  const refresh = useCallback(async () => {
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    try {
      setClips(await listStandardAudio(datasetUuid, textUuid));
    } catch (e) {
      const msg = String(e);
      logError(`list standard audio failed: ${msg}`, LOG_MODULE);
      setError(msg);
    } finally {
      setLoading(false);
    }
  }, [datasetUuid, textUuid]);

  useEffect(() => {
    void refresh();
    // `refreshKey` is a deliberate trigger: a save on the page can change which
    // clips are stale without changing this section's identity.
  }, [refresh, refreshKey]);

  /** Re-read which voice this dataset resolves to. Split from the failure
   *  handling below: the voice list needs the network, the resolution does not. */
  const resolve = useCallback(async (): Promise<VoiceResolution | null> => {
    try {
      const res = await resolveVoice(datasetUuid);
      setResolved(res);
      return res;
    } catch (e) {
      // Leave both pickers empty: Generate then sends no voice and the backend
      // resolves exactly as it would have.
      logError(`resolve voice failed: ${String(e)}`, LOG_MODULE);
      return null;
    }
  }, [datasetUuid]);

  // Voices (network) and the voice this dataset actually speaks with are only
  // needed for the generate action; a failure must not block the rest.
  useEffect(() => {
    if (!isTauri()) return;
    void (async () => {
      try {
        const list = await invoke<Voice[]>("edge_tts_list_voices");
        const sorted = [...list].sort((a, b) =>
          a.locale === b.locale
            ? a.short_name.localeCompare(b.short_name)
            : a.locale.localeCompare(b.locale)
        );
        setVoices(sorted);
      } catch (e) {
        logError(`load TTS voices failed: ${String(e)}`, LOG_MODULE);
        setVoicesFailed(true);
        return;
      }
      // Open the pickers on the *resolved* voice — the one this dataset's
      // language prefers — rather than on the language-less setting's own
      // locale, so the selectors show what will be spoken.
      const res = await resolve();
      if (res) {
        setLang(res.locale);
        setVoice(res.voice);
      }
    })();
  }, [resolve]);

  // The two pickers drive one value: the language narrows the list, and picking
  // a language without a voice would quietly fall back to the backend's default,
  // so a language always selects its first voice. The empty language means "no
  // override" — whatever the dataset resolves to is spoken.
  const locales = useMemo(() => {
    const seen: string[] = [];
    for (const v of voices) if (!seen.includes(v.locale)) seen.push(v.locale);
    return seen;
  }, [voices]);

  const langVoices = lang ? voices.filter((v) => v.locale === lang) : [];

  const chooseLocale = (locale: string) => {
    setOverride(Boolean(locale));
    setLang(locale);
    setVoice(locale ? (voices.find((v) => v.locale === locale)?.short_name ?? "") : "");
    if (!locale) return;
    // The language is the one picker that also writes: a per-language voice
    // preference is keyed by the dataset's language, so naming it here is a
    // fact about the dataset rather than a filter on a list.
    const current = resolved?.dataset_language ?? "";
    if (locale === current || !onLanguageChange) return;
    setLangSaving(true);
    void (async () => {
      try {
        await onLanguageChange(locale);
        // Re-resolve: the preference for the language just named may not be the
        // first voice the locale happened to list (alphabetical order is how
        // the Austrian narrator got picked).
        const res = await resolve();
        if (res && res.locale === locale) setVoice(res.voice);
      } catch (e) {
        const msg = String(e);
        logError(`save dataset language failed: ${msg}`, LOG_MODULE);
        setError(msg);
      } finally {
        setLangSaving(false);
      }
    })();
  };

  const stopAudio = useCallback(() => {
    const el = audioRef.current;
    if (el) {
      el.pause();
      el.currentTime = 0;
      audioRef.current = null;
    }
    setPlayingUuid(null);
  }, []);

  useEffect(() => stopAudio, [stopAudio]);

  const togglePlay = useCallback(
    (clip: ReadStandardAudio) => {
      if (audioRef.current) {
        stopAudio();
        return;
      }
      if (!clip.audio_url) return;
      const el = new Audio(convertFileSrc(clip.audio_url));
      audioRef.current = el;
      el.onended = () => {
        audioRef.current = null;
        setPlayingUuid(null);
      };
      setPlayingUuid(clip.uuid);
      el.play().catch(() => stopAudio());
    },
    [stopAudio]
  );

  const afterChange = useCallback(async () => {
    await refresh();
    onChanged?.();
  }, [onChanged, refresh]);

  const generate = useCallback(async () => {
    setBusy("generate");
    setError(null);
    try {
      await generateStandardAudio({
        datasetUuid,
        textUuid,
        // No voice unless a picker was touched: the dataset's language decides,
        // and naming the voice here would make every clip `param` and freeze
        // the choice into this page.
        voice: override ? voice.trim() || undefined : undefined,
      });
      await afterChange();
      // A preference may have moved since the section opened; re-read so the
      // line under the pickers still says what will be spoken next.
      const res = await resolve();
      if (res && !override) {
        setLang(res.locale);
        setVoice(res.voice);
      }
    } catch (e) {
      const msg = String(e);
      logError(`generate standard audio failed: ${msg}`, LOG_MODULE);
      setError(msg);
    } finally {
      setBusy(null);
    }
  }, [afterChange, datasetUuid, override, resolve, textUuid, voice]);

  const pickSource = useCallback(async () => {
    setError(null);
    try {
      if (!isMobileApp()) {
        // Desktop: the dialog hands back a real path, which is also what the
        // crop track plays. Reading it as base64 would lose that.
        const picked = await open({
          multiple: false,
          directory: false,
          filters: [
            { name: "Audio", extensions: ["wav", "mp3", "m4a", "m4b", "ogg", "flac", "mp4", "webm"] },
          ],
        });
        if (!picked || Array.isArray(picked)) return;
        setBusy("probe");
        const probed = await probeAudioSource({ sourcePath: picked });
        setSource({ path: picked, fileName: probed.file_name || "recording" });
        setProbe(probed);
        setCropStart(0);
        setCropEnd(probed.duration);
        setUploadNote("");
        setGuessNote(null);
        guessTokenRef.current += 1;
      } else {
        // Android: the picker yields a content:// URI, not a path, so the bytes
        // travel as base64 — the same shape the recorder posts WAVs with.
        fileInputRef.current?.click();
      }
    } catch (e) {
      const msg = String(e);
      logError(`probe audio failed: ${msg}`, LOG_MODULE);
      setError(msg);
    } finally {
      setBusy(null);
    }
  }, []);

  const onFileChosen = useCallback(async (file: File | undefined) => {
    if (!file) return;
    setBusy("probe");
    setError(null);
    try {
      const base64 = await readFileAsBase64(file);
      const probed = await probeAudioSource({ sourceBase64: base64, fileName: file.name });
      setSource({ base64, fileName: file.name });
      setProbe(probed);
      setCropStart(0);
      setCropEnd(probed.duration);
      setUploadNote("");
      setGuessNote(null);
      guessTokenRef.current += 1;
    } catch (e) {
      const msg = String(e);
      logError(`probe audio failed: ${msg}`, LOG_MODULE);
      setError(msg);
    } finally {
      setBusy(null);
    }
  }, []);

  // The model listens to the whole file and the text is aligned to what it
  // heard, so this is for a recording that holds more than the text — the one
  // case where the edges cannot be found by looking at the waveform.
  //
  // A guess outlives the dialog it was started from: picking another file or
  // closing this one while the model is still listening must not let the late
  // answer set the edges of whatever is on screen by then.
  const guessSpan = useCallback(async () => {
    if (!source) return;
    const token = ++guessTokenRef.current;
    setBusy("guess");
    setError(null);
    setGuessNote(null);
    try {
      const guess = await guessStandardAudioSpan({
        datasetUuid,
        textUuid,
        sourcePath: source.path,
        sourceBase64: source.base64,
      });
      if (token !== guessTokenRef.current) return;
      setCropStart(guess.start);
      setCropEnd(guess.end);
      const percent = Math.round(guess.confidence * 100);
      setGuessNote({
        kind: "ok",
        text:
          `Heard ${guess.matched} of ${guess.reference} words (${percent}%) and put the edges on them. ` +
          (percent < 80
            ? "That is a loose match, so check both edges before saving."
            : "Check them before saving."),
      });
    } catch (e) {
      if (token !== guessTokenRef.current) return;
      const msg = String(e);
      logError(`guess standard audio span failed: ${msg}`, LOG_MODULE);
      setGuessNote({ kind: "warn", text: msg });
    } finally {
      // A newer pick owns the progress line by now; leave its state alone.
      if (token === guessTokenRef.current) setBusy(null);
    }
  }, [datasetUuid, source, textUuid]);

  const saveUpload = useCallback(async () => {
    if (!source) return;
    setBusy("import");
    setError(null);
    try {
      await importStandardAudio({
        datasetUuid,
        textUuid,
        sourcePath: source.path,
        sourceBase64: source.base64,
        fileName: source.fileName,
        note: uploadNote.trim() || undefined,
        start: cropStart,
        end: cropEnd,
      });
      setSource(null);
      setProbe(null);
      await afterChange();
    } catch (e) {
      const msg = String(e);
      logError(`import standard audio failed: ${msg}`, LOG_MODULE);
      setError(msg);
    } finally {
      setBusy(null);
    }
  }, [afterChange, cropEnd, cropStart, datasetUuid, source, textUuid, uploadNote]);

  const makeDefault = useCallback(
    async (clip: ReadStandardAudio) => {
      setBusy(clip.uuid);
      setError(null);
      try {
        await setDefaultStandardAudio(datasetUuid, clip.uuid);
        await afterChange();
      } catch (e) {
        const msg = String(e);
        logError(`set default standard audio failed: ${msg}`, LOG_MODULE);
        setError(msg);
      } finally {
        setBusy(null);
      }
    },
    [afterChange, datasetUuid]
  );

  const remove = useCallback(
    async (clip: ReadStandardAudio) => {
      setBusy(clip.uuid);
      setError(null);
      try {
        await deleteStandardAudio(datasetUuid, clip.uuid);
        if (playingUuid === clip.uuid) stopAudio();
        await afterChange();
      } catch (e) {
        const msg = String(e);
        logError(`delete standard audio failed: ${msg}`, LOG_MODULE);
        setError(msg);
      } finally {
        setBusy(null);
      }
    },
    [afterChange, datasetUuid, playingUuid, stopAudio]
  );

  // One control height for the whole section: a select sitting beside a button a
  // few pixels shorter reads as a mistake, and the clip rows inherit the rhythm.
  const controlHeight = "h-8";
  const field = `w-full px-2 rounded-md bg-bg-muted border border-border-default text-text-primary text-sm focus:outline-none focus:border-accent-bg ${controlHeight}`;
  const selectClass = `px-2 rounded-md bg-bg-muted border border-border-default text-text-primary text-xs focus:outline-none focus:border-accent-bg cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed ${controlHeight}`;
  const smallButton = `inline-flex items-center gap-1 px-2 rounded-md text-[11px] bg-bg-muted hover:bg-bg-hover text-text-secondary cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed ${controlHeight}`;
  // Generating is the one action that produces a scorable reference, so it is
  // the only filled button here.
  const primaryButton = `inline-flex items-center gap-1.5 px-2.5 rounded-md text-[11px] font-medium bg-accent-bg text-white hover:opacity-90 cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed ${controlHeight}`;
  const chip = "text-[10px] px-1.5 py-0.5 rounded-full";

  const label = (clip: ReadStandardAudio) => clip.note || KIND_LABELS[clip.kind] || "Upload";
  const defaultClip = clips.find((c) => c.is_default) ?? null;
  const voiceLabel = voice ? voiceName(voice) : "the resolved voice";
  // One progress line beats a spinner hidden in an 11px chip: synthesis is a
  // network round trip and the import decodes a file, both slow enough that a
  // second click is tempting.
  const busyLabel =
    busy === "generate"
      ? `Synthesising with ${voiceLabel}…`
      : busy === "probe"
        ? "Reading the file…"
        : busy === "guess"
          ? "Listening to the file — this takes as long as the recording is long…"
          : busy === "import"
            ? "Saving the clip…"
            : busy
              ? "Updating the clip…"
              : null;

  // Where the voice comes from, said once instead of discovered per clip: this
  // section shows the resolution, it does not own the setting. The preference
  // is edited where voices are audited (the Edge TTS page), because
  // `tts_synthesize` speaks with it too.
  const provenance = resolved && (
    <div className="flex flex-col gap-0.5">
      <div className="flex flex-wrap items-baseline gap-x-1.5 text-[11px]">
        <span className="text-text-primary">
          {localeLabel(resolved.dataset_language || resolved.locale)}
        </span>
        <span className="text-text-tertiary">·</span>
        <span className="text-text-primary">{voiceName(resolved.voice)}</span>
        <span className="text-text-tertiary">({SOURCE_LABELS[resolved.source] ?? resolved.source})</span>
        <button
          className="text-accent-bg hover:underline cursor-pointer"
          onClick={() => {
            // The voice setting lives on the Edge TTS page; a picker here would
            // read as dataset-local while being global.
            if (isTauri()) void emit("agent-action", { type: "navigate", tab: "edge-tts" });
          }}
          title="Set the voice this language uses on the Edge TTS page"
        >
          Change
        </button>
        {langSaving && <span className="text-text-tertiary">saving the language…</span>}
      </div>
      {resolved.warning && (
        <span className="text-[10px] text-warning-text">{resolved.warning}</span>
      )}
      {resolved.hint && (
        <span className="text-[10px] text-text-tertiary">{resolved.hint}</span>
      )}
    </div>
  );

  const actions = (
    <div className="flex flex-col gap-1.5">
      {provenance}
      <div className="flex flex-wrap items-center gap-2">
        <select
          className={`${selectClass} max-w-[170px]`}
          value={lang}
          onChange={(e) => chooseLocale(e.target.value)}
          disabled={disabled || busy === "generate" || langSaving || voices.length === 0}
          title="Language the synthesis speaks; the voice list follows it, and the dataset remembers it"
        >
          <option value="">The dataset's voice</option>
          {locales.map((l) => (
            <option key={l} value={l}>
              {localeLabel(l)}
            </option>
          ))}
        </select>
        <select
          className={`${selectClass} max-w-[200px]`}
          value={voice}
          onChange={(e) => {
            setOverride(true);
            setVoice(e.target.value);
          }}
          disabled={disabled || busy === "generate" || !lang}
          title={
            lang
              ? "Voice of the chosen language"
              : "Pick a language first, or leave the voice resolved from the dataset's language"
          }
        >
          {lang ? (
            langVoices.map((v) => (
              <option key={v.short_name} value={v.short_name}>
                {voiceName(v.short_name)} ({v.gender})
              </option>
            ))
          ) : (
            <option value="">{voice ? voiceName(voice) : "the resolved voice"}</option>
          )}
        </select>
        <button
          className={primaryButton}
          disabled={disabled || busy === "generate"}
          onClick={() => void generate()}
          title="Synthesise this text with Edge TTS. The word timings come with it, so per-word pronunciation is exact."
        >
          {busy === "generate" ? <Loader2 size={12} className="animate-spin" /> : <Wand2 size={12} />}
          Generate with Edge TTS
        </button>
        <button
          className={smallButton}
          disabled={disabled || busy === "probe"}
          onClick={() => void pickSource()}
          title="Use a person reading the text. An uploaded clip has no word timings, so its words are attributed by time — the overall score is unaffected."
        >
          {busy === "probe" ? <Loader2 size={12} className="animate-spin" /> : <Upload size={12} />}
          Upload audio
        </button>
        {voicesFailed && (
          <span className="text-[11px] text-text-tertiary">
            Voice list unavailable offline — the dataset's resolved voice is used.
          </span>
        )}
      </div>
    </div>
  );

  return (
    <section className="rounded-lg border border-border-default bg-bg-card">
      <div className="flex flex-wrap items-center gap-2 px-3 py-2 border-b border-border-default">
        <h3 className="text-xs font-semibold uppercase tracking-wide text-text-tertiary">
          Standard audio
        </h3>
        <span className="text-[11px] text-text-secondary truncate">
          {loading
            ? "loading…"
            : clips.length === 0
              ? "none yet"
              : `${clips.length} clip${clips.length === 1 ? "" : "s"} · ${
                  defaultClip ? label(defaultClip) : "nothing in use"
                }`}
        </span>
        <div className="flex-1" />
        {/* Why the buttons below are grey, said once, where the eye lands first. */}
        {disabled ? (
          <span className="text-[11px] text-warning-text">paused until the text is saved</span>
        ) : (
          <span
            className="text-[11px] text-text-tertiary"
            title="Nothing here is staged: a clip is added, chosen or deleted at once, so closing the page never loses one."
          >
            saves immediately
          </span>
        )}
      </div>

      <div className="flex flex-col gap-2 p-3">
        {disabled && (
          <p className="text-[11px] text-text-secondary bg-bg-muted rounded-md p-2">
            A clip describes the text as it is stored — save your changes to add, choose or delete
            one. Playback stays available.
          </p>
        )}

        {error && (
          <div className="p-2 rounded-md bg-error-bg text-error-text text-xs break-words">{error}</div>
        )}

        {busyLabel && (
          <p className="text-[11px] text-text-secondary flex items-center gap-1.5">
            <Loader2 size={11} className="animate-spin" />
            {busyLabel}
          </p>
        )}

        {loading ? (
          <p className="text-xs text-text-tertiary flex items-center gap-2">
            <Loader2 size={12} className="animate-spin" /> Loading clips…
          </p>
        ) : clips.length === 0 ? (
          <div className="flex flex-col gap-2 p-3 rounded-md bg-bg-muted">
            <p className="text-xs text-text-secondary">
              No standard audio yet — pronunciation cannot be scored until this text has one.
            </p>
            {actions}
          </div>
        ) : (
          <>
            <ul className="flex flex-col gap-1">
              {clips.map((clip) => {
                const inUse = clip.is_default;
                const missing = !clip.audio_url;
                return (
                  <li
                    key={clip.uuid}
                    className={`flex items-start gap-2 px-2 py-1.5 rounded-md border ${
                      inUse ? "border-accent-bg/40 bg-accent-bg/5" : "border-transparent bg-bg-muted"
                    }`}
                  >
                    <input
                      id={`clip-${clip.uuid}`}
                      type="radio"
                      name={`standard-audio-${textUuid}`}
                      className="mt-1 h-3.5 w-3.5 shrink-0 cursor-pointer accent-[var(--color-accent-bg)] disabled:cursor-not-allowed"
                      checked={inUse}
                      disabled={disabled || busy === clip.uuid}
                      onChange={() => void makeDefault(clip)}
                      title="Measure pronunciation against this clip"
                    />
                    <label
                      htmlFor={`clip-${clip.uuid}`}
                      className="flex flex-col min-w-0 flex-1 cursor-pointer"
                    >
                      <span className="text-xs text-text-primary truncate">{label(clip)}</span>
                      <span className="text-[10px] text-text-tertiary flex items-center gap-x-1.5 flex-wrap">
                        <span>{KIND_LABELS[clip.kind] ?? "Upload"}</span>
                        <span className="tabular-nums">{seconds(clip.duration)}</span>
                        {inUse && (
                          <span className={`${chip} bg-accent-bg/20 text-accent-bg`}>in use</span>
                        )}
                        {clip.stale && (
                          <span
                            className={`${chip} bg-error-bg text-error-text`}
                            title="Made from an earlier version of the text — generate or upload a new one"
                          >
                            Stale
                          </span>
                        )}
                        {missing && (
                          <span
                            className="inline-flex items-center gap-0.5 text-warning-text"
                            title="The audio file is not on this device — sync the dataset to fetch it"
                          >
                            <AlertTriangle size={10} />
                            not on this device
                          </span>
                        )}
                      </span>
                    </label>
                    {clip.audio_url && (
                      <button
                        className={smallButton}
                        onClick={() => togglePlay(clip)}
                        title={playingUuid === clip.uuid ? "Stop" : "Play"}
                      >
                        {playingUuid === clip.uuid ? <Square size={12} /> : <Play size={12} />}
                      </button>
                    )}
                    <button
                      className={`${smallButton} text-error-text`}
                      disabled={disabled || busy === clip.uuid}
                      onClick={() => setConfirmDelete(clip)}
                      title="Delete this clip and its audio file"
                    >
                      <Trash2 size={12} />
                    </button>
                  </li>
                );
              })}
            </ul>
            {actions}
          </>
        )}

        {/* Upload + crop */}
        {source && probe && (
          <div className="flex flex-col gap-2 mt-1 p-3 rounded-md border border-border-default bg-bg-muted">
            <p className="text-[11px] text-text-secondary">
              New clip from <span className="text-text-primary">{source.fileName}</span> — drag the
              handles to the part that reads the text, or press <span className="text-text-primary">Guess
              from speech</span> when the file holds more than the text.
              {!source.path && " Preview is unavailable for files picked on mobile."}
            </p>
            <AudioTrimTrack
              peaks={probe.peaks}
              duration={probe.duration}
              start={cropStart}
              end={cropEnd}
              onChange={(s, e) => {
                setCropStart(s);
                setCropEnd(e);
              }}
              audioUrl={source.path}
              guess={{
                running: busy === "guess",
                note: guessNote,
                run: () => void guessSpan(),
              }}
            />
            <input
              className={field}
              value={uploadNote}
              onChange={(e) => setUploadNote(e.target.value)}
              placeholder={`Note (optional) — defaults to "Upload · ${source.fileName}"`}
            />
            <div className="flex justify-end gap-2">
              <button
                className={smallButton}
                onClick={() => {
                  setSource(null);
                  setProbe(null);
                  setGuessNote(null);
                  guessTokenRef.current += 1;
                }}
              >
                Cancel
              </button>
              <button
                className={primaryButton}
                disabled={busy === "import" || busy === "guess"}
                onClick={() => void saveUpload()}
              >
                {busy === "import" ? (
                  <Loader2 size={12} className="animate-spin" />
                ) : (
                  <Check size={12} />
                )}
                Save clip
              </button>
            </div>
          </div>
        )}

        <input
          ref={fileInputRef}
          type="file"
          accept="audio/*"
          className="hidden"
          onChange={(e) => {
            void onFileChosen(e.target.files?.[0]);
            e.target.value = "";
          }}
        />
      </div>

      <ConfirmDialog
        request={
          confirmDelete
            ? {
                title: "Delete standard audio",
                message: confirmDelete.is_default
                  ? `"${label(confirmDelete)}" is the clip pronunciation is measured against, so deleting it stops pronunciation scoring until another one is chosen.`
                  : `Delete "${label(confirmDelete)}"? Its audio file is removed with it.`,
                confirmLabel: "Delete",
                onConfirm: () => void remove(confirmDelete),
              }
            : null
        }
        onClose={() => setConfirmDelete(null)}
      />
    </section>
  );
}
