"use client";

import { Check, FolderOpen, Loader2, ScanText, Square, Wand2 } from "lucide-react";
import type { AlignSource } from "@/lib/read_aloud/sourceAlign";
import { clockShort, percent } from "./format";

/**
 * Everything that has to happen before a passage can be judged: a folder of
 * recordings, one transcript per file, and a guess that places the passages
 * along them.
 *
 * These were four chips on a bar above the review, competing for attention with
 * the work itself and printing their numbers in ten-point type. They are a page
 * of their own now — one roomy card per job, with the per-file table sitting
 * where the transcripts are actually made.
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
  mobile: boolean;
  directory: string;
  sources: AlignSource[];
  transcribed: number;
  words: number;
  busy: string | null;
  passagesTotal: number;
  placed: number;
  refused: number;
  meanConfidence: number | null;
  onPick: () => void;
  onTranscribeAll: () => void;
  onTranscribeFile: (sourceUuid: string) => void;
  onStop: () => void;
  onGuess: () => void;
};

export default function RecordingsTab({
  mobile,
  directory,
  sources,
  transcribed,
  words,
  busy,
  passagesTotal,
  placed,
  refused,
  meanConfidence,
  onPick,
  onTranscribeAll,
  onTranscribeFile,
  onStop,
  onGuess,
}: Props) {
  const busyLabel = busy != null;
  const transcribing = busy === "transcribing";
  const listening = busy === "listening";
  const guessing = busy === "guessing";
  const allDone = sources.length > 0 && transcribed === sources.length;

  return (
    <div className="flex flex-col gap-3 max-w-[820px]">
      <section className={CARD}>
        <div className={CARD_HEAD}>
          <h3 className="text-xs font-semibold text-text-primary">Folder</h3>
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
          <p className="text-[11px] text-text-tertiary leading-relaxed">
            The recordings stay where they are. Picking the folder reads their names and lengths;
            only step 4 writes anything into the dataset.
          </p>
          <div>
            <button
              type="button"
              onClick={onPick}
              disabled={mobile || busyLabel}
              className={NEUTRAL}
              title={
                mobile
                  ? "Picking a folder is a desktop action; this device can review spans and play them"
                  : "Pick the folder that holds the audiobook's files"
              }
            >
              <FolderOpen size={12} />
              {directory ? "Change folder" : "Pick folder"}
            </button>
          </div>
        </div>
      </section>

      <section className={CARD}>
        <div className={CARD_HEAD}>
          <h3 className="text-xs font-semibold text-text-primary">Transcripts</h3>
          <span className="ml-auto text-[10px] text-text-tertiary tabular-nums">
            {transcribed}/{sources.length} files
            {words > 0 ? ` · ${words.toLocaleString()} words` : ""}
          </span>
        </div>
        <div className={CARD_BODY}>
          <p className="text-[11px] text-text-tertiary leading-relaxed">
            Each file is heard once and its word timings are cached beside the dataset, so a
            re-guess costs a second rather than an hour. One file per request — the loop stops
            between files.
          </p>

          {sources.length === 0 ? (
            <p className="text-xs text-text-tertiary">No recordings yet: pick the folder above.</p>
          ) : (
            <div className="flex flex-col gap-1">
              {sources.map((file) => {
                const listened = file.words != null;
                return (
                  <div key={file.uuid} className={ROW}>
                    <span className="shrink-0 text-accent-bg/70" aria-hidden>
                      {listened ? <Check size={11} /> : <span className="block w-[11px]" />}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block text-xs text-text-primary truncate" title={file.path}>
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
                    <button
                      type="button"
                      onClick={() => onTranscribeFile(file.uuid)}
                      disabled={mobile || busyLabel}
                      aria-label={`Transcribe ${file.file_name}`}
                      title={
                        mobile
                          ? "Transcribing is a desktop action"
                          : listened
                            ? "Listen again — replaces this file's transcript, so re-guess afterwards"
                            : "Listen to this file"
                      }
                      className="shrink-0 inline-flex items-center justify-center w-6 h-6 rounded-md bg-bg-muted text-text-secondary hover:bg-bg-hover cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed"
                    >
                      <ScanText size={12} />
                    </button>
                  </div>
                );
              })}
            </div>
          )}

          <div className="flex items-center gap-2 flex-wrap">
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
                {transcribing || listening ? (
                  <Loader2 size={12} className="animate-spin" />
                ) : (
                  <ScanText size={12} />
                )}
                {transcribed === 0 ? "Transcribe all" : "Transcribe the rest"}
              </button>
            )}
            {listening && (
              <span className="text-[11px] text-text-tertiary">Listening to a file…</span>
            )}
          </div>
        </div>
      </section>

      <section className={CARD}>
        <div className={CARD_HEAD}>
          <h3 className="text-xs font-semibold text-text-primary">Place passages</h3>
          <span className="ml-auto text-[10px] text-text-tertiary tabular-nums">
            {placed}/{passagesTotal}
          </span>
        </div>
        <div className={CARD_BODY}>
          <p className="text-[11px] text-text-tertiary leading-relaxed">
            Every passage is walked in reading order along the transcripts, so a misheard word
            costs one passage rather than the rest of the book. Nothing is written to media/.
          </p>
          {placed > 0 && (
            <p className="text-[11px] text-text-secondary tabular-nums">
              {placed} of {passagesTotal} placed
              {meanConfidence == null ? "" : ` · mean ${percent(meanConfidence)}%`}
              {refused > 0 ? ` · ${refused} refused` : ""}
            </p>
          )}
          <div>
            <button
              type="button"
              onClick={onGuess}
              disabled={busyLabel || transcribed === 0}
              className={placed > 0 ? NEUTRAL : PRIMARY}
              title={
                transcribed === 0
                  ? "Transcribe at least one file first"
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
          </div>
        </div>
      </section>
    </div>
  );
}
