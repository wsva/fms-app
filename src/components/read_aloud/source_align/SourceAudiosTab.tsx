"use client";

import { useMemo, useState } from "react";
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
import type { AlignSource, AlignTranscript, AlignTranscriptWord } from "@/lib/read_aloud/sourceAlign";
import { clock, clockShort, percent } from "./format";

/**
 * The recordings side of the page: the folder, one line per file, and what the
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

/** Silence longer than this ends a transcript line. */
const LINE_GAP = 0.8;
/** ...and so does a line this long: the pane is a column, not a paragraph. */
const LINE_WORDS = 12;

type Line = { start: number; text: string };

/**
 * Timed words into readable lines.
 *
 * The cache holds one entry per word, which reads as a wall of tokens. Breaking
 * on a pause keeps the lines the speaker's, and the hard cap keeps a fast
 * reader's run from becoming one 200-word line the pane has to wrap anyway.
 */
function groupWords(words: AlignTranscriptWord[]): Line[] {
  const lines: Line[] = [];
  let current: AlignTranscriptWord[] = [];
  const flush = () => {
    if (current.length === 0) return;
    lines.push({ start: current[0].start, text: current.map((w) => w.text).join(" ") });
    current = [];
  };
  for (const word of words) {
    const previous = current[current.length - 1];
    if (previous && (word.start - previous.end > LINE_GAP || current.length >= LINE_WORDS)) flush();
    current = [...current, word];
  }
  flush();
  return lines;
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

  return (
    <div className="flex flex-col gap-3 lg:h-full lg:min-h-0 lg:grid lg:grid-cols-[minmax(0,420px)_minmax(0,1fr)]">
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
            <h3 className="text-xs font-semibold text-text-primary">Recordings</h3>
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
                    className={`flex flex-wrap items-center gap-x-2 gap-y-1.5 rounded-md border px-2 py-1.5 ${
                      isSelected
                        ? "border-accent-bg/60 bg-accent-bg/5"
                        : "border-border-default"
                    }`}
                  >
                    <span className="shrink-0 text-accent-bg/70" aria-hidden>
                      {listened ? <Check size={11} /> : <span className="block w-[11px]" />}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span
                        className="block text-xs text-text-primary truncate"
                        title={file.path}
                      >
                        {file.file_name}
                      </span>
                      <span className="block text-[10px] text-text-tertiary tabular-nums">
                        {listened
                          ? `${(file.words ?? 0).toLocaleString()} words · ${clockShort(
                              file.duration ?? 0
                            )}`
                          : "no transcript"}
                      </span>
                    </span>
                    <span className="flex items-center gap-1 shrink-0">
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
                    </span>
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
 * The words themselves.
 *
 * A chapter is thousands of words, so the pane is searchable rather than
 * paginated: typing narrows the lines to those that contain what was typed,
 * which is how a person checks a name or the start of a passage.
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
  const lines = useMemo(() => groupWords(transcript?.words ?? []), [transcript]);

  const shown = useMemo(() => {
    const text = needle.trim().toLowerCase();
    if (text === "") return lines;
    return lines.filter((line) => line.text.toLowerCase().includes(text));
  }, [lines, needle]);

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
          onChange={(event) => setNeedle(event.target.value)}
          placeholder="Find a word or a phrase"
          className="flex-1 min-w-0 bg-transparent text-xs text-text-primary outline-none placeholder:text-text-tertiary"
        />
        <span className="shrink-0 text-[10px] text-text-tertiary tabular-nums">
          {needle.trim() === ""
            ? `${lines.length} lines`
            : `${shown.length} of ${lines.length}`}
        </span>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto px-4 py-3">
        {shown.length === 0 ? (
          <p className="text-xs text-text-tertiary">Nothing in this transcript matches.</p>
        ) : (
          <div className="flex flex-col gap-1">
            {shown.map((line) => (
              <div key={`${line.start}-${line.text.slice(0, 12)}`} className="flex gap-3">
                <span className="shrink-0 w-[54px] pt-[1px] text-[10px] text-text-tertiary tabular-nums">
                  {clock(line.start)}
                </span>
                <span className="min-w-0 text-xs text-text-primary leading-relaxed break-words">
                  {line.text}
                </span>
              </div>
            ))}
          </div>
        )}
      </div>

      <p className="shrink-0 border-t border-border-default px-4 py-2 text-[10px] text-text-tertiary">
        Words as the aligner scored them — lowercased, numbers spelled out — which is why a guess
        lands where it does.
      </p>
    </>
  );
}
