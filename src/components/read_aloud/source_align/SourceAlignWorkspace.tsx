"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useIsMobile } from "@/hooks/useIsMobile";
import {
  applyAlignProposals,
  decideAlignProposals,
  guessAlignSpans,
  listAlignProposals,
  registerAlignSources,
  transcribeAlignSource,
  type AlignProposal,
  type AlignProposalState,
  type AlignProposals,
  type AlignSourceIndex,
} from "@/lib/read_aloud/sourceAlign";
import AlignStages from "./AlignStages";
import PassageEditor from "./PassageEditor";
import PassageInspector from "./PassageInspector";
import PassageQueue, { QUEUE_FILTERS, matchesFilter, type QueueFilter } from "./PassageQueue";
import { COARSE_STEP, CONTEXT_PAD, MIN_SPAN, NUDGE_STEP, clock, errorText, roundMs } from "./format";

/**
 * The Advanced page: a folder of recordings in, one clip per text out.
 *
 * The panel this replaced did the same job as one stacked column, so the review
 * — the actual work — began below every control and every passage row showed
 * all ten of its buttons at once. Here the page is three zones: the pipeline
 * runs across the top, the queue walks the passages on the left, one passage is
 * open at full size in the middle, and the numbers behind its verdict sit on the
 * right. State and every `read_aloud_align_*` call live here; the four views are
 * renderers.
 */

/** Spans at or above this confidence are accepted without listening to them. */
const DEFAULT_THRESHOLD = 0.9;

/** How many spans one `apply` call cuts; the loop repeats until done. */
const APPLY_BATCH = 10;

/** How long a burst of nudges waits before it becomes one rewrite of the file. */
const NUDGE_COMMIT_MS = 300;

