"use client";

/**
 * AgentJobCard — what the panel shows while an agent request is outstanding.
 *
 * One request, one card, wherever the request was made: a generated artifact tab
 * and the glossary both ask the mailbox the same way, so both render the same
 * state. The card speaks *state* rather than instructions — an attached agent
 * claims the request by itself, so what the learner actually wants to know is who
 * holds it and how long is left. The copy-paste prompt survives one disclosure
 * down, for the one case no sentence here can fix: nobody is attached at all.
 */

import { useEffect, useState } from "react";
import { Bot, Check, Copy, Loader2, Wand2, X } from "lucide-react";
import type { ArtifactJob } from "@/lib/read/types";

/** The short label the header row carries beside the busy spinner. */
export function jobChipText(job: ArtifactJob): string {
  if (job.status === "waiting_user") return "Needs your answer";
  if (job.status === "claimed" || job.status === "delivering") return "Agent working…";
  return "Waiting for agent…";
}

/** The inline "an agent has this" line the tab header shows in place of the
 *  artifact's provenance. The amber wand is the passage having changed under a
 *  live request — the card below spells that out in words. */
export function JobStatusChip({ job }: { job: ArtifactJob }) {
  return (
    <span className="flex items-center gap-1 text-accent truncate">
      <Loader2 size={12} className="animate-spin shrink-0" /> {jobChipText(job)}
      {job.stale && <Wand2 size={12} className="text-amber-500 shrink-0" />}
    </span>
  );
}

/** "7 min left on the lease", or the honest alternative when there is no
 *  deadline to count. Pure, so the ticker only has to supply `now`. */
function leaseText(expiresAt: number | null | undefined, now: number): string {
  if (!expiresAt) return "on a live lease";
  const left = Math.ceil((expiresAt - now) / 60_000);
  if (left <= 0) return "the lease is about to lapse";
  return `${left} min left on the lease`;
}

/** The one sentence the card shows where it used to print a prompt.
 *
 *  With the mailbox, an attached agent claims a request by itself, so the
 *  question the learner actually has is "who has it, and is anything
 *  happening" — not "what do I paste where". The prompt survives a disclosure
 *  below, for the one case this sentence cannot help: no agent attached yet. */
function jobStateLine(job: ArtifactJob, now: number): string {
  let state: string;
  if (job.status === "waiting_user") {
    state = "The agent asked a question — answer it in the Mailbox (Ctrl+Space).";
  } else if (job.status === "claimed" || job.status === "delivering") {
    const who = job.claimed_by?.trim() || "Your agent";
    state = `${who} is working on this · ${leaseText(job.expires_at, now)}.`;
  } else if (job.attempts > 0) {
    const n = job.attempts === 1 ? "one attempt" : `${job.attempts} attempts`;
    // The ledger's own sentence, when it kept one: an agent that released
    // the item said why, and that is the part the learner cannot infer from
    // a count.
    const why = job.reason?.trim() ? ` Last one: ${job.reason.trim()}` : "";
    state = `The last attempt came back unfulfilled (${n} so far).${why} An attached agent can claim it again.`;
  } else {
    // The one case the sentence has to answer for: nobody is attached, and the
    // request will sit here until someone is. The page that fixes it is named.
    state =
      "Nobody has picked this up yet. An attached agent claims it by itself — Tools → Agent if none is attached.";
  }
  // The chip's amber wand is the icon; this is the sentence that says what it
  // means, because the passage changing under a live request is the one thing a
  // learner should not have to infer from a colour.
  return job.stale ? `${state} The passage changed since you asked.` : state;
}

type Props = {
  job: ArtifactJob;
  /** The copy/paste fallback, built by the caller: only it knows whether this
   *  request replaces an artifact that already exists. */
  agentPrompt: string;
  /** Cancel the request — the user changed their mind. */
  onCancel: () => void;
};

export default function AgentJobCard({ job, agentPrompt, onCancel }: Props) {
  // The lease line names minutes, so a claimed card has to re-render. 30 s is
  // enough for a countdown read at a glance, and nothing else here ticks. The
  // effect keys on the status, not the row: the poll replaces the object every
  // 12 s, and keying on it would restart the interval before it ever fired.
  const [now, setNow] = useState(() => Date.now());
  const jobStatus = job.status;
  useEffect(() => {
    if (jobStatus !== "claimed" && jobStatus !== "delivering") return;
    // First tick is the claim itself, so the countdown starts from when the row
    // was read rather than from when this panel mounted.
    setNow(Date.now());
    const t = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(t);
  }, [jobStatus]);

  const [copied, setCopied] = useState(false);
  const [clipFailed, setClipFailed] = useState(false);
  // A new job (or its disappearance) resets the copy feedback.
  useEffect(() => {
    setCopied(false);
    setClipFailed(false);
  }, [job.uuid]);
  const copyPrompt = async () => {
    try {
      await navigator.clipboard.writeText(agentPrompt);
      setCopied(true);
      setClipFailed(false);
    } catch {
      setClipFailed(true);
    }
  };

  return (
    <div className="flex flex-col gap-2 rounded-lg border border-border-default bg-bg-muted px-2.5 py-2">
      <div className="flex items-start gap-1.5 text-xs text-text-secondary">
        <span className="mt-px shrink-0 text-text-tertiary">
          {job.status === "claimed" || job.status === "delivering" ? (
            <Loader2 size={12} className="animate-spin" />
          ) : (
            <Bot size={12} />
          )}
        </span>
        <span className="min-w-0 flex-1">{jobStateLine(job, now)}</span>
        <button
          title="Cancel agent request"
          className="shrink-0 p-1 rounded text-text-tertiary hover:bg-bg-hover hover:text-red-500 cursor-pointer"
          onClick={onCancel}
        >
          <X size={13} />
        </button>
      </div>
      <details>
        <summary className="cursor-pointer text-[11px] text-text-tertiary hover:text-text-secondary">
          No agent attached? Copy the request for one.
        </summary>
        <div className="mt-2 flex flex-col gap-1.5">
          <div className="flex items-center justify-between gap-2">
            <span className="text-[11px] text-text-tertiary">
              Paste it into an agent that cannot attach on its own.
            </span>
            <button
              type="button"
              className="flex items-center gap-1 px-2 py-1 rounded-md text-xs font-medium border border-border-default text-text-secondary hover:bg-bg-hover cursor-pointer shrink-0"
              onClick={() => void copyPrompt()}
            >
              {copied ? <Check size={13} className="text-green-500" /> : <Copy size={13} />}
              {copied ? "Copied" : "Copy prompt"}
            </button>
          </div>
          <textarea
            readOnly
            value={agentPrompt}
            onFocus={(e) => e.currentTarget.select()}
            className="w-full h-32 p-2 rounded-md bg-bg-base border border-border-default text-[11px] text-text-primary font-mono resize-y"
          />
          {clipFailed && (
            <p className="text-[11px] text-amber-500">
              Could not reach the clipboard — select the text above and copy it.
            </p>
          )}
        </div>
      </details>
    </div>
  );
}
