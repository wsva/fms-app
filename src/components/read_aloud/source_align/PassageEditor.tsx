"use client";

import { ChevronLeft, ChevronRight, Crosshair, Loader2, Play, RotateCcw, Square, Wand2 } from "lucide-react";
import { useState } from "react";
import type { AlignProposal } from "@/lib/read_aloud/sourceAlign";
import { ClipChip, DeleteClipButton, SaveClipButton } from "./ClipActions";
import SpanTrack from "./SpanTrack";
import { MIN_SPAN, clock, clockHms, clockShort, confidenceTone, parseClock, percent, roundMs } from "./format";

/**
 * One passage, open at full size: the audio in a card on top, the text in a
 * second card underneath — the same two-panel shape as the queue beside them,
 * with a gap instead of a handle: the audio card is as tall as its contents and
 * only the text scrolls.
 *
 * The audio card closes with one bar, directly under the two edges everything
 * in it acts on: the transport on the left, and on the right the verbs —
 * Re-guess, and Save, or the Delete clip that has to come first, since a clip
 * holds the seconds its wav was cut from and a save never overwrites one.
 */

type Edge = "start" | "end";

/**
 * One edge as `hh:mm:ss.ms` — the clock the recording is read in, at the
 * millisecond the backend stores.
 *
 * A draft keeps typing from being fought by the formatted value. The parser is
 * the tolerant one in `format.ts`, so a bare `12.5` still means twelve and a
 * half seconds for the moment a reader types a delta rather than a time, and the
 * step buttons beside it are the reason this is not a slider: 10 ms is finer
 * than a pixel, and a span edge that is 10 ms late clips a consonant.
 */
function SpanField({ label, value, disabled, onCommit }: {
  label: string;
  value: number;
  disabled: boolean;
  onCommit: (seconds: number) => void;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const commit = () => {
    if (draft == null) return;
    const parsed = parseClock(draft);
    setDraft(null);
    if (parsed != null && Math.abs(parsed - value) > 1e-9) onCommit(parsed);
  };
  return (
    <input
      // An inset well rather than a tinted patch: `bg-input` steps away from the
      // card in every theme (down in the dark ones, paper in solarized), and the
      // `border-light` edge plus primary-weight digits are what make a whole
      // clock read as something you may type into.
      className="w-[92px] px-1 py-1 rounded-md bg-bg-input text-center text-[11px] font-medium tabular-nums border border-border-light text-text-primary focus:outline-none focus:border-accent-bg focus:ring-1 focus:ring-accent-bg/40 disabled:opacity-50"
      value={draft ?? clockHms(value)}
      disabled={disabled}
      aria-label={label}
      title={`${label}: hh:mm:ss.ms — plain seconds work too (${value.toFixed(3)} s)`}
      inputMode="decimal"
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          e.currentTarget.blur();
        } else if (e.key === "Escape") {
          setDraft(null);
        }
      }}
    />
  );
}

// ── Control surfaces ─────────────────────────────────────────────────────────
//
// Every button under the waveform — the ±10 / ±100 steps, the transport, and
// Re-guess — is drawn on one of these. `bg-muted` alone is only a shade off the
// card behind it in all five themes, so the fill was never what made these
// visible; the `border-light` outline and the primary-weight text are. The
// transport's Play is the one control tinted with the accent, because it is the
// verb the whole card exists to serve.
const CONTROL_BOX =
  "inline-flex items-center justify-center rounded-md border cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed";

/** Neutral control: steps, ±0.5 s, Re-guess, the passage chevrons. */
const CONTROL_QUIET = `${CONTROL_BOX} bg-bg-muted border-border-light text-text-primary hover:border-accent-bg hover:bg-bg-hover disabled:hover:border-border-light disabled:hover:bg-bg-muted`;

/** The transport's Play: accent-tinted, so the ear's button is found first. */
const CONTROL_PLAY = `${CONTROL_BOX} bg-accent-bg/10 border-accent-bg/60 text-accent-bg hover:bg-accent-bg/15 hover:border-accent-bg disabled:hover:border-accent-bg/60 disabled:hover:bg-accent-bg/10`;

const STEP_BUTTON = `${CONTROL_QUIET} px-1.5 py-1 text-[10px] font-medium tabular-nums`;

