"use client";

import type { Assessment, Pause, PronUnavailableReason, RateBand } from "@/lib/read_aloud/types";
import { GOOD_SCORE, PASS_SCORE } from "@/lib/read_aloud/types";

/**
 * The three-axis result of one take: accuracy (the unchanged WIP score),
 * fluency (VAD + word timings) and pronunciation (CTC phoneme GOP).
 *
 * Each axis is rendered independently and may say "not measured" — an absent
 * axis is never replaced by a proxy number. Used by the practice view right
 * after submitting, and by the attempts view from the stored `metrics`.
 */
export default function AssessmentCards({ assessment }: { assessment: Assessment }) {
  const { accuracy, fluency, pronunciation } = assessment;

  return (
    <div className="grid grid-cols-1 sm:grid-cols-3 gap-2 min-w-0">
      {/* Accuracy — the number XP and best_score have always used. */}
      <Card title="Accuracy" hint={`${accuracy.counts.ref} words in the text`}>
        <div className="flex items-baseline gap-1">
          <span className="text-2xl font-bold tabular-nums text-text-primary">
            {Math.round(accuracy.score)}%
          </span>
          {accuracy.lenient_score > accuracy.score && (
            <span
              className="text-[10px] text-text-tertiary tabular-nums"
              title="The same alignment counting near-miss (homophone, one-letter) substitutions as correct"
            >
              {Math.round(accuracy.lenient_score)}% lenient
            </span>
          )}
        </div>
        <p className="text-[11px] text-text-tertiary tabular-nums">
          {accuracy.counts.correct} correct · {accuracy.counts.minor} near-miss ·{" "}
          {accuracy.counts.substituted} wrong · {accuracy.counts.omitted} missed ·{" "}
          {accuracy.counts.inserted} extra · {Math.round(accuracy.wer * 100)}% WER
        </p>
      </Card>

      {/* Fluency — needs per-word timings from the STT model. */}
      <Card title="Fluency" hint="paces pauses from the waveform, not from the transcript">
        {fluency ? (
          <>
            <div className="flex items-baseline gap-1.5">
              <span className="text-2xl font-bold tabular-nums text-text-primary">
                {Math.round(fluency.wcpm)}
              </span>
              <span className="text-[11px] text-text-tertiary">correct words/min</span>
              <BandChip band={fluency.band} />
            </div>
            <p className="text-[11px] text-text-tertiary tabular-nums">
              {Math.round(fluency.articulation_rate)} wpm speaking · {fluency.pause_count} pause
              {fluency.pause_count === 1 ? "" : "s"}
              {fluency.long_pause_count > 0 ? ` (${fluency.long_pause_count} long)` : ""} ·{" "}
              {fluency.mean_length_of_run.toFixed(1)} words between pauses
            </p>
            {(fluency.repetition_count > 0 || fluency.restart_count > 0) && (
              <p className="text-[11px] text-text-tertiary tabular-nums">
                {fluency.repetition_count} repeated word
                {fluency.repetition_count === 1 ? "" : "s"} · {fluency.restart_count} restart
                {fluency.restart_count === 1 ? "" : "s"}
              </p>
            )}
          </>
        ) : (
          <p className="text-[11px] text-text-tertiary">
            Not measured — the loaded speech model reports no word timings. Load a
            Parakeet model on the Models page, then recognize the take again.
          </p>
        )}
      </Card>

      {/* Pronunciation — a real GOP score, or an honest reason why not. */}
      <Card title="Pronunciation" hint="phoneme accuracy against a synthesized reference">
        {pronunciation.score != null ? (
          <>
            <div className="flex items-baseline gap-1">
              <span className="text-2xl font-bold tabular-nums text-text-primary">
                {Math.round(pronunciation.score * 100)}%
              </span>
              {pronunciation.weak_ratio != null && (
                <span className="text-[10px] text-text-tertiary tabular-nums">
                  {Math.round(pronunciation.weak_ratio * 100)}% of words weak
                </span>
              )}
            </div>
            <WeakWords words={assessment.words.filter((w) => (w.pron ?? 1) < 0.4).slice(0, 3)} />
          </>
        ) : (
          <p className="text-[11px] text-text-tertiary">
            Not measured — {describeReason(pronunciation.unavailable_reason)}
          </p>
        )}
      </Card>
    </div>
  );
}

export type ScoreAxesSize = "sm" | "md";

