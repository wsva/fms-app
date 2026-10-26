"use client";

import { Loader2, Save, Trash2 } from "lucide-react";
import type { AlignClip, AlignProposal } from "@/lib/read_aloud/sourceAlign";

/**
 * The chip and the two buttons that decide whether a text may get a clip.
 *
 * A text owns at most one standard audio, and a save never overwrites one: the
 * clip on disk still holds the seconds its wav was cut from, so the only way to
 * give a text a new span is to delete the clip first. The chip says a clip is
 * there — in the queue, and at the head of the passage — and the button beside
 * it is the one verb that state allows, so Save is never offered for a text
 * whose click would silently do nothing.
 */

export type SaveBlocker = "busy" | "refused" | "has-clip";

/**
 * Why this row's span cannot become a clip right now, or `null` when it can.
 *
 * One function for every surface that asks: the button's own `disabled`, the
 * title that explains it, the Save-all count and the Unsaved filter. When they
 * each wrote the condition out themselves they could disagree — a row counted
 * but not clickable, or the other way round.
 */
export function saveBlocker(proposal: AlignProposal, busy: boolean): SaveBlocker | null {
  if (proposal.status === "refused") return "refused";
  if (proposal.clip != null) return "has-clip";
  if (busy) return "busy";
  return null;
}

/** True when this row's span could still become a clip (the Unsaved filter, Save all). */
export function canSave(proposal: AlignProposal): boolean {
  return saveBlocker(proposal, false) === null;
}

export function ClipChip({ clip }: { clip: AlignClip }) {
  const kind = clip.kind === "align" ? "cut from a recording" : clip.kind === "tts" ? "generated" : "uploaded";
  return (
    <span
      className="shrink-0 inline-flex items-center px-1 py-0.5 rounded text-[9px] uppercase tracking-wide bg-accent-bg/15 text-accent-bg"
      title={`This text already has standard audio (${kind})${
        clip.is_default ? "" : ", not the default"
      } — delete the clip to save this span instead`}
    >
      Clip exists
    </span>
  );
}

const BUTTON =
  "shrink-0 inline-flex items-center gap-1.5 px-2.5 py-1.5 rounded-md border text-[11px] cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed disabled:hover:bg-transparent";

/**
 * Store this passage's span. Off while there is nothing to store — a refusal, or
 * a text whose clip has to go first — and while another run holds the page; the
 * title says which.
 */
export function SaveClipButton({
  proposal,
  busy,
  saving,
  onSave,
}: {
  proposal: AlignProposal;
  busy: boolean;
  saving: boolean;
  onSave: () => void;
}) {
  const blocker = saveBlocker(proposal, busy);
  return (
    <button
      type="button"
      onClick={onSave}
      disabled={blocker !== null}
      title={
        blocker === "refused"
          ? "The guess refused this passage — there is no span to save"
          : blocker === "has-clip"
            ? "This text already has a clip — delete it first"
            : saving
              ? "Saving this span…"
              : "Save this span as the text's standard audio"
      }
      className={`${BUTTON} border-accent-bg bg-accent-bg text-white hover:bg-accent-bg/90`}
    >
      {saving ? <Loader2 size={11} className="animate-spin" /> : <Save size={11} />}
      Save
    </button>
  );
}

/** The other half of the rule: a clip has to be deleted before its span can change. */
export function DeleteClipButton({
  clip,
  busy,
  onDelete,
}: {
  clip: AlignClip | null;
  busy: boolean;
  onDelete: () => void;
}) {
  if (!clip) return null;
  return (
    <button
      type="button"
      onClick={onDelete}
      disabled={busy}
      title="Delete this clip — the span can then be saved again"
      className={`${BUTTON} border-border-default bg-bg-muted text-text-secondary hover:bg-bg-hover hover:text-error-text`}
    >
      <Trash2 size={11} />
      Delete clip
    </button>
  );
}