type Note = { kind: "ok" | "warn"; text: string };

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
  const [threshold, setThreshold] = useState(DEFAULT_THRESHOLD);
  /** The threshold while it is being typed, so a half-typed "9" is not 9 %. */
  const [thresholdDraft, setThresholdDraft] = useState<string | null>(null);
  const [playing, setPlaying] = useState(false);
  const [playhead, setPlayhead] = useState<number | null>(null);
  const [loop, setLoop] = useState(false);
  /** Nudged spans, by text uuid, until the write is acknowledged. */
  const [edits, setEdits] = useState<Record<string, { start: number; end: number }>>({});
  const [selected, setSelected] = useState<string | null>(null);
  const [filter, setFilter] = useState<QueueFilter>("all");
  const [search, setSearch] = useState("");
  /** Which edge the arrow keys move; the fields show it. */
  const [activeEdge, setActiveEdge] = useState<"start" | "end">("start");

  const audioRef = useRef<HTMLAudioElement | null>(null);
  const boundsRef = useRef<{ from: number; to: number } | null>(null);
  const loopRef = useRef(false);
  const stopRef = useRef(false);
  const busyRef = useRef<string | null>(null);
  const proposalsRef = useRef<AlignProposals | null>(null);
  const timersRef = useRef<Record<string, ReturnType<typeof setTimeout>>>({});
  const pendingRef = useRef<Record<string, { start: number; end: number }>>({});

  useEffect(() => {
    loopRef.current = loop;
  }, [loop]);

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
   * The verdict is read from the current proposals rather than captured when
   * the nudge was made: pressing A during the debounce must not be undone by a
   * queued write that still remembers "pending".
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

  const transcribeOne = (sourceUuid: string) => {
    // A re-listen replaces the word timings the spans were placed against, so
    // the note has to say that a re-guess is what puts them back in order.
    const repeat = sources.find((s) => s.uuid === sourceUuid)?.words != null;
    return pipeline("listening", async () => {
      const result = await transcribeAlignSource(datasetUuid, sourceUuid);
      setIndex(result);
      setNote({
        kind: "ok",
        text: `Heard ${result.words ?? 0} words in ${result.file_name ?? ""} (${clock(
          result.duration ?? 0
        )})${repeat ? " — re-guess to re-place this file's passages." : "."}`,
      });
    });
  };

  const guess = () =>
    pipeline("guessing", async () => {
      const result = await guessAlignSpans(datasetUuid);
      setSelected(null);
      await refresh(true);
      setNote({
        kind: result.refused > 0 ? "warn" : "ok",
        text: `${result.proposed} of ${result.passages} passages placed (mean ${Math.round(
          result.mean_confidence * 100
        )}%), ${result.refused} refused.`,
      });
    });

  const decide = (proposal: AlignProposal, state: AlignProposalState) => {
    if (busyRef.current || proposal.status !== "proposed" || proposal.applied_uuid) return;
    pipeline("deciding", async () => {
      await decideAlignProposals({ datasetUuid, textUuid: proposal.text_uuid, state });
      await refresh();
    });
  };

  const decideMany = (state: AlignProposalState) =>
    pipeline("deciding", async () => {
      const result = await decideAlignProposals({
        datasetUuid,
        state,
        minConfidence: threshold,
      });
      await refresh();
      setNote({
        kind: "ok",
        text: `${result.changed} passage${result.changed === 1 ? "" : "s"} ${state} (≥ ${Math.round(
          threshold * 100
        )}%).`,
      });
    });

  /**
   * Cut the accepted spans. The loop repeats because `apply` is bounded, and it
   * stops on no progress as well as on nothing left: a span that fails stays
   * accepted, so repeating it would fail forever.
   */
  const applyAccepted = () =>
    pipeline("cutting", async () => {
      let stored = 0;
      let failures = 0;
      for (;;) {
        const result = await applyAlignProposals(datasetUuid, APPLY_BATCH);
        stored += result.applied;
        failures += result.failed_count;
        if (result.remaining === 0 || result.applied === 0) break;
      }
      await refresh();
      onApplied();
      setNote({
        kind: failures > 0 ? "warn" : "ok",
        text:
          failures > 0
            ? `${stored} clip${stored === 1 ? "" : "s"} stored, ${failures} failed — see the rows.`
            : `${stored} clip${stored === 1 ? "" : "s"} stored as standard audio.`,
      });
    });

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
      if (el.currentTime >= bounds.to) {
        if (loopRef.current) {
          el.currentTime = bounds.from;
          setPlayhead(bounds.from);
        } else {
          stopAudio();
        }
      }
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
  const counts = proposals?.counts ?? null;
  const passagesTotal = proposals?.passages_total ?? 0;
  const transcribed = index?.transcribed ?? 0;
  const words = useMemo(() => sources.reduce((sum, s) => sum + (s.words ?? 0), 0), [sources]);
  const placed = spans.filter((p) => p.status === "proposed").length;
  const meanConfidence = useMemo(() => {
    const proposed = spans.filter((p) => p.status === "proposed");
    if (proposed.length === 0) return null;
    return proposed.reduce((sum, p) => sum + p.confidence, 0) / proposed.length;
  }, [spans]);

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

  const firstBySource = useMemo(() => {
    const map: Record<string, string | undefined> = {};
    for (const proposal of spans) {
      if (proposal.source_uuid && proposal.status === "proposed" && !map[proposal.source_uuid]) {
        map[proposal.source_uuid] = proposal.text_uuid;
      }
    }
    return map;
  }, [spans]);

  const acceptAllCount = spans.filter(
    (p) => p.status === "proposed" && p.state !== "accepted" && p.confidence >= threshold
  ).length;
  const rejectAllCount = spans.filter(
    (p) => p.status === "proposed" && p.state !== "rejected" && p.confidence >= threshold
  ).length;

  // The selection follows the rows: a filter that hides it, a re-guess that
  // drops it, or the first load all land on the first passage still visible.
  useEffect(() => {
    if (selected && visible.some((p) => p.text_uuid === selected)) return;
    setSelected(visible[0]?.text_uuid ?? null);
  }, [visible, selected]);

  const emptyHint =
    sources.length === 0
      ? "Pick the folder that holds the audiobook's files to start."
      : transcribed === 0
        ? "Transcribe at least one recording, then place the passages."
        : spans.length === 0
          ? "Every recording is transcribed — place the passages now."
          : "Select a passage in the queue.";

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
        if (busy != null || selectedProposal.applied_uuid) return;
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
      if (event.key === "a" || event.key === "A") {
        event.preventDefault();
        decide(selectedProposal, "accepted");
        return;
      }
      if (event.key === "r" || event.key === "R") {
        event.preventDefault();
        decide(selectedProposal, "rejected");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // `moveSpan`, `togglePlay` and `decide` are re-created with this render; the
    // listener is cheap to re-attach and this keeps every handler current.
  });

  return (
    <div className="flex-1 min-h-0 flex flex-col">
      <AlignStages
        mobile={mobile}
        directory={index?.directory ?? ""}
        sourceCount={index?.source_count ?? 0}
        transcribed={transcribed}
        words={words}
        passagesTotal={passagesTotal}
        placed={placed}
        meanConfidence={meanConfidence}
        open={counts?.open ?? 0}
        applied={counts?.applied ?? 0}
        busy={busy}
        error={error}
        note={note}
        onPick={pickFolder}
        onTranscribe={transcribeAll}
        onStop={() => {
          stopRef.current = true;
        }}
        onGuess={guess}
        onApply={applyAccepted}
      />

      <div className="flex-1 min-h-0 overflow-y-auto p-4 lg:overflow-hidden">
        <div className="grid gap-4 lg:h-full lg:min-h-0 lg:grid-cols-[300px_minmax(0,1fr)_320px]">
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
            busy={busy != null}
            threshold={threshold}
            thresholdDraft={thresholdDraft}
            onThresholdDraft={setThresholdDraft}
            onThresholdCommit={(value) => {
              // Committed on blur, not on every keystroke: a half-typed "9"
              // would otherwise move the batch verdicts to 9 %.
              const parsed = Number(value.replace("%", "").trim());
              if (Number.isFinite(parsed)) {
                setThreshold(Math.min(100, Math.max(0, parsed)) / 100);
              }
              setThresholdDraft(null);
            }}
            acceptAllCount={acceptAllCount}
            rejectAllCount={rejectAllCount}
            onDecideMany={decideMany}
          />

          <PassageEditor
            proposal={selectedProposal}
            span={span}
            siblings={siblings}
            position={position}
            duration={duration}
            playhead={playhead}
            playing={playing}
            loop={loop}
            busy={busy != null}
            activeEdge={activeEdge}
            emptyHint={emptyHint}
            onActivateEdge={setActiveEdge}
            onMove={(start, end) => {
              if (selectedProposal) moveSpan(selectedProposal, start, end);
            }}
            onDecide={(state) => {
              if (selectedProposal) decide(selectedProposal, state);
            }}
            onPlay={togglePlay}
            onToggleLoop={() => setLoop((value) => !value)}
            onScrub={scrub}
            onSelect={setSelected}
          />

          <PassageInspector
            proposal={selectedProposal}
            source={selectedSource}
            position={position}
            sources={sources}
            firstBySource={firstBySource}
            busy={busy}
            mobile={mobile}
            canPrev={position != null && position.index > 1}
            canNext={position != null && position.index < position.total}
            onStep={(direction) => {
              if (!position) return;
              const next = siblings[position.index - 1 + direction];
              if (next) setSelected(next.text_uuid);
            }}
            onSelectSource={(sourceUuid) => {
              const first = firstBySource[sourceUuid];
              if (first) setSelected(first);
            }}
            onTranscribeFile={transcribeOne}
          />
        </div>
      </div>
    </div>
  );
}
