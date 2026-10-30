"use client";

import { Loader2, Square } from "lucide-react";
import { tbBase, tbBaseStacked, TB_ICON } from "@/lib/ui/toolbar";
import { MicGlyph } from "@/components/voice/MicGlyph";
import { useToolbarMobile } from "./mobile-context";
import { useIsMobile } from "@/hooks/useIsMobile";

type RecordButtonProps = {
  /** Actively recording — solid red, pulsing, label reads "Stop". */
  recording?: boolean;
  /** Transient work (encoding/scoring) — spinner, label reads "Working…". */
  busy?: boolean;
  disabled?: boolean;
  title?: string;
  className?: string;
  onClick?: () => void;
};

/**
 * The read-aloud Record / Stop control. Encapsulates the one solid-red state in
 * the toolbar (recording) so pages no longer hand-roll the base class string.
 * Idle uses the accent CTA color; recording pulses red; busy shows a spinner.
 *
 * The label always renders: on a phone it uses the kit's stacked layout (icon
 * over a `text-xs` label) like every other labeled toolbar button, so the one
 * primary action on the page is never reduced to a bare microphone glyph.
 */
export function RecordButton({
  recording = false,
  busy = false,
  disabled = false,
  title,
  className = "",
  onClick,
}: RecordButtonProps) {
  const ctx = useToolbarMobile();
  const detected = useIsMobile();
  const mobile = ctx ?? detected;

  const color = recording
    ? "bg-red-500 text-white animate-pulse"
    : "bg-accent text-white hover:bg-accent/90";
  const label = recording ? "Stop" : busy ? "Working…" : "Record";
  const box = mobile ? tbBaseStacked : tbBase;

  return (
    <button
      type="button"
      className={`${box} ${color} ${className}`.trim()}
      disabled={disabled}
      title={title ?? (recording ? "Stop recording" : "Record yourself reading")}
      aria-label={label}
      onClick={onClick}
    >
      {busy ? (
        <Loader2 size={TB_ICON} className="animate-spin" />
      ) : recording ? (
        <Square size={TB_ICON} />
      ) : (
        <MicGlyph size={TB_ICON} />
      )}
      {label}
    </button>
  );
}

export default RecordButton;
