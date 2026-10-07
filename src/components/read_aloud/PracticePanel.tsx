"use client";

import { useCallback, useEffect, useState } from "react";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { Square, Play, Trash2, Loader2, RotateCcw } from "lucide-react";
import { MicGlyph } from "@/components/voice/MicGlyph";
import type { ReadAttempt, ReadAloudSubmitResult, ReadText } from "@/lib/read_aloud/types";
import { GOOD_SCORE, PASS_SCORE, scoreBadgeClasses } from "@/lib/read_aloud/types";
import { useRecorder } from "@/components/read_book/useRecorder";
import { highlightDifferences } from "@/components/read_book/diff";
import ConfirmDialog, { type ConfirmRequest } from "@/components/read_book/ConfirmDialog";
import { isTauri } from "@/lib/tauri";
import { logError, logInfo } from "@/lib/logger";

const LOG_MODULE = "read-aloud";

type Props = {
  datasetUuid: string;
  text: ReadText;
  /** Called after a submission so the caller can refresh best score / counts. */
  onScored: () => void;
};

function playUrl(url?: string | null) {
  if (!url) return;
  new Audio(convertFileSrc(url)).play().catch(() => {});
}

/**
 * Practice panel for a single text: record yourself reading it, transcribe via
 * STT, score against the reference, and review past attempts.
 */
export default function PracticePanel({ datasetUuid, text, onScored }: Props) {
  const [attempts, setAttempts] = useState<ReadAttempt[]>([]);
  const [result, setResult] = useState<ReadAloudSubmitResult | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);

  const loadAttempts = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const rows = await invoke<ReadAttempt[]>("read_aloud_list_attempts", {
        datasetUuid,
        textUuid: text.uuid,
      });
      setAttempts(rows);
    } catch (e) {
      logError(`list attempts failed: ${String(e)}`, LOG_MODULE);
    }
  }, [datasetUuid, text.uuid]);

  // Reset transient state and reload history whenever the selected text changes.
  useEffect(() => {
    setResult(null);
    setError(null);
    loadAttempts();
  }, [loadAttempts]);

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
        await loadAttempts();
        onScored();
      } catch (e) {
        const msg = String(e);
        logError(`submit failed: ${msg}`, LOG_MODULE);
        setError(msg);
      } finally {
        setSubmitting(false);
      }
    },
    [datasetUuid, text.uuid, loadAttempts, onScored]
  );

  const recorder = useRecorder(handleResult, (msg) => setError(msg));

  const deleteAttempt = (a: ReadAttempt) => {
    setConfirm({
      title: "Delete attempt",
      message: "Delete this recording and its score? This cannot be undone.",
      confirmLabel: "Delete",
      onConfirm: async () => {
        try {
          await invoke("read_aloud_delete_attempt", { datasetUuid, uuid: a.uuid });
          setResult((prev) => (prev && prev.attempt.uuid === a.uuid ? null : prev));
          await loadAttempts();
          onScored();
        } catch (e) {
          setError(String(e));
        }
      },
    });
  };

  const busy = recorder.processing || submitting;
  const score = result?.score ?? null;

  return (
    <div className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden p-4 flex flex-col gap-4 min-w-0">
      {/* Reference text */}
      <section className="flex flex-col gap-1 min-w-0">
        <h2 className="text-lg font-semibold text-text-primary break-words">{text.title || "Untitled"}</h2>
        {text.note && (
          <p className="text-xs text-text-tertiary break-words">{text.note}</p>
        )}
        <div className="mt-2 p-4 rounded-xl bg-bg-muted text-text-primary text-lg leading-relaxed whitespace-pre-wrap break-words">
          {text.content}
        </div>
      </section>

      {/* Recorder */}
      <section className="flex flex-col items-center gap-3">
        <button
          onClick={recorder.toggle}
          disabled={busy}
          title={recorder.recording ? "Stop recording" : "Record yourself reading"}
          className={`flex items-center justify-center gap-2 rounded-full w-20 h-20 shrink-0 transition-colors cursor-pointer disabled:cursor-not-allowed disabled:opacity-60 ${
            recorder.recording
              ? "bg-red-500 text-white animate-pulse"
              : "bg-accent-bg text-white hover:bg-accent-bg-hover"
          }`}
        >
          {busy ? (
            <Loader2 size={28} className="animate-spin" />
          ) : recorder.recording ? (
            <Square size={26} />
          ) : (
            <MicGlyph size={30} />
          )}
        </button>
        <p className="text-xs text-text-tertiary text-center">
          {recorder.recording
            ? "Reading… tap to stop"
            : recorder.processing
            ? "Transcribing…"
            : submitting
            ? "Scoring…"
            : "Tap the mic and read the text aloud"}
        </p>
      </section>

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
          {/* Diff of transcript vs reference */}
          <div className="mt-1 p-2 rounded bg-bg-muted text-text-primary leading-relaxed whitespace-pre-wrap break-words">
            {highlightDifferences(text.content, result.attempt.recognized)}
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

      {/* Attempt history */}
      <section className="flex flex-col gap-2 min-w-0">
        <h3 className="text-sm font-medium text-text-secondary">
          Attempts ({attempts.length})
        </h3>
        {attempts.length === 0 ? (
          <p className="text-xs text-text-tertiary">No attempts yet.</p>
        ) : (
          <ul className="flex flex-col gap-1.5">
            {attempts.map((a) => (
              <li
                key={a.uuid}
                className="flex items-center gap-2 p-2 rounded-lg border border-border-default bg-bg-card"
              >
                <span className={scoreBadgeClasses(a.score)}>{Math.round(a.score)}%</span>
                <span className="text-xs text-text-secondary truncate flex-1 min-w-0" title={a.recognized}>
                  {a.recognized || "(no speech recognized)"}
                </span>
                {a.audio_url && (
                  <button
                    onClick={() => playUrl(a.audio_url)}
                    title="Play recording"
                    className="p-1 rounded text-text-tertiary hover:text-text-primary hover:bg-bg-hover cursor-pointer shrink-0"
                  >
                    <Play size={15} />
                  </button>
                )}
                <button
                  onClick={() => deleteAttempt(a)}
                  title="Delete attempt"
                  className="p-1 rounded text-text-tertiary hover:text-error-text hover:bg-bg-hover cursor-pointer shrink-0"
                >
                  <Trash2 size={15} />
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>

      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}
