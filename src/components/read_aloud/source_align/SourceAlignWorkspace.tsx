"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useIsMobile } from "@/hooks/useIsMobile";
import { FolderOpen, ListChecks } from "lucide-react";
import { TabBar, type TabBarItem } from "@/components/ui/toolbar";
import ConfirmDialog, { type ConfirmRequest } from "@/components/books/ConfirmDialog";
import { canSave } from "./ClipActions";
import PassageEditor from "./PassageEditor";
import PassageQueue, { QUEUE_FILTERS, matchesFilter, type QueueFilter } from "./PassageQueue";
import SourceAudiosTab from "./SourceAudiosTab";
import { Splitter, useSplitWidth } from "./Splitter";
import { COARSE_STEP, MIN_SPAN, NUDGE_STEP, errorText, roundMs } from "./format";
import { useAlignSources } from "./useAlignSources";
import { useSpanDrafts } from "./useSpanDrafts";
import { useSpanPlayer } from "./useSpanPlayer";

/**
 * The standard-audio page: a folder of recordings in, one clip per text out.
 *
 * Two tabs, in the order the work is done. **Source Audios** is the folder, one
 * row per recording with its two verbs — listen, read what came back — and the
 * transcript on the right, because "did it hear this right?" is answered by the
 * words and their times, not by a number. **Texts** is the work: the queue on
 * the left — one line per text, no verbs — and one passage open beside it with
 * its waveform, its typed edges, and the verbs that act on them in a bar under
 * the edges: place it again, and save (or delete) the one clip it is allowed.
 *
 * The page's model — a span is the truth and a clip is a cache of it, there is
 * no verdict to give, and a save never overwrites a clip — is written down once,
 * in `docs/design/read-aloud-align.md`. This file only arranges the buttons.
 *
 * The state behind all that is in three hooks beside this file — the player
 * (`useSpanPlayer`), the spans and their writes (`useSpanDrafts`), the folder
 * and its transcripts (`useAlignSources`). What is left here is the chrome that
 * arranges them: the two tabs, the seam, the note line, the keys, and the one
 * lock that keeps two runs from overlapping.
 */

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
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<Note | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [tab, setTab] = useState<TabId>("sources");
  /** The question a click asks before it throws something away. */
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const [filter, setFilter] = useState<QueueFilter>("all");
  const [search, setSearch] = useState("");
  /** Which edge the arrow keys move; the fields show it. */
  const [activeEdge, setActiveEdge] = useState<"start" | "end">("start");

  // The seam between the rail and the passage, remembered like the other tab's.
  const textsSplit = useSplitWidth(TEXTS_SPLIT);

  const busyRef = useRef<string | null>(null);
  /**
   * The nudge writer, filled in by the drafts hook below.
   *
   * A step must write the spans still waiting on their debounce *before* it
   * reads them, and only the drafts hook knows which those are; the ref is what
   * lets the lock and that flush be the same function without one hook having
   * to own the other.
   */
  const flushPendingRef = useRef<(force?: boolean) => Promise<unknown>>(async () => {});

  /**
   * One operation at a time: the progress text is also the lock.
   *
   * The lock goes up immediately rather than on the next render, because a
   * double click must not start two transcriptions.
   */
  const pipeline = useCallback((label: string, task: () => Promise<void>) => {
    if (busyRef.current) return;
    busyRef.current = label;
    setBusy(label);
    setError(null);
    void (async () => {
      try {
        await flushPendingRef.current(true);
        await task();
      } catch (e) {
        setError(errorText(e));
      } finally {
        busyRef.current = null;
        setBusy(null);
      }
    })();
  }, []);

  const isBusy = useCallback(() => busyRef.current != null, []);

  const sourceList = useAlignSources({
    datasetUuid,
    pipeline,
    setError,
    setNote,
    setConfirm,
    setSelected,
  });

  const drafts = useSpanDrafts({
    datasetUuid,
    pipeline,
    isBusy,
    setError,
    setNote,
    setConfirm,
    setSelected,
    revealQueue: () => setTab("texts"),
    onApplied,
    reloadSources: sourceList.reload,
  });

  flushPendingRef.current = drafts.flushPending;

  useEffect(() => {
    void drafts.refresh(true);
  }, [drafts.refresh]);

  // A different dataset is a different folder, a different set of spans and a
  // different transcript cache: nothing from the old one may survive.
  useEffect(() => {
    sourceList.reset();
    drafts.reset();
    setNote(null);
    setError(null);
    setSelected(null);
    setTab("sources");
    setFilter("all");
    setSearch("");
  }, [datasetUuid, sourceList.reset, drafts.reset]);

  // ---- derived ------------------------------------------------------------

  const spans = drafts.spans;

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
    ? drafts.edits[selectedProposal.text_uuid] ?? {
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
      ? sourceList.sources.find((s) => s.uuid === selectedProposal.source_uuid)
      : null) ?? null;
  const duration =
    selectedSource?.duration && selectedSource.duration > 0 ? selectedSource.duration : null;

  /** Rows a save would still store: placed, and free of a clip. */
  const saveable = spans.filter(canSave).length;

  // The selection follows the rows: a filter that hides it, a re-guess that
  // drops it, or the first load all land on the first passage still visible.
  useEffect(() => {
    if (selected && visible.some((p) => p.text_uuid === selected)) return;
    setSelected(visible[0]?.text_uuid ?? null);
  }, [visible, selected]);

  const emptyHint =
    sourceList.sources.length === 0
      ? "Pick the folder that holds the audiobook's files — Source Audios tab."
      : sourceList.transcribed === 0
        ? "Transcribe one recording on the Source Audios tab, then guess."
        : spans.length === 0
          ? "Every recording is transcribed — press Guess for all."
          : "Select a text in the queue.";

  // One element for the whole page, and it is told which span is in front of
  // the reader rather than reaching for it.
  const player = useSpanPlayer({
    textUuid: selected,
    path: selectedProposal?.source_path ?? null,
    span,
  });

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
        if (player.playing) {
          event.preventDefault();
          player.stopAudio();
        }
        return;
      }
      // The transport keys are for the passage in front of you, and the passage
      // is only in front of you on the Texts tab.
      if (tab !== "texts") return;
      if (!selectedProposal) return;

      if (event.key === " ") {
        event.preventDefault();
        player.togglePlay(false);
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
          drafts.moveSpan(selectedProposal, roundMs(start), span.end);
        } else {
          const end = Math.max(span.end + delta, span.start + MIN_SPAN);
          drafts.moveSpan(selectedProposal, span.start, roundMs(end));
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
    { id: "sources", label: "Source Audios", icon: FolderOpen, count: sourceList.sources.length },
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
              onGuessAll={drafts.guess}
              onSaveAll={drafts.saveAll}
              saveable={saveable}
              busy={busy != null}
              canGuess={sourceList.transcribed > 0}
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
              playhead={player.playhead}
              playing={player.playing}
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
                if (selectedProposal) drafts.moveSpan(selectedProposal, start, end);
              }}
              onPlay={player.togglePlay}
              onScrub={player.scrub}
              onSelect={setSelected}
              onReguess={() => {
                if (selectedProposal) drafts.guessOne(selectedProposal.text_uuid);
              }}
              onSave={() => {
                if (selectedProposal) drafts.saveOne(selectedProposal.text_uuid);
              }}
              onDeleteClip={() => {
                if (selectedProposal) drafts.deleteClip(selectedProposal);
              }}
              saving={drafts.savingText != null && drafts.savingText === selectedProposal?.text_uuid}
              reguessing={selectedProposal?.text_uuid === drafts.guessingText}
              canGuess={sourceList.transcribed > 0}
            />
          </div>
        </div>
      ) : (
        <div className="flex-1 min-h-0 overflow-y-auto p-4 lg:overflow-hidden">
          <SourceAudiosTab
            mobile={mobile}
            directory={sourceList.index?.directory ?? ""}
            sources={sourceList.sources}
            transcribed={sourceList.transcribed}
            words={sourceList.words}
            busy={busy}
            listeningUuid={sourceList.listeningUuid}
            selected={sourceList.sourceSel}
            transcript={sourceList.transcript}
            transcriptBusy={sourceList.transcriptBusy}
            transcriptError={sourceList.transcriptError}
            onPick={sourceList.pickFolder}
            onTranscribeAll={sourceList.transcribeAll}
            onTranscribeFile={sourceList.transcribeOne}
            onStop={sourceList.stopTranscribing}
            onOpenResult={(sourceUuid: string) => {
              void sourceList.openTranscript(sourceUuid);
            }}
          />
        </div>
      )}

      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}
