"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import {
  Check,
  FileText,
  FolderOpen,
  Loader2,
  ScanText,
  Search,
  Square,
  Wand2,
} from "lucide-react";
import type { AlignSource, AlignTranscript } from "@/lib/read_aloud/sourceAlign";
import { clockHms, clockShort, percent } from "./format";

/**
 * The recordings side of the page: the folder, one row per file, and what the
 * model heard in the file you are looking at.
 *
 * The work here is sequential — pick, listen, place — but the *result* of a
 * listen is the thing a person wants to read, and a row cannot hold a chapter's
 * word timings. So the tab is two columns: the list on the left with its two
 * verbs per row (listen / view), and the transcript on the right, where the
 * words and their times have room. Judging a passage is the Review tab's job;
 * this is where you check that the words are the right words.
 */

const CARD = "rounded-xl border border-border-default bg-bg-card overflow-hidden";
const CARD_HEAD = "flex items-baseline gap-2 px-4 py-2.5 border-b border-border-default";
const CARD_BODY = "px-4 py-3 flex flex-col gap-2.5";
const ACTION =
  "inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md text-xs cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed";
const NEUTRAL = `${ACTION} bg-bg-muted text-text-secondary hover:bg-bg-hover`;
const PRIMARY = `${ACTION} bg-accent-bg text-white`;
/** The per-row verbs: small, but they say what they do. */
const ROW_BUTTON =
  "inline-flex items-center gap-1 px-2 py-1 rounded-md border border-border-default text-[11px] text-text-secondary hover:bg-bg-hover hover:text-text-primary cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed disabled:hover:bg-transparent";

/** How many transcript rows render at once; the rest wait behind one click. */
const ROWS_PER_PAGE = 1000;

/** The two columns' seam: how wide the list starts, and how far either side can go. */
// `split` rather than `left-width`: the stored number now only exists because
// someone dragged, and the default it overrides changed with it.
const LEFT_WIDTH_KEY = "read-aloud:source-audios:split";
/** Used only when there is no room to measure — a container that has not laid out yet. */
const LEFT_WIDTH_FALLBACK = 420;
const LEFT_WIDTH_MIN = 260;
/** Room the transcript keeps whatever happens — it is the side being read. */
const RIGHT_WIDTH_MIN = 320;
const HANDLE_WIDTH = 12;

/**
 * An even split.
 *
 * Neither side is the point of the page at first sight — the list and the
 * transcript are read together — so the seam starts, and returns on a
 * double-click, midway.
 */
function evenWidth(container: number): number {
  return container > 0 ? Math.round((container - HANDLE_WIDTH) / 2) : LEFT_WIDTH_FALLBACK;
}

/**
 * Keep both sides usable.
 *
 * Without a floor on each, a drag can leave a column so thin that its content is
 * a sliver, and the state that follows is a page that looks broken rather than
 * resized.
 */
function clampWidth(next: number, container: number): number {
  const room = container > 0 ? container - RIGHT_WIDTH_MIN - HANDLE_WIDTH : LEFT_WIDTH_FALLBACK;
  const max = Math.max(LEFT_WIDTH_MIN, room);
  return Math.round(Math.min(Math.max(next, LEFT_WIDTH_MIN), max));
}

/**
 * The seam between the list and the transcript.
 *
 * How much room the file names and the timestamps each deserve is a taste, not
 * a rule, so the seam moves: drag it, or use the arrow keys when it has focus,
 * and a double-click puts it back in the middle.
 */
