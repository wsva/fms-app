/**
 * The update question, asked once and answered everywhere.
 *
 * Three surfaces read it — the About card (Settings → About and the Help >
 * About FmS dialog) and the Help menu's *Check for updates* row — and they used
 * to be one component's private state, which is why the same answer could only
 * be seen by opening Settings. A module store is what lets a menu row and a card
 * share one ask: `app_update_check` answers with a status, and the install lives
 * here too, because a download that starts from a second place would have a
 * second progress bar to get wrong.
 *
 * `describeUpdate` is the other half of "one status, one sentence". The status
 * carries a dozen fields and a given build has half of them true, so a caller
 * rendering them directly ends up with a wall of chips on one line — which is
 * what the old About block was. Here every combination collapses into one
 * sentence, at most one muted detail, and at most one action. The detail is the
 * backend's own `hint` wherever that wording is policy the UI must not
 * paraphrase (a build that cannot self-update, a dev build); everywhere else
 * there is nothing to add.
 */

import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "@/lib/platform";
import { logError } from "@/lib/logger";

/** Where a human goes for the newest installer. Mirrors `RELEASES_PAGE` in
 *  `src-tauri/src/update.rs`; that one is what the status answers with, this one
 *  is what a "download it yourself" button opens before any answer exists. */
export const RELEASES_URL = "https://github.com/wsva/fms-app/releases/latest";
export const ISSUES_URL = "https://github.com/wsva/fms-app/issues";

/** Mirrors `UpdateStatus` in `src-tauri/src/update.rs`. The version comparison
 *  and the signature check both happen in Rust: the webview only renders what
 *  `app_update_check` answered. */
export interface UpdateStatus {
  current: string;
  latest: string;
  available: boolean;
  /** Release notes out of `latest.json` — the changelog, already fetched. */
  notes: string;
  release_url: string;
  /** False when this build cannot install updates itself. */
  supported: boolean;
  /** "not_appimage" | "requires_signing" while `supported` is false. */
  unsupported_reason: string | null;
  /** True for a `tauri dev` run, where checking and installing are off. */
  dev: boolean;
  /** The next step in words, written in Rust for agents to read. */
  hint: string;
}

/** Payload of the `update://progress` event emitted while downloading. */
export interface UpdateProgress {
  downloaded: number;
  total: number | null;
  done: boolean;
}

export type UpdatePhase = "idle" | "checking" | "done" | "error";

export interface UpdateState {
  phase: UpdatePhase;
  status: UpdateStatus | null;
  installing: boolean;
  progress: { downloaded: number; total: number | null } | null;
  /** The raw `String(err)` of a failed install — the one string on this card
   *  not worth rewording, because nobody has a better sentence than the plugin's. */
  installError: string;
  /** `Date.now()` of the last answer, so a second surface can skip an ask it
   *  would make a moment after the first one. */
  checkedAt: number | null;
}

const IDLE: UpdateState = {
  phase: "idle",
  status: null,
  installing: false,
  progress: null,
  installError: "",
  checkedAt: null,
};

let state: UpdateState = IDLE;
const subscribers = new Set<() => void>();

function patch(next: Partial<UpdateState>) {
  state = { ...state, ...next };
  subscribers.forEach((fn) => fn());
}

function subscribe(fn: () => void) {
  subscribers.add(fn);
  return () => {
    subscribers.delete(fn);
  };
}

export function getUpdateState(): UpdateState {
  return state;
}

/** Read the update state. The server snapshot is the idle state: a prerender has
 *  no answer yet, and returning "checking…" there would hydrate away from it. */
export function useUpdate(): UpdateState {
  return useSyncExternalStore(subscribe, getUpdateState, () => IDLE);
}

let inFlight: Promise<void> | null = null;

/**
 * Ask the release manifest. Never downloads.
 *
 * `maxAgeMs` is for a caller that opens onto an answer someone else just fetched
 * (Help > Check for updates, then Help > About FmS a second later): it skips the
 * ask while the state is younger than that, so the card does not blink back to
 * "checking…" for an answer it can already show.
 */
export function checkForUpdates(opts: { maxAgeMs?: number } = {}): Promise<void> {
  if (inFlight) return inFlight;
  const { maxAgeMs = 0 } = opts;
  if (maxAgeMs > 0 && state.checkedAt && Date.now() - state.checkedAt < maxAgeMs) {
    return Promise.resolve();
  }
  if (!isTauri()) {
    // Nothing to ask in a browser preview — the plugin is not there at all.
    patch({ phase: "error", status: null, checkedAt: Date.now() });
    return Promise.resolve();
  }
  patch({ phase: "checking" });
  inFlight = invoke<UpdateStatus>("app_update_check")
    .then((status) => patch({ phase: "done", status, checkedAt: Date.now() }))
    .catch((err) => {
      logError(`update: check failed — ${String(err)}`, "settings");
      patch({ phase: "error", status: null, checkedAt: Date.now() });
    })
    .finally(() => {
      inFlight = null;
    });
  return inFlight;
}

let progressListener: Promise<unknown> | null = null;

