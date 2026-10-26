"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import {
  applyAlignProposals,
  guessAlignSpanOne,
  guessAlignSpans,
  listAlignProposals,
  moveAlignSpan,
  type AlignProposal,
  type AlignProposals,
} from "@/lib/read_aloud/sourceAlign";
import { deleteStandardAudio } from "@/lib/read_aloud/standardAudio";
import type { ConfirmRequest } from "@/components/read_book/ConfirmDialog";
import { clock, errorText } from "./format";

/** How many spans one `apply` call stores; the loop repeats until done. */
const APPLY_BATCH = 10;

/** How long a burst of nudges waits before it becomes one rewrite of the file. */
const NUDGE_COMMIT_MS = 300;

/**
 * The Texts half of the page: the spans, and every write that touches them.
 *
 * Two kinds of write live here, and they are not the same act. **Guessing** and
 * **moving an edge** rewrite `proposals.json` — the span is the truth, and a
 * nudge is the reader saying where it goes. **Saving** writes a standard-audio
 * row and leaves that file alone; it never overwrites a clip, so a text that
 * already has one comes back in `skipped` until that clip is deleted.
 *
 * A nudge is a local edit first and a write second: the field must not wait a
 * round trip to show where the edge went, and a burst of clicks must become one
 * rewrite rather than ten.
 */
