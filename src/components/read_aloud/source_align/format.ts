/**
 * The numbers this page is built out of, formatted in one place.
 *
 * A span edge is judged in milliseconds, so every view — the queue, the track,
 * the fields, the rails — has to print a time the same way. Keeping the
 * formatter and the two step sizes here is what stops a 10 ms nudge in one view
 * from being a 100 ms nudge in another.
 */

/** One press of an edge's arrow: the finest step a clip edge is judged at. */
export const NUDGE_STEP = 0.01;

/** The same press with Shift: a tenth of a second, for a miss that is obvious. */
export const COARSE_STEP = 0.1;

/** Shortest span the backend accepts; also the gap the two fields keep. */
export const MIN_SPAN = 0.3;

/** How much audio the transport plays beyond each edge when asked for context. */
export const CONTEXT_PAD = 0.5;

export function roundMs(seconds: number): number {
  return Math.round(seconds * 1000) / 1000;
}

/** `1:02:03.456` — a span is read in the same units the recording is. */
export function clock(seconds: number): string {
  const s = Math.max(0, seconds);
  const hours = Math.floor(s / 3600);
  const minutes = Math.floor((s % 3600) / 60);
  const rest = (s % 60).toFixed(3).padStart(6, "0");
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${rest}`
    : `${minutes}:${rest}`;
}

/** `12:40` — for a recording's length, where seconds are noise. */
export function clockShort(seconds: number): string {
  const s = Math.max(0, seconds);
  const hours = Math.floor(s / 3600);
  const minutes = Math.floor((s % 3600) / 60);
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}`
    : `${minutes}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
}

/** A percentage as the page prints it: whole numbers, never `86.5 %`. */
export function percent(value: number): number {
  return Math.round(value * 100);
}

/** How much of the passage was heard, in the colour that admits how much. */
export function confidenceTone(confidence: number): string {
  if (confidence >= 0.9) return "text-accent-bg";
  if (confidence >= 0.7) return "text-text-secondary";
  return "text-warning-text";
}

export function errorText(error: unknown): string {
  return typeof error === "string"
    ? error
    : error instanceof Error
      ? error.message
      : String(error);
}
