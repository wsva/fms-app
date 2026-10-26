"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useIsMobile } from "@/hooks/useIsMobile";
import {
  applyAlignProposals,
  decideAlignProposals,
  guessAlignSpanOne,
  guessAlignSpans,
  listAlignProposals,
  readAlignTranscript,
  registerAlignSources,
  transcribeAlignSource,
  type AlignProposal,
  type AlignProposals,
  type AlignSourceIndex,
  type AlignTranscript,
} from "@/lib/read_aloud/sourceAlign";
import { deleteStandardAudio } from "@/lib/read_aloud/standardAudio";
import { FolderOpen, ListChecks } from "lucide-react";
import { TabBar, type TabBarItem } from "@/components/ui/toolbar";
import ConfirmDialog, { type ConfirmRequest } from "@/components/read_book/ConfirmDialog";
import PassageEditor from "./PassageEditor";
import PassageQueue, {
  QUEUE_FILTERS,
  canSave,
  matchesFilter,
  type QueueFilter,
} from "./PassageQueue";
import SourceAudiosTab from "./SourceAudiosTab";
import { Splitter, useSplitWidth } from "./Splitter";
import { COARSE_STEP, CONTEXT_PAD, MIN_SPAN, NUDGE_STEP, clock, errorText, roundMs } from "./format";

/**
 * The standard-audio page: a folder of recordings in, one clip per text out.
 *
 * Two tabs, in the order the work is done. **Source Audios** is the folder, one
 * row per recording with its two verbs — listen, read what came back — and the
 * transcript on the right, because "did it hear this right?" is answered by the
 * words and their times, not by a number. **Texts** is the work: the queue on
 * the left, one passage open beside it with its waveform and its typed edges,
 * and the two verbs each row owns — place it again, and save (or delete) the
 * one clip it is allowed to have.
 *
 * There is no verdict to give: a placed span is the answer, and saving it writes
 * a row that names the recording and the seconds it covers. A save never
 * overwrites a clip, so a text that already has standard audio is skipped until
 * that clip is deleted here — which is why every row carries a delete as well as
 * a save.
 *
 * State and every `read_aloud_align_*` call live here; the views are renderers.
 */

/** How many spans one `apply` call stores; the loop repeats until done. */
const APPLY_BATCH = 10;

/** How long a burst of nudges waits before it becomes one rewrite of the file. */
const NUDGE_COMMIT_MS = 300;

/**
 * The Texts seam: the rail as it was, and neither side becomes a sliver. The
 * storage key keeps the old name so a width someone already dragged survives.
 */
const TEXTS_SPLIT = {
  storageKey: "read-aloud:review:split",
  initial: 300,
  leftMin: 240,
  rightMin: 420,
};

type Note = { kind: "ok" | "warn"; text: string };

/** The two jobs, one per tab, in the order they are done. */
type TabId = "sources" | "texts";

type Props = {
  datasetUuid: string;
  /** Called after spans were cut, so the page reloads its clip badges. */
  onApplied: () => void;
};