function EdgeRow({
  edge,
  value,
  active,
  locked,
  playhead,
  onChange,
  onActivate,
}: {
  edge: Edge;
  value: number;
  active: boolean;
  locked: boolean;
  playhead: number | null;
  onChange: (seconds: number) => void;
  onActivate: () => void;
}) {
  const label = edge === "start" ? "Start" : "End";
  return (
    <div
      onPointerDown={onActivate}
      onFocusCapture={onActivate}
      className={`flex flex-wrap items-center gap-1 rounded-md border px-2 py-1.5 ${
        active ? "border-accent-bg bg-accent-bg/5" : "border-border-light"
      }`}
    >
      <span className="w-9 shrink-0 text-[11px] font-medium text-text-primary">{label}</span>
      <button
        type="button"
        className={STEP_BUTTON}
        disabled={locked}
        onClick={() => onChange(value - 0.1)}
        title={`${label} 100 ms earlier`}
      >
        −100
      </button>
      <button
        type="button"
        className={STEP_BUTTON}
        disabled={locked}
        onClick={() => onChange(value - 0.01)}
        title={`${label} 10 ms earlier`}
      >
        −10
      </button>
      <SpanField label={`Span ${edge}`} value={value} disabled={locked} onCommit={onChange} />
      <button
        type="button"
        className={STEP_BUTTON}
        disabled={locked}
        onClick={() => onChange(value + 0.01)}
        title={`${label} 10 ms later`}
      >
        +10
      </button>
      <button
        type="button"
        className={STEP_BUTTON}
        disabled={locked}
        onClick={() => onChange(value + 0.1)}
        title={`${label} 100 ms later`}
      >
        +100
      </button>
      <button
        type="button"
        className={`${STEP_BUTTON} ml-1`}
        disabled={locked || playhead == null}
        onClick={() => {
          if (playhead != null) onChange(playhead);
        }}
        title={
          playhead == null
            ? "Click the waveform first to place the playhead"
            : `Take the playhead (${clock(playhead)}) as the ${edge}`
        }
        aria-label={`Take the playhead as the ${edge}`}
      >
        <Crosshair size={11} />
      </button>
      {/* No trailing clock readout: the field beside it *is* the clock now, in
          the same units, so a second copy of the same number would only be
          something else to read. */}
    </div>
  );
}

/**
 * Where this passage sits in its recording.
 *
 * Every proposal carries the recording it was placed in, so the whole chapter
 * can be drawn from the rows themselves: one block per passage, gaps where the
 * guess placed nothing. Clicking a block is how you move to a neighbour without
 * going back to the queue — the spatial half of what the list cannot show.
 */
function ChapterMap({
  siblings,
  current,
  duration,
  onSelect,
}: {
  siblings: AlignProposal[];
  current: string;
  duration: number;
  onSelect: (textUuid: string) => void;
}) {
  return (
    <div className="relative h-9 rounded-md bg-bg-muted overflow-hidden">
      {siblings.map((proposal) => {
        if (proposal.status === "refused" || !(proposal.end > proposal.start)) return null;
        const isCurrent = proposal.text_uuid === current;
        // Three tones: the passage you are on, one that already owns a clip, and
        // one whose span is only in `proposals.json`.
        const tone = isCurrent
          ? "bg-accent-bg ring-1 ring-accent-bg"
          : proposal.clip
            ? "bg-accent-bg/60"
            : "bg-text-tertiary/20 hover:bg-text-tertiary/40";
        return (
          <button
            key={proposal.text_uuid}
            type="button"
            onClick={() => onSelect(proposal.text_uuid)}
            aria-label={`${proposal.title || "Untitled"} ${clock(proposal.start)}`}
            aria-current={isCurrent ? "true" : undefined}
            title={`${proposal.title || "Untitled"} · ${clock(proposal.start)} – ${clock(proposal.end)}`}
            className={`absolute top-1 bottom-1 min-w-[2px] rounded-sm cursor-pointer transition-colors ${tone}`}
            style={{
              left: `${(proposal.start / duration) * 100}%`,
              width: `${((proposal.end - proposal.start) / duration) * 100}%`,
            }}
          />
        );
      })}
    </div>
  );
}

type Props = {
  proposal: AlignProposal | null;
  span: { start: number; end: number };
  siblings: AlignProposal[];
  position: { index: number; total: number } | null;
  duration: number | null;
  playhead: number | null;
  playing: boolean;
  busy: boolean;
  activeEdge: Edge;
  emptyHint: string;
  canPrev: boolean;
  canNext: boolean;
  /** Walk to the previous / next passage of the same recording. */
  onStep: (direction: -1 | 1) => void;
  onActivateEdge: (edge: Edge) => void;
  onMove: (start: number, end: number) => void;
  onPlay: (context: boolean) => void;
  onScrub: (seconds: number) => void;
  onSelect: (textUuid: string) => void;
  /** Place this text again, leaving every other span in the file alone. */
  onReguess: () => void;
  /** Save this span as the text's clip. */
  onSave: () => void;
  /** Delete the clip standing between this span and that save. */
  onDeleteClip: () => void;
  /** A re-guess of this text is in flight. */
  reguessing: boolean;
  /** A save is in flight. */
  saving: boolean;
  /** Some recording is transcribed, so there is something to guess against. */
  canGuess: boolean;
};

