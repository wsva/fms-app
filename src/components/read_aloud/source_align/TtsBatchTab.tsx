"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import {
  Ban,
  Check,
  Headphones,
  Loader2,
  Play,
  Square,
  Volume2,
  Wand2,
} from "lucide-react";
import type { ReadStandardAudio, ReadText, VoiceResolution } from "@/lib/read_aloud/types";
import {
  generateAllStandardAudio,
  generateStandardAudio,
  listStandardAudio,
  resolveVoice,
} from "@/lib/read_aloud/standardAudio";
import { localeLabel, voiceName } from "@/lib/edge_tts/voices";
import { isTauri } from "@/lib/tauri";
import { acquireAudioSrc, releaseAudioSrc } from "@/lib/media/audioSrc";
import { logError } from "@/lib/logger";

const LOG_MODULE = "read-aloud";

/** How many texts one batch call speaks. Bounded so the network round trips
 *  land often enough to show progress and so a partial run is resumable. */
const BATCH_LIMIT = 10;

/** The two source labels that decide whether the batch skips a text: a usable
 *  clip (not stale, and present or cuttable) counts as done. Mirrors the badge
 *  the texts list uses. */
function clipState(clips: ReadStandardAudio[] | undefined) {
  const list = clips ?? [];
  const usable = list.find((c) => c.usable) ?? null;
  const stale = !usable && list.some((c) => c.stale);
  return { usable, stale, any: list.length > 0 };
}

type Props = {
  datasetUuid: string;
  /** The workspace's single-operation lock, so a run never overlaps a transcribe. */
  busy: string | null;
  /** Ask the workspace to run `task` under that lock. */
  pipeline: (label: string, task: () => Promise<void>) => void;
  /** Tell the owning page its clip badges are stale — call after any write. */
  onApplied: () => void;
};

/**
 * The TTS tab of the standard-audio page: synthesize a clip for every text at
 * once, instead of opening each one and pressing Generate on its manage card.
 *
 * It is the batch twin of `StandardAudioSection` — same voice resolution, same
 * one text per call under the hood — but the unit of work is the whole dataset:
 * a row per text with its status and a Generate verb, and one button that walks
 * the missing rows to the end. The `voice` here is the batch's, so a run speaks
 * every text alike; leaving it unset lets each text take the voice the dataset's
 * language resolves to, exactly as a single generate would.
 */
