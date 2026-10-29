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

/**
 * `00:12:34.567` — the transcript table's clock, and the edge fields'.
 *
 * Hours are always present, so the two ends of a row can be compared by eye
 * without counting colons, and the column does not change width at the
 * hour mark. The edge fields print this because a reader places a text by
 * the clock the recording is read in, not by seconds from zero.
 */
export function clockHms(seconds: number): string {
  const s = Math.max(0, seconds);
  const hours = Math.floor(s / 3600);
  const minutes = Math.floor((s % 3600) / 60);
  const rest = (s % 60).toFixed(3).padStart(6, "0");
  return `${String(hours).padStart(2, "0")}:${String(minutes).padStart(2, "0")}:${rest}`;
}

/**
 * `00:12:34.567` back to seconds — the edge fields' parser, and the inverse of
 * [`clockHms`].
 *
 * Tolerant on purpose, because the field is typed into: a bare number is seconds
 * (`12.5`), one colon is `mm:ss`, two are `hh:mm:ss`, a comma works as the
 * decimal point, and a component past 60 rolls over instead of being refused.
 * Anything that does not parse at all is `null` — the caller keeps the value it
 * had rather than storing a guess.
 */
export function parseClock(text: string): number | null {
  const parts = text.trim().replace(",", ".").split(":");
  if (parts.length > 3) return null;
  const numbers = parts.map((part) => (part.trim() === "" ? NaN : Number(part)));
  if (numbers.some((n) => !Number.isFinite(n) || n < 0)) return null;
  // The parts are positional: `h:mm:ss` folds to `h * 3600 + mm * 60 + ss`.
  return roundMs(numbers.reduce((total, n) => total * 60 + n, 0));
}

/** A percentage as the page prints it: whole numbers, never `86.5 %`. */
export function percent(value: number): number {
  return Math.round(value * 100);
}

/** How much of the text was heard, in the colour that admits how much. */
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
