"use client";

import { useEffect, useRef, useState } from "react";
import type { AlignPreview } from "@/lib/read_aloud/sourceAlign";
import { COARSE_STEP, MIN_SPAN, NUDGE_STEP, clock, roundMs } from "./format";

/**
 * The audio around one span, at the size a person can work in.
 *
 * The backend sends a *window* — the span plus a quarter second each side, at
 * most a few hundred peak pairs — rather than the whole chapter, so this draws
 * in absolute seconds but only between `preview.start` and the window's end.
 * Everything is painted on one canvas (bars, selection band, ruler); only the
 * two edges and the playhead are DOM, because those are the things a pointer
 * and a tab stop have to reach.
 */

const WAVE_H = 96;
const RULER_H = 16;
const HEIGHT = WAVE_H + RULER_H;

/** Candidate ruler steps, coarsest last: 10 ms is finer than a pixel. */
const TICK_STEPS = [0.1, 0.2, 0.5, 1, 2, 5, 10, 15, 20, 30, 60, 120, 300, 600];

type Edge = "start" | "end";

type Props = {
  preview: AlignPreview | null;
  /** Absolute seconds, as stored. */
  start: number;
  end: number;
  /** Absolute seconds of the sound, when it is known. */
  playhead: number | null;
  playing: boolean;
  disabled: boolean;
  /** Both edges, absolute; drags and arrows land inside the window. */
  onChange: (start: number, end: number) => void;
  onScrub: (seconds: number) => void;
  /** Which edge the keyboard nudges, so a drag can claim that role too. */
  onActivateEdge?: (edge: Edge) => void;
};

/** A tick interval that keeps labels roughly 70 px apart. */
function tickStep(windowSeconds: number, width: number): number {
  if (!(width > 0) || !(windowSeconds > 0)) return 1;
  const target = (70 / width) * windowSeconds;
  return TICK_STEPS.find((step) => step >= target) ?? TICK_STEPS[TICK_STEPS.length - 1];
}

