"use client";

import { useEffect, useMemo, useRef } from "react";
import { AlertTriangle, Check, Search, Upload, X } from "lucide-react";
import type { AlignProposal, AlignProposalState } from "@/lib/read_aloud/sourceAlign";
import { clock, confidenceTone, percent } from "./format";

/**
 * The queue: every passage in reading order, narrowed by what you still have to
 * decide.
 *
 * It is a list to walk, not a form — one line per passage, a filter, and the
 * batch verdicts at the foot where they act on what is above them. The rail is
 * 300 px wide on a desktop, so this is deliberately glanceable: title, where the
 * span starts, how much of the text was heard, and what was decided.
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
  busy: boolean;
  threshold: number;
  /** The half-typed threshold, so "9" is not read as 9 % while it is typed. */
  thresholdDraft: string | null;
  onThresholdDraft: (value: string) => void;
  onThresholdCommit: (value: string) => void;
  /** How many rows each batch verdict would actually change. */
  acceptAllCount: number;
  rejectAllCount: number;
  onDecideMany: (state: AlignProposalState) => void;
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
  busy,
  threshold,
  thresholdDraft,
  onThresholdDraft,
  onThresholdCommit,
  acceptAllCount,
  rejectAllCount,
  onDecideMany,
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
        <div className="flex items-baseline justify-between gap-2 px-1">
          <h2 className="text-xs font-semibold text-text-primary">Passages</h2>
          <span className="text-[10px] text-text-tertiary tabular-nums">
            {visible.length === proposals.length
              ? proposals.length
              : `${visible.length} of ${proposals.length}`}
          </span>
        </div>

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
                title={`${option.label}: ${filterCounts[option.id]} passage${
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
            aria-label="Filter passages by title or text"
            className="min-w-0 flex-1 bg-transparent text-[11px] text-text-primary focus:outline-none placeholder:text-text-tertiary"
          />
          {search !== "" && (
            <button
              type="button"
              onClick={() => onSearch("")}
              aria-label="Clear the passage filter"
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
                className={`w-full text-left flex items-center gap-2 pl-2 pr-2 py-1.5 border-l-2 cursor-pointer ${
                  active
                    ? "border-accent-bg bg-bg-muted"
                    : "border-transparent hover:bg-bg-hover"
                }`}
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
            );
          })
        )}
      </div>

      <div className="shrink-0 border-t border-border-default px-2 py-2 flex flex-col gap-1.5">
        <label className="flex items-center gap-1 px-1 text-[10px] text-text-tertiary">
          <span>Decide everything at confidence ≥</span>
          <input
            value={thresholdDraft ?? String(percent(threshold))}
            onChange={(e) => onThresholdDraft(e.target.value)}
            onBlur={(e) => onThresholdCommit(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                e.currentTarget.blur();
              }
            }}
            inputMode="numeric"
            aria-label="Confidence threshold in percent"
            className="w-[42px] px-1 py-0.5 rounded-md bg-bg-muted text-center text-[10px] tabular-nums border border-border-default text-text-primary focus:outline-none focus:border-accent-bg"
          />
          <span>%</span>
        </label>
        <div className="flex items-center gap-1.5">
          <button
            type="button"
            onClick={() => onDecideMany("accepted")}
            disabled={busy || acceptAllCount === 0}
            className="flex-1 inline-flex items-center justify-center gap-1 px-2 py-1 rounded-md bg-bg-muted text-text-secondary text-[11px] hover:bg-bg-hover cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
            title="Accept every undecided span at or above the threshold"
          >
            <Check size={11} /> accept {acceptAllCount}
          </button>
          <button
            type="button"
            onClick={() => onDecideMany("rejected")}
            disabled={busy || rejectAllCount === 0}
            className="flex-1 inline-flex items-center justify-center gap-1 px-2 py-1 rounded-md bg-bg-muted text-text-secondary text-[11px] hover:bg-bg-hover cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
            title="Reject every undecided span at or above the threshold"
          >
            <X size={11} /> reject {rejectAllCount}
          </button>
        </div>
      </div>
    </section>
  );
}