export default function SourceAlignWorkspace({ datasetUuid, onApplied }: Props) {
  const mobile = useIsMobile();
  const [index, setIndex] = useState<AlignSourceIndex | null>(null);
  const [proposals, setProposals] = useState<AlignProposals | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<Note | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [playing, setPlaying] = useState(false);
  const [playhead, setPlayhead] = useState<number | null>(null);
  /** The text whose span is being stored right now; its row shows the spinner. */
  const [savingText, setSavingText] = useState<string | null>(null);
  /** Nudged spans, by text uuid, until the write is acknowledged. */
  const [edits, setEdits] = useState<Record<string, { start: number; end: number }>>({});
  const [selected, setSelected] = useState<string | null>(null);
  const [tab, setTab] = useState<TabId>("sources");
  /** The recording whose result is in the right pane of the Source Audios tab. */
  const [sourceSel, setSourceSel] = useState<string | null>(null);
  /** That recording's cached transcript, read on demand. */
  const [transcript, setTranscript] = useState<AlignTranscript | null>(null);
  const [transcriptBusy, setTranscriptBusy] = useState(false);
  const [transcriptError, setTranscriptError] = useState<string | null>(null);
  /** The file being listened to right now; its row shows the spinner. */
  const [listeningUuid, setListeningUuid] = useState<string | null>(null);
  /** The text being re-guessed right now; its row shows the spinner. */
  const [guessingText, setGuessingText] = useState<string | null>(null);
  /** The "listen again?" question, which is the one destructive-shaped click here. */
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const [filter, setFilter] = useState<QueueFilter>("all");
  const [search, setSearch] = useState("");
  /** Which edge the arrow keys move; the fields show it. */
  const [activeEdge, setActiveEdge] = useState<"start" | "end">("start");

  // The seam between the rail and the passage, remembered like the other tab's.
  const textsSplit = useSplitWidth(TEXTS_SPLIT);

  const audioRef = useRef<HTMLAudioElement | null>(null);
  const boundsRef = useRef<{ from: number; to: number } | null>(null);
  const stopRef = useRef(false);
  const busyRef = useRef<string | null>(null);
  const proposalsRef = useRef<AlignProposals | null>(null);
  const timersRef = useRef<Record<string, ReturnType<typeof setTimeout>>>({});
  const pendingRef = useRef<Record<string, { start: number; end: number }>>({});
  /** Which transcript request is the current one, so a slow answer can be dropped. */
  const transcriptSeq = useRef(0);

  useEffect(() => {
    busyRef.current = busy;
  }, [busy]);

  useEffect(() => {
    proposalsRef.current = proposals;
  }, [proposals]);

  const stopAudio = useCallback(() => {
    if (audioRef.current) {
      audioRef.current.pause();
      audioRef.current = null;
    }
    boundsRef.current = null;
    setPlaying(false);
  }, []);

  useEffect(() => stopAudio, [stopAudio]);

  /**
   * Load the index and the proposals.
   *
   * `dropEdits` is for the calls that move spans on the server: a guess
   * replaces every span, so a nudge kept here would show a number the file no
   * longer holds. Accepting or cutting never moves an edge, so the edits stay —
   * clearing them would revert a nudge the user has just made.
   */
  const refresh = useCallback(
    async (dropEdits = false) => {
      if (!datasetUuid) return;
      try {
        const [sources, list] = await Promise.all([
          registerAlignSources(datasetUuid),
          listAlignProposals(datasetUuid),
        ]);
        setIndex(sources);
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
    [datasetUuid]
  );

  /**
   * Write one nudged span.
   *
   * `state` is read from the current proposals rather than captured when the
   * nudge was made, and it is carried through untouched — this page has no
   * verdicts any more, so the field only travels with the row it came with.
   * Reading it late is what keeps a queued write from resurrecting an old one.
   *
   * `force` exists for the pipeline, which writes the pending spans *before* it
   * starts its own step. A debounce that fires mid-step is still dropped — a
   * nudge must not land on a proposal set a guess is about to replace — but a
   * nudge made just before the step must reach the file, or the step reads a
   * span the user did not choose and writes that one back.
   */
  const flush = useCallback(
    async (textUuid: string, force = false) => {
      const pending = pendingRef.current[textUuid];
      if (!pending) return;
      delete pendingRef.current[textUuid];
      clearTimeout(timersRef.current[textUuid]);
      delete timersRef.current[textUuid];
      if (busyRef.current && !force) {
        setEdits((current) => {
          const next = { ...current };
          delete next[textUuid];
          return next;
        });
        return;
      }
      const state =
        proposalsRef.current?.proposals.find((p) => p.text_uuid === textUuid)?.state ?? "pending";
      try {
        await decideAlignProposals({
          datasetUuid,
          textUuid,
          state,
          start: pending.start,
          end: pending.end,
        });
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
    [datasetUuid]
  );

  useEffect(() => {
    void refresh(true);
  }, [refresh]);

  // A different dataset is a different folder, a different set of spans and a
  // different transcript cache: nothing from the old one may survive.
  useEffect(() => {
    setIndex(null);
    setProposals(null);
    setNote(null);
    setError(null);
    setEdits({});
    setSelected(null);
    setTab("sources");
    setSourceSel(null);
    setTranscript(null);
    setTranscriptError(null);
    setListeningUuid(null);
    setFilter("all");
    setSearch("");
    setPlayhead(null);
    stopAudio();
  }, [datasetUuid, stopAudio]);

  // A nudge made a blink before leaving the page still belongs in the file.
  useEffect(() => {
    const timers = timersRef.current;
    return () => {
      Object.values(timers).forEach(clearTimeout);
      timersRef.current = {};
      const pending = pendingRef.current;
      pendingRef.current = {};
      Object.entries(pending).forEach(([textUuid, span]) => {
        const state =
          proposalsRef.current?.proposals.find((p) => p.text_uuid === textUuid)?.state ?? "pending";
        void decideAlignProposals({
          datasetUuid,
          textUuid,
          state,
          start: span.start,
          end: span.end,
        }).catch(() => {});
      });
    };
  }, [datasetUuid]);

  useEffect(() => {
    stopAudio();
    setPlayhead(null);
  }, [selected, stopAudio]);

  /** Write every span that is still waiting on its debounce. */
  const flushPending = (force = false) =>
    Promise.all(Object.keys(pendingRef.current).map((textUuid) => flush(textUuid, force)));

  /**
   * One operation at a time: the progress text is also the lock.
   *
   * Pending nudges are written first, while the step has not started yet, so
   * the step reads spans the user chose. The lock goes up immediately rather
   * than on the next render, because a double click must not start two
   * transcriptions.
   */
  const pipeline = (label: string, task: () => Promise<void>) => {
    if (busyRef.current) return;
    busyRef.current = label;
    setBusy(label);
    setError(null);
    void (async () => {
      try {
        await flushPending(true);
        await task();
      } catch (e) {
        setError(errorText(e));
      } finally {
        busyRef.current = null;
        setBusy(null);
      }
    })();
  };

  const pickFolder = () =>
    pipeline("picking", async () => {
      const picked = await open({
        directory: true,
        multiple: false,
        title: "Folder that holds the recordings",
      });
      if (typeof picked !== "string") return;
      const sources = await registerAlignSources(datasetUuid, picked);
      setIndex(sources);
      setSelected(null);
      // Another folder is another set of files: the result on the right would
      // be from a file this dataset no longer has.
      setSourceSel(null);
      setTranscript(null);
      setTranscriptError(null);
      setNote({
        kind: "ok",
        text: `${sources.source_count} recording${
          sources.source_count === 1 ? "" : "s"
        } from ${sources.directory}`,
      });
    });

  /**
   * One file per call, because a chapter is minutes of model time and nothing
   * should hold a request open that long. The loop stops between files, so the
   * Stop button is honoured before the next transcription starts.
   */
  const transcribeAll = () =>
    pipeline("transcribing", async () => {
      stopRef.current = false;
      for (;;) {
        const result = await transcribeAlignSource(datasetUuid);
        setIndex(result);
        if (result.done || result.remaining === 0) {
          setNote({ kind: "ok", text: `Every recording is transcribed (${result.source_count}).` });
          break;
        }
        setNote({
          kind: "ok",
          text: `Heard ${result.words ?? 0} words in ${result.file_name ?? ""} (${clock(
            result.duration ?? 0
          )}) — ${result.remaining} left.`,
        });
        if (stopRef.current) break;
      }
    });

  /**
   * Read one recording's transcript into the right pane.
   *
   * A read, not a pipeline step: it takes no lock (the file is already on
   * disk), and two fast clicks must not paint the first answer last, hence the
   * sequence number.
   */
  const openTranscript = async (sourceUuid: string) => {
    const seq = ++transcriptSeq.current;
    setSourceSel(sourceUuid);
    setTranscript(null);
    setTranscriptError(null);
    setTranscriptBusy(true);
    try {
      const result = await readAlignTranscript(datasetUuid, sourceUuid);
      if (seq === transcriptSeq.current) setTranscript(result);
    } catch (e) {
      if (seq === transcriptSeq.current) setTranscriptError(errorText(e));
    } finally {
      if (seq === transcriptSeq.current) setTranscriptBusy(false);
    }
  };

  const transcribeOne = (sourceUuid: string) => {
    const file = sources.find((s) => s.uuid === sourceUuid);
    if (!file) return;
    // A re-listen replaces the word timings the spans were placed against, so
    // it is asked for rather than assumed: the same click on the same row
    // usually means "show me" more often than "do it again".
    if (file.words != null) {
      setConfirm({
        title: "Listen again?",
        message: (
          <>
            <span className="text-text-primary">{file.file_name}</span> already has a transcript
            of {(file.words ?? 0).toLocaleString()} words. Listening again replaces it, and the
            passages already placed from this file keep the spans they were given — place the
            passages again afterwards.
          </>
        ),
        confirmLabel: "Listen again",
        danger: false,
        onConfirm: () => {
          void listenTo(sourceUuid);
        },
      });
      return;
    }
    void listenTo(sourceUuid);
  };

  /**
   * One file per call, because a chapter is minutes of model time and nothing
   * should hold a request open that long. The result is opened on the right as
   * soon as it lands: the answer to "listen" is the words, not a toast.
   */
  const listenTo = (sourceUuid: string) => {
    const repeat = sources.find((s) => s.uuid === sourceUuid)?.words != null;
    return pipeline("listening", async () => {
      setListeningUuid(sourceUuid);
      try {
        const result = await transcribeAlignSource(datasetUuid, sourceUuid);
        setIndex(result);
        await openTranscript(sourceUuid);
        setNote({
          kind: "ok",
          text: `Heard ${result.words ?? 0} words in ${result.file_name ?? ""} (${clock(
            result.duration ?? 0
          )})${repeat ? " — re-guess to re-place this file's passages." : "."}`,
        });
      } finally {
        setListeningUuid(null);
      }
    });
  };

  const guess = () =>
    pipeline("guessing", async () => {
      const result = await guessAlignSpans(datasetUuid);
      setSelected(null);
      await refresh(true);
      // The point of a guess is the queue: land where the spans are saved.
      setTab("texts");
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
        const title = spans.find((p) => p.text_uuid === textUuid)?.title || "This text";
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
        const title = spans.find((p) => p.text_uuid === textUuid)?.title || "This text";
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
    if (!clip || busyRef.current) return;
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

  /**
   * Nudging is a local edit first and a write second: the field must not wait a
   * round trip to show where the edge went, and a burst of clicks must become
   * one rewrite of the proposals file rather than ten.
   */
  const moveSpan = (proposal: AlignProposal, start: number, end: number) => {
    setEdits((current) => ({ ...current, [proposal.text_uuid]: { start, end } }));
    pendingRef.current[proposal.text_uuid] = { start, end };
    clearTimeout(timersRef.current[proposal.text_uuid]);
    timersRef.current[proposal.text_uuid] = setTimeout(() => {
      void flush(proposal.text_uuid);
    }, NUDGE_COMMIT_MS);
  };

  const startPlayback = (from: number, to: number) => {
    const path = selectedProposal?.source_path;
    if (!path) return;
    stopAudio();
    const el = new Audio(convertFileSrc(path));
    audioRef.current = el;
    boundsRef.current = { from, to };
    el.currentTime = Math.max(0, from);
    setPlayhead(Math.max(0, from));
    setPlaying(true);
    // The element plays the whole recording, so the span's end is what stops it.
    el.ontimeupdate = () => {
      setPlayhead(el.currentTime);
      const bounds = boundsRef.current;
      if (!bounds) return;
      if (el.currentTime >= bounds.to) stopAudio();
    };
    el.onended = () => stopAudio();
    el.play().catch(() => stopAudio());
  };

  const togglePlay = (withContext: boolean) => {
    if (playing) {
      stopAudio();
      return;
    }
    if (!selectedProposal) return;
    startPlayback(
      withContext ? Math.max(0, span.start - CONTEXT_PAD) : span.start,
      withContext ? span.end + CONTEXT_PAD : span.end
    );
  };

  const scrub = (seconds: number) => {
    const t = Math.max(0, seconds);
    setPlayhead(t);
    if (audioRef.current) audioRef.current.currentTime = t;
  };

  // ---- derived ------------------------------------------------------------

  const sources = index?.sources ?? [];
  const spans = proposals?.proposals ?? [];
  const transcribed = index?.transcribed ?? 0;
  const words = useMemo(() => sources.reduce((sum, s) => sum + (s.words ?? 0), 0), [sources]);

  const visible = useMemo(() => {
    const needle = search.trim().toLowerCase();
    return spans.filter(
      (proposal) =>
        matchesFilter(proposal, filter) &&
        (needle === "" ||
          `${proposal.title}\n${proposal.content}`.toLowerCase().includes(needle))
    );
  }, [spans, filter, search]);

  const filterCounts = useMemo(
    () =>
      Object.fromEntries(
        QUEUE_FILTERS.map((option) => [
          option.id,
          spans.filter((proposal) => matchesFilter(proposal, option.id)).length,
        ])
      ) as Record<QueueFilter, number>,
    [spans]
  );

  const selectedProposal = spans.find((p) => p.text_uuid === selected) ?? null;
  const span = selectedProposal
    ? edits[selectedProposal.text_uuid] ?? {
        start: selectedProposal.start,
        end: selectedProposal.end,
      }
    : { start: 0, end: 0 };

  const siblings = useMemo(() => {
    if (!selectedProposal?.source_uuid) return [];
    return spans
      .filter((p) => p.source_uuid === selectedProposal.source_uuid)
      .slice()
      .sort((a, b) => a.start - b.start);
  }, [spans, selectedProposal?.source_uuid]);

  const position = selectedProposal
    ? (() => {
        const at = siblings.findIndex((p) => p.text_uuid === selectedProposal.text_uuid);
        return at < 0 ? null : { index: at + 1, total: siblings.length };
      })()
    : null;

  const selectedSource =
    (selectedProposal?.source_uuid
      ? sources.find((s) => s.uuid === selectedProposal.source_uuid)
      : null) ?? null;
  const duration = selectedSource?.duration && selectedSource.duration > 0 ? selectedSource.duration : null;

  /** Rows a save would still store: placed, and free of a clip. */
  const saveable = spans.filter(canSave).length;

  // The selection follows the rows: a filter that hides it, a re-guess that
  // drops it, or the first load all land on the first passage still visible.
  useEffect(() => {
    if (selected && visible.some((p) => p.text_uuid === selected)) return;
    setSelected(visible[0]?.text_uuid ?? null);
  }, [visible, selected]);

  const emptyHint =
    sources.length === 0
      ? "Pick the folder that holds the audiobook's files — Source Audios tab."
      : transcribed === 0
        ? "Transcribe one recording on the Source Audios tab, then guess."
        : spans.length === 0
          ? "Every recording is transcribed — press Guess for all."
          : "Select a text in the queue.";

  // ---- keyboard -----------------------------------------------------------

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey) return;
      const target = event.target as HTMLElement | null;
      if (target) {
        const tag = target.tagName;
        if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || target.isContentEditable) {
          return;
        }
      }
      if (event.key === "Escape") {
        if (playing) {
          event.preventDefault();
          stopAudio();
        }
        return;
      }
      // The transport keys are for the passage in front of you, and the passage
      // is only in front of you on the Texts tab.
      if (tab !== "texts") return;
      if (!selectedProposal) return;

      if (event.key === " ") {
        event.preventDefault();
        togglePlay(false);
        return;
      }
      if (event.key === "ArrowUp" || event.key === "ArrowDown") {
        const at = visible.findIndex((p) => p.text_uuid === selectedProposal.text_uuid);
        if (at < 0) return;
        const next = visible[at + (event.key === "ArrowUp" ? -1 : 1)];
        if (next) {
          event.preventDefault();
          setSelected(next.text_uuid);
        }
        return;
      }
      if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
        // A stored span is read-only until its clip is deleted.
        if (busy != null || selectedProposal.clip) return;
        event.preventDefault();
        const delta =
          (event.key === "ArrowLeft" ? -1 : 1) * (event.shiftKey ? COARSE_STEP : NUDGE_STEP);
        if (activeEdge === "start") {
          const start = Math.min(Math.max(0, span.start + delta), span.end - MIN_SPAN);
          moveSpan(selectedProposal, roundMs(start), span.end);
        } else {
          const end = Math.max(span.end + delta, span.start + MIN_SPAN);
          moveSpan(selectedProposal, span.start, roundMs(end));
        }
        return;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // `moveSpan` and `togglePlay` are re-created with this render; the listener
    // is cheap to re-attach and this keeps every handler current.
  });

  const tabItems: TabBarItem<TabId>[] = [
    { id: "sources", label: "Source Audios", icon: FolderOpen, count: sources.length },
    { id: "texts", label: "Texts", icon: ListChecks, count: saveable },
  ];

  // One line under the tabs, always in the same place: what just happened, or
  // what the keys do on the view you are looking at.
  const tabHint =
    tab === "texts"
      ? spans.length === 0
        ? "Nothing placed yet: press Guess for all to walk the texts along the recordings."
        : "Space play · ← → nudge the highlighted edge (Shift 100 ms) · ↑ ↓ text · Esc stop · Save stores the span"
      : "Listen to each file once, read what came back on the right, then save the texts on the Texts tab.";
  const busyText =
    busy === "picking"
      ? "Picking the folder…"
      : busy === "transcribing"
        ? "Listening to the next file…"
        : busy === "listening"
          ? "Listening to this file…"
          : busy === "guessing"
            ? "Placing every text…"
            : busy === "reguessing"
              ? "Placing this text again…"
              : busy === "saving"
                ? "Saving the span as standard audio…"
                : busy === "deleting"
                  ? "Deleting the clip…"
                  : "";

  return (
    <div className="flex-1 min-h-0 flex flex-col">
      <TabBar
        label="Alignment sections"
        items={tabItems}
        value={tab}
        onChange={setTab}
        className="shrink-0 px-4 bg-bg-card"
      />

      <div className="shrink-0 border-b border-border-default bg-bg-card px-4 py-1.5 text-[11px]">
        {error ? (
          <p className="text-error-text break-words">{error}</p>
        ) : note ? (
          <p
            className={`break-words ${
              note.kind === "warn" ? "text-warning-text" : "text-text-tertiary"
            }`}
          >
            {note.text}
          </p>
        ) : (
          <p className="text-text-tertiary truncate" title={tabHint}>
            {busy ? busyText : tabHint}
          </p>
        )}
      </div>

      {tab === "texts" ? (
        <div className="flex-1 min-h-0 overflow-y-auto p-4 lg:overflow-hidden">
          <div
            ref={textsSplit.containerRef}
            className="flex flex-col gap-3 lg:grid lg:h-full lg:min-h-0 lg:gap-0"
            style={textsSplit.columns}
          >
            <PassageQueue
              proposals={spans}
              visible={visible}
              filter={filter}
              filterCounts={filterCounts}
              onFilter={setFilter}
              search={search}
              onSearch={setSearch}
              selected={selected}
              onSelect={setSelected}
              onGuessAll={guess}
              onSaveAll={saveAll}
              saveable={saveable}
              onReguess={guessOne}
              onSave={saveOne}
              onDeleteClip={deleteClip}
              reguessing={guessingText}
              saving={savingText}
              busy={busy != null}
              canGuess={transcribed > 0}
            />

            <Splitter
              width={textsSplit.width}
              label="Resize the text list and the passage"
              onChange={textsSplit.resize}
              onCommit={textsSplit.commit}
              onReset={textsSplit.reset}
            />

            <PassageEditor
              proposal={selectedProposal}
              span={span}
              siblings={siblings}
              position={position}
              duration={duration}
              playhead={playhead}
              playing={playing}
              busy={busy != null}
              activeEdge={activeEdge}
              emptyHint={emptyHint}
              canPrev={position != null && position.index > 1}
              canNext={position != null && position.index < position.total}
              onStep={(direction: -1 | 1) => {
                if (!position) return;
                const next = siblings[position.index - 1 + direction];
                if (next) setSelected(next.text_uuid);
              }}
              onActivateEdge={setActiveEdge}
              onMove={(start, end) => {
                if (selectedProposal) moveSpan(selectedProposal, start, end);
              }}
              onPlay={togglePlay}
              onScrub={scrub}
              onSelect={setSelected}
              onSave={() => {
                if (selectedProposal) saveOne(selectedProposal.text_uuid);
              }}
              onDeleteClip={() => {
                if (selectedProposal) deleteClip(selectedProposal);
              }}
              saving={savingText != null && savingText === selectedProposal?.text_uuid}
            />
          </div>
        </div>
      ) : (
        <div className="flex-1 min-h-0 overflow-y-auto p-4 lg:overflow-hidden">
          <SourceAudiosTab
            mobile={mobile}
            directory={index?.directory ?? ""}
            sources={sources}
            transcribed={transcribed}
            words={words}
            busy={busy}
            listeningUuid={listeningUuid}
            selected={sourceSel}
            transcript={transcript}
            transcriptBusy={transcriptBusy}
            transcriptError={transcriptError}
            onPick={pickFolder}
            onTranscribeAll={transcribeAll}
            onTranscribeFile={transcribeOne}
            onStop={() => {
              stopRef.current = true;
            }}
            onOpenResult={(sourceUuid: string) => {
              void openTranscript(sourceUuid);
            }}
          />
        </div>
      )}

      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}
