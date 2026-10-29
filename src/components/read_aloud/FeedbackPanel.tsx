"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { AlertTriangle, Bot, Check, Copy, Loader2, RefreshCw, Sparkles } from "lucide-react";
import MarkdownViewer from "@/components/wiki/markdown/markdown";
import {
  feedbackContext,
  feedbackProvenance,
  requestFeedback,
  type FeedbackReport,
} from "@/lib/read_aloud/feedback";
import { isTauri } from "@/lib/tauri";
import { logError } from "@/lib/logger";

const LOG_MODULE = "read-aloud";

/** The three axes a report can talk about, in the reader's words. */
const AREA_LABEL: Record<string, string> = {
  accuracy: "Accuracy",
  fluency: "Fluency",
  pronunciation: "Pronunciation",
};

type Note = { kind: "info" | "good" | "warn" | "error"; text: string };

const NOTE_CLASS: Record<Note["kind"], string> = {
  info: "text-text-tertiary",
  good: "text-success-text",
  warn: "text-warning-text",
  error: "text-error-text",
};

type Props = {
  datasetUuid: string;
  attemptUuid: string;
  /** The attempt's text — named only so a wrong pairing is caught server-side. */
  textUuid?: string | null;
  /** The report already stored on this attempt, when there is one. */
  stored?: FeedbackReport | null;
  /**
   * Whether the attempt has stored metrics. Feedback explains measurements, so
   * with none there is nothing to explain — the panel says so and points at
   * "Assess again" instead of asking a model to invent advice about a blank.
   */
  measured: boolean;
  /**
   * Full path of this attempt's recording on this device. The manual "Copy
   * prompt" route cannot carry audio in the prompt, so the panel names the file
   * the learner must attach themselves; an attached agent reaches it on its own.
   */
  audioPath?: string | null;
  /**
   * Language the report is written in. The app's interface is English, so the
   * explanation is too; the text's own words stay quoted in its language
   * either way.
   */
  language?: string;
  /** Offered when nothing was measured yet. */
  onAssessAgain?: () => void;
  /** Paused while the page is saving, or another attempt is being re-assessed. */
  disabled?: boolean;
};

const smallButton =
  "inline-flex items-center gap-1 px-2 py-1 rounded-md text-[11px] bg-bg-muted hover:bg-bg-hover text-text-secondary cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed";
const primaryButton =
  "inline-flex items-center gap-1.5 px-2.5 py-1 rounded-md text-[11px] font-medium bg-accent-bg text-white hover:opacity-90 cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed";

/**
 * The feedback card for one attempt: the stored report, and the button that
 * asks an agent for one.
 *
 * A report about a reading needs the reading, and the app's text-only LLM seam
 * cannot carry audio. So "Get Help" hands the take to the agent mailbox: the
 * evidence, the exact prompt and the recording's location go out, an attached
 * agent that can *hear* the take answers, and its report lands back in this
 * card when the attempts reload (the `read-aloud-feedback-saved` event). "Copy
 * prompt" is the manual escape hatch: it copies the prompt but not the audio,
 * so the card names the recording file the learner must attach themselves.
 *
 * It is deliberately a card and not another score: nothing here changes a
 * number, awards XP, or is written anywhere but the attempt's own
 * `feedback_json` on this device.
 */
