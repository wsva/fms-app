"use client";

import { useEffect, useMemo, useRef } from "react";
import { AlertTriangle, Save, Search, Wand2, X } from "lucide-react";
import type { AlignProposal } from "@/lib/read_aloud/sourceAlign";
import { ClipChip, canSave } from "./ClipActions";
import { clock, confidenceTone, percent } from "./format";

/**
 * The queue: every text in reading order, one line each — its number, its
 * title, the chip that says it already owns a clip, and where the span starts.
 *
 * The verbs that act on a text are deliberately not here. They live in the pane
 * beside the queue, under the two edges they act on: a row is a target, and a
 * target the width of a rail cannot hold three buttons and still be easy to hit.
 * What a row has to answer is "which text is this, and does it have a clip" —
 * so that is all it draws.
 */

export type QueueFilter = "all" | "unsaved" | "saved" | "refused";

export const QUEUE_FILTERS: { id: QueueFilter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "unsaved", label: "Unsaved" },
  { id: "saved", label: "Saved" },
  { id: "refused", label: "Refused" },
];

export function matchesFilter(proposal: AlignProposal, filter: QueueFilter): boolean {
  switch (filter) {
    case "all":
      return true;
    case "unsaved":
      return canSave(proposal);
    case "saved":
      return proposal.clip != null;
    case "refused":
      return proposal.status === "refused";
  }
}

function StateMark({ proposal }: { proposal: AlignProposal }) {
  if (proposal.status === "refused") {
    return (
      <span className="shrink-0 text-warning-text" title="The guess refused this passage">
        <AlertTriangle size={11} />
      </span>
    );
  }
  return null;
}

type Props = {
  /** Every proposal, for the ordinals and the filter counts. */
  proposals: AlignProposal[];
  /** The rows under the current filter and search. */
  visible: AlignProposal[];
  filter: QueueFilter;
  filterCounts: Record<QueueFilter, number>;
  onFilter: (filter: QueueFilter) => void;
  search: string;
  onSearch: (value: string) => void;
  selected: string | null;
  onSelect: (textUuid: string) => void;
  /** Place every text along the recordings again, in reading order. */
  onGuessAll: () => void;
  /** Save every placed span whose text has no clip yet. */
  onSaveAll: () => void;
  /** How many rows Save all would store — 0 turns the button off. */
  saveable: number;
  /** A run of any kind is in flight: the queue's buttons step aside for it. */
  busy: boolean;
  /** Nothing to guess against until at least one recording is transcribed. */
  canGuess: boolean;
};

