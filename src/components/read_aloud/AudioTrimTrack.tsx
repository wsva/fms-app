"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { ChevronLeft, ChevronRight, Loader2, Play, ScanText, Square } from "lucide-react";

/** Shortest clip the import path accepts; a smaller drag is a mis-drag. */
export const MIN_CLIP_SECONDS = 0.3;

/** One press of an edge's arrows — the 100 ms an arrow key on a handle moves. */
const EDGE_STEP = 0.1;

type Props = {
  /** Interleaved min/max signed 8-bit pairs — the `WaveformPeaks` layout. */
  peaks: number[];
  /** Seconds of the whole source. */
  duration: number;
  start: number;
  end: number;
  onChange: (start: number, end: number) => void;
  /**
   * Absolute path of the source for playback. Absent when the bytes came from a
   * mobile picker (a `content://` URI has no playable local path), in which case
   * the Play button is not offered.
   */
  audioUrl?: string | null;
  /**
   * Let the loaded speech model find the selection, for a recording that holds
   * more than this text. The track only offers the button: where the edges land
   * is the parent's business, because only it knows the file and the text.
   * Absent when the caller has nothing to listen for.
   */
  guess?: {
    running: boolean;
    /** What the last guess found, or why it refused. */
    note: { kind: "ok" | "warn"; text: string } | null;
    run: () => void;
  };
};

function clampRange(start: number, end: number, duration: number): [number, number] {
  const s = Math.max(0, Math.min(start, Math.max(0, duration - MIN_CLIP_SECONDS)));
  const e = Math.max(s + MIN_CLIP_SECONDS, Math.min(end, duration));
  return [s, Math.min(e, duration)];
}

function fmt(seconds: number): string {
  const s = Math.max(0, seconds);
  return `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}.${String(
    Math.round((s % 1) * 10)
  )}`;
}

/**
 * `1:02.5`, `62.5` or `62,5` → seconds; `null` when it is not a time. A comma
 * is accepted because the decimal separator on a German keyboard is one, and
 * typing a time is the one place the user may not be thinking in dots.
 */
function parseTime(text: string): number | null {
  const t = text.trim().replace(",", ".");
  const m = /^(?:(\d+):)?(\d+(?:\.\d*)?)$/.exec(t);
  if (!m) return null;
  const seconds = Number(m[2]);
  if (!Number.isFinite(seconds)) return null;
  return (m[1] ? Number(m[1]) * 60 : 0) + seconds;
}

/**
 * A selection edge as a field you can type into, which is the only way to name
 * a time exactly: a handle lands where the pointer is, the arrows move 100 ms,
 * and "1:23.4" says what it means.
 *
 * The draft is local so typing is never fought by the formatted value coming
 * back from the parent; blur or Enter commits, Escape gives up, and an
 * unparseable draft is marked while it is wrong and reverted when left.
 */