export default function FeedbackPanel({
  datasetUuid,
  attemptUuid,
  textUuid,
  stored,
  measured,
  audioPath,
  language = "en",
  onAssessAgain,
  disabled,
}: Props) {
  const [report, setReport] = useState<FeedbackReport | null>(stored ?? null);
  const [busy, setBusy] = useState<"" | "generate" | "copy">("");
  const [note, setNote] = useState<Note | null>(null);
  /** The prompt, shown only when the clipboard refused it. */
  const [prompt, setPrompt] = useState("");

  // A different attempt is a different report. Resetting on `stored` too is what
  // makes an agent's answer appear after the caller reloads the attempts.
  useEffect(() => {
    setReport(stored ?? null);
    setNote(null);
    setPrompt("");
  }, [attemptUuid, stored]);

  const args = useMemo(
    () => ({ datasetUuid, attemptUuid, textUuid: textUuid ?? null, language }),
    [datasetUuid, attemptUuid, textUuid, language]
  );

  const paused = !!disabled || busy !== "";

  const ask = useCallback(async () => {
    if (!isTauri()) return;
    setBusy("generate");
    setPrompt("");
    try {
      const receipt = await requestFeedback(args);
      // The report is not here yet — an agent has to claim the request, listen
      // to the take and write the answer back. It lands through the reload event.
      setNote({
        kind: "info",
        text: `Asked an agent to listen to this take (${receipt.subject}). Its report appears here as soon as it answers — leave the app open, or reload the attempts once it has finished.`,
      });
    } catch (e) {
      const msg = String(e);
      logError(`feedback request failed: ${msg}`, LOG_MODULE);
      setNote({ kind: "error", text: msg });
    } finally {
      setBusy("");
    }
  }, [args]);

  const copyPrompt = useCallback(async () => {
    if (!isTauri()) return;
    setBusy("copy");
    setPrompt("");
    setNote(null);
    try {
      const ctx = await feedbackContext(args);
      try {
        await navigator.clipboard.writeText(ctx.prompt);
        setNote({
          kind: "good",
          text: "Prompt copied. It carries the evidence but never the audio, so attach the recording below to any model you run it in — then save its JSON report and reload the attempts.",
        });
      } catch (e) {
        logError(`clipboard write failed: ${String(e)}`, LOG_MODULE);
        setPrompt(ctx.prompt);
        setNote({
          kind: "warn",
          text: "Could not reach the clipboard — the prompt is below, select and copy it.",
        });
      }
    } catch (e) {
      const msg = String(e);
      logError(`building the feedback prompt failed: ${msg}`, LOG_MODULE);
      setNote({ kind: "error", text: msg });
    } finally {
      setBusy("");
    }
  }, [args]);

  // The manual route needs the recording itself: copy its on-disk path so the
  // learner can attach that exact file when they send the prompt by hand.
  const copyPath = useCallback(async () => {
    if (!audioPath) return;
    try {
      await navigator.clipboard.writeText(audioPath);
      setNote({ kind: "good", text: "Recording path copied. Attach this audio file when you send the prompt yourself." });
    } catch (e) {
      logError(`copying the recording path failed: ${String(e)}`, LOG_MODULE);
      setNote({ kind: "warn", text: "Could not reach the clipboard — select the path below and copy it." });
    }
  }, [audioPath]);

  return (
    <section className="flex flex-col gap-2 p-3 rounded-xl border border-dashed border-border-default bg-bg-card min-w-0">
      <div className="flex flex-wrap items-center gap-2 min-w-0">
        <Sparkles size={16} className="text-text-tertiary shrink-0" />
        <div className="flex-1 min-w-0">
          <h3 className="text-sm font-semibold text-text-primary">Feedback</h3>
          <p className="text-[11px] text-text-tertiary break-words">
            What these numbers say, and what to do differently next take.
          </p>
        </div>

        <button
          type="button"
          className={primaryButton}
          disabled={paused || !measured}
          onClick={() => void ask()}
          title={
            measured
              ? "Ask an attached agent to listen to this take and explain what to fix"
              : "Nothing was measured for this attempt yet — assess it again first"
          }
        >
          {busy === "generate" ? <Loader2 size={12} className="animate-spin" /> : <Bot size={12} />}
          {busy === "generate" ? "Sending…" : "Get Help"}
        </button>

        <button
          type="button"
          className={smallButton}
          disabled={paused || !measured}
          onClick={() => void copyPrompt()}
          title="Copies the exact prompt the button above would send, evidence included. The audio is not in the prompt — attach the recording yourself."
        >
          {busy === "copy" ? <Loader2 size={12} className="animate-spin" /> : <Copy size={12} />}
          Copy prompt
        </button>
      </div>

      {/* The manual "Copy prompt" route cannot carry audio, so name the file the
          learner must upload alongside the copied prompt. An attached agent
          reaches this path on its own through the mailbox. */}
      {audioPath && (
        <div className="flex flex-col gap-1 min-w-0">
          <p className="text-[11px] text-text-tertiary break-words">
            Send the prompt yourself? Attach this recording — the prompt holds only the
            measurements, so a model can hear the take just by reading this file.
          </p>
          <div className="flex items-center gap-1.5 min-w-0">
            <code className="flex-1 min-w-0 px-2 py-1 rounded bg-bg-muted border border-border-default text-[11px] text-text-primary break-all select-all">
              {audioPath}
            </code>
            <button type="button" className={smallButton} onClick={() => void copyPath()} title="Copy the recording's file path">
              <Copy size={12} /> Path
            </button>
          </div>
        </div>
      )}

      {!measured && (
        <div className="flex flex-wrap items-center gap-2">
          <p className="flex-1 min-w-0 text-[11px] text-text-tertiary break-words">
            This attempt has no measurements stored (its recording may be gone from this device),
            so there is nothing to explain yet.
          </p>
          {onAssessAgain && (
            <button type="button" className={smallButton} disabled={paused} onClick={onAssessAgain}>
              <RefreshCw size={12} /> Assess again
            </button>
          )}
        </div>
      )}

      {note && (
        <p className={`flex items-start gap-1.5 text-[11px] ${NOTE_CLASS[note.kind]}`}>
          {note.kind === "good" ? (
            <Check size={12} className="mt-0.5 shrink-0" />
          ) : (
            <AlertTriangle size={12} className="mt-0.5 shrink-0" />
          )}
          <span className="break-words">{note.text}</span>
        </p>
      )}

      {prompt && (
        <textarea
          readOnly
          value={prompt}
          onFocus={(e) => e.currentTarget.select()}
          className="w-full h-40 p-2 rounded-md bg-bg-muted border border-border-default text-[11px] text-text-primary font-mono resize-y"
        />
      )}

      {report && <ReportBody report={report} />}
    </section>
  );
}