/**
 * The three scores of one take as a single strip: accuracy, fluency,
 * pronunciation — the same three [`AssessmentCards`] breaks down.
 *
 * The attempts list shows it on every card, so a take is scanned by its three
 * numbers instead of only its accuracy; an attempt with no stored assessment
 * shows what it can and reads "—" for the rest, never a substitute number.
 * Tooltips carry the detail the cards carry at full size.
 */
export function ScoreAxes({
  score,
  metrics,
  size = "sm",
  className = "",
}: {
  /** Accuracy 0–100 — the one score every attempt has. */
  score: number;
  /** Absent for a take recorded before assessment existed, or whose recording
   *  is gone. */
  metrics?: Assessment | null;
  size?: ScoreAxesSize;
  className?: string;
}) {
  const accuracy = metrics?.accuracy;
  const fluency = metrics?.fluency ?? null;
  const pronunciation = metrics?.pronunciation ?? null;
  const pronPercent = pronunciation?.score != null ? pronunciation.score * 100 : null;
  // A take with no assessment at all has no per-axis reason to give, only the
  // way to measure one.
  const unmeasured =
    "Not measured — this take has no stored assessment (it predates the three-axis " +
    "score, or its recording is gone). Use Assess again to measure it.";

  const axes: ScoreAxis[] = [
    {
      key: "accuracy",
      label: "Accuracy",
      value: String(Math.round(score)),
      unit: "%",
      tone: percentTone(score),
      title: accuracy
        ? `${accuracy.counts.correct} of ${accuracy.counts.ref} words correct · ${Math.round(
            accuracy.wer * 100
          )}% WER${accuracy.lenient_score > accuracy.score ? ` · ${Math.round(accuracy.lenient_score)}% lenient` : ""}`
        : "Share of the text's words the transcript got right.",
    },
    {
      key: "fluency",
      label: "Fluency",
      value: fluency ? String(Math.round(fluency.wcpm)) : "—",
      unit: fluency ? "wcpm" : undefined,
      tone: fluency ? bandTone(fluency.band) : "text-text-tertiary",
      title: fluency
        ? `${Math.round(fluency.wcpm)} correct words per minute (${fluency.band} pace) · ${fluency.pause_count} pause${fluency.pause_count === 1 ? "" : "s"} · reading ${Math.round(fluency.speaking_time)} s of ${Math.round(fluency.duration)} s`
        : unmeasured,
    },
    {
      key: "pronunciation",
      label: "Pronunciation",
      value: pronPercent != null ? String(Math.round(pronPercent)) : "—",
      unit: pronPercent != null ? "%" : undefined,
      tone: percentTone(pronPercent),
      title:
        pronPercent != null
          ? `Phoneme accuracy against the text's standard audio · ${Math.round(
              (pronunciation?.weak_ratio ?? 0) * 100
            )}% of words weak`
          : pronunciation?.unavailable_reason
          ? `Not measured — ${describeReason(pronunciation.unavailable_reason)}`
          : unmeasured,
    },
  ];

  return (
    <div className={`grid grid-cols-3 gap-1.5 min-w-0 ${className}`}>
      {axes.map((axis) => (
        <div
          key={axis.key}
          className="flex flex-col gap-0.5 px-2 py-1.5 rounded-lg bg-bg-muted min-w-0"
          title={axis.title}
        >
          <span className="text-[10px] font-semibold uppercase tracking-wide text-text-tertiary truncate">
            {axis.label}
          </span>
          <span className="flex items-baseline gap-0.5 min-w-0">
            <span
              className={`font-semibold tabular-nums leading-none ${axis.tone} ${
                size === "md" ? "text-2xl" : "text-lg"
              }`}
            >
              {axis.value}
            </span>
            {axis.unit && (
              <span className="text-[10px] text-text-tertiary tabular-nums">{axis.unit}</span>
            )}
          </span>
        </div>
      ))}
    </div>
  );
}

type ScoreAxis = {
  key: string;
  label: string;
  /** Formatted value, or "—" when the axis was not measured. */
  value: string;
  unit?: string;
  tone: string;
  title: string;
};

/** Number colour for a 0–100 score: the badge thresholds, as text. */
function percentTone(value: number | null): string {
  if (value == null) return "text-text-tertiary";
  if (value >= GOOD_SCORE) return "text-success-text";
  if (value >= PASS_SCORE) return "text-warning-text";
  return "text-error-text";
}

