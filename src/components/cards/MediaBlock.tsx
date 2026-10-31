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
//!
//! `zoomable` is the reading host's option: a diagram on a card can be worth a
//! screen. The editor leaves it off, because there a click on the field already
//! means "edit this" and two meanings for one click is one too many.
//!
//! Images and video are block-level and `mx-auto`: in a prompt they sit centred
//! under centred text, and in prose a figure belongs on the page's axis, not
//! flush against the answer's left edge while the heading above it is centred.

import { useEffect, useRef, useState } from "react";
import { FileQuestion, X } from "lucide-react";
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
  /** Let a click on an image open it at screen size (reading hosts only). */
  zoomable?: boolean;
};

export default function MediaBlock({ media, handle, missing, datasetPath, reveal = true, zoomable = false }: Props) {
  const { manage } = useMediaPlay();
  const audioRef = useRef<HTMLAudioElement>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  // The zoomed image owns the keyboard while it is up: the backdrop takes focus
  // and swallows Escape, so closing the picture never also leaves the card.
  const zoomRef = useRef<HTMLDivElement>(null);
  const [zoomed, setZoomed] = useState(false);
  useEffect(() => {
    if (zoomed) zoomRef.current?.focus();
  }, [zoomed]);
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
      <>
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img
          src={src}
          alt={media.title || handle}
          title={media.title || undefined}
          onClick={
            zoomable
              ? (e) => {
                  e.stopPropagation();
                  setZoomed(true);
                }
              : undefined
          }
          className={`block max-w-full rounded-md mx-auto ${zoomable ? "cursor-zoom-in" : ""}`}
          style={{ height: "auto" }}
        />
        {zoomed && (
          <div
            ref={zoomRef}
            tabIndex={-1}
            role="dialog"
            aria-modal="true"
            aria-label={media.title || handle}
            onClick={() => setZoomed(false)}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.stopPropagation();
                e.preventDefault();
                setZoomed(false);
              }
            }}
            className="fixed inset-0 z-50 flex items-center justify-center bg-black/80 p-6 outline-none"
          >
            {/* eslint-disable-next-line @next/next/no-img-element */}
            <img
              src={src}
              alt={media.title || handle}
              className="max-h-full max-w-full rounded-md object-contain"
            />
            <button
              type="button"
              onClick={() => setZoomed(false)}
              title="Close"
              aria-label="Close"
              className="absolute top-4 right-4 flex h-9 w-9 items-center justify-center rounded-md bg-black/50 text-white hover:bg-black/70"
            >
              <X size={18} />
            </button>
          </div>
        )}
      </>
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
      className="block max-w-full rounded-md mx-auto"
      title={media.title || handle}
    />
  );
}