export default function PassageEditor({
  proposal,
  span,
  siblings,
  position,
  duration,
  playhead,
  playing,
  busy,
  activeEdge,
  emptyHint,
  canPrev,
  canNext,
  onStep,
  onActivateEdge,
  onMove,
  onPlay,
  onScrub,
  onSelect,
  onReguess,
  onSave,
  onDeleteClip,
  reguessing,
  saving,
  canGuess,
}: Props) {
  // No local state beyond the field drafts: the workspace owns the span and
  // the audio element, so this pane can be re-rendered freely.
  if (!proposal) {
    return (
      <section className="rounded-xl border border-border-default bg-bg-card flex flex-col min-w-0 min-h-0 overflow-hidden">
        <div className="flex-1 min-h-0 overflow-y-auto p-4">
          <div className="h-full min-h-[220px] flex flex-col items-center justify-center gap-2 text-center">
            <Wand2 size={20} className="text-text-tertiary" />
            <p className="text-sm text-text-secondary max-w-[420px]">{emptyHint}</p>
            <p className="text-[11px] text-text-tertiary max-w-[420px]">
              The Source Audios tab listens to each file once and places the passages; this tab is
              where a span is checked by ear and saved as the text's clip.
            </p>
          </div>
        </div>
      </section>
    );
  }

  const refused = proposal.status === "refused";
  // A clip is a cut wav that holds the seconds it was cut from, so a stored
  // span is read-only until that clip is deleted.
  const blocked = proposal.clip != null;
  const locked = busy || refused || blocked;

  const setStart = (raw: number) => {
    const start = Math.min(Math.max(0, raw), span.end - MIN_SPAN);
    onMove(roundMs(start), span.end);
  };
  const setEnd = (raw: number) => {
    const end = Math.max(raw, span.start + MIN_SPAN);
    onMove(span.start, roundMs(end));
  };

  return (
    <div className="flex flex-col gap-3 min-w-0 min-h-0">
      {/* The audio, in its own card — a sibling of the queue on the other side
          of the seam, and of the text card below it. Nothing in here scrolls:
          the waveform and the two edges have to stay in reach, so the card is
          only ever as tall as what it holds. */}
      <section className="shrink-0 rounded-xl border border-border-default bg-bg-card px-4 py-3 flex flex-col gap-3 min-w-0 overflow-hidden">
        <div className="flex items-start gap-2 flex-wrap">
          <h2 className="text-base font-medium text-text-primary min-w-0 break-words">
            {proposal.title || "Untitled"}
          </h2>
          {proposal.clip && <ClipChip clip={proposal.clip} />}
          <span className="ml-auto flex items-center gap-2 text-[11px] tabular-nums">
            {!refused && (
              <span
                className={confidenceTone(proposal.confidence)}
                title="How much of the text was heard, in order"
              >
                {percent(proposal.confidence)}%
              </span>
            )}
            {!refused && (
              <span className="text-text-tertiary" title="Words matched of the passage's words">
                {proposal.matched_words}/{proposal.reference_words} w
              </span>
            )}
          </span>
        </div>

        {refused ? (
          <p className="text-[11px] text-warning-text">
            {proposal.note || "The guess refused this passage."} A refused passage has no span to
            move or save — re-guess with another folder, or leave it out.
          </p>
        ) : (
          <>
            <div className="flex flex-col gap-1">
              <div className="flex items-center gap-2 text-[10px] text-text-tertiary">
                <span className="min-w-0 truncate" title={proposal.source_path}>
                  {proposal.source_name}
                  {duration != null && duration > 0 ? ` · ${clockShort(duration)}` : ""}
                  {position ? ` · passage ${position.index} of ${position.total}` : ""}
                </span>
                <span className="ml-auto flex items-center gap-1 shrink-0">
                  <button
                    type="button"
                    onClick={() => onStep(-1)}
                    disabled={!canPrev}
                    aria-label="Previous passage in this recording"
                    title="Previous passage in this recording"
                    className={`${CONTROL_QUIET} w-5 h-5`}
                  >
                    <ChevronLeft size={11} />
                  </button>
                  <button
                    type="button"
                    onClick={() => onStep(1)}
                    disabled={!canNext}
                    aria-label="Next passage in this recording"
                    title="Next passage in this recording"
                    className={`${CONTROL_QUIET} w-5 h-5`}
                  >
                    <ChevronRight size={11} />
                  </button>
                </span>
              </div>
              {duration != null && duration > 0 && (
                <ChapterMap
                  siblings={siblings}
                  current={proposal.text_uuid}
                  duration={duration}
                  onSelect={onSelect}
                />
              )}
            </div>

            <SpanTrack
              preview={proposal.preview}
              start={span.start}
              end={span.end}
              playhead={playhead}
              playing={playing}
              disabled={locked}
              onChange={onMove}
              onScrub={onScrub}
              onActivateEdge={onActivateEdge}
            />

            <div className="flex flex-col gap-1.5">
              <EdgeRow
                edge="start"
                value={span.start}
                active={activeEdge === "start"}
                locked={locked}
                playhead={playhead}
                onChange={setStart}
                onActivate={() => onActivateEdge("start")}
              />
              <EdgeRow
                edge="end"
                value={span.end}
                active={activeEdge === "end"}
                locked={locked}
                playhead={playhead}
                onChange={setEnd}
                onActivate={() => onActivateEdge("end")}
              />
            </div>
          </>
        )}

        {/* The edges above are read-only for a reason, and the button that
            unlocks them is in the bar just below — so the reason is stated
            here, where the locked fields and the way out are in sight at once. */}
        {blocked && (
          <p className="text-[11px] text-text-secondary">
            This text already has standard audio, so its span is locked. Delete the clip to cut
            this one instead.
          </p>
        )}

        {/* One bar closes the band, directly under the two edges everything in
            it acts on: the transport on the left, the verbs that decide whether
            this span becomes (or has to give way to) a clip on the right. Above
            the text, so none of them has to be scrolled for. A refused passage
            keeps Re-guess here — that is the one thing it can do — and its Save
            explains why it is off. */}
        <div className="flex flex-wrap items-center gap-1.5">
          <button
            type="button"
            onClick={() => onPlay(false)}
            disabled={!proposal.source_path}
            className={`${CONTROL_PLAY} gap-1.5 px-2.5 py-1.5 text-[11px] font-medium`}
            title={playing ? "Stop (Space)" : "Play this span (Space)"}
          >
            {playing ? <Square size={11} /> : <Play size={11} />}
            {playing ? "Stop" : "Play span"}
          </button>
          <button
            type="button"
            onClick={() => onPlay(true)}
            disabled={!proposal.source_path}
            className={`${CONTROL_QUIET} gap-1 px-2.5 py-1.5 text-[11px] font-medium tabular-nums`}
            title="Play half a second each side, to hear the edge in context"
          >
            ±0.5 s
          </button>
          <span className="text-[10px] text-text-tertiary tabular-nums">
            {(span.end - span.start).toFixed(3)} s
          </span>
          <span className="ml-auto flex items-center gap-1.5">
            <button
              type="button"
              onClick={onReguess}
              disabled={busy || !canGuess}
              title={
                canGuess
                  ? "Guess this text again — the other texts keep their spans"
                  : "Listen to at least one recording first — Source Audios tab"
              }
              className={`${CONTROL_QUIET} gap-1.5 px-2.5 py-1.5 text-[11px]`}
            >
              {reguessing ? (
                <Loader2 size={11} className="animate-spin" />
              ) : (
                <RotateCcw size={11} />
              )}
              Re-guess
            </button>
            <DeleteClipButton clip={proposal.clip} busy={busy} onDelete={onDeleteClip} />
            <SaveClipButton proposal={proposal} busy={busy} saving={saving} onSave={onSave} />
          </span>
        </div>
      </section>

      {/* The text, in the second card: the same shape as the audio card and as
          the queue beside it, separated by a gap rather than a border so the
          two read as siblings. It takes whatever height is left and scrolls
          inside itself — no handle between the cards, because the audio card is
          already as tall as its own contents. */}
      <section className="flex-1 min-h-0 rounded-xl border border-border-default bg-bg-muted flex flex-col min-w-0 overflow-hidden">
        <div className="flex-1 min-h-0 overflow-y-auto px-4 py-3">
          <p className="text-sm leading-relaxed text-text-primary whitespace-pre-wrap break-words">
            {proposal.content}
          </p>
        </div>
      </section>
    </div>
  );
}
