"use client";

//! One attachment, rendered as a native element the reader can act on.
//!
//! The row (`CardMedia`) says *what* the media is and *how* it plays; the
//! bytes come from the dataset dir via `resolveSrc`. A missing attachment —
//! a token whose row was deleted, or whose bytes live on another device and
//! have not landed yet — renders a bordered placeholder that names the file,
//! never a silent vanish (the reader must know something belongs there).
//!
//! Audio/video register with `useMediaPlay`, so only one clip plays at a time
//! across the whole app. `play_on: 'reveal'` (and the per-attachment
//! `autoplay` flag) fires when `reveal` flips true — the moment a review card
//! is turned over — and plays through the *mounted* element so the native
//! controls stay in sync and the one-at-a-time rule holds.

import { useEffect, useRef } from "react";
import { FileQuestion } from "lucide-react";
import type { CardMedia } from "@/lib/types";
import { resolveSrc } from "@/lib/media/anchors";
import { useMediaPlay } from "@/hooks/useMediaPlay";

type Props = {
  /** The row this segment resolves to, or null when the handle has no row. */
  media: CardMedia | null;
  /** The anchor handle — shown by the placeholder when there is no row. */
  handle: string;
  /** True when the bytes are not available here (deleted row or other device). */
  missing: boolean;
  /** Absolute dataset directory (from `CardDatasetSummary.path`). */
  datasetPath: string;
  /** Whether the card is currently revealed; gates the `play_on: reveal` fire. */
  reveal?: boolean;
};

export default function MediaBlock({ media, handle, missing, datasetPath, reveal = true }: Props) {
  const { manage } = useMediaPlay();
  const audioRef = useRef<HTMLAudioElement>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  // Register whichever mounted element exists with the shared manager, so a
  // second clip pauses this one. Refs are stable across renders of the same
  // kind, so this runs on kind change only.
  useEffect(() => {
    manage(audioRef.current);
    manage(videoRef.current);
  }, [manage, media?.rel_path]);

  const kind = media?.kind;
  const src = !missing && media ? resolveSrc(datasetPath, media.rel_path) : "";

  // Fire the reveal play exactly once per reveal window (re-arms when the card
  // flips back, so flipping again replays). Plays the mounted element so the
  // native control state and the one-at-a-time rule stay correct.
  const firedRef = useRef(false);
  useEffect(() => {
    if (!reveal || !src || !kind || kind === "image") return;
    const wantsReveal = media?.play_on === "reveal" || media?.autoplay;
    if (!wantsReveal || firedRef.current) return;
    firedRef.current = true;
    const el = kind === "audio" ? audioRef.current : videoRef.current;
    el?.play().catch(() => {
      /* missing file or blocked autoplay — not user-facing on a flip */
    });
  }, [reveal, src, kind, media?.play_on, media?.autoplay]);
  useEffect(() => {
    if (!reveal) firedRef.current = false;
  }, [reveal]);

  // Placeholder for anything we cannot show: no row, or a row whose bytes or
  // resolvable URL are absent on this device.
  if (missing || !media || !src) {
    const name = media?.rel_path?.split(/[/\\]/).pop() || handle;
    return (
      <div className="flex items-center gap-2 rounded-lg border border-dashed border-border-default bg-bg-surface px-3 py-2 text-sm text-text-tertiary">
        <FileQuestion size={16} className="shrink-0" />
        <span className="min-w-0">
          Attachment not on this device · <span className="font-mono">{name}</span>
        </span>
      </div>
    );
  }

  if (kind === "image") {
    return (
      // eslint-disable-next-line @next/next/no-img-element
      <img
        src={src}
        alt={media.title || handle}
        title={media.title || undefined}
        className="max-w-full rounded-md"
        style={{ height: "auto" }}
      />
    );
  }

  if (kind === "audio") {
    return (
      <audio
        ref={audioRef}
        src={src}
        controls
        className="w-full"
        title={media.title || handle}
      />
    );
  }

  // video
  return (
    <video
      ref={videoRef}
      src={src}
      controls
      playsInline
      className="max-w-full rounded-md"
      title={media.title || handle}
    />
  );
}
