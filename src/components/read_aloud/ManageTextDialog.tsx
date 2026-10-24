"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import {
  AudioLines,
  Check,
  Loader2,
  Pencil,
  Play,
  Square,
  Trash2,
  Upload,
  Wand2,
  X,
} from "lucide-react";
import type { AudioProbe, ReadStandardAudio, ReadText } from "@/lib/read_aloud/types";
import {
  deleteStandardAudio,
  generateStandardAudio,
  importStandardAudio,
  listStandardAudio,
  probeAudioSource,
  setDefaultStandardAudio,
} from "@/lib/read_aloud/standardAudio";
import { isMobileApp } from "@/lib/platform";
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
  text: ReadText;
  onClose: () => void;
  /** Close this dialog and open the inline text editor instead. */
  onEditText: () => void;
  /** Called after any clip change, so the page can refresh its badges. */
  onChanged?: () => void;
};

function wordCount(content: string): number {
  return content.trim().split(/\s+/).filter(Boolean).length;
}

function seconds(value: number): string {
  if (!(value > 0)) return "—";
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

/**
 * Everything about one text that is not the reading itself: its content, and
 * the standard audio its pronunciation is measured against.
 *
 * A text with no clip cannot be scored for pronunciation at all, so this dialog
 * is where the scorer's reference comes from: either synthesised (word timings
 * included, which is what makes per-word attribution exact) or uploaded and cut.
 */
export default function ManageTextDialog({
  datasetUuid,
  text,
  onClose,
  onEditText,
  onChanged,
}: Props) {
  const [clips, setClips] = useState<ReadStandardAudio[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Generate
  const [voices, setVoices] = useState<Voice[]>([]);
  const [voice, setVoice] = useState("");
  const [voicesFailed, setVoicesFailed] = useState(false);

  // Upload + crop
  const [source, setSource] = useState<Source | null>(null);
  const [probe, setProbe] = useState<AudioProbe | null>(null);
  const [cropStart, setCropStart] = useState(0);
  const [cropEnd, setCropEnd] = useState(0);
  const [uploadNote, setUploadNote] = useState("");
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  // Playback of a stored clip
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const [playingUuid, setPlayingUuid] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    try {
      setClips(await listStandardAudio(datasetUuid, text.uuid));
    } catch (e) {
      const msg = String(e);
      logError(`list standard audio failed: ${msg}`, LOG_MODULE);
      setError(msg);
    } finally {
      setLoading(false);
    }
  }, [datasetUuid, text.uuid]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Voices (network) and the configured default are only needed for the
  // generate action; a failure must not block the rest of the dialog.
  useEffect(() => {
    if (!isTauri()) return;
    void (async () => {
      try {
        const list = await invoke<Voice[]>("edge_tts_list_voices");
        setVoices(
          [...list].sort((a, b) =>
            a.locale === b.locale
              ? a.short_name.localeCompare(b.short_name)
              : a.locale.localeCompare(b.locale)
          )
        );
        const settings = await invoke<{ read_aloud_tts_voice?: string }>("settings_get");
        setVoice(settings.read_aloud_tts_voice ?? "");
      } catch (e) {
        logError(`load TTS voices failed: ${String(e)}`, LOG_MODULE);
        setVoicesFailed(true);
      }
    })();
  }, []);

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
        textUuid: text.uuid,
        voice: voice.trim() || undefined,
      });
      await afterChange();
    } catch (e) {
      const msg = String(e);
      logError(`generate standard audio failed: ${msg}`, LOG_MODULE);
      setError(msg);
    } finally {
      setBusy(null);
    }
  }, [afterChange, datasetUuid, text.uuid, voice]);

  const pickSource = useCallback(async () => {
    setError(null);
    try {
      if (!isMobileApp()) {
        // Desktop: the dialog hands back a real path, which is also what the
        // crop track plays. Reading it as base64 would lose that.
        const picked = await open({
          multiple: false,
          directory: false,
          filters: [{ name: "Audio", extensions: ["wav", "mp3", "m4a", "m4b", "ogg", "flac", "mp4", "webm"] }],
        });
        if (!picked || Array.isArray(picked)) return;
        setBusy("probe");
        const probed = await probeAudioSource({ sourcePath: picked });
        setSource({ path: picked, fileName: probed.file_name || "recording" });
        setProbe(probed);
        setCropStart(0);
        setCropEnd(probed.duration);
        setUploadNote("");
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
    } catch (e) {
      const msg = String(e);
      logError(`probe audio failed: ${msg}`, LOG_MODULE);
      setError(msg);
    } finally {
      setBusy(null);
    }
  }, []);

  const saveUpload = useCallback(async () => {
    if (!source) return;
    setBusy("import");
    setError(null);
    try {
      await importStandardAudio({
        datasetUuid,
        textUuid: text.uuid,
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
  }, [afterChange, cropEnd, cropStart, datasetUuid, source, text.uuid, uploadNote]);

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

  const field =
    "w-full p-2 rounded-md bg-bg-muted border border-border-default text-text-primary text-sm focus:outline-none focus:border-accent-bg";
  const sectionTitle = "text-xs font-semibold uppercase tracking-wide text-text-tertiary";
  const smallButton =
    "inline-flex items-center gap-1 px-2 py-1 rounded-md text-[11px] bg-bg-muted hover:bg-bg-hover text-text-secondary cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed";

  const effectiveReference = text.normalized_content?.trim() || text.content;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
      onClick={onClose}
    >
      <div
        className="bg-bg-card rounded-2xl shadow-2xl w-full max-w-2xl max-h-[85vh] flex flex-col border border-border-default"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between p-5 pb-3">
          <h2 className="text-base font-semibold text-text-primary flex items-center gap-2">
            <AudioLines size={16} className="text-accent-bg" />
            Manage text
          </h2>
          <button
            onClick={onClose}
            className="p-1 rounded text-text-tertiary hover:text-text-primary hover:bg-bg-hover cursor-pointer"
            aria-label="Close"
          >
            <X size={18} />
          </button>
        </div>

        <div className="flex-1 min-h-0 overflow-y-auto px-5 pb-5 flex flex-col gap-5">
          {error && (
            <div className="p-2 rounded-md bg-error-bg text-error-text text-xs break-words">{error}</div>
          )}

          {/* ── Text ─────────────────────────────────────────────── */}
          <section className="flex flex-col gap-2">
            <h3 className={sectionTitle}>Text</h3>
            <div className="flex items-start justify-between gap-3">
              <div className="min-w-0">
                <p className="text-sm font-medium text-text-primary truncate">
                  {text.title || "Untitled"}
                </p>
                <p className="text-[11px] text-text-tertiary">
                  {wordCount(effectiveReference)} words
                  {text.normalized_content?.trim() ? " · scoring uses the normalized text" : ""}
                </p>
              </div>
              <button
                onClick={() => {
                  onClose();
                  onEditText();
                }}
                className={smallButton}
                title="Edit the text, its note and its normalized reference"
              >
                <Pencil size={12} />
                Edit text
              </button>
            </div>
            <p className="text-xs text-text-secondary whitespace-pre-wrap break-words line-clamp-4 bg-bg-muted rounded-md p-2">
              {effectiveReference.slice(0, 400)}
              {effectiveReference.length > 400 ? "…" : ""}
            </p>
            {text.normalized_content?.trim() && (
              <p className="text-[11px] text-text-tertiary flex items-center gap-1">
                <Wand2 size={11} className="text-accent-bg" />
                A normalized reference is set, so a generated clip reads that instead of the text
                above.
              </p>
            )}
          </section>

          {/* ── Standard audio ───────────────────────────────────── */}
          <section className="flex flex-col gap-2">
            <div className="flex items-center justify-between gap-2">
              <h3 className={sectionTitle}>Standard audio</h3>
              <span className="text-[11px] text-text-tertiary">
                Pronunciation is measured against the default clip only.
              </span>
            </div>

            {loading ? (
              <p className="text-xs text-text-tertiary flex items-center gap-2">
                <Loader2 size={12} className="animate-spin" /> Loading clips…
              </p>
            ) : clips.length === 0 ? (
              <p className="text-xs text-text-secondary bg-bg-muted rounded-md p-2">
                No standard audio yet — pronunciation cannot be scored until this text has one.
              </p>
            ) : (
              <ul className="flex flex-col gap-1">
                {clips.map((clip) => (
                  <li
                    key={clip.uuid}
                    className="flex items-center gap-2 px-2 py-1.5 rounded-md bg-bg-muted"
                  >
                    <div className="flex flex-col min-w-0 flex-1">
                      <span className="text-xs text-text-primary truncate flex items-center gap-1">
                        {clip.note || (clip.kind === "tts" ? "Edge TTS" : "Upload")}
                        {clip.is_default && (
                          <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-accent-bg/20 text-accent-bg">
                            Default
                          </span>
                        )}
                        {clip.stale && (
                          <span
                            className="text-[10px] px-1.5 py-0.5 rounded-full bg-error-bg text-error-text"
                            title="Made from an earlier version of the text"
                          >
                            Stale
                          </span>
                        )}
                      </span>
                      <span className="text-[10px] text-text-tertiary">
                        {clip.kind === "tts" ? "Edge TTS" : "Upload"} · {seconds(clip.duration)}
                        {!clip.audio_url ? " · not on this device" : ""}
                      </span>
                    </div>
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
                      className={smallButton}
                      disabled={clip.is_default || busy === clip.uuid}
                      onClick={() => void makeDefault(clip)}
                      title="Measure pronunciation against this clip"
                    >
                      <Check size={12} />
                      Default
                    </button>
                    <button
                      className={`${smallButton} text-error-text`}
                      disabled={busy === clip.uuid}
                      onClick={() => void remove(clip)}
                      title="Delete this clip and its audio file"
                    >
                      <Trash2 size={12} />
                    </button>
                  </li>
                ))}
              </ul>
            )}

            {/* Generate */}
            <div className="flex flex-wrap items-center gap-2">
              <select
                className={`${field} max-w-[240px]`}
                value={voice}
                onChange={(e) => setVoice(e.target.value)}
                title="Edge TTS voice"
              >
                <option value="">Configured voice</option>
                {voices.map((v) => (
                  <option key={v.short_name} value={v.short_name}>
                    {v.locale} · {v.short_name.replace(/Neural$/, "")} ({v.gender})
                  </option>
                ))}
              </select>
              <button
                className={smallButton}
                disabled={busy === "generate"}
                onClick={() => void generate()}
                title="Synthesise this text with Edge TTS and store it as a clip"
              >
                {busy === "generate" ? (
                  <Loader2 size={12} className="animate-spin" />
                ) : (
                  <Wand2 size={12} />
                )}
                Generate with Edge TTS
              </button>
              <button className={smallButton} disabled={busy === "probe"} onClick={() => void pickSource()}>
                {busy === "probe" ? (
                  <Loader2 size={12} className="animate-spin" />
                ) : (
                  <Upload size={12} />
                )}
                Upload audio
              </button>
              {voicesFailed && (
                <span className="text-[11px] text-text-tertiary">
                  Voice list unavailable offline — the configured voice is used.
                </span>
              )}
            </div>

            {/* Upload + crop */}
            {source && probe && (
              <div className="flex flex-col gap-2 mt-1 p-2 rounded-md border border-border-default">
                <p className="text-[11px] text-text-tertiary">
                  {source.fileName} — drag the handles to the part that reads the text.
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
                    }}
                  >
                    Cancel
                  </button>
                  <button
                    className={smallButton}
                    disabled={busy === "import"}
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

            <p className="text-[11px] text-text-tertiary">
              Generated clips carry word timings, so per-word pronunciation is exact. An uploaded
              clip has none, so its words are attributed by time — the overall score is unaffected.
            </p>
          </section>
        </div>

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
    </div>
  );
}
