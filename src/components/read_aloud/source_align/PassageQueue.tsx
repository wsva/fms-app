"use client";

import { useEffect, useMemo, useRef } from "react";
import { AlertTriangle, Check, Loader2, RotateCcw, Search, Upload, Wand2, X } from "lucide-react";
import type { AlignProposal } from "@/lib/read_aloud/sourceAlign";
import { clock, confidenceTone, percent } from "./format";

/**
 * The queue: every text in reading order, narrowed by what you still have to
 * decide.
 *
 * It is a list to walk, not a form — one line per text, a filter, and nothing
 * else. The verbs live here because a span is judged next to its neighbours:
 * "guess for all" re-places the whole set in order, and "re-guess" re-places
 * one text without touching the edges already nudged on any other. The batch
 * verdicts and the accepted set live on the Cut tab, so this rail stays narrow
 * enough to sit beside the passage being worked on.
 */

export type QueueFilter = "all" | "review" | "accepted" | "rejected" | "refused" | "stored";

export const QUEUE_FILTERS: { id: QueueFilter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "review", label: "Review" },
  { id: "accepted", label: "Accepted" },
  { id: "rejected", label: "Rejected" },
  { id: "refused", label: "Refused" },
  { id: "stored", label: "Stored" },
];

export function matchesFilter(proposal: AlignProposal, filter: QueueFilter): boolean {
  switch (filter) {
    case "all":
      return true;
    case "review":
      return proposal.status === "proposed" && proposal.state === "pending";
    case "accepted":
      return proposal.state === "accepted";
    case "rejected":
      return proposal.state === "rejected";
    case "refused":
      return proposal.status === "refused";
    case "stored":
      return proposal.applied_uuid != null;
  }
}

function StateMark({ proposal }: { proposal: AlignProposal }) {
  if (proposal.applied_uuid) {
    return (
      <span className="shrink-0 text-accent-bg" title="Cut into the dataset">
        <Upload size={11} />
      </span>
    );
  }
  if (proposal.status === "refused") {
    return (
      <span className="shrink-0 text-warning-text" title="The guess refused this passage">
        <AlertTriangle size={11} />
      </span>
    );
  }
  if (proposal.state === "accepted") {
    return (
      <span className="shrink-0 text-accent-bg" title="Accepted">
        <Check size={11} />
      </span>
    );
  }
  if (proposal.state === "rejected") {
    return (
      <span className="shrink-0 text-text-tertiary" title="Rejected">
        <X size={11} />
      </span>
    );
  }
  return (
    <span
      className="shrink-0 w-[9px] h-[9px] rounded-full border border-text-tertiary/60"
      title="Not decided yet"
    />
  );
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
  /** Place one text again, leaving every other span in the file alone. */
  onReguess: (textUuid: string) => void;
  /** The text being re-guessed right now, so its row shows the spinner. */
  reguessing: string | null;
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
  onReguess,
  reguessing,
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
            const working = reguessing === proposal.text_uuid;
            return (
              // The row is a box with two buttons in it rather than one button
              // with another inside: a button cannot nest a button.
              <div
                key={proposal.text_uuid}
                className={`flex items-center border-l-2 ${
                  active ? "border-accent-bg bg-bg-muted" : "border-transparent hover:bg-bg-hover"
                }`}
              >
                <button
                  type="button"
                  ref={(node) => {
                    rowsRef.current[proposal.text_uuid] = node;
                  }}
                  onClick={() => onSelect(proposal.text_uuid)}
                  aria-current={active ? "true" : undefined}
                  className="min-w-0 flex-1 text-left flex items-center gap-2 pl-2 pr-1 py-1.5 cursor-pointer"
                >
                  <span className="w-6 shrink-0 text-right text-[10px] text-text-tertiary tabular-nums">
                    {ordinal.get(proposal.text_uuid) ?? ""}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block text-xs text-text-primary truncate">
                      {proposal.title || "Untitled"}
                    </span>
                    <span className="block text-[10px] text-text-tertiary tabular-nums">
                      {refused ? "no span" : clock(proposal.start)}
                    </span>
                  </span>
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

                {/* Re-places this text alone: the spans of its neighbours,
                    hand-nudged or not, are read but never rewritten. */}
                <button
                  type="button"
                  onClick={() => onReguess(proposal.text_uuid)}
                  disabled={busy || !canGuess}
                  aria-label={`Guess ${proposal.title || "this text"} again`}
                  title={
                    canGuess
                      ? "Guess this text again — the other texts keep their spans"
                      : "Listen to at least one recording first — Source Audios tab"
                  }
                  className="shrink-0 mr-1 inline-flex items-center gap-1 px-1.5 py-1 rounded-md border border-border-default text-[10px] text-text-secondary hover:bg-bg-hover hover:text-text-primary cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed disabled:hover:bg-transparent"
                >
                  {working ? (
                    <Loader2 size={10} className="animate-spin" />
                  ) : (
                    <RotateCcw size={10} />
                  )}
                  Re-guess
                </button>
              </div>
            );
          })
        )}
      </div>
    </section>
  );
}
