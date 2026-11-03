"use client";

import { Monitor, Server } from "lucide-react";

/**
 * The take-ends choice on a follower: where should this recording be scored?
 *
 * The hub machine holds the full models (the pronunciation axis in particular)
 * and is usually faster, but the phone can do everything itself — and the
 * phone stays the author either way, because the hub call persists nothing
 * (docs/design/hub-mcp-compute-offload.md). So the choice is the learner's,
 * asked per take and answered by one tap; nothing here decides or remembers.
 *
 * The third line is not an exit from the take but from the question: the
 * recording is already saved by the time this dialog appears, so "Not now"
 * keeps it and leaves it unmeasured until "Assess again" is pressed on its row.
 *
 * The chrome matches `ProgressOverlay`: it appears exactly where that ring
 * would have, dimming the same window.
 */
type Props = {
  onHub: () => void;
  onLocal: () => void;
  /** Leave the take unscored for now. Nothing is thrown away — the attempt is
   *  already stored, only its numbers are waiting. */
  onLater: () => void;
};

export default function ScoringChoiceDialog({ onHub, onLocal, onLater }: Props) {
  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/40 p-4">
      <div className="flex w-full max-w-sm flex-col gap-3 rounded-2xl bg-bg-card p-6 shadow-xl">
        <h3 className="text-base font-semibold text-text-primary">
          Where should we score this take?
        </h3>
        <button
          onClick={onHub}
          className="flex items-start gap-3 rounded-xl bg-accent-bg/10 p-3 text-left cursor-pointer hover:bg-accent-bg/20"
        >
          <Server size={18} className="mt-0.5 shrink-0 text-accent-bg" />
          <span className="flex flex-col gap-0.5 min-w-0">
            <span className="text-sm font-medium text-text-primary">On the Hub</span>
            <span className="text-xs text-text-secondary">
              the hub machine holds the full models — pronunciation included — and usually
              scores faster
            </span>
          </span>
        </button>
        <button
          onClick={onLocal}
          className="flex items-start gap-3 rounded-xl bg-bg-muted p-3 text-left cursor-pointer hover:bg-bg-progress"
        >
          <Monitor size={18} className="mt-0.5 shrink-0 text-text-secondary" />
          <span className="flex flex-col gap-0.5 min-w-0">
            <span className="text-sm font-medium text-text-primary">On this device</span>
            <span className="text-xs text-text-secondary">
              transcribe and score here, exactly as before — no network needed
            </span>
          </span>
        </button>
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