/** The report itself: provenance, the prose, then the parts a learner acts on. */
function ReportBody({ report }: { report: FeedbackReport }) {
  // A prose answer has no structure to render — the model answered in text and
  // the answer was kept rather than lost, so it is shown as what it is.
  const prose = report.format === "text";

  return (
    <div className="flex flex-col gap-2.5 min-w-0">
      <p className="text-[10px] text-text-tertiary tabular-nums">{feedbackProvenance(report)}</p>

      {report.summary &&
        (prose ? (
          <div className="text-sm text-text-primary leading-relaxed whitespace-pre-wrap break-words">
            {report.summary}
          </div>
        ) : (
          <MarkdownViewer content={report.summary} />
        ))}

      {report.diagnosis && (
        <p className="text-sm text-text-primary break-words">
          <span className="font-medium">Biggest cause: </span>
          {report.diagnosis}
        </p>
      )}

      {report.issues.length > 0 && (
        <ul className="flex flex-col gap-2">
          {report.issues.map((issue, index) => (
            <li
              key={`${issue.area}-${index}`}
              className="p-2.5 rounded-lg bg-bg-muted flex flex-col gap-1 min-w-0"
            >
              <div className="flex flex-wrap items-center gap-1.5 min-w-0">
                <span className="text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded-full bg-bg-card text-text-secondary shrink-0">
                  {AREA_LABEL[issue.area] ?? issue.area}
                </span>
                {issue.evidence && (
                  <span className="text-[11px] text-text-tertiary break-words tabular-nums">
                    {issue.evidence}
                  </span>
                )}
              </div>
              {issue.detail && (
                <p className="text-sm text-text-primary break-words">{issue.detail}</p>
              )}
              {issue.fix && (
                <p className="text-sm text-text-primary break-words">
                  <span className="font-medium text-success-text">Try this: </span>
                  {issue.fix}
                </p>
              )}
              {issue.drill && (
                <p className="text-xs text-text-secondary break-words">
                  <span className="font-medium">Drill: </span>
                  {issue.drill}
                </p>
              )}
            </li>
          ))}
        </ul>
      )}

      {report.focus_words.length > 0 && (
        <div className="flex flex-col gap-1 min-w-0">
          <h4 className="text-[11px] font-medium text-text-secondary">Words to work on</h4>
          <div className="flex flex-wrap gap-1.5">
            {report.focus_words.map((word, index) => (
              <span
                key={`${word.word}-${index}`}
                title={word.hint || undefined}
                className="inline-flex items-baseline gap-1 px-2 py-0.5 rounded-full bg-bg-muted text-xs text-text-primary"
              >
                <span className="font-medium">{word.word}</span>
                {word.ipa && <span className="text-text-tertiary">/{word.ipa}/</span>}
                {word.score != null && (
                  <span className="text-text-tertiary tabular-nums">{word.score.toFixed(2)}</span>
                )}
              </span>
            ))}
          </div>
        </div>
      )}

      {report.drill && (
        <div className="flex flex-col gap-1 min-w-0">
          <h4 className="text-[11px] font-medium text-text-secondary">Practice plan</h4>
          <MarkdownViewer content={report.drill} />
        </div>
      )}

      {report.next_try && (
        <div className="p-2.5 rounded-lg bg-bg-muted border-l-2 border-accent-bg min-w-0">
          <h4 className="text-[10px] uppercase tracking-wide text-text-tertiary">On your next take</h4>
          <p className="text-sm text-text-primary break-words">{report.next_try}</p>
        </div>
      )}
    </div>
  );
}
