"use client";

import { Check, Loader2, Upload, X } from "lucide-react";
import type { AlignCounts, AlignProposal, AlignProposalState } from "@/lib/read_aloud/sourceAlign";
import { clock, confidenceTone, percent } from "./format";

/**
 * The end of the job: what has been accepted, what still has to be applied, and
 * the batch verdicts that decide a whole confidence band at once.
 *
 * These lived in the review rail's footer, four lines tall and always in the
 * way of the queue. They are their own page now, where the accepted set can be
 * read as a list before any audio is cut.
 */

const CARD = "rounded-xl border border-border-default bg-bg-card overflow-hidden";
const CARD_HEAD = "flex items-baseline gap-2 px-4 py-2.5 border-b border-border-default";
const CARD_BODY = "px-4 py-3 flex flex-col gap-2.5";
const ACTION =
  "inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md text-xs cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed";
const NEUTRAL = `${ACTION} bg-bg-muted text-text-secondary hover:bg-bg-hover`;
const PRIMARY = `${ACTION} bg-accent-bg text-white`;
const ROW = "flex items-center gap-2 rounded-md border border-border-default px-2 py-1.5";

type Props = {
  proposals: AlignProposal[];
  counts: AlignCounts | null;
  busy: string | null;
  threshold: number;
  /** The half-typed threshold, so "9" is not read as 9 % while it is typed. */
  thresholdDraft: string | null;
  onThresholdDraft: (value: string) => void;
  onThresholdCommit: (value: string) => void;
  acceptAllCount: number;
  rejectAllCount: number;
  onDecideMany: (state: AlignProposalState) => void;
  onApply: () => void;
  /** Jump to this passage in the Review tab. */
  onOpen: (textUuid: string) => void;
  onReject: (proposal: AlignProposal) => void;
};

