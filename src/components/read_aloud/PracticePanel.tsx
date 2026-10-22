"use client";

import { useCallback, useEffect, useState, type CSSProperties } from "react";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { Play, RotateCcw, Wand2 } from "lucide-react";
import type { ReadAloudSubmitResult, ReadText } from "@/lib/read_aloud/types";
import { GOOD_SCORE, PASS_SCORE, scoreBadgeClasses } from "@/lib/read_aloud/types";
import { useRecorder } from "@/components/read_book/useRecorder";
import AsrDiff from "@/components/read_aloud/AsrDiff";
import { isTauri } from "@/lib/tauri";
import { logError, logInfo } from "@/lib/logger";

const LOG_MODULE = "read-aloud";

/** Recorder control surfaced to the page so the Record button can live in the
 *  toolbar while the recording/submit logic stays here. */
export type RecorderCtl = { toggle: () => void; recording: boolean; busy: boolean };

type Props = {
  datasetUuid: string;
  text: ReadText;
  /** Show the LLM-normalized reference instead of the raw text (view toggle
   *  driven from the page toolbar's overflow menu). */
  showNormalized?: boolean;
  /** `font-family` for the text to read - one of the Playwrite DE school
   *  scripts, or empty for the interface font. See src/lib/read_aloud/fonts.ts. */
  fontCss?: string;
  /** `word-spacing` added between its words - one or two of the font's own
   *  spaces, or empty for the spacing the font itself has. */
  spaceCss?: string;
  /** Called after a submission so the caller can refresh best score / counts. */
  onScored: () => void;
  /** Publishes recorder status/toggle upward so the toolbar can render the
   *  Record button; called with null on unmount. */
  onRecorder?: (ctl: RecorderCtl | null) => void;
};

function playUrl(url?: string | null) {
  if (!url) return;
  new Audio(convertFileSrc(url)).play().catch(() => {});
}

/**
 * Practice panel for a single text: record yourself reading it, transcribe via
 * STT, score against the reference, and review past attempts.
 */
export default function PracticePanel({ datasetUuid, text, showNormalized = false, fontCss = "", spaceCss = "", onScored, onRecorder }: Props) {
  const [result, setResult] = useState<ReadAloudSubmitResult | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Reset transient state whenever the selected text changes.
  useEffect(() => {
    setResult(null);
    setError(null);
  }, [text.uuid]);

  const handleResult = useCallback(
    async ({ wavBase64, text: recognized }: { wavBase64: string; text: string }) => {
      if (!isTauri()) return;
      setSubmitting(true);
      setError(null);
      try {
        const res = await invoke<ReadAloudSubmitResult>("read_aloud_submit", {
          datasetUuid,
          textUuid: text.uuid,
          wavBase64,
          recognized,
        });
        logInfo(
          `submit: score=${res.score.toFixed(1)} xp=+${res.xp_awarded} recognized="${recognized}"`,
          LOG_MODULE
        );
        setResult(res);
        onScored();
      } catch (e) {
        const msg = String(e);
        logError(`submit failed: ${msg}`, LOG_MODULE);
        setError(msg);
      } finally {
        setSubmitting(false);
      }
    },
    [datasetUuid, text.uuid, onScored]
  );

  const recorder = useRecorder(handleResult, (msg) => setError(msg));

  const busy = recorder.processing || submitting;
  const score = result?.score ?? null;

  // Surface the recorder to the page toolbar: publish the latest toggle/status
  // whenever they change, and clear it on unmount so the Record button disables.
  useEffect(() => {
    onRecorder?.({ toggle: recorder.toggle, recording: recorder.recording, busy });
  }, [onRecorder, recorder.toggle, recorder.recording, busy]);

  useEffect(() => () => onRecorder?.(null), [onRecorder]);

  return (
    <div className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden p-4 flex flex-col gap-4 min-w-0">
      {/* Reference text */}
      <section className="flex flex-col gap-1 min-w-0">
        <h2 className="text-lg font-semibold text-text-primary break-words">{text.title || "Untitled"}</h2>
        {text.note && (
          <p className="text-xs text-text-tertiary break-words">{text.note}</p>
        )}
        {showNormalized && text.normalized_content && (
          <p className="text-[11px] text-text-tertiary flex items-center gap-1">
            <Wand2 size={12} className="text-accent-bg" /> Showing the normalized reference used for
            scoring
          </p>
        )}
        {/* The one line the script and the spacing apply to: this is the text
            being read aloud, and nothing else on the page is meant to look
            like handwriting. */}
        <div
          className="ra-text mt-2 p-4 rounded-xl bg-bg-muted text-text-primary text-lg leading-relaxed whitespace-pre-wrap break-words"
          style={{ "--ra-font": fontCss || "inherit", "--ra-space": spaceCss || "normal" } as CSSProperties}
        >
          {showNormalized && text.normalized_content ? text.normalized_content : text.content}
        </div>
      </section>

      {/* Recorder status — the Record button itself lives in the page toolbar.
          Silent when idle; only surfaces an in-progress state. */}
      {(recorder.recording || recorder.processing || submitting) && (
        <p className="text-xs text-text-tertiary text-center">
          {recorder.recording
            ? "Reading… tap Stop in the toolbar when done"
            : recorder.processing
            ? "Transcribing…"
            : "Scoring…"}
        </p>
      )}

      {error && (
        <div className="p-3 rounded-lg bg-error-bg text-error-text text-sm break-words">{error}</div>
      )}

      {/* Latest result */}
      {result && (
        <section className="flex flex-col gap-2 p-4 rounded-xl border border-border-default bg-bg-card min-w-0">
          <div className="flex items-center gap-3">
            <span className={`text-2xl font-bold tabular-nums ${scoreBadgeClasses(score)}`}>
              {score != null ? `${Math.round(score)}%` : "—"}
            </span>
            <span className="text-sm text-text-secondary">
              {score != null && score >= GOOD_SCORE
                ? "Excellent read!"
                : score != null && score >= PASS_SCORE
                ? "Passed — nice work."
                : "Below 60% — give it another try."}
            </span>
            {result.xp_awarded > 0 && (
              <span className="ml-auto text-sm font-semibold text-yellow-500">
                +{result.xp_awarded} XP
              </span>
            )}
          </div>
          <div className="text-xs text-text-tertiary">
            XP: +1 at {PASS_SCORE}% or more, +1 more at {GOOD_SCORE}% or more (once per text).
          </div>
          {/* Diff of transcript vs reference (rendered from the scoring alignment) */}
          <div className="mt-1 p-2 rounded bg-bg-muted text-text-primary leading-relaxed whitespace-pre-wrap break-words">
            <AsrDiff summary={result.summary} />
          </div>
          {result.attempt.audio_url && (
            <button
              onClick={() => playUrl(result.attempt.audio_url)}
              className="mt-1 flex items-center gap-1.5 self-start text-xs text-text-secondary hover:text-text-primary cursor-pointer"
            >
              <Play size={14} /> Play your recording
            </button>
          )}
          <button
            onClick={() => setResult(null)}
            className="mt-1 flex items-center gap-1.5 self-start text-xs text-text-tertiary hover:text-text-primary cursor-pointer"
          >
            <RotateCcw size={13} /> Dismiss result
          </button>
        </section>
      )}

      {/* Attempt history lives on its own page now — reached from the toolbar's
          overflow menu ("Show Attempts"). */}
    </div>
  );
}