/** One listener for the life of the webview. An install ends the run either way
 *  — Windows quits into the installer, Linux re-executes — so there is nothing
 *  to unlisten for and nothing to leak past a single update. */
function ensureProgressListener() {
  if (progressListener || !isTauri()) return;
  progressListener = listen<UpdateProgress>("update://progress", ({ payload }) => {
    // The last event carries done=true and means the installer is taking over,
    // which the "Installing…" state already says.
    patch({ progress: payload.done ? null : payload });
  }).catch((err) => logError(`update: progress listener failed — ${String(err)}`, "settings"));
}

/** Download and install the newest release. Only ever called from a button the
 *  user pressed: this is the one place a download starts. */
export async function installUpdate(): Promise<void> {
  if (state.installing) return;
  ensureProgressListener();
  patch({ installing: true, installError: "", progress: null });
  try {
    // Resolves on Linux (the app then re-executes) and never on Windows (the
    // installer quits the process), so a rejection is the only outcome that
    // reaches this frame.
    await invoke("app_update_install");
  } catch (err) {
    logError(`update: install failed — ${String(err)}`, "settings");
    patch({ installing: false, installError: String(err) });
  }
}

export type UpdateTone = "neutral" | "good" | "warn" | "bad";

/** The colour a tone is allowed to carry. Kept beside `describeUpdate` rather
 *  than in one of its renderers, so the About card and the Help menu's row
 *  cannot describe the same state in two colours — the same reason
 *  `stateIconClass` holds the status bar's. */
export const UPDATE_TONE_CLASS: Record<UpdateTone, string> = {
  neutral: "text-text-secondary",
  good: "text-green-500",
  warn: "text-yellow-500",
  bad: "text-red-500",
};

export type UpdateActionKind = "check" | "install" | "download";

export interface UpdateSummary {
  /** The one colour the sentence is allowed to carry. */
  tone: UpdateTone;
  /** What the About card says, in one line. */
  sentence: string;
  /** A muted second line: the backend's `hint`, or the download's own numbers. */
  detail: string | null;
  /** Short form for a menu row's trailing slot — "checking…", "v0.1.9 available". */
  short: string;
  /** The single thing a person can do about it, if anything. */
  action: { kind: UpdateActionKind; label: string } | null;
}

function formatMb(bytes: number): string {
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** Collapse every field of `UpdateState` into one sentence, one detail and one
 *  action. Everything that renders the update state goes through here, so the
 *  card and the menu cannot describe the same status differently. */
export function describeUpdate(s: UpdateState): UpdateSummary {
  if (s.installing) {
    return {
      tone: "warn",
      sentence: s.status?.latest ? `Installing v${s.status.latest}…` : "Installing the update…",
      detail: s.progress
        ? `${formatMb(s.progress.downloaded)}${
            s.progress.total ? ` of ${formatMb(s.progress.total)}` : " downloaded"
          }`
        : "Downloading and verifying the installer…",
      short: "installing…",
      action: null,
    };
  }
  if (s.phase === "checking") {
    return {
      tone: "neutral",
      sentence: "Checking for updates…",
      detail: null,
      short: "checking…",
      action: { kind: "check", label: "Checking…" },
    };
  }
  if (s.phase === "error") {
    return {
      tone: "bad",
      sentence: "Couldn't reach the release manifest.",
      detail: "Either this machine is offline, or the release server did not answer.",
      short: "check failed",
      action: { kind: "check", label: "Try again" },
    };
  }

  const st = s.status;
  if (!st) {
    return {
      tone: "neutral",
      sentence: "Not checked yet.",
      detail: null,
      short: "not checked",
      action: { kind: "check", label: "Check now" },
    };
  }
  // No self-update path on this install (deb/rpm, unsigned macOS): say what to
  // do instead rather than offer a button that cannot work. The sentence is the
  // backend's own, since it is the one place that knows the reason.
  if (!st.supported) {
    return {
      tone: "neutral",
      sentence: st.hint || "This build cannot update itself.",
      detail: null,
      short: "manual update",
      action: { kind: "download", label: "Download release" },
    };
  }
  // `available` is forced false for a dev run, so this has to come first: the
  // fact worth showing there is the published version, not "up to date".
  if (st.dev) {
    return {
      tone: "neutral",
      sentence: "Development build — updates are checked but never installed.",
      // Composed from `dev` + `latest` rather than reusing `hint`: the hint opens
      // with "Development build:" too, and this card would then say it twice.
      detail: st.latest
        ? `Newest published release: v${st.latest}.`
        : "No newer release is published.",
      short: st.latest ? `v${st.latest} published` : "dev build",
      action: { kind: "check", label: "Check again" },
    };
  }
  if (st.available) {
    return {
      tone: "warn",
      sentence: `v${st.latest} is available.`,
      // The installed version is on the card already; saying it twice would be
      // the redundancy this function exists to remove.
      detail: null,
      short: `v${st.latest} available`,
      action: { kind: "install", label: `Download & install v${st.latest}` },
    };
  }
  return {
    tone: "good",
    sentence: "You're on the newest release.",
    detail: null,
    short: "up to date",
    action: { kind: "check", label: "Check again" },
  };
}
