// ---------------------------------------------------------------------------
// The app's one notice channel.
//
// The shell (`app/(app)/home/page.tsx`) used to own a private `showToast`
// banner, which meant anything outside the shell — the card editor, which is a
// root overlay above it, or a page that caught a failed `invoke` — had either to
// reach through props it did not have or fall back to `alert()`. `alert()` is
// the wrong shape for an app whose errors are supposed to be actionable: it
// blocks the webview, cannot be styled with the theme, and dismisses itself on
// one click with nothing recorded.
//
// So this is a module-scope publish/subscribe store — the same shape as
// `lib/agent/ui-actions.ts` and `lib/refresh/page-refresh.tsx`, for the same
// reason: the writer and the renderer are far apart in the tree and neither
// should know the other exists. The shell renders it; anyone can post to it.
//
// One banner at a time, on purpose. A queue would let a stale failure surface
// minutes later as if it were current, and a second notice almost always
// supersedes the first.
// ---------------------------------------------------------------------------

import { useSyncExternalStore } from "react";

export type ToastTone = "info" | "error";

/** A button inside the notice. `run` fires once and the notice is dismissed. */
export type ToastAction = {
  /** The verb on the button — "Edit now", "Undo". Two words at most. */
  label: string;
  run: () => void;
};

export type Toast = {
  /** The sentence shown verbatim — write it as what to do next, not just what failed. */
  message: string;
  tone: ToastTone;
  /** An optional button, for notices that name something the reader can go do. */
  action?: ToastAction;
  /** Monotonic id, so a re-post of the same text still restarts the dismissal. */
  id: number;
};

export type NotifyOptions = {
  tone?: ToastTone;
  action?: ToastAction;
  /** Override the default lifetime, in milliseconds. */
  ms?: number;
};

/** How long a notice stays up. Long enough to read a sentence and start a fix. */
const DISMISS_MS = 4_000;
/** A notice with a button asks for a decision, so it may not vanish mid-reach. */
const DISMISS_WITH_ACTION_MS = 8_000;

let current: Toast | null = null;
let seq = 0;
let timer: ReturnType<typeof setTimeout> | null = null;
const subscribers = new Set<() => void>();

function emit(): void {
  subscribers.forEach((notifySub) => notifySub());
}

/** Show `message` in the shell's banner, replacing whatever was there. */
export function notify(
  message: string,
  toneOrOptions: ToastTone | NotifyOptions = "info"
): void {
  const opts: NotifyOptions =
    typeof toneOrOptions === "string" ? { tone: toneOrOptions } : toneOrOptions;
  current = {
    message,
    tone: opts.tone ?? "info",
    action: opts.action,
    id: ++seq,
  };
  if (timer) clearTimeout(timer);
  timer = setTimeout(
    () => {
      timer = null;
      // Drop the action with the notice: a button that outlived its sentence
      // would fire against state the reader can no longer see.
      current = null;
      emit();
    },
    opts.ms ?? (opts.action ? DISMISS_WITH_ACTION_MS : DISMISS_MS)
  );
  emit();
}

/** A failure, in the tone the theme reserves for errors. */
export function notifyError(message: string): void {
  notify(message, "error");
}

/** Dismiss now — the shell calls it when the banner's own close button is used. */
export function dismissToast(): void {
  if (timer) {
    clearTimeout(timer);
    timer = null;
  }
  current = null;
  emit();
}

function subscribe(notifySub: () => void): () => void {
  subscribers.add(notifySub);
  return () => subscribers.delete(notifySub);
}

/** The notice on screen, or null. Rendered by the shell, once. */
export function useToast(): Toast | null {
  return useSyncExternalStore(subscribe, () => current, () => null);
}