export default function CutTab({
  proposals,
  counts,
  busy,
  threshold,
  thresholdDraft,
  onThresholdDraft,
  onThresholdCommit,
  acceptAllCount,
  rejectAllCount,
  onDecideMany,
  onApply,
  onOpen,
  onReject,
}: Props) {
  const open = counts?.open ?? 0;
  const applied = counts?.applied ?? 0;
  const cutting = busy === "cutting";
  const accepted = proposals.filter((p) => p.state === "accepted" && !p.applied_uuid);
  const stored = proposals.filter((p) => p.applied_uuid);

  return (
    <div className="flex flex-col gap-3 max-w-[820px]">
      <section className={CARD}>
        <div className={CARD_HEAD}>
          <h3 className="text-xs font-semibold text-text-primary">Cut clips</h3>
          <span className="ml-auto text-[10px] text-text-tertiary tabular-nums">
            {open} to cut · {applied} stored
          </span>
        </div>
        <div className={CARD_BODY}>
          <p className="text-xs text-text-secondary leading-relaxed">
            Accepted spans become this dataset's standard audio — one clip per text, cut from the
            recording it was placed in. Applying is the only step that writes to media/.
          </p>
          <div className="flex items-center gap-2 flex-wrap">
            <button
              type="button"
              onClick={onApply}
              disabled={busy != null || open === 0}
              className={open > 0 ? PRIMARY : NEUTRAL}
              title={
                open === 0
                  ? "Nothing accepted is waiting to be cut"
                  : `Cut ${open} accepted span${open === 1 ? "" : "s"} into the dataset`
              }
            >
              {cutting ? <Loader2 size={12} className="animate-spin" /> : <Upload size={12} />}
              {cutting ? "Cutting…" : open > 0 ? `Apply ${open}` : "Apply"}
            </button>
            {open === 0 && applied === 0 && (
              <span className="text-[11px] text-text-tertiary">
                Accept spans in Review first, or use the batch verdicts below.
              </span>
            )}
          </div>
        </div>
      </section>

      <section className={CARD}>
        <div className={CARD_HEAD}>
          <h3 className="text-xs font-semibold text-text-primary">Batch verdicts</h3>
          <span className="ml-auto text-[10px] text-text-tertiary tabular-nums">
            ≥ {percent(threshold)}%
          </span>
        </div>
        <div className={CARD_BODY}>
          <p className="text-[11px] text-text-tertiary leading-relaxed">
            Decide every undecided span at or above a confidence in one go. The counts are
            computed here, so a button never offers a number it would not change.
          </p>
          <label className="flex items-center gap-2 text-[11px] text-text-tertiary">
            <span>Confidence at or above</span>
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
              className="w-[52px] px-1.5 py-1 rounded-md bg-bg-muted text-center text-[11px] tabular-nums border border-border-default text-text-primary focus:outline-none focus:border-accent-bg"
            />
            <span>%</span>
          </label>
          <div className="flex items-center gap-2 flex-wrap">
            <button
              type="button"
              onClick={() => onDecideMany("accepted")}
              disabled={busy != null || acceptAllCount === 0}
              className={NEUTRAL}
              title="Accept every undecided span at or above the threshold"
            >
              <Check size={12} />
              accept {acceptAllCount}
            </button>
            <button
              type="button"
              onClick={() => onDecideMany("rejected")}
              disabled={busy != null || rejectAllCount === 0}
              className={NEUTRAL}
              title="Reject every undecided span at or above the threshold"
            >
              <X size={12} />
              reject {rejectAllCount}
            </button>
          </div>
        </div>
      </section>

      <section className={CARD}>
        <div className={CARD_HEAD}>
          <h3 className="text-xs font-semibold text-text-primary">Accepted, not cut</h3>
          <span className="ml-auto text-[10px] text-text-tertiary tabular-nums">
            {accepted.length}
          </span>
        </div>
        <div className={CARD_BODY}>
          {accepted.length === 0 ? (
            <p className="text-xs text-text-tertiary">
              Nothing accepted is waiting. Accept a span in Review, or use the batch verdicts
              above.
            </p>
          ) : (
            <div className="flex flex-col gap-1">
              {accepted.map((proposal) => (
                <div key={proposal.text_uuid} className={ROW}>
                  <button
                    type="button"
                    onClick={() => onOpen(proposal.text_uuid)}
                    className="min-w-0 flex-1 text-left cursor-pointer"
                    title="Open this passage in Review"
                  >
                    <span className="block text-xs text-text-primary truncate">
                      {proposal.title || "Untitled"}
                    </span>
                    <span className="block text-[10px] text-text-tertiary tabular-nums">
                      {clock(proposal.start)} → {clock(proposal.end)}
                    </span>
                  </button>
                  <span
                    className={`shrink-0 text-[10px] tabular-nums ${confidenceTone(
                      proposal.confidence
                    )}`}
                  >
                    {percent(proposal.confidence)}%
                  </span>
                  <button
                    type="button"
                    onClick={() => onReject(proposal)}
                    disabled={busy != null}
                    aria-label={`Reject ${proposal.title || "this passage"}`}
                    title="Reject — leave this span uncut"
                    className="shrink-0 inline-flex items-center justify-center w-6 h-6 rounded-md bg-bg-muted text-text-secondary hover:bg-bg-hover cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed"
                  >
                    <X size={12} />
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
      </section>

      {applied > 0 && (
        <section className={CARD}>
          <div className={CARD_HEAD}>
            <h3 className="text-xs font-semibold text-text-primary">Stored</h3>
            <span className="ml-auto text-[10px] text-text-tertiary tabular-nums">{applied}</span>
          </div>
          <div className={CARD_BODY}>
            <div className="flex flex-col gap-1">
              {stored.map((proposal) => (
                <button
                  key={proposal.text_uuid}
                  type="button"
                  onClick={() => onOpen(proposal.text_uuid)}
                  className={`${ROW} text-left cursor-pointer hover:bg-bg-hover`}
                  title="Open this passage in Review"
                >
                  <span className="shrink-0 text-accent-bg" aria-hidden>
                    <Check size={11} />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block text-xs text-text-primary truncate">
                      {proposal.title || "Untitled"}
                    </span>
                    <span className="block text-[10px] text-text-tertiary tabular-nums">
                      {clock(proposal.start)} → {clock(proposal.end)}
                    </span>
                  </span>
                  <span className="shrink-0 text-[10px] text-text-tertiary">stored</span>
                </button>
              ))}
            </div>
          </div>
        </section>
      )}
    </div>
  );
}
