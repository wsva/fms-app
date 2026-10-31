"use client";

// ---------------------------------------------------------------------------
// "Hear it" for one field of a card.
//
// The reading half of a card can be a paragraph of a language you are learning,
// and the one thing a paper card cannot do is say it. This is the button over
// the machinery that already exists: `lib/edge_tts/speak` owns a single
// app-wide audio element, a session blob cache, and the backend's voice
// resolution (the deck's `info.language` picks the voice, so a German deck is
// read in German without a picker here).
//
// Two things this button must not get wrong:
//   - it may only say "playing" while *its* utterance plays, so it listens for
//     a global stop — its own end, or another card taking over;
//   - it speaks prose, not markup: `stripAnchors` drops `{{handle}}` tokens
//     (they are filenames) and `stripMarkdown` in `speakText` drops the rest.
// ---------------------------------------------------------------------------

import { useEffect, useState } from "react";
import { Volume2, Square, Loader2 } from "lucide-react";
import { speakText, stopSpeech, onSpeechStop } from "@/lib/edge_tts/speak";
import { stripAnchors } from "@/lib/media/anchors";

type Props = {
  /** Raw field text, anchors included; Markdown is stripped before synthesis. */
  text: string;
  /** BCP 47 primary subtag from the deck; "" = the app's preferred voice. */
  language: string;
  /** The field's name, for the title and accessible name. */
  label: string;
};

export default function SpeakButton({ text, language, label }: Props) {
  const [playing, setPlaying] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  // Any global stop (this utterance ended, or another replaced it) resets us,
  // or the button would claim to be playing after the audio has stopped.
  useEffect(() => onSpeechStop(() => setPlaying(false)), []);

  const plain = stripAnchors(text).trim();

  const toggle = async () => {
    if (playing) {
      stopSpeech();
      setPlaying(false);
      return;
    }
    if (!plain || loading) return;
    setLoading(true);
    setError("");
    try {
      await speakText(plain, language);
      setPlaying(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  return (
    <button
      type="button"
      onClick={(e) => {
        // The header row is outside the field's click surface, but a host may
        // later put this inside one; speaking is never "open the editor".
        e.stopPropagation();
        void toggle();
      }}
      disabled={!playing && loading}
      title={error || (playing ? "Stop" : `Read ${label.toLowerCase()} aloud`)}
      aria-label={playing ? "Stop reading" : `Read ${label.toLowerCase()} aloud`}
      className={`flex h-7 w-7 items-center justify-center rounded-md disabled:cursor-wait ${
        error
          ? "text-error-text hover:bg-bg-hover"
          : "text-text-tertiary hover:bg-bg-hover hover:text-text-primary"
      }`}
    >
      {loading ? (
        <Loader2 size={13} className="animate-spin" />
      ) : playing ? (
        <Square size={13} />
      ) : (
        <Volume2 size={13} />
      )}
    </button>
  );
}
