"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Play, Square } from "lucide-react";

/** Shortest clip the import path accepts; a smaller drag is a mis-drag. */
export const MIN_CLIP_SECONDS = 0.3;

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
 * Peak envelope of the picked file with two drag handles and a shaded
 * selection: the part of the recording that becomes the standard audio.
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
}: Props) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const trackRef = useRef<HTMLDivElement | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const [width, setWidth] = useState(0);
  const [playing, setPlaying] = useState(false);
  const draggingRef = useRef<"start" | "end" | null>(null);

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

  const playSelection = useCallback(() => {
    if (!audioUrl) return;
    if (audioRef.current) {
      stopAudio();
      return;
    }
    const el = new Audio(convertFileSrc(audioUrl));
    audioRef.current = el;
    el.currentTime = start;
    // Clamp: the element plays the whole file, so end it at the selection edge.
    el.ontimeupdate = () => {
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
    if (!draggingRef.current) return;
    applyHandle(draggingRef.current, e.clientX);
  };

  const endDrag = () => {
    draggingRef.current = null;
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
        <canvas ref={canvasRef} className="block w-full" style={{ height: 72 }} />
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

      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-text-tertiary">
        <span className="tabular-nums">
          Selection {fmt(start)} – {fmt(end)} ({(end - start).toFixed(1)} s of {fmt(duration)})
        </span>
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
      </div>
    </div>
  );
}