/** Number colour for a reading pace: on target is the good news. */
function bandTone(band: RateBand): string {
  return band === "target"
    ? "text-success-text"
    : band === "slow"
    ? "text-warning-text"
    : "text-text-secondary";
}

/**
 * The pauses of one attempt as a ribbon across the recording: silence at
 * punctuation is grey, a mid-phrase hesitation is amber (it is the hesitancy
 * signal), and leading/trailing silence is not drawn at all — it is not part of
 * the reading.
 */
export function PauseRibbon({ pauses, duration }: { pauses: Pause[]; duration: number }) {
  const reading = pauses.filter((p) => p.kind === "at_punctuation" || p.kind === "within_phrase");
  if (reading.length === 0 || !(duration > 0)) return null;
  return (
    <div className="flex flex-col gap-1 min-w-0">
      <div className="relative h-2 rounded-full bg-bg-muted overflow-hidden">
        {reading.map((p, i) => {
          const left = Math.max(0, Math.min(100, (p.start / duration) * 100));
          const width = Math.max(
            0.6,
            Math.min(100 - left, ((p.end - p.start) / duration) * 100)
          );
          return (
            <span
              key={i}
              className={`absolute top-0 h-full rounded-sm ${
                p.kind === "within_phrase" ? "bg-yellow-400/70" : "bg-text-tertiary/40"
              }`}
              style={{ left: `${left}%`, width: `${width}%` }}
              title={`${p.kind === "within_phrase" ? "Hesitation" : "At punctuation"} — ${(
                p.end - p.start
              ).toFixed(2)} s`}
            />
          );
        })}
      </div>
      <p className="text-[10px] text-text-tertiary">
        Pauses while reading · amber = mid-phrase hesitation · grey = at punctuation
      </p>
    </div>
  );
}

function Card({
  title,
  hint,
  children,
}: {
  title: string;
  hint: string;
  children: React.ReactNode;
}) {
  return (
    <div
      className="flex flex-col gap-1 p-3 rounded-xl border border-border-default bg-bg-card min-w-0"
      title={hint}
    >
      <h3 className="text-[11px] font-semibold uppercase tracking-wide text-text-secondary">
        {title}
      </h3>
      {children}
    </div>
  );
}

function BandChip({ band }: { band: RateBand }) {
  const classes =
    band === "target"
      ? "bg-success-bg text-success-text"
      : band === "slow"
      ? "bg-yellow-400/25 text-text-primary"
      : "bg-bg-hover text-text-secondary";
  return (
    <span className={`ml-auto text-[10px] px-1.5 py-0.5 rounded-full font-semibold ${classes}`}>
      {band}
    </span>
  );
}

function WeakWords({ words }: { words: Assessment["words"] }) {
  if (words.length === 0) {
    return <p className="text-[11px] text-text-tertiary">No weak words.</p>;
  }
  return (
    <p className="text-[11px] text-text-tertiary break-words">
      Weakest:{" "}
      {words.map((w, i) => (
        <span key={`${w.ref_index ?? i}`} className="text-text-secondary">
          {i > 0 ? ", " : ""}
          {w.ref}
          <span className="text-text-tertiary tabular-nums">
            {" "}
            ({Math.round((w.pron ?? 0) * 100)}%)
          </span>
        </span>
      ))}
    </p>
  );
}

/** Turn a first-class reason into the sentence a learner can act on. */
function describeReason(reason: PronUnavailableReason | null): string {
  switch (reason) {
    case "no_reference":
      return (
        "this text has no standard audio yet, and that clip is what pronunciation " +
        "is measured against. Open Manage → Standard audio and generate one with " +
        "Edge TTS (or upload a recording), then score again."
      );
    case "stale_reference":
      return (
        "the text's standard audio was made from an earlier version of the text, so " +
        "it no longer matches the words being compared. Regenerate it in Manage → " +
        "Standard audio, then score again."
      );
    case "model_missing":
      return (
        "no pronunciation model is installed. Download it on the Models page " +
        "(Pronunciation → Wav2Vec2 Phoneme), then score again."
      );
    case "model_error":
      return "the phoneme model could not run on this recording.";
    case "no_network":
    case "tts_failed":
      return (
        "this attempt was scored by the old flow, which fetched the reference from " +
        "Edge TTS while scoring. Texts now carry their own standard audio — make " +
        "one in Manage → Standard audio and score again."
      );
    case "too_short":
      return "the attempt is too short to align against the text.";
    default:
      return "not measured.";
  }
}