export default function SpanTrack({
  preview,
  start,
  end,
  playhead,
  playing,
  disabled,
  onChange,
  onScrub,
  onActivateEdge,
}: Props) {
  const boxRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [width, setWidth] = useState(0);
  const dragRef = useRef<Edge | null>(null);
  const scrubRef = useRef(false);

  const pairs = preview ? Math.floor(preview.peaks.length / 2) : 0;
  const origin = preview?.start ?? 0;
  const windowSeconds = preview && pairs > 0 ? pairs * preview.peak_seconds : 0;
  const ready = pairs > 0 && windowSeconds > 0;

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
    if (!canvas || !preview || !ready || width <= 0) return;
    const dpr = typeof window === "undefined" ? 1 : window.devicePixelRatio || 1;
    canvas.width = Math.round(width * dpr);
    canvas.height = Math.round(HEIGHT * dpr);
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, width, HEIGHT);

    const css = getComputedStyle(canvas);
    const accent = css.getPropertyValue("--color-accent-bg").trim() || "#6366f1";
    const muted = css.getPropertyValue("--color-text-tertiary").trim() || "#94a3b8";
    const border = css.getPropertyValue("--color-border-default").trim() || "#e2e8f0";
    const mid = WAVE_H / 2;
    const xOf = (seconds: number) =>
      Math.min(width, Math.max(0, ((seconds - origin) / windowSeconds) * width));
    const fromX = xOf(start);
    const toX = xOf(end);

    // The span as a band: it reads even where the waveform is flat.
    ctx.fillStyle = accent;
    ctx.globalAlpha = 0.12;
    ctx.fillRect(fromX, 0, Math.max(1, toX - fromX), WAVE_H);
    ctx.globalAlpha = 1;

    for (let x = 0; x < width; x++) {
      const index = Math.min(pairs - 1, Math.floor((x / width) * pairs));
      const lo = preview.peaks[index * 2] ?? 0;
      const hi = preview.peaks[index * 2 + 1] ?? 0;
      const top = mid - (hi / 128) * (mid - 3);
      const bottom = mid - (lo / 128) * (mid - 3);
      const inside = x >= fromX && x <= toX;
      ctx.fillStyle = inside ? accent : muted;
      ctx.globalAlpha = inside ? 0.95 : 0.35;
      ctx.fillRect(x, top, 1, Math.max(1, bottom - top));
    }
    ctx.globalAlpha = 1;

    // Edges, drawn over the bars: the padding is what is being judged.
    ctx.fillStyle = accent;
    ctx.fillRect(Math.round(fromX), 0, 2, WAVE_H);
    ctx.fillRect(Math.max(0, Math.round(toX) - 2), 0, 2, WAVE_H);

    // Ruler: absolute timestamps, because the window starts mid-recording.
    ctx.fillStyle = border;
    ctx.fillRect(0, WAVE_H, width, 1);
    const step = tickStep(windowSeconds, width);
    const first = Math.ceil(origin / step) * step;
    ctx.font = "9px ui-monospace, SFMono-Regular, Menlo, monospace";
    ctx.textBaseline = "middle";
    for (let t = first; t <= origin + windowSeconds + 1e-9; t += step) {
      const x = Math.round(((t - origin) / windowSeconds) * width);
      ctx.fillStyle = muted;
      ctx.globalAlpha = 0.6;
      ctx.fillRect(x, WAVE_H, 1, 4);
      ctx.globalAlpha = 1;
      const flip = x > width - 46;
      ctx.textAlign = flip ? "right" : "left";
      ctx.fillText(clock(t), flip ? x - 3 : x + 3, WAVE_H + RULER_H / 2 + 1);
    }
  }, [preview, ready, start, end, origin, windowSeconds, width, pairs]);

  const secondsAt = (clientX: number): number => {
    const el = boxRef.current;
    if (!el || !ready) return origin;
    const rect = el.getBoundingClientRect();
    const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / Math.max(1, rect.width)));
    return origin + ratio * windowSeconds;
  };

  const clampSpan = (s: number, e: number): [number, number] => {
    const lo = origin;
    const hi = origin + windowSeconds;
    const ns = Math.min(Math.max(s, lo), Math.max(lo, hi - MIN_SPAN));
    const ne = Math.min(Math.max(e, ns + MIN_SPAN), hi);
    return [roundMs(ns), roundMs(ne)];
  };

  const emit = (s: number, e: number) => {
    if (Math.abs(s - start) < 1e-9 && Math.abs(e - end) < 1e-9) return;
    onChange(s, e);
  };

  const applyEdge = (edge: Edge, clientX: number) => {
    const t = secondsAt(clientX);
    const [s, e] = edge === "start" ? clampSpan(t, end) : clampSpan(start, t);
    emit(s, e);
  };

  const nudge = (edge: Edge, delta: number) => {
    const base = (edge === "start" ? start : end) + delta;
    const [s, e] = edge === "start" ? clampSpan(base, end) : clampSpan(start, base);
    emit(s, e);
  };

  const endDrag = () => {
    dragRef.current = null;
    scrubRef.current = false;
  };

  const pct = (seconds: number) =>
    ready ? Math.min(100, Math.max(0, ((seconds - origin) / windowSeconds) * 100)) : 0;

  const playheadVisible =
    ready && playhead != null && playhead >= origin && playhead <= origin + windowSeconds;

  return (
    <div
      ref={boxRef}
      className="relative select-none rounded-md bg-bg-muted overflow-hidden"
      style={{ height: HEIGHT }}
      onPointerMove={(e) => {
        if (disabled) return;
        if (scrubRef.current) {
          onScrub(secondsAt(e.clientX));
          return;
        }
        if (dragRef.current) applyEdge(dragRef.current, e.clientX);
      }}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      title={ready ? "Drag an edge, or click to move the playhead" : undefined}
    >
      {ready ? (
        <>
          <canvas
            ref={canvasRef}
            className={`block w-full ${disabled ? "opacity-60" : "cursor-text"}`}
            style={{ height: HEIGHT }}
            aria-hidden
            onPointerDown={(e) => {
              if (disabled) return;
              scrubRef.current = true;
              (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
              onScrub(secondsAt(e.clientX));
            }}
          />

          {(["start", "end"] as const).map((edge) => {
            const value = edge === "start" ? start : end;
            return (
              <div
                key={edge}
                role="slider"
                tabIndex={disabled ? -1 : 0}
                aria-label={edge === "start" ? "Span start" : "Span end"}
                aria-valuemin={roundMs(origin)}
                aria-valuemax={roundMs(origin + windowSeconds)}
                aria-valuenow={roundMs(value)}
                aria-valuetext={clock(value)}
                className="absolute top-0 w-3 -ml-1.5 cursor-ew-resize group focus:outline-none"
                style={{ left: `${pct(value)}%`, height: WAVE_H }}
                onPointerDown={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  if (disabled) return;
                  dragRef.current = edge;
                  onActivateEdge?.(edge);
                  (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
                  applyEdge(edge, e.clientX);
                }}
                onFocus={() => onActivateEdge?.(edge)}
                onKeyDown={(e) => {
                  if (disabled) return;
                  const dir = e.key === "ArrowLeft" ? -1 : e.key === "ArrowRight" ? 1 : 0;
                  if (!dir) return;
                  e.preventDefault();
                  onActivateEdge?.(edge);
                  nudge(edge, dir * (e.shiftKey ? COARSE_STEP : NUDGE_STEP));
                }}
              >
                <span className="absolute inset-y-0 left-1/2 -translate-x-1/2 w-0.5 bg-accent-bg" />
                <span className="absolute left-1/2 -translate-x-1/2 top-1 h-5 w-2.5 rounded-sm bg-accent-bg shadow group-focus-visible:ring-2 group-focus-visible:ring-accent-bg/40" />
                <span className="absolute left-1/2 -translate-x-1/2 bottom-1 h-5 w-2.5 rounded-sm bg-accent-bg shadow" />
              </div>
            );
          })}

          {playheadVisible && (
            <span
              aria-hidden
              className={`absolute top-0 w-px pointer-events-none ${
                playing ? "bg-accent-bg" : "bg-text-secondary"
              }`}
              style={{ left: `${pct(playhead as number)}%`, height: WAVE_H }}
            />
          )}
        </>
      ) : (
        <div className="h-full flex items-center justify-center text-[11px] text-text-tertiary">
          No audio cached on this device
        </div>
      )}
    </div>
  );
}
