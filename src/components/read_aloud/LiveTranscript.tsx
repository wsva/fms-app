"use client";

import { useEffect, useRef } from "react";

type Props = {
  /** What the model has recognized of the take so far. */
  text: string;
  /** A stretch is being recognized right now. */
  busy: boolean;
  /** Seconds recorded so far. */
  seconds: number;
};

function mmss(total: number): string {
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}

/**
 * The bottom dock of the practice view while a take is recording: the transcript
 * the model is producing as the reader speaks.
 *
 * Read aloud scores a take against the text, so seeing the transcript before the
 * recording ends is how a reader notices the microphone is picking up nothing —
 * or that the loaded model is in the wrong language — while there is still time
 * to start over. Ending the take stays where it always was: the toolbar's Stop.
 */
export default function LiveTranscript({ text, busy, seconds }: Props) {
  // Keep the newest words in view: the preview grows downward as the reader speaks.
  const scrollRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [text, busy]);

  return (
    <section className="shrink-0 min-w-0 border-t border-border-default bg-bg-card">
      <div className="flex items-center gap-2 px-3 pt-2">
        <span aria-hidden className="h-2 w-2 rounded-full bg-error-text animate-pulse shrink-0" />
        <span className="text-xs font-semibold text-text-primary">Live recognition</span>
        <span className="text-[11px] text-text-tertiary tabular-nums">{mmss(seconds)}</span>
        <span className="ml-auto text-[11px] text-text-tertiary">
          {busy ? "Recognizing…" : "Listening…"}
        </span>
      </div>

      <div
        ref={scrollRef}
        className="max-h-[24vh] min-h-11 overflow-y-auto px-3 pb-2 text-sm leading-relaxed text-text-primary whitespace-pre-wrap break-words"
        aria-live="polite"
      >
        {text || (
          <span className="text-text-tertiary">
            Waiting for the first words — what the model hears appears here while you read.
          </span>
        )}
      </div>
    </section>
  );
}
