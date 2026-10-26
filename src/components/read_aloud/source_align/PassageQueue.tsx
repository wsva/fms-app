"use client";

import { useEffect, useMemo, useRef } from "react";
import { AlertTriangle, Loader2, RotateCcw, Save, Search, Wand2, X } from "lucide-react";
import type { AlignProposal } from "@/lib/read_aloud/sourceAlign";
import { ClipChip, DeleteClipButton, SaveClipButton } from "./ClipActions";
import { clock, confidenceTone, percent } from "./format";

/**
 * The queue: every text in reading order, with the two verbs that belong to it
 * at the bottom right of its row — place it again, and save (or delete) the one
 * clip it is allowed to have.
 *
 * Two lines per row, because that is what the rule needs to fit: the title and
 * the "clip exists" chip on top, where the eye starts, and Save / Delete clip
 * underneath, where it ends. A save never overwrites a clip, so a text that
 * already has one shows the delete instead of a Save that would do nothing.
 */

export type QueueFilter = "all" | "unsaved" | "saved" | "refused";

export const QUEUE_FILTERS: { id: QueueFilter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "unsaved", label: "Unsaved" },
  { id: "saved", label: "Saved" },
  { id: "refused", label: "Refused" },
];

/** True when this row's span could still become a clip. */
export function canSave(proposal: AlignProposal): boolean {
  return proposal.status === "proposed" && proposal.clip == null;
}

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
  /** Place one text again, leaving every other span in the file alone. */
  onReguess: (textUuid: string) => void;
  /** Save one row's span as its text's clip. */
  onSave: (textUuid: string) => void;
  /** Delete a text's clip, which is what unblocks its Save. */
  onDeleteClip: (proposal: AlignProposal) => void;
  /** The text being re-guessed right now, so its row shows the spinner. */
  reguessing: string | null;
  /** The text being saved right now, for the same reason. */
  saving: string | null;
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
  onReguess,
  onSave,
  onDeleteClip,
  reguessing,
  saving,
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
            const working = reguessing === proposal.text_uuid;
            return (
              // The row is a box with several buttons in it rather than one
              // button with others inside: a button cannot nest a button.
              <div
                key={proposal.text_uuid}
                className={`flex flex-col border-l-2 ${
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
                  className="min-w-0 text-left flex items-center gap-2 px-2 pt-1.5 pb-0.5 cursor-pointer"
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

                {/* The verbs, bottom right: where the row ends, so a column of
                    them lines up down the rail. */}
                <div className="flex items-center gap-1 pl-10 pr-2 pb-1.5 pt-0.5">
                  <span
                    className="mr-auto min-w-0 truncate text-[10px] text-text-tertiary tabular-nums"
                    title={
                      refused
                        ? undefined
                        : `${clock(proposal.start)} – ${clock(proposal.end)}`
                    }
                  >
                    {refused ? proposal.note || "no span" : clock(proposal.start)}
                  </span>
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
                    className="shrink-0 inline-flex items-center gap-1 px-1.5 py-1 rounded-md border border-border-default text-[10px] text-text-secondary hover:bg-bg-hover hover:text-text-primary cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed disabled:hover:bg-transparent"
                  >
                    {working ? (
                      <Loader2 size={10} className="animate-spin" />
                    ) : (
                      <RotateCcw size={10} />
                    )}
                    Re-guess
                  </button>
                  <DeleteClipButton
                    clip={proposal.clip}
                    busy={busy}
                    onDelete={() => onDeleteClip(proposal)}
                  />
                  <SaveClipButton
                    proposal={proposal}
                    busy={busy}
                    saving={saving === proposal.text_uuid}
                    onSave={() => onSave(proposal.text_uuid)}
                  />
                </div>
              </div>
            );
          })
        )}
      </div>
    </section>
  );
}