export function useSpanDrafts({
  datasetUuid,
  pipeline,
  isBusy,
  setError,
  setNote,
  setConfirm,
  setSelected,
  revealQueue,
  onApplied,
  reloadSources,
}: {
  datasetUuid: string;
  /** Run one locked step, writing pending nudges before it starts. */
  pipeline: (label: string, task: () => Promise<void>) => void;
  /** Whether any step is in flight — the nudge writer must know. */
  isBusy: () => boolean;
  setError: (message: string | null) => void;
  setNote: (note: { kind: "ok" | "warn"; text: string } | null) => void;
  /** The question asked before deleting a clip this page did not cut. */
  setConfirm: (request: ConfirmRequest | null) => void;
  setSelected: (textUuid: string | null) => void;
  /** A guess is about the queue, so it lands there. */
  revealQueue: () => void;
  /** Called after rows were stored, so the page reloads its clip badges. */
  onApplied: () => void;
  /** Re-read the recording index — `refresh` reloads both halves of the page. */
  reloadSources: () => Promise<unknown>;
}) {
  const [proposals, setProposals] = useState<AlignProposals | null>(null);
  /** The text whose span is being stored right now; its row shows the spinner. */
  const [savingText, setSavingText] = useState<string | null>(null);
  /** The text being re-guessed right now; its row shows the spinner. */
  const [guessingText, setGuessingText] = useState<string | null>(null);
  /** Nudged spans, by text uuid, until the write is acknowledged. */
  const [edits, setEdits] = useState<Record<string, { start: number; end: number }>>({});
  const timersRef = useRef<Record<string, ReturnType<typeof setTimeout>>>({});
  const pendingRef = useRef<Record<string, { start: number; end: number }>>({});

  const spans = proposals?.proposals ?? [];
  const titleOf = (textUuid: string) =>
    spans.find((p) => p.text_uuid === textUuid)?.title || "This text";

  /**
   * Load the proposals, and with them the recording index.
   *
   * `dropEdits` is for the calls that move spans on the server: a guess replaces
   * every span, so a nudge kept here would show a number the file no longer
   * holds. Saving or deleting never moves an edge, so the edits stay — clearing
   * them would revert a nudge the reader has just made.
   */
  const refresh = useCallback(
    async (dropEdits = false) => {
      if (!datasetUuid) return;
      try {
        const [, list] = await Promise.all([reloadSources(), listAlignProposals(datasetUuid)]);
        setProposals(list);
        if (dropEdits) {
          Object.values(timersRef.current).forEach(clearTimeout);
          timersRef.current = {};
          pendingRef.current = {};
          setEdits({});
        }
        setError(null);
      } catch (e) {
        setError(errorText(e));
      }
    },
    [datasetUuid, reloadSources, setError]
  );

  /**
   * Write one nudged span.
   *
   * The span is written as it stands; there is nothing else on the row to
   * carry. A clip the text already owns is left alone — changing a saved span
   * means deleting that clip first.
   *
   * `force` exists for the pipeline, which writes the pending spans *before* it
   * starts its own step. A debounce that fires mid-step is still dropped — a
   * nudge must not land on a proposal set a guess is about to replace — but a
   * nudge made just before the step must reach the file, or the step reads a
   * span the reader did not choose and writes that one back.
   */
  const flush = useCallback(
    async (textUuid: string, force = false) => {
      const pending = pendingRef.current[textUuid];
      if (!pending) return;
      delete pendingRef.current[textUuid];
      clearTimeout(timersRef.current[textUuid]);
      delete timersRef.current[textUuid];
      if (isBusy() && !force) {
        setEdits((current) => {
          const next = { ...current };
          delete next[textUuid];
          return next;
        });
        return;
      }
      try {
        await moveAlignSpan(datasetUuid, textUuid, pending.start, pending.end);
        // The command answers with counts, not the row, so patch it here and
        // drop the optimistic copy: the field then shows what the file holds.
        setProposals((current) =>
          current
            ? {
                ...current,
                proposals: current.proposals.map((p) =>
                  p.text_uuid === textUuid ? { ...p, start: pending.start, end: pending.end } : p
                ),
              }
            : current
        );
        setEdits((current) => {
          const edit = current[textUuid];
          if (!edit || edit.start !== pending.start || edit.end !== pending.end) return current;
          const next = { ...current };
          delete next[textUuid];
          return next;
        });
      } catch (e) {
        setError(errorText(e));
        setEdits((current) => {
          const next = { ...current };
          delete next[textUuid];
          return next;
        });
      }
    },
    [datasetUuid, isBusy, setError]
  );

  /** Write every span that is still waiting on its debounce. */
  const flushPending = useCallback(
    (force = false) =>
      Promise.all(Object.keys(pendingRef.current).map((textUuid) => flush(textUuid, force))),
    [flush]
  );

  // A nudge made a blink before leaving the page still belongs in the file.
  useEffect(() => {
    const timers = timersRef.current;
    return () => {
      Object.values(timers).forEach(clearTimeout);
      timersRef.current = {};
      const pending = pendingRef.current;
      pendingRef.current = {};
      Object.entries(pending).forEach(([textUuid, span]) => {
        void moveAlignSpan(datasetUuid, textUuid, span.start, span.end).catch(() => {});
      });
    };
  }, [datasetUuid]);

  /** A different dataset is a different set of spans: nothing from the old one survives. */
  const reset = useCallback(() => {
    setProposals(null);
    setEdits({});
    Object.values(timersRef.current).forEach(clearTimeout);
    timersRef.current = {};
    pendingRef.current = {};
  }, []);

  const guess = () =>
    pipeline("guessing", async () => {
      const result = await guessAlignSpans(datasetUuid);
      setSelected(null);
      await refresh(true);
      // The point of a guess is the queue: land where the spans are saved.
      revealQueue();
      setNote({
        kind: result.refused > 0 ? "warn" : "ok",
        text: `${result.proposed} of ${result.passages} texts placed (mean ${Math.round(
          result.mean_confidence * 100
        )}%), ${result.refused} refused.`,
      });
    });

  /**
   * Re-place one text.
   *
   * The whole-set guess rewrites every row, so it is the wrong tool for fixing
   * one: it would replace the edges that were just nudged elsewhere. This one
   * starts where the span before it ends, keeps that span, and replaces only
   * this row of `proposals.json`.
   */
  const guessOne = (textUuid: string) =>
    pipeline("reguessing", async () => {
      setGuessingText(textUuid);
      try {
        const result = await guessAlignSpanOne(datasetUuid, textUuid);
        await refresh(true);
        setSelected(textUuid);
        const title = titleOf(textUuid);
        setNote({
          kind: result.placed && result.note === "" ? "ok" : "warn",
          text: result.placed
            ? `${title}: ${clock(result.start)} → ${clock(result.end)} in ${result.source_name}${
                result.note === "" ? "." : ` — ${result.note}`
              }`
            : `${title} was not placed: ${result.note}`,
        });
      } finally {
        setGuessingText(null);
      }
    });

  /**
   * Save one text's span — the row's Save button.
   *
   * `apply` skips a text that already has a clip, so the row only offers Save
   * when there is nothing to overwrite; the note still says so, in case a clip
   * was made on another page between two renders.
   */
  const saveOne = (textUuid: string) =>
    pipeline("saving", async () => {
      setSavingText(textUuid);
      try {
        const result = await applyAlignProposals(datasetUuid, 1, textUuid);
        const saved = result.applied > 0;
        await refresh();
        if (saved) onApplied();
        const title = titleOf(textUuid);
        setNote(
          saved
            ? { kind: "ok", text: `${title}: the span is now this text's clip.` }
            : result.skipped_count > 0
              ? { kind: "warn", text: `${title} already has a clip — delete it, then save again.` }
              : { kind: "warn", text: `${title} has no span to save.` }
        );
      } finally {
        setSavingText(null);
      }
    });

  /**
   * Save every span whose text has no clip yet.
   *
   * The loop repeats because `apply` is bounded, and it stops on no progress as
   * well as on nothing left: a span that failed stays unsavable, so repeating it
   * would fail forever. The texts that already have a clip are not touched —
   * they are counted on the rows, not overwritten here.
   */
  const saveAll = () =>
    pipeline("saving", async () => {
      let stored = 0;
      let failures = 0;
      for (;;) {
        const result = await applyAlignProposals(datasetUuid, APPLY_BATCH);
        stored += result.applied;
        failures += result.failed_count;
        if (result.remaining === 0 || result.applied === 0) break;
      }
      await refresh();
      if (stored > 0) onApplied();
      setNote({
        kind: failures > 0 || stored === 0 ? "warn" : "ok",
        text:
          stored === 0
            ? "Nothing was saved: no placed text was free of a clip."
            : `${stored} clip${stored === 1 ? "" : "s"} saved as standard audio${
                failures > 0 ? `, ${failures} failed — see the rows` : ""
              }.`,
      });
    });

  /**
   * Delete a text's clip, which is the only way its span can be replaced.
   *
   * A clip this page cut is one Save away from coming back, so it goes without
   * asking; a generated or uploaded clip is not, and that one asks first.
   */
  const deleteClip = (proposal: AlignProposal) => {
    const clip = proposal.clip;
    if (!clip || isBusy()) return;
    const remove = () =>
      pipeline("deleting", async () => {
        await deleteStandardAudio(datasetUuid, clip.uuid);
        await refresh();
        onApplied();
        setNote({
          kind: "ok",
          text: `${proposal.title || "This text"}: clip deleted — Save now stores the span.`,
        });
      });
    if (clip.kind === "align") {
      remove();
      return;
    }
    setConfirm({
      title: "Delete this clip?",
      message: (
        <>
          <span className="text-text-primary">{proposal.title || "This text"}</span> has
          {clip.kind === "tts" ? " a generated clip" : " an uploaded clip"} ({clip.note}). Deleting it
          removes the file, and nothing on this page can make the same one again — the span would
          take its place.
        </>
      ),
      confirmLabel: "Delete clip",
      danger: true,
      onConfirm: remove,
    });
  };

  /** Nudge one edge: show it now, write it once the burst settles. */
  const moveSpan = (proposal: AlignProposal, start: number, end: number) => {
    setEdits((current) => ({ ...current, [proposal.text_uuid]: { start, end } }));
    pendingRef.current[proposal.text_uuid] = { start, end };
    clearTimeout(timersRef.current[proposal.text_uuid]);
    timersRef.current[proposal.text_uuid] = setTimeout(() => {
      void flush(proposal.text_uuid);
    }, NUDGE_COMMIT_MS);
  };

  return {
    proposals,
    spans,
    edits,
    savingText,
    guessingText,
    refresh,
    reset,
    flush,
    flushPending,
    moveSpan,
    guess,
    guessOne,
    saveOne,
    saveAll,
    deleteClip,
  };
}
