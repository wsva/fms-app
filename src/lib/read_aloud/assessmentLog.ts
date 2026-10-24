import { listen } from "@tauri-apps/api/event";
import { isTauri } from "@/lib/tauri";

/**
 * Live log of one read-aloud assessment run.
 *
 * A re-assessment is slow — decode, STT, then fluency + pronunciation against
 * the standard audio — and the wait is mostly silent. The runner reports each
 * of its own steps here, and while it runs the store also mirrors the backend
 * `app-log` events so the model's own progress (model load, transcription
 * passes, phoneme decode) shows up as it happens.
 *
 * Module-level rather than component state: the attempts list triggers a run
 * from its own row button and the attempt page triggers the same run from the
 * toolbar, and both must feed the one window at the bottom of the page.
 */

export type AssessmentLogLevel = "INFO" | "WARN" | "ERROR" | "DEBUG";

export type AssessmentLogEntry = {
  id: number;
  /** Local wall-clock time of the line, `HH:MM:SS`. */
  time: string;
  level: AssessmentLogLevel;
  message: string;
};

export type AssessmentLogState = {
  entries: AssessmentLogEntry[];
  running: boolean;
  /** Frozen at the final duration once the run ends. */
  elapsedMs: number;
  /** null while running (or before any run), then whether it succeeded. */
  ok: boolean | null;
};

/** The window is a live tail, not an archive — the newest lines are the useful ones. */
const MAX_ENTRIES = 400;

const EMPTY: AssessmentLogState = { entries: [], running: false, elapsedMs: 0, ok: null };

let state: AssessmentLogState = EMPTY;
let startedAt = 0;
let seq = 0;
let ticker: ReturnType<typeof setInterval> | null = null;
let unlisten: (() => void) | null = null;

const subscribers = new Set<() => void>();

function notify() {
  for (const fn of subscribers) fn();
}

function set(patch: Partial<AssessmentLogState>) {
  state = { ...state, ...patch };
  notify();
}

function formatTime(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

function append(level: AssessmentLogLevel, message: string, time?: string) {
  const next = state.entries.concat({
    id: ++seq,
    time: time ?? formatTime(Date.now()),
    level,
    message,
  });
  // Trim from the front so the ids stay monotonic for React keys.
  if (next.length > MAX_ENTRIES) next.splice(0, next.length - MAX_ENTRIES);
  set({ entries: next });
}

/**
 * Only the lines that belong to an assessment: the read-aloud scoring modules
 * and the model runtime they drive. Warnings and errors are always kept, since
 * a failure elsewhere is exactly what a stuck run needs explained.
 */
function isRelevant(module: string, level: string): boolean {
  if (level === "WARN" || level === "ERROR") return true;
  return /read_aloud|models|phoneme|transcribe|audio/i.test(module);
}

type AppLogEntry = {
  timestamp: string;
  level: string;
  message: string;
  module: string;
};

function stopListening() {
  if (unlisten) {
    unlisten();
    unlisten = null;
  }
}

function startListening() {
  stopListening();
  if (!isTauri()) return;
  listen<AppLogEntry>("app-log", (evt) => {
    const e = evt.payload;
    if (!e || !isRelevant(e.module ?? "", e.level ?? "")) return;
    const level = (e.level === "WARN" || e.level === "ERROR" || e.level === "DEBUG"
      ? e.level
      : "INFO") as AssessmentLogLevel;
    // `2026-10-24 12:34:56.789` → `12:34:56`; the panel is a wall-clock tail.
    const time = typeof e.timestamp === "string" ? e.timestamp.slice(11, 19) : undefined;
    append(level, (e.message ?? "").trim(), time && time.length === 8 ? time : undefined);
  })
    .then((fn) => {
      unlisten = fn;
    })
    .catch(() => {
      // No event bridge (or it was torn down mid-subscribe): the runner's own
      // step lines still fill the window.
    });
}

/** Start a run: clears the previous tail and begins mirroring the backend log. */
export function beginAssessment(label: string): void {
  startedAt = Date.now();
  state = { entries: [], running: true, elapsedMs: 0, ok: null };
  notify();
  append("INFO", label);
  startListening();
  if (ticker) clearInterval(ticker);
  ticker = setInterval(() => {
    if (state.running) set({ elapsedMs: Date.now() - startedAt });
  }, 250);
}

/** One step of the run, in the runner's own words. */
export function logStep(message: string, level: AssessmentLogLevel = "INFO"): void {
  append(level, message);
}

/** Finish a run: freezes the elapsed time, stops the event bridge, and says how it ended. */
export function finishAssessment(ok: boolean, message: string): void {
  stopListening();
  if (ticker) {
    clearInterval(ticker);
    ticker = null;
  }
  set({ running: false, elapsedMs: Date.now() - startedAt, ok });
  append(ok ? "INFO" : "ERROR", message);
}

/** Drop the tail and hide the window. */
export function clearAssessmentLog(): void {
  stopListening();
  if (ticker) {
    clearInterval(ticker);
    ticker = null;
  }
  state = EMPTY;
  notify();
}

// ---------------------------------------------------------------------------
// Public API — subscription (for React components)
// ---------------------------------------------------------------------------

export function subscribe(fn: () => void): () => void {
  subscribers.add(fn);
  return () => subscribers.delete(fn);
}

export function getAssessmentLog(): AssessmentLogState {
  return state;
}