export default function TtsBatchTab({ datasetUuid, busy, pipeline, onApplied }: Props) {
  const [texts, setTexts] = useState<ReadText[]>([]);
  const [clipsByText, setClipsByText] = useState<Map<string, ReadStandardAudio[]>>(
    () => new Map()
  );
  const [loading, setLoading] = useState(true);
  const [filter, setFilter] = useState<"all" | "missing" | "done">("all");
  const [search, setSearch] = useState("");
  const [rowBusy, setRowBusy] = useState<string | null>(null);

  // Voice: the resolved provenance, plus an optional override the whole batch
  // speaks with (the manage page's two pickers, kept for parity).
  const [resolved, setResolved] = useState<VoiceResolution | null>(null);
  const [voices, setVoices] = useState<{ short_name: string; locale: string; gender: string }[]>(
    []
  );
  const [override, setOverride] = useState(false);
  const [lang, setLang] = useState("");
  const [voice, setVoice] = useState("");

  // Batch progress. `stopRef` ends the loop from the Stop button or unmount.
  const [batch, setBatch] = useState<{ done: number; total: number } | null>(null);
  const stopRef = useRef(false);
  const aliveRef = useRef(true);

  // Playback of a stored clip — one element for the whole tab.
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const srcRef = useRef<string>("");
  const playTokenRef = useRef(0);
  const [playingUuid, setPlayingUuid] = useState<string | null>(null);

  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
      stopRef.current = true;
    };
  }, []);

  const load = useCallback(async () => {
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    try {
      const [textList, clips] = await Promise.all([
        invoke<ReadText[]>("read_aloud_list_texts", { datasetUuid }),
        listStandardAudio(datasetUuid),
      ]);
      if (!aliveRef.current) return;
      setTexts(textList);
      const map = new Map<string, ReadStandardAudio[]>();
      for (const clip of clips) {
        const list = map.get(clip.text_uuid);
        if (list) list.push(clip);
        else map.set(clip.text_uuid, [clip]);
      }
      setClipsByText(map);
    } catch (e) {
      logError(`TTS tab load failed: ${String(e)}`, LOG_MODULE);
    } finally {
      if (aliveRef.current) setLoading(false);
    }
  }, [datasetUuid]);

  const resolve = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const res = await resolveVoice(datasetUuid);
      if (!aliveRef.current) return;
      setResolved(res);
      setVoice(res.voice);
      setLang(res.locale);
    } catch (e) {
      logError(`TTS tab resolve voice failed: ${String(e)}`, LOG_MODULE);
    }
  }, [datasetUuid]);

  useEffect(() => {
    setLoading(true);
    void load();
    void resolve();
  }, [load, resolve]);

  useEffect(() => {
    if (!isTauri()) return;
    void (async () => {
      try {
        const list = await invoke<{ short_name: string; locale: string; gender: string }[]>(
          "edge_tts_list_voices",
        );
        const sorted = [...list].sort((a, b) =>
          a.locale === b.locale
            ? a.short_name.localeCompare(b.short_name)
            : a.locale.localeCompare(b.locale),
        );
        if (aliveRef.current) setVoices(sorted);
      } catch (e) {
        logError(`TTS tab load voices failed: ${String(e)}`, LOG_MODULE);
      }
    })();
  }, []);

  // A new dataset is a new set of texts and clips.
  useEffect(() => {
    stopRef.current = true;
    setBatch(null);
    setRowBusy(null);
    setOverride(false);
  }, [datasetUuid]);

  const locales = useMemo(() => {
    const seen: string[] = [];
    for (const v of voices) if (!seen.includes(v.locale)) seen.push(v.locale);
    return seen;
  }, [voices]);

  const langVoices = lang ? voices.filter((v) => v.locale === lang) : [];

  const missing = useMemo(
    () => texts.filter((t) => !clipState(clipsByText.get(t.uuid)).usable).length,
    [texts, clipsByText],
  );
  const total = texts.length;

  const rows = useMemo(() => {
    const needle = search.trim().toLowerCase();
    return texts.filter((t) => {
      const { usable } = clipState(clipsByText.get(t.uuid));
      // A stale clip is not usable, so it falls under "missing": it needs a
      // regeneration, not just a listen.
      if (filter === "missing" && usable) return false;
      if (filter === "done" && !usable) return false;
      return needle === "" || `${t.title}\n${t.content}`.toLowerCase().includes(needle);
    });
  }, [texts, clipsByText, filter, search]);

  const stopAudio = useCallback(() => {
    playTokenRef.current += 1;
    const el = audioRef.current;
    if (el) {
      el.pause();
      el.currentTime = 0;
      audioRef.current = null;
    }
    releaseAudioSrc(srcRef.current);
    srcRef.current = "";
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
      const token = ++playTokenRef.current;
      setPlayingUuid(clip.uuid);
      void (async () => {
        const src = await acquireAudioSrc(clip.audio_url);
        if (token !== playTokenRef.current) {
          releaseAudioSrc(src);
          return;
        }
        const el = new Audio(src);
        audioRef.current = el;
        srcRef.current = src;
        el.onended = () => stopAudio();
        el.play().catch(() => stopAudio());
      })();
    },
    [stopAudio],
  );

  /** The voice a batch/row generation sends: the override when a picker was
   *  touched, otherwise nothing so the backend resolves per the dataset. */
  const voiceArg = override ? voice.trim() || undefined : undefined;

  const generateOne = (textUuid: string) => {
    if (busy != null) return;
    setRowBusy(textUuid);
    pipeline("tts", async () => {
      try {
        await generateStandardAudio({ datasetUuid, textUuid, voice: voiceArg });
        await load();
        onApplied();
      } finally {
        if (aliveRef.current) setRowBusy(null);
      }
    });
  };

  const runBatch = () => {
    if (busy != null || missing === 0) return;
    stopRef.current = false;
    setBatch({ done: 0, total: missing });
    pipeline("tts", async () => {
      let done = 0;
      try {
        for (;;) {
          if (stopRef.current || !aliveRef.current) break;
          const out = await generateAllStandardAudio({
            datasetUuid,
            voice: voiceArg,
            onlyMissing: true,
            limit: BATCH_LIMIT,
          });
          done += out.generated ?? 0;
          if (!aliveRef.current) break;
          setBatch({ done: out.texts_with_audio ?? done, total: out.texts_total ?? total });
          await load();
          if ((out.remaining ?? 0) <= 0 || (out.generated ?? 0) === 0) break;
        }
      } finally {
        onApplied();
        if (aliveRef.current) {
          void resolve();
          // Leave a beat so a finished run reads as done before the row clears.
          window.setTimeout(() => {
            if (aliveRef.current) setBatch(null);
          }, 600);
        }
      }
    });
  };

  const stopBatch = () => {
    stopRef.current = true;
  };

  const running = batch != null;

  // ── styling, matched to the page's other cards ────────────────────────
  const CARD = "rounded-xl border border-border-default bg-bg-card overflow-hidden flex flex-col min-h-0";
  const ROW_BUTTON =
    "shrink-0 inline-flex items-center gap-1 px-2 py-1 rounded-md border border-border-default text-[11px] text-text-secondary hover:bg-bg-hover hover:text-text-primary cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed disabled:hover:bg-transparent";
  const PRIMARY =
    "inline-flex items-center justify-center gap-1.5 px-2.5 py-1.5 rounded-md text-[11px] font-medium bg-accent-bg text-white hover:bg-accent-bg/90 cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed";
  const NEUTRAL =
    "inline-flex items-center justify-center gap-1.5 px-2.5 py-1.5 rounded-md text-[11px] bg-bg-muted text-text-secondary border border-border-default hover:bg-bg-hover hover:text-text-primary cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed";
  const SELECT =
    "px-2 rounded-md bg-bg-muted border border-border-default text-text-primary text-xs focus:outline-none focus:border-accent-bg cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed h-8";

  const voiceLabel = voice ? voiceName(voice) : "the resolved voice";

  return (
    <div className="grid gap-3 lg:h-full lg:min-h-0 lg:grid-cols-[minmax(280px,340px)_1fr]">
      {/* Left rail: what will speak, and the one button that does the batch. */}
      <aside className={CARD}>
        <div className="shrink-0 px-4 py-3 border-b border-border-default flex flex-col gap-2.5">
          <div className="flex items-baseline gap-2">
            <h2 className="text-xs font-semibold text-text-primary flex items-center gap-1.5">
              <Volume2 size={13} /> Batch TTS
            </h2>
            <span className="ml-auto text-[10px] text-text-tertiary tabular-nums">
              {total === 0 ? "no texts" : `${total - missing}/${total} voiced`}
            </span>
          </div>

          {resolved && (
            <p className="text-[11px] text-text-secondary flex flex-wrap items-baseline gap-x-1.5">
              <span className="text-text-primary">
                {localeLabel(resolved.dataset_language || resolved.locale)}
              </span>
              <span className="text-text-tertiary">·</span>
              <span className="text-text-primary">{voiceName(resolved.voice)}</span>
              <button
                className="text-accent-bg hover:underline cursor-pointer"
                onClick={() => {
                  if (isTauri()) void emit("agent-action", { type: "navigate", tab: "edge-tts" });
                }}
                title="Set the voice this language uses on the Edge TTS page"
              >
                Change
              </button>
            </p>
          )}
          {resolved?.hint && <p className="text-[10px] text-text-tertiary">{resolved.hint}</p>}
          {resolved?.warning && (
            <p className="text-[10px] text-warning-text">{resolved.warning}</p>
          )}

          {/* The batch's own voice override — the same two pickers the manage
              card offers, so a whole dataset can be re-voiced in one pass. */}
          <div className="flex flex-wrap items-center gap-1.5">
            <select
              className={`${SELECT} max-w-[150px]`}
              value={lang}
              onChange={(e) => {
                setOverride(true);
                setLang(e.target.value);
                setVoice(voices.find((v) => v.locale === e.target.value)?.short_name ?? "");
              }}
              disabled={running || busy != null || voices.length === 0}
              title="Language the whole batch speaks with"
            >
              <option value="">The dataset's voice</option>
              {locales.map((l) => (
                <option key={l} value={l}>
                  {localeLabel(l)}
                </option>
              ))}
            </select>
            <select
              className={`${SELECT} max-w-[160px]`}
              value={voice}
              onChange={(e) => {
                setOverride(true);
                setVoice(e.target.value);
              }}
              disabled={running || busy != null || !lang}
              title={lang ? "Voice of the chosen language" : "Pick a language first, or leave it resolved"}
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
            {override && (
              <button
                type="button"
                className="text-[10px] text-text-tertiary hover:text-text-secondary cursor-pointer"
                disabled={running || busy != null}
                onClick={() => {
                  setOverride(false);
                  void resolve();
                }}
                title="Let each text take the voice the dataset's language resolves to"
              >
                reset
              </button>
            )}
          </div>

          {running ? (
            <div className="flex flex-col gap-1.5">
              <div className="h-1.5 rounded-full bg-bg-muted overflow-hidden">
                <div
                  className="h-full bg-accent-bg transition-[width] duration-300"
                  style={{ width: batch ? `${Math.min(100, (batch.done / Math.max(1, batch.total)) * 100)}%` : "0%" }}
                />
              </div>
              <div className="flex items-center justify-between">
                <span className="text-[11px] text-text-secondary flex items-center gap-1.5">
                  <Loader2 size={11} className="animate-spin" />
                  Speaking with {voiceLabel} · {batch?.done ?? 0}/{batch?.total ?? missing}
                </span>
                <button type="button" className={`${ROW_BUTTON} text-error-text`} onClick={stopBatch}>
                  <Ban size={11} /> Stop
                </button>
              </div>
            </div>
          ) : (
            <button type="button" className={PRIMARY} disabled={busy != null || missing === 0} onClick={runBatch}>
              <Wand2 size={12} />
              Generate all missing
              <span className="tabular-nums opacity-70">{missing}</span>
            </button>
          )}
          {missing === 0 && total > 0 && !running && (
            <p className="text-[11px] text-text-tertiary flex items-center gap-1.5">
              <Check size={11} className="text-accent-bg" /> Every text has a usable clip.
            </p>
          )}
        </div>

        <div className="px-4 py-3 flex flex-col gap-2 text-[11px] text-text-tertiary">
          <p>
            Per-text playback uses the generated clip on this device. A clip made
            for an older version of a text is marked stale — regenerating it here
            replaces it.
          </p>
          <p>
            Need to check one before doing the rest? Generate the first texts,
            listen, then run the batch — texts that already have a clip are
            skipped.
          </p>
        </div>
      </aside>

      {/* Right: one row per text, with its status, a play verb and its own
          Generate, so a single correction never means opening the manage card. */}
      <section className={`${CARD} max-h-[60vh] lg:max-h-none`}>
        <div className="shrink-0 px-3 py-2 border-b border-border-default flex flex-col gap-1.5">
          <div className="flex items-center gap-2 px-1">
            <h2 className="text-xs font-semibold text-text-primary">Texts</h2>
            <span className="ml-auto text-[10px] text-text-tertiary tabular-nums">
              {rows.length === total ? total : `${rows.length} of ${total}`}
            </span>
          </div>
          <div className="flex flex-wrap gap-1 px-1">
            {(["all", "missing", "done"] as const).map((id) => {
              const active = id === filter;
              const count = id === "all" ? total : id === "missing" ? missing : total - missing;
              return (
                <button
                  key={id}
                  type="button"
                  onClick={() => setFilter(id)}
                  aria-pressed={active}
                  className={`inline-flex items-center gap-1 px-1.5 py-0.5 rounded-md text-[10px] capitalize cursor-pointer ${
                    active ? "bg-accent-bg/15 text-accent-bg" : "bg-bg-muted text-text-secondary hover:bg-bg-hover"
                  }`}
                >
                  {id}
                  <span className="tabular-nums opacity-70">{count}</span>
                </button>
              );
            })}
          </div>
          <input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Filter by title or text"
            aria-label="Filter texts by title or text"
            className="px-2 py-1 rounded-md bg-bg-muted border border-border-default text-[11px] text-text-primary focus:outline-none focus:border-accent-bg placeholder:text-text-tertiary"
          />
        </div>

        <div className="flex-1 min-h-0 overflow-y-auto py-1">
          {loading ? (
            <p className="px-3 py-4 text-[11px] text-text-tertiary flex items-center gap-2">
              <Loader2 size={12} className="animate-spin" /> Loading texts…
            </p>
          ) : rows.length === 0 ? (
            <p className="px-3 py-4 text-[11px] text-text-tertiary">
              {total === 0 ? "This dataset has no texts yet." : "Nothing under this filter."}
            </p>
          ) : (
            rows.map((t, index) => {
              const { usable, stale } = clipState(clipsByText.get(t.uuid));
              const isGenerating = rowBusy === t.uuid && busy === "tts";
              return (
                <div
                  key={t.uuid}
                  className="w-full min-w-0 flex items-center gap-2 px-2.5 py-1.5 border-l-2 border-transparent hover:bg-bg-hover"
                >
                  <span className="w-6 shrink-0 text-right text-[10px] text-text-tertiary tabular-nums">
                    {index + 1}
                  </span>
                  <span className="min-w-0 flex-1 flex flex-col">
                    <span className="min-w-0 truncate text-xs text-text-primary">
                      {t.title || "Untitled"}
                    </span>
                    <span className="min-w-0 truncate text-[10px] text-text-tertiary">
                      {t.content.replace(/\s+/g, " ").slice(0, 120)}
                    </span>
                  </span>

                  <span className="shrink-0 text-[10px] tabular-nums w-14 text-right text-text-tertiary">
                    {usable?.duration != null ? `${Math.round(usable.duration)}s` : stale ? "stale" : "—"}
                  </span>

                  {usable?.audio_url ? (
                    <button
                      type="button"
                      className={ROW_BUTTON}
                      onClick={() => togglePlay(usable)}
                      title={playingUuid === usable.uuid ? "Stop" : "Play this clip"}
                    >
                      {playingUuid === usable.uuid ? <Square size={11} /> : <Play size={11} />}
                    </button>
                  ) : usable ? (
                    <span className={ROW_BUTTON} title="Clip is on another device — sync to fetch it">
                      <Headphones size={11} />
                    </span>
                  ) : null}

                  <button
                    type="button"
                    className={ROW_BUTTON}
                    disabled={busy != null || isGenerating}
                    onClick={() => generateOne(t.uuid)}
                    title={
                      stale
                        ? "Regenerate this stale clip from the current text"
                        : usable
                          ? "Generate another clip for this text"
                          : "Generate this text's standard audio with Edge TTS"
                    }
                  >
                    {isGenerating ? <Loader2 size={11} className="animate-spin" /> : <Wand2 size={11} />}
                    {usable && !stale ? "Again" : "Generate"}
                  </button>
                </div>
              );
            })
          )}
        </div>

        <div className="shrink-0 px-3 py-1.5 border-t border-border-default flex items-center justify-between">
          <span className="text-[10px] text-text-tertiary">{total} text{total === 1 ? "" : "s"}</span>
          <button type="button" className={NEUTRAL} disabled={busy != null || missing === 0} onClick={runBatch}>
            <Wand2 size={12} />
            Generate all missing
            <span className="tabular-nums opacity-70">{missing}</span>
          </button>
        </div>
      </section>
    </div>
  );
}
