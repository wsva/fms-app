"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { CONTEXT_PAD } from "./format";

/**
 * The page's one audio element.
 *
 * A span is a pair of seconds *inside* a recording, so playing it means playing
 * the whole file and stopping at the far edge: nothing is cut, and there is no
 * clip to play. The element lives here rather than in a component so the whole
 * page shares it — starting a second playback stops the first, which is what a
 * page with one pair of speakers should do.
 */
export function useSpanPlayer({
  textUuid,
  path,
  span,
}: {
  /** The text in front of the reader. A change stops playback. */
  textUuid: string | null;
  /** The recording that text was placed in; null when there is nothing to play. */
  path: string | null;
  /** The span as the fields show it, nudges included. */
  span: { start: number; end: number };
}) {
  const [playing, setPlaying] = useState(false);
  const [playhead, setPlayhead] = useState<number | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const boundsRef = useRef<{ from: number; to: number } | null>(null);
  /**
   * This render's text and span.
   *
   * `togglePlay` is called from a key listener that outlives the render it was
   * made in, so it reads the selection from here rather than from a closure.
   */
  const current = useRef({ path, span });
  current.current = { path, span };

  const stopAudio = useCallback(() => {
    if (audioRef.current) {
      audioRef.current.pause();
      audioRef.current = null;
    }
    boundsRef.current = null;
    setPlaying(false);
  }, []);

  // Leaving the page must not leave a file playing.
  useEffect(() => stopAudio, [stopAudio]);

  // Another text is another span: the old playhead would sit over audio that
  // is no longer the one on screen.
  useEffect(() => {
    stopAudio();
    setPlayhead(null);
  }, [textUuid, stopAudio]);

  /**
   * Play a recording from `from` to `to` — the one entry point, so a caller
   * holding a file but no placed text still goes through this element.
   */
  const playFrom = useCallback(
    (file: string, from: number, to: number) => {
      stopAudio();
      const el = new Audio(convertFileSrc(file));
      audioRef.current = el;
      boundsRef.current = { from, to };
      el.currentTime = Math.max(0, from);
      setPlayhead(Math.max(0, from));
      setPlaying(true);
      // The element plays the whole recording, so the span's end is what stops it.
      el.ontimeupdate = () => {
        setPlayhead(el.currentTime);
        const bounds = boundsRef.current;
        if (!bounds) return;
        if (el.currentTime >= bounds.to) stopAudio();
      };
      el.onended = () => stopAudio();
      el.play().catch(() => stopAudio());
    },
    [stopAudio]
  );

  /** Space: the span, or the span with a breath of context around each edge. */
  const togglePlay = useCallback(
    (withContext: boolean) => {
      if (playing) {
        stopAudio();
        return;
      }
      const { path: file, span: at } = current.current;
      if (!file) return;
      playFrom(
        file,
        withContext ? Math.max(0, at.start - CONTEXT_PAD) : at.start,
        withContext ? at.end + CONTEXT_PAD : at.end
      );
    },
    [playing, playFrom, stopAudio]
  );

  const scrub = useCallback((seconds: number) => {
    const t = Math.max(0, seconds);
    setPlayhead(t);
    if (audioRef.current) audioRef.current.currentTime = t;
  }, []);

  return { playing, playhead, playFrom, togglePlay, scrub, stopAudio };
}