function Splitter({
  width,
  onChange,
  onCommit,
  onReset,
}: {
  width: number;
  onChange: (next: number) => void;
  onCommit: (width: number) => void;
  onReset: () => void;
}) {
  const drag = useRef<{ x: number; from: number; to: number } | null>(null);

  const stop = () => {
    drag.current = null;
    // Both were set on the body for the length of the drag, so the pointer keeps
    // its shape over the two panes and nothing under it gets selected.
    document.body.style.userSelect = "";
    document.body.style.cursor = "";
  };

  // A drag that outlives the seam — the tab is switched from under it — would
  // otherwise leave the whole window in col-resize.
  useEffect(
    () => () => {
      document.body.style.userSelect = "";
      document.body.style.cursor = "";
    },
    []
  );

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize the audios list and the transcript"
      aria-valuenow={Math.round(width)}
      aria-valuemin={LEFT_WIDTH_MIN}
      tabIndex={0}
      title="Drag to resize · double-click to reset"
      className="group hidden lg:flex items-center justify-center touch-none cursor-col-resize outline-none"
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        event.preventDefault();
        event.currentTarget.setPointerCapture(event.pointerId);
        drag.current = { x: event.clientX, from: width, to: width };
        document.body.style.userSelect = "none";
        document.body.style.cursor = "col-resize";
      }}
      onPointerMove={(event) => {
        const start = drag.current;
        if (!start) return;
        start.to = start.from + (event.clientX - start.x);
        onChange(start.to);
      }}
      onPointerUp={(event) => {
        const start = drag.current;
        if (!start) return;
        if (event.currentTarget.hasPointerCapture(event.pointerId)) {
          event.currentTarget.releasePointerCapture(event.pointerId);
        }
        stop();
        onCommit(start.to);
      }}
      onPointerCancel={stop}
      onDoubleClick={onReset}
      onKeyDown={(event) => {
        if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
        event.preventDefault();
        const next = width + (event.key === "ArrowLeft" ? -24 : 24);
        onChange(next);
        onCommit(next);
      }}
    >
      <span className="h-12 w-[3px] rounded-full bg-border-default transition-colors group-hover:bg-accent-bg group-focus-visible:bg-accent-bg" />
    </div>
  );
}

type Props = {
  mobile: boolean;
  directory: string;
  sources: AlignSource[];
  transcribed: number;
  words: number;
  busy: string | null;
  /** The file being listened to right now, so its row shows the spinner. */
  listeningUuid: string | null;
  passagesTotal: number;
  placed: number;
  refused: number;
  meanConfidence: number | null;
  /** The file whose result is on the right. */
  selected: string | null;
  transcript: AlignTranscript | null;
  transcriptBusy: boolean;
  transcriptError: string | null;
  onPick: () => void;
  onTranscribeAll: () => void;
  onTranscribeFile: (sourceUuid: string) => void;
  onStop: () => void;
  onOpenResult: (sourceUuid: string) => void;
  onGuess: () => void;
};

