"use client";

import type {
  Assessment,
  AttemptMeasurement,
  FluencyMetrics,
  Pause,
  PronUnavailableReason,
  RateBand,
} from "@/lib/read_aloud/types";
import { GOOD_SCORE, PASS_SCORE } from "@/lib/read_aloud/types";

/**
 * The three-axis result of one take: accuracy (the unchanged WIP score),
 * fluency (VAD + the STT run's placement — word timings, or phrase spans when
 * the model places no words) and pronunciation (CTC phoneme GOP).
 *
 * Each axis is rendered independently and may say "not measured" — an absent
 * axis is never replaced by a proxy number. Used by the practice view right
 * after a take is measured, and by the attempts view from the stored `metrics`.
 */
export default function AssessmentCards({ assessment }: { assessment: Assessment }) {
  const { accuracy, fluency, pronunciation, capabilities } = assessment;

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

      {/* Fluency — needs the STT run to have placed the take at all. */}
      <Card title="Fluency" hint="paces pauses from the waveform, judged against the text's own audio">
        {fluency ? (
          <>
            <div className="flex items-baseline gap-1.5">
              <span className="text-2xl font-bold tabular-nums text-text-primary">
                {fluency.score != null ? `${Math.round(fluency.score)}%` : Math.round(fluency.wcpm)}
              </span>
              <span className="text-[11px] text-text-tertiary">
                {fluency.score != null ? "fluency score" : "correct words/min"}
              </span>
              <BandChip band={fluency.band} />
            </div>
            <p className="text-[11px] text-text-tertiary tabular-nums">
              {fluency.score != null
                ? `${Math.round(fluency.wcpm)} correct wpm${
                    fluency.ref_wcpm != null ? ` against ${Math.round(fluency.ref_wcpm)}` : ""
                  } · `
                : ""}
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
            {capabilities.timing_evidence === "segments" && (
              <p className="text-[11px] text-text-tertiary">
                Paced from phrase spans — this model places phrases rather than
                words, so a pause is labelled by phrase and no word carries its
                own position.
              </p>
            )}
          </>
        ) : (
          <p className="text-[11px] text-text-tertiary">
            Not measured — the loaded speech model gave no timestamps at all, so
            nothing placed this take in time. Recognize it again with a model
            that reports them.
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
 * The one composite number of a take — the frontend mirror of the backend's
 * `assess::total_score`, kept in step with it:
 * `accuracy · (0.5 + 0.5 · min(weakest measured delivery axis) / 100)`.
 * An axis nobody measured drops out of the minimum instead of counting as
 * zero; with neither delivery axis measured the total is the accuracy alone.
 * Returns null when there is no stored assessment to derive it from, so the
 * strip shows the gap rather than a number computed from nothing.
 */
function totalScore(score: number, fluency: FluencyMetrics | null, pronPercent: number | null): number | null {
  const axes = [fluency?.score ?? null, pronPercent].filter((v): v is number => v != null);
  if (axes.length === 0) return null;
  const weakest = Math.min(...axes);
  return score * (0.5 + (0.5 * Math.max(0, Math.min(100, weakest))) / 100);
}

/**
 * The scores of one take as a single strip: total, accuracy, fluency,
 * pronunciation — the same axes [`AssessmentCards`] breaks down, optionally
 * preceded by the model that produced them (`model`, as the attempts list
 * cards show). The total is the composite of the three (`totalScore`, the
 * text card's badge for one take) and joins the strip whenever an assessment
 * was stored to derive it from.
 *
 * The attempts list shows it on every card, so a take is scanned by its numbers
 * instead of only its accuracy; an attempt with no stored assessment shows what
 * it can and reads "—" for the rest, never a substitute number.
 * Accuracy is the one axis re-derived from the transcript on read, so a take
 * with a transcript has a real accuracy even unmeasured — and a take with *no*
 * transcript has none, and reads "—" rather than a 0% nobody measured.
 * Tooltips carry the detail the cards carry at full size.
 */
export function ScoreAxes({
  score,
  metrics,
  recognized,
  model,
  size = "sm",
  className = "",
}: {
  /** Accuracy 0–100 — re-scored from the transcript on read, so even an
   *  unmeasured take shows the one number it has. */
  score: number;
  /** Absent for a take saved before anything measured it, one recorded before
   *  assessment existed, or one whose recording is gone. */
  metrics?: Assessment | null;
  /** The take's transcript. Without one there is nothing accuracy was computed
   *  from, so accuracy shows the gap instead of `score`. */
  recognized?: string | null;
  /** The speech model that produced these numbers, shown as the leading column
   *  (the best measurement, same as the headline `score`). Pass null when the
   *  take predates per-model scoring. Omit the prop for the plain three-axis
   *  strip. */
  model?: string | null;
  size?: ScoreAxesSize;
  className?: string;
}) {
  const accuracy = metrics?.accuracy;
  const fluency = metrics?.fluency ?? null;
  const pronunciation = metrics?.pronunciation ?? null;
  const pronPercent = pronunciation?.score != null ? pronunciation.score * 100 : null;
  // A take with a stored assessment has numbers whether or not it was
  // transcribed; one with neither has nothing yet measured, and showing its
  // default 0 would be exactly the stand-in number this strip promises never to
  // invent.
  const hasAccuracy = metrics != null || Boolean(recognized?.trim());
  // The composite rides on the stored axes only: an unmeasured take has no
  // delivery axis to gate the accuracy with, so the column is a gap, not the
  // accuracy wearing a different label.
  const total = totalScore(score, fluency, pronPercent);
  // A take with no assessment at all has no per-axis reason to give, only the
  // way to measure one.
  const unmeasured =
    "Not measured — this take has no stored assessment (it was saved before anything " +
    "measured it, it predates the three-axis score, or its recording is gone). Use " +
    "Assess again to measure it.";

  const axes: ScoreAxis[] = [
    ...(total != null
      ? [
          {
            key: "total",
            label: "Total",
            value: String(Math.round(total)),
            unit: "%",
            tone: percentTone(total),
            title: `Total of this take — accuracy ${Math.round(
              score,
            )}% gated by its weakest delivery axis (fluency, pronunciation); an unmeasured axis drops out instead of counting as zero`,
          },
        ]
      : []),
    ...(model !== undefined
      ? [
          {
            key: "model",
            label: "Model",
            value: model != null ? modelLabel(model) : "—",
            tone: "text-text-primary",
            small: true,
            title:
              model != null
                ? `Measured by ${model}${model === "unrecorded" ? "" : ` (catalog id ${model})`}`
                : "This take was measured before per-model scoring existed, so no model is recorded.",
          },
        ]
      : []),
    {
      key: "accuracy",
      label: "Accuracy",
      value: hasAccuracy ? String(Math.round(score)) : "—",
      unit: hasAccuracy ? "%" : undefined,
      tone: hasAccuracy ? percentTone(score) : "text-text-tertiary",
      title: accuracy
        ? `${accuracy.counts.correct} of ${accuracy.counts.ref} words correct · ${Math.round(
            accuracy.wer * 100
          )}% WER${accuracy.lenient_score > accuracy.score ? ` · ${Math.round(accuracy.lenient_score)}% lenient` : ""}`
        : hasAccuracy
        ? "Share of the text's words the transcript got right."
        : unmeasured,
    },
    {
      key: "fluency",
      label: "Fluency",
      // The score is the axis's number; a take stored before it existed shows
      // the raw wcpm it would have been derived from, unit and all.
      value: fluency ? String(Math.round(fluency.score ?? fluency.wcpm)) : "—",
      unit: fluency ? (fluency.score != null ? "%" : "wcpm") : undefined,
      tone: fluency
        ? fluency.score != null
          ? percentTone(fluency.score)
          : bandTone(fluency.band)
        : "text-text-tertiary",
      title: fluency
        ? `${
            fluency.score != null
              ? `Fluency score ${Math.round(fluency.score)} — pace against the reference${
                  fluency.ref_wcpm != null
                    ? ` (${Math.round(fluency.ref_wcpm)} wpm)`
                    : " (fallback norm)"
                }, less pauses the reference did not take · `
              : ""
          }${Math.round(
            fluency.wcpm
          )} correct words per minute (${fluency.band} pace) · ${fluency.pause_count} pause${
            fluency.pause_count === 1 ? "" : "s"
          } · reading ${Math.round(fluency.speaking_time)} s of ${Math.round(fluency.duration)} s`
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
    <div
      className={`grid ${
        (model !== undefined ? 1 : 0) + (total != null ? 1 : 0) === 2
          ? "grid-cols-5"
          : model !== undefined || total != null
          ? "grid-cols-4"
          : "grid-cols-3"
      } gap-1.5 min-w-0 ${className}`}
    >
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
              className={`leading-none min-w-0 ${axis.tone} ${
                axis.small
                  ? "text-xs font-medium truncate"
                  : `font-semibold tabular-nums ${size === "md" ? "text-2xl" : "text-lg"}`
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
  /** Render the value as small text (a name, not a number). */
  small?: boolean;
};

/**
 * Every speech model's measurement of one take, best score first.
 *
 * The same recording heard by two models does not give two versions of one
 * number: it gives two numbers, often tens of points apart, and a lone `92`
 * cannot say that another model would have heard `61`. So the attempt keeps one
 * row per model and its final score is the best of them — this is that list.
 *
 * Nothing is measured or invented here: the three columns are the accuracy,
 * fluency and pronunciation [`ScoreAxes`] shows, read per model instead of only
 * for the headline. It renders for a single measurement too, because a take
 * should always say who measured it.
 */
export function ModelScores({
  measurements,
  className = "",
}: {
  measurements?: AttemptMeasurement[] | null;
  className?: string;
}) {
  if (!measurements?.length) return null;
  const best = measurements[0].score;
  return (
    <section className={`flex flex-col gap-1 min-w-0 ${className}`}>
      <h3 className="text-xs font-medium text-text-secondary">
        Measured by {measurements.length} model{measurements.length === 1 ? "" : "s"} · the best is the
        final score
      </h3>
      <ul className="flex flex-col gap-1 min-w-0">
        {measurements.map((m) => (
          <li key={m.model} className="flex items-center gap-2 text-xs min-w-0">
            <span className="flex-1 min-w-0 truncate text-text-primary" title={m.model}>
              {modelLabel(m.model)}
            </span>
            {m.via === "hub" && (
              <span
                className="text-[10px] px-1.5 py-0.5 rounded-full bg-bg-muted text-text-tertiary shrink-0"
                title="Measured on the hub, not on this device"
              >
                hub
              </span>
            )}
            <span
              className={`w-12 text-right font-semibold tabular-nums ${percentTone(m.score)}`}
              title="Accuracy"
            >
              {Math.round(m.score)}%
            </span>
            <span
              className="w-20 text-right tabular-nums text-text-secondary"
              title="Correct words per minute"
            >
              {m.wcpm != null ? `${Math.round(m.wcpm)} wcpm` : "—"}
            </span>
            <span
              className={`w-10 text-right tabular-nums ${
                m.pronunciation != null ? percentTone(m.pronunciation) : "text-text-tertiary"
              }`}
              title="Pronunciation"
            >
              {m.pronunciation != null ? `${Math.round(m.pronunciation)}%` : "—"}
            </span>
            {m.score === best && (
              <span
                className="text-[10px] px-1.5 py-0.5 rounded-full bg-success-bg text-success-text font-semibold shrink-0"
                title="The best measurement, so the score this attempt keeps and the XP it earned"
              >
                best
              </span>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}

/** The part of a catalog id a learner recognises rather than its packaging:
 *  `gguf::whisper-large-v3` reads `whisper-large-v3`. A measurement taken before
 *  the engine was named says so in words instead of showing a placeholder id. */
function modelLabel(model: string): string {
  const separator = model.lastIndexOf("::");
  const name = separator >= 0 ? model.slice(separator + 2) : model;
  return name === "unrecorded" ? "Model not recorded" : name;
}

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
