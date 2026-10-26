"use client";

import { Check, ChevronLeft, ChevronRight, Crosshair, Play, Repeat, Square, Wand2, X } from "lucide-react";
import { useState } from "react";
import type { ReactNode } from "react";
import type { AlignProposal, AlignProposalState } from "@/lib/read_aloud/sourceAlign";
import SpanTrack from "./SpanTrack";
import { MIN_SPAN, clock, clockShort, confidenceTone, percent, roundMs } from "./format";

/**
 * One passage, open at full size: what it says, where it sits in its recording,
 * the audio around its span, and the two edges at millisecond precision.
 *
 * This is the pane the job is actually done in. It shows one passage rather
 * than forty rows so the waveform can be big enough to judge, and it keeps the
 * verdict beside the numbers that justify it.
 */

type Edge = "start" | "end";

/**
 * One edge in seconds, at the millisecond the backend stores.
 *
 * A draft keeps typing from being fought by the formatted value, and the step
 * buttons beside it are the reason this is not a slider: 10 ms is finer than a
 * pixel, and a span edge that is 10 ms late clips a consonant.
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
    const parsed = Number(draft.replace(",", "."));
    setDraft(null);
    if (Number.isFinite(parsed) && Math.abs(parsed - value) > 1e-9) onCommit(parsed);
  };
  return (
    <input
      className="w-[76px] px-1 py-1 rounded-md bg-bg-muted text-center text-[11px] tabular-nums border border-border-default text-text-primary focus:outline-none focus:border-accent-bg disabled:opacity-50"
      value={draft ?? value.toFixed(3)}
      disabled={disabled}
      aria-label={label}
      title={`${label}: seconds, three decimals (${clock(value)})`}
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

const STEP_BUTTON =
  "inline-flex items-center justify-center px-1.5 py-1 rounded-md bg-bg-muted text-text-secondary text-[10px] tabular-nums hover:bg-bg-hover cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed";

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
        active ? "border-accent-bg bg-accent-bg/5" : "border-border-default"
      }`}
    >
      <span className="w-9 shrink-0 text-[11px] text-text-secondary">{label}</span>
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
      <span className="ml-auto text-[10px] text-text-tertiary tabular-nums">{clock(value)}</span>
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
        const tone = isCurrent
          ? "bg-accent-bg ring-1 ring-accent-bg"
          : proposal.applied_uuid
            ? "bg-accent-bg/60"
            : proposal.state === "accepted"
              ? "bg-accent-bg/40"
              : proposal.state === "rejected"
                ? "bg-text-tertiary/30"
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
  loop: boolean;
  busy: boolean;
  activeEdge: Edge;
  emptyHint: string;
  canPrev: boolean;
  canNext: boolean;
  /** Walk to the previous / next passage of the same recording. */
  onStep: (direction: -1 | 1) => void;
  onActivateEdge: (edge: Edge) => void;
  onMove: (start: number, end: number) => void;
  onDecide: (state: AlignProposalState) => void;
  onPlay: (context: boolean) => void;
  onToggleLoop: () => void;
  onScrub: (seconds: number) => void;
  onSelect: (textUuid: string) => void;
};

