"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import {
  Check,
  ChevronDown,
  ChevronRight,
  FolderOpen,
  Loader2,
  Play,
  ScanText,
  Square,
  Upload,
  Wand2,
  X,
} from "lucide-react";
import { isMobileApp } from "@/lib/platform";
import {
  applyAlignProposals,
  decideAlignProposals,
  guessAlignSpans,
  listAlignProposals,
  registerAlignSources,
  transcribeAlignSource,
  type AlignCounts,
  type AlignPreview,
  type AlignProposal,
  type AlignProposalState,
  type AlignProposals,
  type AlignSourceIndex,
} from "@/lib/read_aloud/sourceAlign";

/**
 * Bulk span guessing: one cached transcript per recording, one monotonic pass
 * over the passages, a table the user reviews, and a batch cut.
 *
 * The per-text "Guess from speech" decodes and transcribes the whole file for
 * *every* passage, which is fine for one text and hopeless for a book. Here the
 * folders are picked once, each file is transcribed once, and every passage is
 * placed along them in reading order — so the review rows are the panel's real
 * work, and this file is mostly that table.
 */

/** One press of an edge's arrows, and the step the fields commit at. */
const NUDGE = 0.01;

/** Shortest span the backend accepts; also the gap the two fields keep. */
const MIN_SPAN = 0.3;

/** Spans at or above this confidence are accepted without listening to them. */
const DEFAULT_THRESHOLD = 0.9;

/** How many spans one `apply` call cuts; the loop repeats until done. */
const APPLY_BATCH = 10;

function roundMs(seconds: number): number {
  return Math.round(seconds * 1000) / 1000;
}

