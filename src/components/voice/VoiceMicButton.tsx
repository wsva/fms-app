"use client";

/**
 * The microphone toggle, defined once.
 *
 * Dictation used to spell "stop recording" three different ways — an X in the
 * cue rail, a square in the chat composer, a crossed-out mic next to a "Stop"
 * label in the reading drawer — and the cue rail's X sat two buttons away from
 * the *close editor* X, so the same glyph meant two unrelated things in one
 * row. The states now live here:
 *
 *  idle        microphone on a quiet ghost button
 *  recording   the SAME microphone, tinted with the app's own error tokens and
 *              marked by a pulsing dot — the button itself never pulses, so
 *              pressing it neither swaps box models nor blinks like an alarm
 *  processing  the spinner every other busy control in this app already uses
 *              (OcrPage, TtsPage, StageRow, BookAdvanced)
 *
 * The tint uses `bg-error-bg`/`text-error-text` rather than HeroUI's `danger`
 * variant on purpose: those are the app's theme variables, redefined by every
 * `data-theme` block in globals.css, so Solarized and Gruvbox get an
 * on-palette recording state instead of HeroUI's default red. HeroUI ships its
 * component rules in `@layer components`, so these utilities do override the
 * button's own `background-color: var(--button-bg)`.
 *
 * Deliberately a controlled component: the dictation rail is driven by the
 * module-level voice store, while the chat composer keeps its own local state
 * because it appends the transcript to its own input instead of the focused
 * field. Both pass `state` in, so the meaning of a state cannot drift.
 */

import { Loader2, Mic } from "lucide-react";
import { Button, Tooltip } from "@heroui/react";
import type { VoiceState } from "@/lib/voice-input";

/** Where the button lives: the compact cue rail, or the chat input row. */
export type VoiceSurface = "rail" | "composer";

type Props = {
  state: VoiceState;
  onPress: () => void;
  surface?: VoiceSurface;
  /** Extra reason the toggle is unavailable (e.g. no chat model selected). */
  disabled?: boolean;
  /** Override the idle hint where the Ctrl+C shortcut is not the entry point. */
  idleHint?: string;
};

const DEFAULT_IDLE_HINT = "Dictate with the microphone — shortcut: Ctrl+C (no selection)";

/** The recording tint, identical on both surfaces so they cannot drift apart. */
const RECORDING_CLASSES = "bg-error-bg text-error-text ring-1 ring-error-text/40";

function viewOf(state: VoiceState, idleHint: string): { label: string; hint: string } {
  switch (state) {
    case "recording":
      return {
        label: "Stop dictating",
        hint: "Stop dictating — the microphone is live. Shortcut: Ctrl+C (no selection)",
      };
    case "processing":
      return { label: "Recognising\u2026", hint: "Recognising\u2026 the audio with the loaded STT model" };
    default:
      return { label: "Voice input", hint: idleHint };
  }
}

function Glyph({ state, size }: { state: VoiceState; size: number }) {
  if (state === "processing") return <Loader2 size={size} className="animate-spin" />;
  return (
    <span className="relative inline-flex items-center justify-center">
      <Mic size={size} />
      {state === "recording" && (
        <span
          aria-hidden
          className="absolute -right-1 -top-1 h-1.5 w-1.5 rounded-full bg-error-text animate-pulse"
        />
      )}
    </span>
  );
}

export function VoiceMicButton({
  state,
  onPress,
  surface = "rail",
  disabled = false,
  idleHint = DEFAULT_IDLE_HINT,
}: Props) {
  const view = viewOf(state, idleHint);
  const recording = state === "recording";
  const busy = state === "processing" || disabled;

  if (surface === "composer") {
    return (
      <button
        type="button"
        aria-label={view.label}
        title={view.hint}
        disabled={busy}
        onClick={onPress}
        className={`p-2.5 rounded-lg cursor-pointer transition-colors disabled:cursor-wait disabled:opacity-60 ${
          recording ? RECORDING_CLASSES : "bg-bg-muted text-text-secondary enabled:hover:bg-bg-hover"
        }`}
      >
        <Glyph state={state} size={18} />
      </button>
    );
  }

  return (
    <Tooltip>
      <Tooltip.Trigger>
        <Button
          isIconOnly
          variant="ghost"
          size="sm"
          aria-label={view.label}
          isDisabled={busy}
          onPress={onPress}
          className={recording ? RECORDING_CLASSES : ""}
        >
          <Glyph state={state} size={14} />
        </Button>
      </Tooltip.Trigger>
      <Tooltip.Content>{view.hint}</Tooltip.Content>
    </Tooltip>
  );
}
