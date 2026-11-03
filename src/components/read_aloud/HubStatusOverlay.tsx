"use client";

import { Check, Monitor } from "lucide-react";

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
  /** The escape while the call is in flight: bump the token so the late
   *  reply lands nowhere, and re-ask the choice. */
  onCancel?: () => void;
  /** Error-phase action: score the held take on this device. */
  onLocal?: () => void;
  /** Error-phase action: drop the take. */
  onDiscard?: () => void;
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
 * text with the way back to a local score one tap away.
 *
 * Same chrome as `ScoringChoiceDialog` and `ProgressOverlay`, because it sits
 * between exactly those two: the choice that started it and the result that
 * follows it.
 */
export default function HubStatusOverlay({ stage, error, onCancel, onLocal, onDiscard }: Props) {
  const sending: StepState =
    stage === "sending" ? "active" : stage === "waiting" || stage === "done" ? "done" : "pending";
  const awaiting: StepState =
    stage === "waiting" ? "active" : stage === "done" ? "done" : "pending";
  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/40 p-4">
      <div className="flex w-full max-w-sm flex-col gap-3 rounded-2xl bg-bg-card p-6 shadow-xl">
        <h3 className="text-base font-semibold text-text-primary">Scoring on the Hub</h3>
        {stage === "error" ? (
          <>
            <div className="p-3 rounded-lg bg-error-bg text-error-text text-sm break-words min-w-0">
              <div className="font-medium">Error invoking the hub</div>
              <div className="mt-1 text-xs">{error}</div>
            </div>
            {onLocal && (
              <button
                onClick={onLocal}
                className="flex items-center gap-2.5 rounded-xl bg-accent-bg/10 p-3 text-left cursor-pointer hover:bg-accent-bg/20"
              >
                <Monitor size={18} className="shrink-0 text-accent-bg" />
                <span className="text-sm font-medium text-text-primary">
                  Score it on this device instead
                </span>
              </button>
            )}
            {onDiscard && (
              <button
                onClick={onDiscard}
                className="self-center mt-1 text-xs text-text-tertiary hover:text-text-primary cursor-pointer"
              >
                Discard take
              </button>
            )}
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
            {onCancel && (
              <button
                onClick={onCancel}
                className="self-center mt-1 text-xs text-text-tertiary hover:text-text-primary cursor-pointer"
              >
                On this device instead
              </button>
            )}
          </>
        )}
      </div>
    </div>
  );
}