export default function SourceAudiosTab({
  mobile,
  directory,
  sources,
  transcribed,
  words,
  busy,
  listeningUuid,
  passagesTotal,
  placed,
  refused,
  meanConfidence,
  selected,
  transcript,
  transcriptBusy,
  transcriptError,
  onPick,
  onTranscribeAll,
  onTranscribeFile,
  onStop,
  onOpenResult,
  onGuess,
}: Props) {
  const busyLabel = busy != null;
  const transcribing = busy === "transcribing";
  const guessing = busy === "guessing";
  const allDone = sources.length > 0 && transcribed === sources.length;
  const selectedSource = sources.find((s) => s.uuid === selected) ?? null;

  // The seam between the columns is a taste, so it is remembered per device —
  // and it lives here rather than in the workspace because this is the only
  // view that has two columns to divide.
  const gridRef = useRef<HTMLDivElement | null>(null);
  const [leftWidth, setLeftWidth] = useState(LEFT_WIDTH_FALLBACK);

  useEffect(() => {
    const stored = Number(window.localStorage.getItem(LEFT_WIDTH_KEY));
    // Chosen once is chosen: a remembered seam wins over the even split, which
    // is only the shape of a page that has never been dragged.
    const container = gridRef.current?.clientWidth ?? 0;
    const wanted = Number.isFinite(stored) && stored > 0 ? stored : evenWidth(container);
    // A window that is already narrow must not open with the transcript squeezed
    // below its floor — and a container that has not been laid out yet (width 0)
    // must not be read as "very narrow".
    setLeftWidth(container > 0 ? clampWidth(wanted, container) : wanted);
  }, []);

  // Whatever takes the room away — the window, the sidebar, the page's own
  // padding — the floor is re-applied to the width already chosen rather than
  // the width being thrown away.
  useEffect(() => {
    const node = gridRef.current;
    if (!node || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      const container = node.clientWidth;
      // A view hidden by the shell reports 0; it has no width to divide yet.
      if (container <= 0) return;
      setLeftWidth((current) => clampWidth(current, container));
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  const resizeLeft = (next: number) =>
    setLeftWidth(clampWidth(next, gridRef.current?.clientWidth ?? 0));

  /** Written on release rather than on every pointer move of a drag. */
  const rememberLeft = (next: number) => {
    try {
      window.localStorage.setItem(
        LEFT_WIDTH_KEY,
        String(clampWidth(next, gridRef.current?.clientWidth ?? 0))
      );
    } catch {
      // Storage off: the seam still works, it just forgets between visits.
    }
  };

  /** The double-click: back to half each. */
  const resetLeft = () => {
    const next = evenWidth(gridRef.current?.clientWidth ?? 0);
    resizeLeft(next);
    rememberLeft(next);
  };

  return (
    <div
      ref={gridRef}
      className="flex flex-col gap-3 lg:h-full lg:min-h-0 lg:grid lg:gap-0"
      style={{ gridTemplateColumns: `${leftWidth}px ${HANDLE_WIDTH}px minmax(0, 1fr)` }}
    >
      {/* ---- left: the list, and the two verbs on every row ---------- */}
      <div className="flex flex-col gap-3 lg:min-h-0">
        <section className={`${CARD} shrink-0`}>
          <div className={CARD_HEAD}>
            <h3 className="text-xs font-semibold text-text-primary">Source folder</h3>
            <span className="ml-auto text-[10px] text-text-tertiary tabular-nums">
              {sources.length === 0
                ? "no files"
                : `${sources.length} recording${sources.length === 1 ? "" : "s"}`}
            </span>
          </div>
          <div className={CARD_BODY}>
            <p className="text-xs text-text-primary break-all">
              {directory || "No folder picked yet."}
            </p>
            <div>
              <button
                type="button"
                onClick={onPick}
                disabled={mobile || busyLabel}
                className={NEUTRAL}
                title={
                  mobile
                    ? "Picking a folder is a desktop action; this device can read the transcripts"
                    : "Pick the folder that holds the audiobook's files"
                }
              >
                <FolderOpen size={12} />
                {directory ? "Change folder" : "Pick folder"}
              </button>
            </div>
          </div>
        </section>

        <section className={`${CARD} flex flex-col lg:flex-1 lg:min-h-0`}>
          <div className={CARD_HEAD}>
            <h3 className="text-xs font-semibold text-text-primary">Audios</h3>
            <span className="ml-auto text-[10px] text-text-tertiary tabular-nums">
              {transcribed}/{sources.length} listened
              {words > 0 ? ` · ${words.toLocaleString()} words` : ""}
            </span>
          </div>

          <div className="flex flex-col gap-1 px-3 py-3 lg:flex-1 lg:min-h-0 lg:overflow-y-auto">
            {sources.length === 0 ? (
              <p className="text-xs text-text-tertiary px-1">No recordings yet: pick the folder above.</p>
            ) : (
              sources.map((file) => {
                const listened = file.words != null;
                const isSelected = file.uuid === selected;
                const working = listeningUuid === file.uuid;
                return (
                  <div
                    key={file.uuid}
                    className={`flex flex-col gap-1 rounded-md border px-2 py-1.5 ${
                      isSelected
                        ? "border-accent-bg/60 bg-accent-bg/5"
                        : "border-border-default"
                    }`}
                  >
                    <div className="flex items-start gap-2">
                      <span className="shrink-0 pt-[3px] text-accent-bg/70" aria-hidden>
                        {listened ? <Check size={11} /> : <span className="block w-[11px]" />}
                      </span>
                      {/* The whole name, wrapped: a chapter's file name is how it
                          is told apart from the one beside it, and an ellipsis
                          makes ten rows look identical. */}
                      <span className="min-w-0 flex-1 text-xs text-text-primary break-all">
                        {file.file_name}
                      </span>
                    </div>
                    <div className="flex flex-wrap items-center gap-x-1.5 gap-y-1 pl-[19px]">
                      <span className="mr-auto text-[10px] text-text-tertiary tabular-nums">
                        {listened
                          ? `${(file.words ?? 0).toLocaleString()} words · ${clockShort(
                              file.duration ?? 0
                            )}`
                          : "no transcript"}
                      </span>
                      <button
                        type="button"
                        onClick={() => onTranscribeFile(file.uuid)}
                        disabled={mobile || busyLabel}
                        className={ROW_BUTTON}
                        title={
                          mobile
                            ? "Listening is a desktop action"
                            : listened
                              ? "Listen again — replaces this file's transcript"
                              : "Listen to this file and cache its word timings"
                        }
                      >
                        {working ? (
                          <Loader2 size={11} className="animate-spin" />
                        ) : (
                          <ScanText size={11} />
                        )}
                        {listened ? "Regenerate" : "Transcribe"}
                      </button>
                      <button
                        type="button"
                        onClick={() => onOpenResult(file.uuid)}
                        disabled={!listened}
                        className={ROW_BUTTON}
                        title={
                          listened
                            ? "Read what the model heard in this file"
                            : "No transcript yet — listen to the file first"
                        }
                      >
                        <FileText size={11} />
                        View result
                      </button>
                    </div>
                  </div>
                );
              })
            )}
          </div>

          <div className="shrink-0 flex items-center gap-2 flex-wrap border-t border-border-default px-3 py-2">
            {transcribing ? (
              <button type="button" onClick={onStop} className={NEUTRAL}>
                <Square size={12} />
                Stop
              </button>
            ) : (
              <button
                type="button"
                onClick={onTranscribeAll}
                disabled={mobile || busyLabel || sources.length === 0 || allDone}
                className={NEUTRAL}
                title={
                  allDone
                    ? "Every file has a transcript; redo one with the button on its row"
                    : "Listen to every file without a transcript yet, one file per request"
                }
              >
                <ScanText size={12} />
                {transcribed === 0 ? "Transcribe all" : "Transcribe the rest"}
              </button>
            )}
            <span className="text-[10px] text-text-tertiary">
              One file per request; the cache is saved between files.
            </span>
          </div>
        </section>

        <section className={`${CARD} shrink-0`}>
          <div className={CARD_HEAD}>
            <h3 className="text-xs font-semibold text-text-primary">Place passages</h3>
            <span className="ml-auto text-[10px] text-text-tertiary tabular-nums">
              {placed}/{passagesTotal}
            </span>
          </div>
          <div className={CARD_BODY}>
            {placed > 0 && (
              <p className="text-[11px] text-text-secondary tabular-nums">
                {placed} of {passagesTotal} placed
                {meanConfidence == null ? "" : ` · mean ${percent(meanConfidence)}%`}
                {refused > 0 ? ` · ${refused} refused` : ""}
              </p>
            )}
            <div className="flex items-center gap-2 flex-wrap">
              <button
                type="button"
                onClick={onGuess}
                disabled={busyLabel || transcribed === 0}
                className={placed > 0 ? NEUTRAL : PRIMARY}
                title={
                  transcribed === 0
                    ? "Listen to at least one file first"
                    : "Place every passage along the recordings, in order"
                }
              >
                {guessing ? (
                  <Loader2 size={12} className="animate-spin" />
                ) : (
                  <Wand2 size={12} />
                )}
                {guessing ? "Placing…" : placed > 0 ? "Re-guess" : "Guess all"}
              </button>
              <span className="text-[10px] text-text-tertiary">
                Walks the passages in reading order; nothing is written to media/.
              </span>
            </div>
          </div>
        </section>
      </div>

      <Splitter
        width={leftWidth}
        onChange={resizeLeft}
        onCommit={rememberLeft}
        onReset={resetLeft}
      />

      {/* ---- right: what the model heard ---------------------------- */}
      <section className={`${CARD} flex flex-col lg:min-h-0`}>
        <div className={CARD_HEAD}>
          <h3 className="text-xs font-semibold text-text-primary min-w-0 truncate">
            {selectedSource ? selectedSource.file_name : "Transcript"}
          </h3>
          {transcript && (
            <span className="ml-auto shrink-0 text-[10px] text-text-tertiary tabular-nums">
              {clockShort(transcript.duration)} · {transcript.word_count.toLocaleString()} words
              {transcript.duration > 0
                ? ` · ${Math.round((transcript.word_count / transcript.duration) * 60)} wpm`
                : ""}
            </span>
          )}
        </div>

        <TranscriptBody
          key={transcript?.source_uuid ?? "none"}
          source={selectedSource}
          transcript={transcript}
          busy={transcriptBusy}
          error={transcriptError}
        />
      </section>
    </div>
  );
}

/**
 * The words themselves, as a table.
 *
 * Start, end and the tokens the aligner actually matched — one row per heard
 * word, at millisecond precision, which is the only way to see that a span
 * begins where it does because the word before it ended there. A chapter is
 * thousands of rows, so the table is searchable rather than readable, and it
 * paints a page at a time: ten thousand rows to scroll past nine thousand of
 * them would make the pane stutter for nothing.
 */
function TranscriptBody({
  source,
  transcript,
  busy,
  error,
}: {
  source: AlignSource | null;
  transcript: AlignTranscript | null;
  busy: boolean;
  error: string | null;
}) {
  const [needle, setNeedle] = useState("");
  const [limit, setLimit] = useState(ROWS_PER_PAGE);
  const words = useMemo(() => transcript?.words ?? [], [transcript]);

  const shown = useMemo(() => {
    const text = needle.trim().toLowerCase();
    if (text === "") return words;
    return words.filter((word) => word.text.toLowerCase().includes(text));
  }, [words, needle]);

  const rows = shown.slice(0, limit);
  const hidden = shown.length - rows.length;

  if (error) {
    return (
      <p className="px-4 py-3 text-xs text-error-text break-words flex-1 min-h-0">{error}</p>
    );
  }
  if (busy) {
    return (
      <p className="px-4 py-3 text-xs text-text-tertiary flex-1 min-h-0 flex items-center gap-1.5">
        <Loader2 size={12} className="animate-spin" />
        Reading the transcript…
      </p>
    );
  }
  if (!source) {
    return (
      <p className="px-4 py-3 text-xs text-text-tertiary leading-relaxed flex-1 min-h-0">
        Choose a recording on the left, then <span className="text-text-secondary">View result</span>{" "}
        — the words the model heard in that file, and when it heard them.
      </p>
    );
  }
  if (source.words == null) {
    return (
      <p className="px-4 py-3 text-xs text-text-tertiary leading-relaxed flex-1 min-h-0">
        {source.file_name} has no transcript yet. Use{" "}
        <span className="text-text-secondary">Transcribe</span> on its row.
      </p>
    );
  }
  if (!transcript) {
    return (
      <p className="px-4 py-3 text-xs text-text-tertiary leading-relaxed flex-1 min-h-0">
        Choose a recording on the left, then <span className="text-text-secondary">View result</span>.
      </p>
    );
  }

  return (
    <>
      <div className="shrink-0 flex items-center gap-2 border-b border-border-default px-4 py-2">
        <Search size={12} className="shrink-0 text-text-tertiary" />
        <input
          value={needle}
          onChange={(event) => {
            setNeedle(event.target.value);
            setLimit(ROWS_PER_PAGE);
          }}
          placeholder="Find a word or a phrase"
          className="flex-1 min-w-0 bg-transparent text-xs text-text-primary outline-none placeholder:text-text-tertiary"
        />
        <span className="shrink-0 text-[10px] text-text-tertiary tabular-nums">
          {needle.trim() === "" ? `${words.length} words` : `${shown.length} of ${words.length}`}
        </span>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto">
        {shown.length === 0 ? (
          <p className="px-4 py-3 text-xs text-text-tertiary">
            Nothing in this transcript matches.
          </p>
        ) : (
          <table className="w-full border-collapse text-xs">
            <thead className="sticky top-0 z-10 bg-bg-card">
              <tr className="border-b border-border-default text-left text-[10px] uppercase tracking-wide text-text-tertiary">
                <th className="w-[104px] px-3 py-1.5 font-medium">Start</th>
                <th className="w-[104px] px-3 py-1.5 font-medium">End</th>
                <th className="px-3 py-1.5 font-medium">Tokens</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((word, index) => (
                <tr
                  key={`${index}-${word.start}`}
                  className="border-b border-border-default/50 last:border-0 hover:bg-bg-hover"
                >
                  <td className="px-3 py-1 align-top tabular-nums text-text-tertiary whitespace-nowrap">
                    {clockHms(word.start)}
                  </td>
                  <td className="px-3 py-1 align-top tabular-nums text-text-tertiary whitespace-nowrap">
                    {clockHms(word.end)}
                  </td>
                  <td className="px-3 py-1 align-top text-text-primary break-words">
                    {word.text}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <div className="shrink-0 flex items-center gap-2 border-t border-border-default px-4 py-2">
        <p className="text-[10px] text-text-tertiary">
          Words as the aligner scored them — lowercased, numbers spelled out — which is why a guess
          lands where it does.
        </p>
        {hidden > 0 && (
          <button
            type="button"
            onClick={() => setLimit((value) => value + ROWS_PER_PAGE)}
            className="ml-auto shrink-0 px-2 py-1 rounded-md border border-border-default text-[11px] text-text-secondary hover:bg-bg-hover cursor-pointer"
          >
            Show {Math.min(ROWS_PER_PAGE, hidden).toLocaleString()} more
          </button>
        )}
      </div>
    </>
  );
}
