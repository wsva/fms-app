"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Play, Square } from "lucide-react";
import type { ReadStandardAudio } from "@/lib/read_aloud/types";

/** `m:ss` — the shape a clip's length reads in next to a progress bar. */
function clock(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "0:00";
  return `${Math.floor(seconds / 60)}:${String(Math.floor(seconds % 60)).padStart(2, "0")}`;
}

type Props = {
  /** The clip scoring would read — the one marked default. */
  clip: ReadStandardAudio;
  /** A take is open, so playback would bleed into the microphone. */
  disabled?: boolean;
};

/**
 * A compact player for a text's standard audio, sitting above the passage it
 * reads.
 *
 * This is the *listening* half of the reference: the clip itself is what
 * pronunciation is measured against, and hearing it before recording is how a
 * reader learns the intended pacing. It plays and seeks only — every change to
 * the clips themselves stays on the Manage page's Standard audio card.
 */
export default function StandardAudioPlayer({ clip, disabled = false }: Props) {
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const [playing, setPlaying] = useState(false);
  // The dataset-wide listing does not decode the wav, so `clip.duration` is
  // null here; metadata is what fills it in.
  const [duration, setDuration] = useState(clip.duration ?? 0);
  const [currentTime, setCurrentTime] = useState(0);

  const stop = useCallback(() => {
    const el = audioRef.current;
    if (!el) return;
    el.pause();
    el.currentTime = 0;
    setCurrentTime(0);
  }, []);

  // Starting a take must not pick up the reference through the speakers.
  useEffect(() => {
    if (disabled) stop();
  }, [disabled, stop]);

  // Never leave the reference playing behind the practice pane.
  useEffect(() => () => audioRef.current?.pause(), []);

  const toggle = () => {
    const el = audioRef.current;
    if (!el || disabled) return;
    if (!el.paused) {
      stop();
      return;
    }
    if (el.ended) el.currentTime = 0;
    el.play().catch(() => setPlaying(false));
  };

  const seek = (seconds: number) => {
    const el = audioRef.current;
    if (!el || !Number.isFinite(seconds)) return;
    el.currentTime = seconds;
    setCurrentTime(seconds);
  };

  return (
    <div className="mt-2 flex items-center gap-3 px-3 py-2 rounded-xl bg-bg-muted border border-border-default min-w-0">
      <button
        type="button"
        onClick={toggle}
        disabled={disabled}
        aria-label={playing ? "Stop standard audio" : "Play standard audio"}
        title={
          disabled
            ? "Stop the recording to play the standard audio"
            : playing
              ? "Stop"
              : "Play standard audio"
        }
        className="shrink-0 flex items-center justify-center w-8 h-8 rounded-full bg-accent-bg text-white cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
      >
        {playing ? <Square size={13} /> : <Play size={15} className="ml-0.5" />}
      </button>

      <div className="flex flex-col gap-1 flex-1 min-w-0">
        <div className="flex items-baseline justify-between gap-2 text-[11px] text-text-tertiary min-w-0">
          <span className="truncate" title={clip.note || undefined}>
            <span className="text-text-secondary">Standard audio</span>
            {clip.note ? ` · ${clip.note}` : ""}
            {clip.stale ? " · made from an earlier version of the text" : ""}
          </span>
          <span className="tabular-nums shrink-0">
            {clock(currentTime)} / {clock(duration)}
          </span>
        </div>
        <input
          type="range"
          min={0}
          max={duration > 0 ? duration : 0.1}
          step={0.05}
          value={Math.min(currentTime, duration > 0 ? duration : 0.1)}
          onChange={(e) => seek(Number(e.target.value))}
          disabled={disabled || duration <= 0}
          aria-label="Standard audio position"
          className="w-full cursor-pointer accent-accent disabled:opacity-50 disabled:cursor-not-allowed"
        />
      </div>

      <audio
        ref={audioRef}
        className="hidden"
        src={convertFileSrc(clip.audio_url ?? "")}
        preload="metadata"
        onLoadedMetadata={(e) => {
          const d = e.currentTarget.duration;
          if (Number.isFinite(d) && d > 0) setDuration(d);
        }}
        onTimeUpdate={(e) => setCurrentTime(e.currentTarget.currentTime)}
        onPlay={() => setPlaying(true)}
        onPause={() => setPlaying(false)}
        onEnded={() => {
          setPlaying(false);
          setCurrentTime(0);
        }}
      />
    </div>
  );
}