export default function PassageQueue({
  proposals,
  visible,
  filter,
  filterCounts,
  onFilter,
  search,
  onSearch,
  selected,
  onSelect,
  onGuessAll,
  onSaveAll,
  saveable,
  busy,
  canGuess,
}: Props) {
  const rowsRef = useRef<Record<string, HTMLButtonElement | null>>({});
  const ordinal = useMemo(() => {
    const map = new Map<string, number>();
    proposals.forEach((proposal, index) => map.set(proposal.text_uuid, index + 1));
    return map;
  }, [proposals]);

  useEffect(() => {
    if (!selected) return;
    rowsRef.current[selected]?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  return (
    <section className="rounded-xl border border-border-default bg-bg-card flex flex-col min-w-0 min-h-0 overflow-hidden max-h-[38vh] lg:max-h-none">
      <div className="shrink-0 px-2 py-2 border-b border-border-default flex flex-col gap-1.5">
        <div className="flex items-center gap-2 px-1">
          <h2 className="text-xs font-semibold text-text-primary">Texts</h2>
          <span className="ml-auto text-[10px] text-text-tertiary tabular-nums">
            {visible.length === proposals.length
              ? proposals.length
              : `${visible.length} of ${proposals.length}`}
          </span>
        </div>

        {/* One pass over the whole set: the order is the reading order, so it
            is also the only way to place texts that have never been placed. */}
        <button
          type="button"
          onClick={onGuessAll}
          disabled={busy || !canGuess}
          className="inline-flex items-center justify-center gap-1.5 px-2 py-1 rounded-md text-[11px] bg-accent-bg text-white hover:bg-accent-bg/90 cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
          title={
            canGuess
              ? "Place every text along the recordings again, in reading order"
              : "Listen to at least one recording first — Source Audios tab"
          }
        >
          <Wand2 size={11} />
          Guess for all
        </button>

        {/* The save follows the guess, in the same place: it stores every span
            that has no clip yet and skips the ones that do. */}
        <button
          type="button"
          onClick={onSaveAll}
          disabled={busy || saveable === 0}
          className="inline-flex items-center justify-center gap-1.5 px-2 py-1 rounded-md text-[11px] bg-bg-muted text-text-secondary border border-border-default hover:bg-bg-hover hover:text-text-primary cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:bg-bg-muted"
          title={
            saveable === 0
              ? "Nothing to save: every placed text already has a clip, or none is placed"
              : `Save ${saveable} span${saveable === 1 ? "" : "s"} as standard audio — texts that already have a clip are skipped`
          }
        >
          <Save size={11} />
          Save all clips
          <span className="tabular-nums opacity-70">{saveable}</span>
        </button>

        <div className="flex flex-wrap gap-1">
          {QUEUE_FILTERS.map((option) => {
            const active = option.id === filter;
            return (
              <button
                key={option.id}
                type="button"
                onClick={() => onFilter(option.id)}
                aria-pressed={active}
                className={`inline-flex items-center gap-1 px-1.5 py-0.5 rounded-md text-[10px] cursor-pointer ${
                  active
                    ? "bg-accent-bg/15 text-accent-bg"
                    : "bg-bg-muted text-text-secondary hover:bg-bg-hover"
                }`}
                title={`${option.label}: ${filterCounts[option.id]} text${
                  filterCounts[option.id] === 1 ? "" : "s"
                }`}
              >
                {option.label}
                <span className="tabular-nums opacity-70">{filterCounts[option.id]}</span>
              </button>
            );
          })}
        </div>

        <div className="flex items-center gap-1 px-1.5 py-1 rounded-md bg-bg-muted border border-border-default">
          <Search size={11} className="shrink-0 text-text-tertiary" />
          <input
            value={search}
            onChange={(e) => onSearch(e.target.value)}
            placeholder="Filter by title or text"
            aria-label="Filter texts by title or text"
            className="min-w-0 flex-1 bg-transparent text-[11px] text-text-primary focus:outline-none placeholder:text-text-tertiary"
          />
          {search !== "" && (
            <button
              type="button"
              onClick={() => onSearch("")}
              aria-label="Clear the text filter"
              className="shrink-0 text-text-tertiary hover:text-text-secondary cursor-pointer"
            >
              <X size={11} />
            </button>
          )}
        </div>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto py-1">
        {visible.length === 0 ? (
          <p className="px-3 py-4 text-[11px] text-text-tertiary">
            {proposals.length === 0
              ? "Nothing placed yet — run the guess."
              : "Nothing under this filter."}
          </p>
        ) : (
          visible.map((proposal) => {
            const refused = proposal.status === "refused";
            const active = proposal.text_uuid === selected;
            return (
              <button
                key={proposal.text_uuid}
                type="button"
                ref={(node) => {
                  rowsRef.current[proposal.text_uuid] = node;
                }}
                onClick={() => onSelect(proposal.text_uuid)}
                aria-current={active ? "true" : undefined}
                className={`w-full min-w-0 text-left flex items-center gap-2 px-2 py-1.5 border-l-2 cursor-pointer ${
                  active ? "border-accent-bg bg-bg-muted" : "border-transparent hover:bg-bg-hover"
                }`}
              >
                <span className="w-6 shrink-0 text-right text-[10px] text-text-tertiary tabular-nums">
                  {ordinal.get(proposal.text_uuid) ?? ""}
                </span>
                <span className="min-w-0 flex-1 flex items-center gap-1.5">
                  <span className="min-w-0 truncate text-xs text-text-primary">
                    {proposal.title || "Untitled"}
                  </span>
                  {proposal.clip && <ClipChip clip={proposal.clip} />}
                  {!refused && !proposal.clip && (
                    <span
                      className="shrink-0 text-[9px] uppercase tracking-wide text-text-tertiary"
                      title="Placed, but not saved as this text's standard audio yet"
                    >
                      unsaved
                    </span>
                  )}
                </span>
                {refused ? (
                  <span
                    className="shrink-0 max-w-[45%] truncate text-[10px] text-text-tertiary"
                    title={proposal.note}
                  >
                    no span
                  </span>
                ) : (
                  <span
                    className="shrink-0 text-[10px] text-text-tertiary tabular-nums"
                    title={`${clock(proposal.start)} – ${clock(proposal.end)}`}
                  >
                    {clock(proposal.start)}
                  </span>
                )}
                {!refused && (
                  <span
                    className={`shrink-0 text-[10px] tabular-nums ${confidenceTone(proposal.confidence)}`}
                    title="How much of the passage was heard, in order"
                  >
                    {percent(proposal.confidence)}%
                  </span>
                )}
                <StateMark proposal={proposal} />
              </button>
            );
          })
        )}
      </div>
    </section>
  );
}