/** `1:02:03.456` — a span is read in the same units the recording is. */
function clock(seconds: number): string {
  const s = Math.max(0, seconds);
  const hours = Math.floor(s / 3600);
  const minutes = Math.floor((s % 3600) / 60);
  const rest = (s % 60).toFixed(3).padStart(6, "0");
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${rest}`
    : `${minutes}:${rest}`;
}

function errorText(error: unknown): string {
  return typeof error === "string" ? error : error instanceof Error ? error.message : String(error);
}

/**
 * The peak envelope around one span, with the span drawn at full strength.
 *
 * Only the window the backend sent is drawn — a thumbnail, not the whole
 * recording — so the pair count is bounded and one canvas per row stays cheap.
 * The edges are the padding a person is judging, which is why they are drawn
 * rather than implied.
 */
function PeakStrip({ preview, start, end }: { preview: AlignPreview; start: number; end: number }) {
  const boxRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [width, setWidth] = useState(0);
  const pairs = Math.floor(preview.peaks.length / 2);
  const windowSeconds = pairs * preview.peak_seconds;

  useEffect(() => {
    const el = boxRef.current;
    if (!el) return;
    const update = () => setWidth(el.clientWidth);
    update();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || width <= 0 || pairs === 0 || !(windowSeconds > 0)) return;
    const dpr = typeof window === "undefined" ? 1 : window.devicePixelRatio || 1;
    const height = 28;
    canvas.width = Math.round(width * dpr);
    canvas.height = Math.round(height * dpr);
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, width, height);

    const css = getComputedStyle(canvas);
    const accent = css.getPropertyValue("--color-accent-bg").trim() || "#6366f1";
    const muted = css.getPropertyValue("--color-text-tertiary").trim() || "#94a3b8";
    const mid = height / 2;
    const fromX = Math.max(0, ((start - preview.start) / windowSeconds) * width);
    const toX = Math.min(width, ((end - preview.start) / windowSeconds) * width);

    for (let x = 0; x < width; x++) {
      const index = Math.min(pairs - 1, Math.floor((x / width) * pairs));
      const lo = preview.peaks[index * 2];
      const hi = preview.peaks[index * 2 + 1];
      const top = mid - (hi / 128) * mid;
      const bottom = mid - (lo / 128) * mid;
      const inside = x >= fromX && x <= toX;
      ctx.fillStyle = inside ? accent : muted;
      ctx.globalAlpha = inside ? 0.9 : 0.35;
      ctx.fillRect(x, top, 1, Math.max(1, bottom - top));
    }
    ctx.globalAlpha = 1;
    ctx.fillStyle = accent;
    ctx.fillRect(Math.round(fromX), 0, 2, height);
    ctx.fillRect(Math.max(0, Math.round(toX) - 2), 0, 2, height);
  }, [preview, start, end, width, pairs, windowSeconds]);

  return (
    <div ref={boxRef} className="rounded-md bg-bg-muted overflow-hidden" title="Audio around this span">
      <canvas ref={canvasRef} className="block w-full" style={{ height: 28 }} aria-hidden />
    </div>
  );
}

/**
 * One edge in seconds, at the millisecond the backend stores.
 *
 * A draft keeps typing from being fought by the formatted value, and the step
 * buttons beside it are the reason this is not a slider: 10 ms is finer than a
 * pixel, and a span edge that is 10 ms late clips a consonant.
 */
function SpanField({
  label,
  value,
  disabled,
  onCommit,
}: {
  label: string;
  value: number;
  disabled: boolean;
  onCommit: (seconds: number) => void;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const commit = () => {
    if (draft == null) return;
    const parsed = Number(draft.replace(",", "."));
    setDraft(null);
    if (Number.isFinite(parsed) && Math.abs(parsed - value) > 1e-9) onCommit(parsed);
  };
  return (
    <input
      className="w-[74px] px-1 py-1 rounded-md bg-bg-muted text-center text-[11px] tabular-nums border border-border-default text-text-primary focus:outline-none focus:border-accent-bg disabled:opacity-50"
      value={draft ?? value.toFixed(3)}
      disabled={disabled}
      aria-label={label}
      title={`${label}: seconds, three decimals (${clock(value)})`}
      inputMode="decimal"
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          e.currentTarget.blur();
        } else if (e.key === "Escape") {
          setDraft(null);
        }
      }}
    />
  );
}

function nudgeButtonClasses(disabled: boolean): string {
  return `inline-flex items-center justify-center px-1.5 py-1 bg-bg-muted text-text-secondary text-[10px] tabular-nums ${
    disabled ? "opacity-50 cursor-not-allowed" : "hover:bg-bg-hover cursor-pointer"
  }`;
}

function confidenceClasses(confidence: number): string {
  if (confidence >= 0.9) return "text-accent-bg";
  if (confidence >= 0.7) return "text-text-secondary";
  return "text-warning-text";
}

/**
 * One passage: what it says, where the guess put it, and the verdict.
 *
 * The numbers shown are the panel's (`span`), which is the stored span plus any
 * nudge not yet written back — so a click is visible immediately without
 * waiting for the file to be rewritten.
 */
function PassageRow({
  proposal,
  span,
  playing,
  busy,
  onMove,
  onDecide,
  onPlay,
}: {
  proposal: AlignProposal;
  span: { start: number; end: number };
  playing: boolean;
  busy: boolean;
  onMove: (start: number, end: number) => void;
  onDecide: (state: AlignProposalState) => void;
  onPlay: () => void;
}) {
  const refused = proposal.status === "refused";
  const applied = Boolean(proposal.applied_uuid);
  const locked = busy || refused || applied;
  const setStart = (seconds: number) => {
    const start = Math.min(Math.max(0, seconds), span.end - MIN_SPAN);
    onMove(roundMs(start), span.end);
  };
  const setEnd = (seconds: number) => {
    const end = Math.max(seconds, span.start + MIN_SPAN);
    onMove(span.start, roundMs(end));
  };

  const decideButton = (state: AlignProposalState, active: boolean, icon: React.ReactNode, label: string) => (
    <button
      type="button"
      onClick={() => onDecide(state)}
      disabled={locked || active}
      className={`inline-flex items-center gap-1 px-2 py-1 rounded-md text-[11px] ${
        active
          ? "bg-accent-bg/15 text-accent-bg"
          : locked
            ? "bg-bg-muted text-text-tertiary opacity-50 cursor-not-allowed"
            : "bg-bg-muted text-text-secondary hover:bg-bg-hover cursor-pointer"
      }`}
      title={label}
    >
      {icon}
      {label}
    </button>
  );

  return (
    <div className="flex flex-col gap-1.5 p-2 rounded-md border border-border-default">
      <div className="flex items-center gap-2 min-w-0">
        <span className="text-sm text-text-primary truncate">{proposal.title || "Untitled"}</span>
        {applied && (
          <span
            className="inline-flex items-center gap-1 text-[10px] text-accent-bg shrink-0"
            title="This span was cut and stored as the text's standard audio"
          >
            <Check size={11} /> stored
          </span>
        )}
        <span className="ml-auto flex items-center gap-2 shrink-0 text-[10px] tabular-nums">
          {!refused && (
            <span className={confidenceClasses(proposal.confidence)} title="How much of the text was heard, in order">
              {Math.round(proposal.confidence * 100)}%
            </span>
          )}
          <span className="text-text-tertiary" title="Words matched of the passage's words">
            {proposal.matched_words}/{proposal.reference_words} w
          </span>
          {proposal.state !== "pending" && (
            <span className={proposal.state === "accepted" ? "text-accent-bg" : "text-text-tertiary"}>
              {proposal.state}
            </span>
          )}
        </span>
      </div>

      <p className="text-[11px] text-text-secondary line-clamp-2 break-words">{proposal.content}</p>

      {refused ? (
        <p className="text-[11px] text-warning-text">{proposal.note || "The guess refused this passage."}</p>
      ) : (
        <>
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="text-[11px] text-text-tertiary truncate max-w-[220px]" title={proposal.source_path}>
              {proposal.source_name}
            </span>
            <div className="inline-flex items-stretch rounded-md border border-border-default overflow-hidden">
              <button
                type="button"
                className={nudgeButtonClasses(locked)}
                disabled={locked}
                onClick={() => setStart(span.start - NUDGE)}
                title="Start 10 ms earlier"
                aria-label="Start 10 ms earlier"
              >
                −10
              </button>
              <span className="px-1 self-center text-[10px] text-text-tertiary">start</span>
              <SpanField label="Span start" value={span.start} disabled={locked} onCommit={setStart} />
              <button
                type="button"
                className={nudgeButtonClasses(locked)}
                disabled={locked}
                onClick={() => setStart(span.start + NUDGE)}
                title="Start 10 ms later"
                aria-label="Start 10 ms later"
              >
                +10
              </button>
            </div>
            <div className="inline-flex items-stretch rounded-md border border-border-default overflow-hidden">
              <button
                type="button"
                className={nudgeButtonClasses(locked)}
                disabled={locked}
                onClick={() => setEnd(span.end - NUDGE)}
                title="End 10 ms earlier"
                aria-label="End 10 ms earlier"
              >
                −10
              </button>
              <span className="px-1 self-center text-[10px] text-text-tertiary">end</span>
              <SpanField label="Span end" value={span.end} disabled={locked} onCommit={setEnd} />
              <button
                type="button"
                className={nudgeButtonClasses(locked)}
                disabled={locked}
                onClick={() => setEnd(span.end + NUDGE)}
                title="End 10 ms later"
                aria-label="End 10 ms later"
              >
                +10
              </button>
            </div>
            <span className="text-[10px] text-text-tertiary tabular-nums">({(span.end - span.start).toFixed(3)} s)</span>
            <button
              type="button"
              onClick={onPlay}
              disabled={!proposal.source_path}
              className="inline-flex items-center gap-1 px-2 py-1 rounded-md bg-bg-muted hover:bg-bg-hover text-text-secondary text-[11px] cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
              title={proposal.source_path ? "Play this span" : "The recording's path is unknown on this device"}
            >
              {playing ? <Square size={11} /> : <Play size={11} />}
              {playing ? "Stop" : "Play"}
            </button>
            {decideButton("accepted", proposal.state === "accepted", <Check size={11} />, "Accept")}
            {decideButton("rejected", proposal.state === "rejected", <X size={11} />, "Reject")}
          </div>

          {proposal.preview && <PeakStrip preview={proposal.preview} start={span.start} end={span.end} />}

          {proposal.note && <p className="text-[11px] text-text-tertiary">{proposal.note}</p>}
        </>
      )}
    </div>
  );
}

type Props = {
  datasetUuid: string;
  /** Called after spans were stored, so the page reloads its clip badges. */
  onApplied: () => void;
};

export default function SourceAlignPanel({ datasetUuid, onApplied }: Props) {
  const [expanded, setExpanded] = useState(false);
  const [index, setIndex] = useState<AlignSourceIndex | null>(null);
  const [proposals, setProposals] = useState<AlignProposals | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<{ kind: "ok" | "warn"; text: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [threshold, setThreshold] = useState(DEFAULT_THRESHOLD);
  /** The threshold while it is being typed, so a half-typed "9" is not 9%. */
  const [thresholdDraft, setThresholdDraft] = useState<string | null>(null);
  /** The batch cut's own tally, kept apart from the busy label. */
  const [applied, setApplied] = useState<{ count: number; failures: number } | null>(null);
  const [playing, setPlaying] = useState<string | null>(null);
  /** Nudged spans, by text uuid, until the write is acknowledged. */
  const [edits, setEdits] = useState<Record<string, { start: number; end: number }>>({});

  const audioRef = useRef<HTMLAudioElement | null>(null);
  const stopRef = useRef(false);
  const timersRef = useRef<Record<string, ReturnType<typeof setTimeout>>>({});

  const stopAudio = useCallback(() => {
    audioRef.current?.pause();
    audioRef.current = null;
    setPlaying(null);
  }, []);

  useEffect(() => stopAudio, [stopAudio]);

  // A pending debounce from the previous dataset must not write into this one.
  useEffect(() => {
    const timers = timersRef.current;
    return () => {
      Object.values(timers).forEach(clearTimeout);
      timersRef.current = {};
    };
  }, [datasetUuid]);

  /**
   * Load the index and the proposals.
   *
   * `dropEdits` is for the calls that move spans on the server: a guess
   * replaces every span, so a nudge kept here would show a number the file no
   * longer holds. Accepting or cutting never moves an edge, so the edits stay —
   * clearing them would revert a nudge the user has just made.
   */
  const refresh = useCallback(
    async (dropEdits = false) => {
      if (!datasetUuid) return;
      try {
        const [sources, list] = await Promise.all([
          registerAlignSources(datasetUuid),
          listAlignProposals(datasetUuid),
        ]);
        setIndex(sources);
        setProposals(list);
        if (dropEdits) setEdits({});
        setError(null);
      } catch (e) {
        setError(errorText(e));
      }
    },
    [datasetUuid]
  );

  useEffect(() => {
    if (!expanded) return;
    void refresh(true);
  }, [expanded, refresh]);

  useEffect(() => {
    setIndex(null);
    setProposals(null);
    setNote(null);
    setError(null);
    setApplied(null);
    setEdits({});
    stopAudio();
  }, [datasetUuid, stopAudio]);

  /** One operation at a time: the progress text is also the lock. */
  const run = async (label: string, task: () => Promise<void>) => {
    setBusy(label);
    setError(null);
    try {
      await task();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  const pickFolder = () =>
    run("picking", async () => {
      const picked = await open({
        directory: true,
        multiple: false,
        title: "Folder that holds the recordings",
      });
      if (typeof picked !== "string") return;
      const sources = await registerAlignSources(datasetUuid, picked);
      setIndex(sources);
      setEdits({});
      setNote({
        kind: "ok",
        text: `${sources.source_count} recording${sources.source_count === 1 ? "" : "s"} from ${sources.directory}`,
      });
    });

  /**
   * One file per call, because a chapter is minutes of model time and nothing
   * should hold a request open that long. The loop stops between files, so the
   * Stop button is honoured before the next transcription starts.
   */
  const transcribeAll = () =>
    run("transcribing", async () => {
      stopRef.current = false;
      for (;;) {
        const result = await transcribeAlignSource(datasetUuid);
        setIndex(result);
        const name = result.file_name ?? "";
        setNote({
          kind: "ok",
          text: `Heard ${result.words ?? 0} words in ${name} (${clock(result.duration ?? 0)}) — ${
            result.remaining
          } left.`,
        });
        if (result.done || result.remaining === 0) {
          setNote({ kind: "ok", text: `Every recording is transcribed (${result.source_count}).` });
          break;
        }
        if (stopRef.current) break;
      }
    });

  const guess = () =>
    run("guessing", async () => {
      const result = await guessAlignSpans(datasetUuid);
      await refresh(true);
      setNote({
        kind: result.refused > 0 ? "warn" : "ok",
        text: `${result.proposed} of ${result.passages} passages placed (mean ${Math.round(
          result.mean_confidence * 100
        )}%), ${result.refused} refused.`,
      });
    });

  const decide = (textUuid: string, state: AlignProposalState, span?: { start: number; end: number }) =>
    run("deciding", async () => {
      await decideAlignProposals({ datasetUuid, textUuid, state, ...span });
      await refresh();
    });

  const decideMany = (state: AlignProposalState) =>
    run("deciding", async () => {
      const result = await decideAlignProposals({ datasetUuid, state, minConfidence: threshold });
      await refresh();
      setNote({
        kind: "ok",
        text: `${result.changed} passage${result.changed === 1 ? "" : "s"} ${state} (≥ ${Math.round(
          threshold * 100
        )}%).`,
      });
    });

  /**
   * Cut the accepted spans. The loop repeats because `apply` is bounded, and it
   * stops on no progress as well as on nothing left: a span that fails stays
   * accepted, so repeating it would fail forever.
   */
  const applyAccepted = () =>
    run("cutting", async () => {
      let stored = 0;
      let failures = 0;
      for (;;) {
        const result = await applyAlignProposals(datasetUuid, APPLY_BATCH);
        stored += result.applied;
        failures += result.failed_count;
        setApplied({ count: stored, failures });
        if (result.remaining === 0 || result.applied === 0) break;
      }
      await refresh();
      onApplied();
      setNote({
        kind: failures > 0 ? "warn" : "ok",
        text:
          failures > 0
            ? `${stored} clip${stored === 1 ? "" : "s"} stored, ${failures} failed — see the rows.`
            : `${stored} clip${stored === 1 ? "" : "s"} stored as standard audio.`,
      });
    });

  /**
   * Nudging is a local edit first and a write second: the field must not wait a
   * round trip to show where the edge went, and a burst of clicks must become
   * one rewrite of the proposals file rather than ten.
   */
  const moveSpan = (proposal: AlignProposal, start: number, end: number) => {
    setEdits((current) => ({ ...current, [proposal.text_uuid]: { start, end } }));
    clearTimeout(timersRef.current[proposal.text_uuid]);
    timersRef.current[proposal.text_uuid] = setTimeout(() => {
      void decideAlignProposals({
        datasetUuid,
        textUuid: proposal.text_uuid,
        state: proposal.state,
        start,
        end,
      }).catch((e) => {
        setError(errorText(e));
        setEdits((current) => {
          const next = { ...current };
          delete next[proposal.text_uuid];
          return next;
        });
      });
    }, 300);
  };

  const playSpan = (proposal: AlignProposal, start: number, end: number) => {
    if (playing === proposal.text_uuid) {
      stopAudio();
      return;
    }
    stopAudio();
    if (!proposal.source_path) return;
    const el = new Audio(convertFileSrc(proposal.source_path));
    audioRef.current = el;
    el.currentTime = start;
    setPlaying(proposal.text_uuid);
    // The element plays the whole recording, so the edge is what stops it.
    el.ontimeupdate = () => {
      if (el.currentTime >= end) stopAudio();
    };
    el.onended = () => setPlaying(null);
    el.play().catch(() => stopAudio());
  };

  const counts: AlignCounts | null = proposals?.counts ?? null;
  const sources = index?.sources ?? [];
  const transcribed = index?.transcribed ?? 0;
  const pending = index?.pending_transcription ?? 0;
  const spans = proposals?.proposals ?? [];
  const mobile = isMobileApp();

  return (
    <div className="rounded-lg border border-border-default bg-bg-card">
      <button
        type="button"
        onClick={() => setExpanded((value) => !value)}
        className="w-full flex items-center gap-2 px-3 py-2 text-left cursor-pointer hover:bg-bg-hover rounded-lg"
        title="Transcribe the recordings once, then place every text along them"
      >
        {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <Wand2 size={14} className="text-text-tertiary shrink-0" />
        <span className="text-sm font-medium text-text-primary truncate">Advanced · align texts to recordings</span>
        <span className="ml-auto text-[11px] text-text-tertiary tabular-nums shrink-0">
          {counts
            ? `${counts.proposed} placed · ${counts.open} to cut`
            : index
              ? `${transcribed}/${index.source_count} transcribed`
              : ""}
        </span>
      </button>

      {expanded && (
        <div className="border-t border-border-default p-3 flex flex-col gap-3">
          <p className="text-[11px] text-text-tertiary">
            Each recording is transcribed <span className="text-text-secondary">once</span>, then every text is
            placed along them in reading order — minutes per chapter, not per text. Review the spans, then cut the
            accepted ones into standard audio. The recordings stay where they are; only the clips are written into
            the dataset.
          </p>

          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              onClick={() => void pickFolder()}
              disabled={mobile || busy != null}
              className="inline-flex items-center gap-1 px-2 py-1 rounded-md bg-bg-muted hover:bg-bg-hover text-text-secondary text-[11px] cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
              title={
                mobile
                  ? "Picking a folder is a desktop action; this device can review spans and play them"
                  : "Pick the folder that holds the audiobook's files"
              }
            >
              <FolderOpen size={12} /> Pick recordings folder
            </button>
            <button
              type="button"
              onClick={() => void transcribeAll()}
              disabled={mobile || busy != null || pending === 0}
              className="inline-flex items-center gap-1 px-2 py-1 rounded-md bg-bg-muted hover:bg-bg-hover text-text-secondary text-[11px] cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
              title="Listen to every file without a transcript yet, one file per request"
            >
              {busy === "transcribing" ? <Loader2 size={12} className="animate-spin" /> : <ScanText size={12} />}
              {busy === "transcribing"
                ? `transcribing (${pending} left)`
                : pending > 0
                  ? `Transcribe ${pending} file${pending === 1 ? "" : "s"}`
                  : "All transcribed"}
            </button>
            {busy === "transcribing" && (
              <button
                type="button"
                onClick={() => {
                  stopRef.current = true;
                }}
                className="inline-flex items-center gap-1 px-2 py-1 rounded-md bg-bg-muted hover:bg-bg-hover text-text-secondary text-[11px] cursor-pointer"
                title="Stop after the file being transcribed now"
              >
                <Square size={12} /> Stop
              </button>
            )}
            <button
              type="button"
              onClick={() => void guess()}
              disabled={busy != null || transcribed === 0}
              className="inline-flex items-center gap-1 px-2 py-1 rounded-md bg-accent-bg/15 text-accent-bg text-[11px] cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
              title={
                transcribed === 0
                  ? "Transcribe at least one recording first"
                  : "Place every passage along the recordings, in order"
              }
            >
              {busy === "guessing" ? <Loader2 size={12} className="animate-spin" /> : <Wand2 size={12} />}
              Guess all spans
            </button>
            <span className="text-[11px] text-text-tertiary truncate max-w-[320px]" title={index?.directory}>
              {index?.directory
                ? `${index.source_count} recording${index.source_count === 1 ? "" : "s"} · ${transcribed} transcribed`
                : index
                  ? "No recordings registered"
                  : ""}
            </span>
          </div>

          {counts && counts.proposed + counts.refused > 0 && (
            <div className="flex flex-wrap items-center gap-2 text-[11px] text-text-tertiary tabular-nums">
              <span>placed {counts.proposed}</span>
              <span>·</span>
              <span>refused {counts.refused}</span>
              <span>·</span>
              <span className="text-accent-bg">accepted {counts.accepted}</span>
              <span>·</span>
              <span>cut {counts.applied}</span>
              <div className="ml-auto flex items-center gap-1">
                <span>≥</span>
                <input
                  className="w-[52px] px-1 py-1 rounded-md bg-bg-muted text-center text-[11px] tabular-nums border border-border-default text-text-primary focus:outline-none focus:border-accent-bg"
                  value={thresholdDraft ?? String(Math.round(threshold * 100))}
                  aria-label="Confidence threshold in percent"
                  inputMode="numeric"
                  onChange={(e) => {
                    setThresholdDraft(e.target.value);
                    const value = Number(e.target.value.replace("%", ""));
                    if (Number.isFinite(value)) setThreshold(Math.min(100, Math.max(0, value)) / 100);
                  }}
                  onBlur={() => setThresholdDraft(null)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") e.currentTarget.blur();
                  }}
                />
                <span>%</span>
                <button
                  type="button"
                  onClick={() => void decideMany("accepted")}
                  disabled={busy != null}
                  className="inline-flex items-center gap-1 px-2 py-1 rounded-md bg-bg-muted hover:bg-bg-hover text-text-secondary cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                  title="Accept every placed span at or above this confidence"
                >
                  <Check size={11} /> accept all
                </button>
                <button
                  type="button"
                  onClick={() => void decideMany("rejected")}
                  disabled={busy != null}
                  className="inline-flex items-center gap-1 px-2 py-1 rounded-md bg-bg-muted hover:bg-bg-hover text-text-secondary cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                  title="Reject every placed span at or above this confidence"
                >
                  <X size={11} /> reject all
                </button>
              </div>
            </div>
          )}

          {counts && counts.open > 0 && (
            <div className="flex flex-wrap items-center gap-2">
              <button
                type="button"
                onClick={() => void applyAccepted()}
                disabled={busy != null}
                className="inline-flex items-center gap-1 px-2 py-1 rounded-md bg-accent-bg text-white text-[11px] cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                title="Cut every accepted span into the dataset as the text's standard audio"
              >
                {busy === "cutting" ? <Loader2 size={12} className="animate-spin" /> : <Upload size={12} />}
                {busy === "cutting" ? "cutting…" : `Apply accepted (${counts.open})`}
              </button>
              <span className="text-[11px] text-text-tertiary">
                Each recording is decoded once however many spans come out of it.
              </span>
            </div>
          )}

          {applied && (
            <p className="text-[11px] text-text-tertiary tabular-nums">
              stored {applied.count} · failed {applied.failures}
            </p>
          )}

          {busy && busy !== "cutting" && busy !== "transcribing" && busy !== "guessing" && (
            <p className="text-[11px] text-text-tertiary">{busy}…</p>
          )}

          {error && <p className="text-[11px] text-error-text break-words">{error}</p>}
          {!error && note && (
            <p className={`text-[11px] ${note.kind === "ok" ? "text-text-tertiary" : "text-warning-text"}`}>
              {note.text}
            </p>
          )}

          {spans.length === 0 ? (
            <p className="text-[11px] text-text-tertiary">
              {sources.length === 0
                ? "Pick the folder that holds the audiobook's files to start."
                : "No spans yet — transcribe the recordings, then guess all spans."}
            </p>
          ) : (
            <div className="flex flex-col gap-1.5 max-h-[520px] overflow-y-auto pr-1">
              {spans.map((proposal) => (
                <PassageRow
                  key={proposal.text_uuid}
                  proposal={proposal}
                  span={edits[proposal.text_uuid] ?? { start: proposal.start, end: proposal.end }}
                  playing={playing === proposal.text_uuid}
                  busy={busy != null}
                  onMove={(start, end) => moveSpan(proposal, start, end)}
                  onDecide={(state) => void decide(proposal.text_uuid, state)}
                  onPlay={() =>
                    playSpan(
                      proposal,
                      edits[proposal.text_uuid]?.start ?? proposal.start,
                      edits[proposal.text_uuid]?.end ?? proposal.end
                    )
                  }
                />
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
