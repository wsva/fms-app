"use client";

/**
 * Modal ring shown after a take ends while it is being transcribed and scored.
 *
 * The old one-line status in the scroll column was easy to miss right when the
 * reader is looking away from the screen (they just finished speaking), so this
 * dims the whole window and puts a spinning progress circle with the phase name
 * at its centre. The bar is indeterminate — transcribing and scoring have no
 * honest percentage — so the arc spins while the label inside says which phase
 * you are in.
 */
type Props = {
  /** The phase to name inside the ring, e.g. "Transcribing…". */
  label: string;
};

export default function ProgressOverlay({ label }: Props) {
  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/40">
      <div className="flex flex-col items-center rounded-2xl bg-bg-card px-8 py-6 shadow-xl">
        {/* The ring is wide on purpose: the phase name fits inside it on one
            line, which is where the eye already is when the arc spins. */}
        <div className="relative h-40 w-40">
          <svg viewBox="0 0 96 96" className="h-full w-full animate-spin">
            {/* Track + a 90° arc of accent, stroke-dasharray in pathLength units.
                bg-progress is the theme's track colour; accent-bg the active one. */}
            <circle cx="48" cy="48" r="42" fill="none" strokeWidth="4" pathLength={100} strokeDasharray="100 0" className="stroke-bg-progress" />
            <circle cx="48" cy="48" r="42" fill="none" strokeWidth="4" strokeLinecap="round" pathLength={100} strokeDasharray="25 75" className="stroke-accent-bg" />
          </svg>
          <span className="absolute inset-0 flex items-center justify-center text-sm font-medium text-text-primary whitespace-nowrap">
            {label}
          </span>
        </div>
      </div>
    </div>
  );
}