export default function PassageEditor({
  proposal,
  span,
  siblings,
  position,
  duration,
  playhead,
  playing,
  loop,
  busy,
  activeEdge,
  emptyHint,
  canPrev,
  canNext,
  onStep,
  onActivateEdge,
  onMove,
  onDecide,
  onPlay,
  onToggleLoop,
  onScrub,
  onSelect,
}: Props) {
  // No local state beyond the field drafts: the workspace owns the span, the
  // audio element and the verdict, so this pane can be re-rendered freely.
  if (!proposal) {
    return (
      <section className="rounded-xl border border-border-default bg-bg-card flex flex-col min-w-0 min-h-0 overflow-hidden">
        <div className="flex-1 min-h-0 overflow-y-auto p-4">
          <div className="h-full min-h-[220px] flex flex-col items-center justify-center gap-2 text-center">
            <Wand2 size={20} className="text-text-tertiary" />
            <p className="text-sm text-text-secondary max-w-[420px]">{emptyHint}</p>
            <p className="text-[11px] text-text-tertiary max-w-[420px]">
              The Recordings tab listens to each file once and places the passages; this tab is
              where they are judged and cut from.
            </p>
          </div>
        </div>
      </section>
    );
  }

  const refused = proposal.status === "refused";
  const applied = Boolean(proposal.applied_uuid);
  const locked = busy || applied || refused;
  const stateChip = applied
    ? "bg-accent-bg/15 text-accent-bg"
    : proposal.state === "accepted"
      ? "bg-accent-bg/15 text-accent-bg"
      : proposal.state === "rejected"
        ? "bg-bg-muted text-text-tertiary"
        : "bg-bg-muted text-text-secondary";

  const setStart = (raw: number) => {
    const start = Math.min(Math.max(0, raw), span.end - MIN_SPAN);
    onMove(roundMs(start), span.end);
  };
  const setEnd = (raw: number) => {
    const end = Math.max(raw, span.start + MIN_SPAN);
    onMove(span.start, roundMs(end));
  };

  const decideButton = (
    state: AlignProposalState,
    icon: ReactNode,
    label: string,
    keyHint: string
  ) => {
    const active = proposal.state === state;
    return (
      <button
        type="button"
        onClick={() => onDecide(state)}
        disabled={locked || active}
        title={`${label} (${keyHint})`}
        className={`inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md text-xs cursor-pointer disabled:cursor-not-allowed ${
          active
            ? state === "accepted"
              ? "bg-accent-bg text-white"
              : "bg-bg-muted text-text-primary border border-border-default"
            : state === "accepted"
              ? "bg-accent-bg/15 text-accent-bg hover:bg-accent-bg/25 disabled:opacity-50"
              : "bg-bg-muted text-text-secondary hover:bg-bg-hover disabled:opacity-50"
        }`}
      >
        {icon}
        {label}
        <span className="text-[10px] opacity-60">{keyHint}</span>
      </button>
    );
  };

  return (
    <section className="rounded-xl border border-border-default bg-bg-card flex flex-col min-w-0 min-h-0 overflow-hidden">
      <div className="flex-1 min-h-0 overflow-y-auto p-4 flex flex-col gap-4">
        <div className="flex items-start gap-2 flex-wrap">
          <h2 className="text-base font-medium text-text-primary min-w-0 break-words">
            {proposal.title || "Untitled"}
          </h2>
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
            <span className={`px-1.5 py-0.5 rounded ${stateChip}`}>
              {applied ? "stored" : proposal.state}
            </span>
          </span>
        </div>

        <p className="text-sm leading-relaxed text-text-primary whitespace-pre-wrap break-words">
          {proposal.content}
        </p>

        {refused ? (
          <p className="text-[11px] text-warning-text">
            {proposal.note || "The guess refused this passage."} A refused passage has no span to
            move or accept — re-guess with another folder, or leave it out.
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
                    className="inline-flex items-center justify-center w-5 h-5 rounded-md bg-bg-muted text-text-secondary hover:bg-bg-hover cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed"
                  >
                    <ChevronLeft size={11} />
                  </button>
                  <button
                    type="button"
                    onClick={() => onStep(1)}
                    disabled={!canNext}
                    aria-label="Next passage in this recording"
                    title="Next passage in this recording"
                    className="inline-flex items-center justify-center w-5 h-5 rounded-md bg-bg-muted text-text-secondary hover:bg-bg-hover cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed"
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

            <div className="flex flex-wrap items-center gap-2">
              <button
                type="button"
                onClick={() => onPlay(false)}
                disabled={!proposal.source_path}
                className="inline-flex items-center gap-1.5 px-2.5 py-1.5 rounded-md bg-bg-muted text-text-secondary text-[11px] hover:bg-bg-hover cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                title={playing ? "Stop (Space)" : "Play this span (Space)"}
              >
                {playing ? <Square size={11} /> : <Play size={11} />}
                {playing ? "Stop" : "Play span"}
              </button>
              <button
                type="button"
                onClick={() => onPlay(true)}
                disabled={!proposal.source_path}
                className="inline-flex items-center gap-1 px-2.5 py-1.5 rounded-md bg-bg-muted text-text-secondary text-[11px] hover:bg-bg-hover cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                title="Play half a second each side, to hear the edge in context"
              >
                ±0.5 s
              </button>
              <button
                type="button"
                onClick={onToggleLoop}
                aria-pressed={loop}
                className={`inline-flex items-center gap-1 px-2.5 py-1.5 rounded-md text-[11px] cursor-pointer ${
                  loop
                    ? "bg-accent-bg/15 text-accent-bg"
                    : "bg-bg-muted text-text-secondary hover:bg-bg-hover"
                }`}
                title={loop ? "Looping — click to play once" : "Loop the span while you listen"}
              >
                <Repeat size={11} />
                Loop
              </button>
              <span className="text-[10px] text-text-tertiary tabular-nums">
                {(span.end - span.start).toFixed(3)} s
              </span>
            </div>

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

            <div className="flex flex-wrap items-center gap-2">
              {decideButton("accepted", <Check size={12} />, "Accept", "A")}
              {decideButton("rejected", <X size={12} />, "Reject", "R")}
              <span className="text-[11px] text-text-tertiary">
                {applied
                  ? "Already cut — the clip is this text's standard audio."
                  : "Accepted spans are cut in step 4, not here."}
              </span>
            </div>

            {proposal.note && <p className="text-[11px] text-text-tertiary">{proposal.note}</p>}
          </>
        )}
      </div>
    </section>
  );
}
