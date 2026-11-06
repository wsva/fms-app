"use client";

import { Check } from "lucide-react";

/** The phases a hub scoring call passes through, as the follower sees them.
 *  `sending` covers the connect + handshake + upload the client starts;
 *  `waiting` arrives from the backend's `hub-mcp-phase` event once the tool
 *  call is dispatched and only the hub's answer remains; `done` names the
 *  reply received (the attempt is still being saved on this device);
 *  `error` holds the failure text in the dialog instead of a toast. */
export type HubStage = "sending" | "waiting" | "error" | "done";

type Props = {
  stage: HubStage;
  /** The hub's error text, shown when `stage === "error"` (already actionable —
   *  e.g. "no hub configured — set the PC address on Devices & Hub"). */
  error?: string | null;
  /** The heading. Defaults to the scoring wording the read-aloud panels have
   *  always shown; another hub-backed feature names its own work here (the OCR
   *  page says "Working with the Hub" / "Sending to the Hub") so the dialog
   *  never claims to be scoring something. */
  title?: string;
  /** What the failed call says about the work left behind, shown under the
   *  error. Defaults to the scoring answer (the take is already saved, so
   *  "Assess again" finishes it later); a feature with nothing persisted says
   *  so itself rather than promising a retry it does not have. */
  errorHint?: string;
  /** The one way out, in every phase: stop watching this call and leave the take
   *  as it is. The recording was stored before the call started, so walking away
   *  costs nothing — measuring it later is the attempt row's "Assess again". */
  onLater: () => void;
};

type StepState = "pending" | "active" | "done";

/** One milestone line: a dot while pending, a spinning arc while active,
 *  a green check when passed — the ring of `ProgressOverlay` at list scale. */
function Step({ state, label }: { state: StepState; label: string }) {
  return (
    <div className="flex items-center gap-2.5">
      <span className="flex h-5 w-5 shrink-0 items-center justify-center">
        {state === "active" ? (
          <svg viewBox="0 0 96 96" className="h-5 w-5 animate-spin">
            <circle cx="48" cy="48" r="42" fill="none" strokeWidth="8" pathLength={100} strokeDasharray="100 0" className="stroke-bg-progress" />
            <circle cx="48" cy="48" r="42" fill="none" strokeWidth="8" strokeLinecap="round" pathLength={100} strokeDasharray="25 75" className="stroke-accent-bg" />
          </svg>
        ) : state === "done" ? (
          <Check size={16} className="text-success-text" />
        ) : (
          <span className="h-2 w-2 rounded-full bg-bg-progress" />
        )}
      </span>
      <span
        className={`text-sm ${
          state === "pending" ? "text-text-tertiary" : "text-text-primary"
        }`}
      >
        {label}
        {state === "active" && "…"}
      </span>
    </div>
  );
}

/**
 * The hub call's status dialog on a follower: it stays open across the whole
 * remote scoring, naming the phase the call is in — sending, waiting for the
 * hub's answer, complete — and, when the call fails, holding the hub's error
 * text.
 *
 * It offers exactly one exit, "Not now", because there is nothing left to
 * protect: the take was saved before this call started, so cancelling, failing
 * or simply closing the app leaves a recorded — if unmeasured — attempt, and
 * "Assess again" on its row finishes the job whenever the learner wants.
 * Scoring here or on this device is chosen one dialog earlier, not from here.
 *
 * Same chrome as `ScoringChoiceDialog` and `ProgressOverlay`, because it sits
 * between exactly those two: the choice that started it and the result that
 * follows it.
 */
export default function HubStatusOverlay({
  stage,
  error,
  title = "Scoring on the Hub",
  errorHint = "Your recording is saved. Score it later with “Assess again” on its attempt.",
  onLater,
}: Props) {
  const sending: StepState =
    stage === "sending" ? "active" : stage === "waiting" || stage === "done" ? "done" : "pending";
  const awaiting: StepState =
    stage === "waiting" ? "active" : stage === "done" ? "done" : "pending";
  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/40 p-4">
      <div className="flex w-full max-w-sm flex-col gap-3 rounded-2xl bg-bg-card p-6 shadow-xl">
        <h3 className="text-base font-semibold text-text-primary">{title}</h3>
        {stage === "error" ? (
          <>
            <div className="p-3 rounded-lg bg-error-bg text-error-text text-sm break-words min-w-0">
              <div className="font-medium">Error invoking the hub</div>
              <div className="mt-1 text-xs">{error}</div>
            </div>
            <p className="text-xs text-text-tertiary">{errorHint}</p>
          </>
        ) : (
          <>
            <Step state={sending} label="Sending to the hub" />
            <Step state={awaiting} label="Waiting for the hub response" />
            {stage === "done" && (
              <div className="flex items-center gap-2.5">
                <span className="flex h-5 w-5 shrink-0 items-center justify-center">
                  <Check size={16} className="text-success-text" />
                </span>
                <span className="text-sm font-medium text-success-text">
                  Complete — saving the result
                </span>
              </div>
            )}
          </>
        )}
        <button
          onClick={onLater}
          className="self-center mt-1 text-xs text-text-tertiary hover:text-text-primary cursor-pointer"
        >
          Not now
        </button>
      </div>
    </div>
  );
}