function TimeField({
  value,
  label,
  onCommit,
}: {
  value: number;
  label: string;
  onCommit: (seconds: number) => void;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const parsed = draft == null ? value : parseTime(draft);
  const commit = () => {
    if (draft == null) return;
    const t = parseTime(draft);
    setDraft(null);
    // The track's own resolution is a tenth of a second, so a typed value is
    // rounded to it rather than kept finer than anything here can show.
    if (t != null) onCommit(Math.round(t * 10) / 10);
  };
  return (
    <input
      className={`w-[54px] px-1 py-1 rounded-md bg-bg-muted text-center text-[11px] tabular-nums border focus:outline-none focus:border-accent-bg ${
        parsed == null ? "border-error-text text-error-text" : "border-border-default text-text-primary"
      }`}
      value={draft ?? fmt(value)}
      aria-label={label}
      aria-invalid={parsed == null}
      title={`${label}: type a time such as 1:02.5 or 62.5`}
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

/**
 * One edge of the selection: `[◀ 100 ms][Set start][100 ms ▶]`.
 *
 * Dragging a handle is quick but not precise — the pixel under the pointer is
 * a tenth of a second at best — so each edge also takes the playhead as its
 * value and moves in audible steps from there. The label sits between the two
 * arrows on purpose: they adjust *this* edge, not the selection as a whole.
 */
function EdgeGroup({
  edge,
  hint,
  onSet,
  onStep,
}: {
  edge: "start" | "end";
  hint: string;
  onSet: () => void;
  onStep: (delta: number) => void;
}) {
  const arrow =
    "inline-flex items-center justify-center w-7 py-1 bg-bg-muted hover:bg-bg-hover text-text-secondary cursor-pointer";
  const side = (delta: number, icon: React.ReactNode) => (
    <button
      type="button"
      className={arrow}
      onClick={() => onStep(delta)}
      title={`${edge === "start" ? "Start" : "End"} 100 ms ${delta < 0 ? "earlier" : "later"}`}
      aria-label={`${edge === "start" ? "Start" : "End"} 100 ms ${delta < 0 ? "earlier" : "later"}`}
    >
      {icon}
    </button>
  );
  return (
    <div className="inline-flex items-stretch rounded-md border border-border-default overflow-hidden text-[11px]">
      {side(-EDGE_STEP, <ChevronLeft size={12} />)}
      <button
        type="button"
        className="px-2 py-1 bg-bg-muted hover:bg-bg-hover text-text-secondary border-x border-border-default cursor-pointer"
        onClick={onSet}
        title={hint}
      >
        Set {edge}
      </button>
      {side(EDGE_STEP, <ChevronRight size={12} />)}
    </div>
  );
}

/**
 * Peak envelope of the picked file with two drag handles and a shaded
 * selection: the part of the recording that becomes the standard audio.
 *
 * The playhead is the other half of the crop. It is where the sound is, so it
 * is the value the two Set buttons take, and it is drawn thin and neutral so
 * it is never mistaken for one of the accent-coloured selection edges.
 *
 * Drawn on a canvas rather than with DOM nodes because a minute of audio is
 * thousands of peaks; the existing `WaveformCanvas` is bound to a `<video>` ref
 * and so cannot be reused here.
 */
export default function AudioTrimTrack({
  peaks,
  duration,
  start,
  end,
  onChange,
  audioUrl,
  guess,
}: Props) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const trackRef = useRef<HTMLDivElement | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const [width, setWidth] = useState(0);
  const [playing, setPlaying] = useState(false);
  // Where the sound is: the value Set start / Set end take. It follows playback
  // and is placed by clicking or dragging the waveform.
  const [playhead, setPlayhead] = useState(0);
  const draggingRef = useRef<"start" | "end" | null>(null);
  const scrubRef = useRef(false);

  // Track the pixel width so the canvas is redrawn at the right resolution.
  useEffect(() => {
    const el = trackRef.current;
    if (!el) return;
    const update = () => setWidth(el.clientWidth);
    update();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Repaint: shading for the selection, one vertical min/max bar per column,
  // then the two handle lines.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || width <= 0 || !(duration > 0) || peaks.length < 2) return;
    const dpr = typeof window === "undefined" ? 1 : window.devicePixelRatio || 1;
    const height = 72;
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

    // Unselected audio is drawn faint, the selection at full strength, so the
    // crop is legible without a second waveform.
    const startX = (start / duration) * width;
    const endX = (end / duration) * width;

    const pairs = peaks.length / 2;
    for (let x = 0; x < width; x++) {
      const from = Math.floor((x / width) * pairs);
      const to = Math.max(from + 1, Math.floor(((x + 1) / width) * pairs));
      let lo = 0;
      let hi = 0;
      for (let i = from; i < to && i < pairs; i++) {
        lo = Math.min(lo, peaks[i * 2]);
        hi = Math.max(hi, peaks[i * 2 + 1]);
      }
      const top = mid - (hi / 128) * mid;
      const bottom = mid - (lo / 128) * mid;
      const selected = x >= startX && x <= endX;
      ctx.fillStyle = selected ? accent : muted;
      ctx.globalAlpha = selected ? 0.85 : 0.3;
      ctx.fillRect(x, top, 1, Math.max(1, bottom - top));
    }
    ctx.globalAlpha = 1;

    // Selection edges.
    ctx.fillStyle = accent;
    ctx.fillRect(Math.round(startX), 0, 2, height);
    ctx.fillRect(Math.max(0, Math.round(endX) - 2), 0, 2, height);
  }, [peaks, duration, start, end, width]);

  const stopAudio = useCallback(() => {
    const el = audioRef.current;
    if (el) {
      el.pause();
      audioRef.current = null;
    }
    setPlaying(false);
  }, []);

  useEffect(() => stopAudio, [stopAudio]);

  // A different file is a different timeline; a playhead left over from the
  // previous one would point at some unrelated moment.
  useEffect(() => {
    setPlayhead(0);
    stopAudio();
  }, [duration, stopAudio]);

  const playSelection = useCallback(() => {
    if (!audioUrl) return;
    if (audioRef.current) {
      stopAudio();
      return;
    }
    const el = new Audio(convertFileSrc(audioUrl));
    audioRef.current = el;
    el.currentTime = start;
    setPlayhead(start);
    // Clamp: the element plays the whole file, so end it at the selection edge.
    el.ontimeupdate = () => {
      setPlayhead(el.currentTime);
      if (el.currentTime >= end) stopAudio();
    };
    el.onended = () => setPlaying(false);
    setPlaying(true);
    el.play().catch(() => stopAudio());
  }, [audioUrl, start, end, stopAudio]);

  const secondsAt = useCallback(
    (clientX: number): number => {
      const el = trackRef.current;
      if (!el || !(duration > 0)) return 0;
      const rect = el.getBoundingClientRect();
      const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / Math.max(1, rect.width)));
      return ratio * duration;
    },
    [duration]
  );

  const placePlayhead = useCallback(
    (clientX: number) => {
      const t = secondsAt(clientX);
      setPlayhead(t);
      const el = audioRef.current;
      if (el) el.currentTime = t;
    },
    [secondsAt]
  );

  const applyHandle = useCallback(
    (handle: "start" | "end", clientX: number) => {
      const t = secondsAt(clientX);
      const [s, e] = handle === "start" ? clampRange(t, end, duration) : clampRange(start, t, duration);
      onChange(s, e);
    },
    [duration, end, onChange, secondsAt, start]
  );

  const beginDrag = (handle: "start" | "end") => (e: React.PointerEvent) => {
    e.preventDefault();
    draggingRef.current = handle;
    (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
    applyHandle(handle, e.clientX);
  };

  const onPointerMove = (e: React.PointerEvent) => {
    if (scrubRef.current) {
      placePlayhead(e.clientX);
      return;
    }
    if (!draggingRef.current) return;
    applyHandle(draggingRef.current, e.clientX);
  };

  const endDrag = () => {
    draggingRef.current = null;
    scrubRef.current = false;
  };

  // Set an edge to the playhead, or move it one 100 ms step from where it is.
  // `clampRange` keeps the two edges apart by the minimum clip length.
  const play = Math.max(0, Math.min(playhead, duration));

  const setEdge = (edge: "start" | "end") => {
    const [s, e] = edge === "start" ? clampRange(play, end, duration) : clampRange(start, play, duration);
    onChange(s, e);
  };

  const nudgeEdge = (edge: "start" | "end", delta: number) => {
    const base = Math.round(((edge === "start" ? start : end) + delta) * 1000) / 1000;
    const [s, e] = edge === "start" ? clampRange(base, end, duration) : clampRange(start, base, duration);
    onChange(s, e);
  };

  const startPct = duration > 0 ? (start / duration) * 100 : 0;
  const endPct = duration > 0 ? (end / duration) * 100 : 100;

  return (
    <div className="flex flex-col gap-2">
      <div
        ref={trackRef}
        className="relative select-none rounded-md bg-bg-muted overflow-hidden"
        style={{ height: 72, touchAction: "none" }}
        onPointerMove={onPointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
      >
        <canvas
          ref={canvasRef}
          className="block w-full cursor-text"
          style={{ height: 72 }}
          onPointerDown={(e) => {
            scrubRef.current = true;
            (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
            placePlayhead(e.clientX);
          }}
        />
        {duration > 0 && play > 0 && (
          <span
            aria-hidden
            className="absolute top-0 h-full w-px bg-text-secondary pointer-events-none"
            style={{ left: `${(play / duration) * 100}%` }}
          />
        )}
        {(["start", "end"] as const).map((handle) => (
          <div
            key={handle}
            role="slider"
            tabIndex={0}
            aria-label={handle === "start" ? "Selection start" : "Selection end"}
            aria-valuemin={0}
            aria-valuemax={Math.round(duration * 10) / 10}
            aria-valuenow={Math.round((handle === "start" ? start : end) * 10) / 10}
            className="absolute top-0 h-full w-3 -ml-1.5 cursor-ew-resize group"
            style={{ left: `${handle === "start" ? startPct : endPct}%` }}
            onPointerDown={beginDrag(handle)}
            onKeyDown={(e) => {
              const step = e.shiftKey ? 1 : 0.1;
              const dir = e.key === "ArrowLeft" ? -1 : e.key === "ArrowRight" ? 1 : 0;
              if (!dir) return;
              e.preventDefault();
              const next = (handle === "start" ? start : end) + dir * step;
              const [s, en] =
                handle === "start" ? clampRange(next, end, duration) : clampRange(start, next, duration);
              onChange(s, en);
            }}
          >
            <span className="absolute inset-y-0 left-1/2 -translate-x-1/2 w-0.5 bg-accent-bg" />
            <span className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 h-6 w-2.5 rounded-sm bg-accent-bg shadow" />
          </div>
        ))}
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <EdgeGroup
          edge="start"
          hint={`Use the playhead (${fmt(play)}) as the start`}
          onSet={() => setEdge("start")}
          onStep={(delta) => nudgeEdge("start", delta)}
        />
        <EdgeGroup
          edge="end"
          hint={`Use the playhead (${fmt(play)}) as the end`}
          onSet={() => setEdge("end")}
          onStep={(delta) => nudgeEdge("end", delta)}
        />
        <span
          className="text-[11px] text-text-tertiary tabular-nums"
          title="Click or drag the waveform to move the playhead"
        >
          playhead {fmt(play)}
        </span>
      </div>

      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-text-tertiary">
        <div className="inline-flex items-center gap-1">
          <span>Selection</span>
          <TimeField
            label="Selection start"
            value={start}
            onCommit={(t) => {
              const [s, e] = clampRange(t, end, duration);
              onChange(s, e);
            }}
          />
          <span>–</span>
          <TimeField
            label="Selection end"
            value={end}
            onCommit={(t) => {
              const [s, e] = clampRange(start, t, duration);
              onChange(s, e);
            }}
          />
          <span className="tabular-nums">
            ({(end - start).toFixed(1)} s of {fmt(duration)})
          </span>
        </div>
        {audioUrl && (
          <button
            type="button"
            onClick={playSelection}
            className="inline-flex items-center gap-1 px-2 py-1 rounded-md bg-bg-muted hover:bg-bg-hover text-text-secondary cursor-pointer"
            title={playing ? "Stop" : "Play the selection"}
          >
            {playing ? <Square size={12} /> : <Play size={12} />}
            {playing ? "Stop" : "Play selection"}
          </button>
        )}
        <button
          type="button"
          onClick={() => onChange(0, duration)}
          className="px-2 py-1 rounded-md bg-bg-muted hover:bg-bg-hover text-text-secondary cursor-pointer"
        >
          Use whole clip
        </button>
        {guess && (
          <button
            type="button"
            onClick={guess.run}
            disabled={guess.running}
            className="inline-flex items-center gap-1 px-2 py-1 rounded-md bg-bg-muted hover:bg-bg-hover text-text-secondary cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
            title="Listen to the whole file with the loaded speech model and put the edges where this text is read. Nothing is saved until Save clip."
          >
            {guess.running ? <Loader2 size={12} className="animate-spin" /> : <ScanText size={12} />}
            {guess.running ? "Listening…" : "Guess from speech"}
          </button>
        )}
      </div>

      {guess?.note && (
        <p
          className={`text-[11px] ${
            guess.note.kind === "ok" ? "text-text-tertiary" : "text-warning-text"
          }`}
        >
          {guess.note.text}
        </p>
      )}
    </div>
  );
}
