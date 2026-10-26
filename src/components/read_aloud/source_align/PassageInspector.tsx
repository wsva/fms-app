"use client";

import type { ReactNode } from "react";
import { Check, ChevronLeft, ChevronRight, ScanText } from "lucide-react";
import type { AlignProposal, AlignSource } from "@/lib/read_aloud/sourceAlign";
import { clock, clockShort, confidenceTone, percent } from "./format";

/**
 * The context rail: what the span is made of, and what the job still owes.
 *
 * The center pane answers "is this the right clip"; this one answers "why", and
 * "what else is left" — the recording the passage came from, the words that
 * matched, the state of every file in the folder, and the keys that drive it.
 */

function Card({
  title,
  aside,
  children,
}: {
  title: string;
  aside?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="rounded-xl border border-border-default bg-bg-card overflow-hidden">
      <div className="flex items-baseline gap-2 px-3 py-2 border-b border-border-default">
        <h3 className="text-xs font-semibold text-text-primary">{title}</h3>
        {aside && (
          <span className="ml-auto text-[10px] text-text-tertiary tabular-nums">{aside}</span>
        )}
      </div>
      <div className="px-3 py-2 flex flex-col gap-1.5">{children}</div>
    </section>
  );
}

function Row({ label, value, title }: { label: string; value: ReactNode; title?: string }) {
  return (
    <div className="flex items-baseline justify-between gap-2 text-[11px]">
      <span className="text-text-tertiary">{label}</span>
      <span className="text-text-primary tabular-nums truncate" title={title}>
        {value}
      </span>
    </div>
  );
}

const KEY_ROWS: { keys: string[]; label: string }[] = [
  { keys: ["Space"], label: "play / stop the span" },
  { keys: ["A"], label: "accept" },
  { keys: ["R"], label: "reject" },
  { keys: ["←", "→"], label: "nudge the active edge 10 ms" },
  { keys: ["Shift", "←/→"], label: "nudge it 100 ms" },
  { keys: ["↑", "↓"], label: "previous / next passage" },
  { keys: ["Esc"], label: "stop the audio" },
];

type Props = {
  proposal: AlignProposal | null;
  source: AlignSource | null;
  position: { index: number; total: number } | null;
  sources: AlignSource[];
  /** The first placed passage of each recording, for the file rows. */
  firstBySource: Record<string, string | undefined>;
  busy: string | null;
  mobile: boolean;
  canPrev: boolean;
  canNext: boolean;
  onStep: (direction: -1 | 1) => void;
  onSelectSource: (sourceUuid: string) => void;
  onTranscribeFile: (sourceUuid: string) => void;
};

export default function PassageInspector({
  proposal,
  source,
  position,
  sources,
  firstBySource,
  busy,
  mobile,
  canPrev,
  canNext,
  onStep,
  onSelectSource,
  onTranscribeFile,
}: Props) {
  const refused = proposal?.status === "refused";
  return (
    <aside className="flex flex-col gap-3 min-w-0 min-h-0 overflow-y-auto">
      {proposal && source && (
        <Card title="Recording" aside={position ? `${position.index}/${position.total}` : undefined}>
          <div className="flex items-center gap-1">
            <span
              className="min-w-0 flex-1 text-[11px] text-text-primary truncate"
              title={source.path}
            >
              {source.file_name}
            </span>
            <button
              type="button"
              onClick={() => onStep(-1)}
              disabled={!canPrev}
              aria-label="Previous passage in this recording"
              title="Previous passage in this recording"
              className="shrink-0 inline-flex items-center justify-center w-5 h-5 rounded-md bg-bg-muted text-text-secondary hover:bg-bg-hover cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed"
            >
              <ChevronLeft size={11} />
            </button>
            <button
              type="button"
              onClick={() => onStep(1)}
              disabled={!canNext}
              aria-label="Next passage in this recording"
              title="Next passage in this recording"
              className="shrink-0 inline-flex items-center justify-center w-5 h-5 rounded-md bg-bg-muted text-text-secondary hover:bg-bg-hover cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed"
            >
              <ChevronRight size={11} />
            </button>
          </div>
          <Row label="Position" value={position ? `${position.index} of ${position.total}` : "—"} />
          <Row
            label="Duration"
            value={source.duration != null ? clockShort(source.duration) : "—"}
            title={source.duration != null ? clock(source.duration) : undefined}
          />
          <Row
            label="Transcript"
            value={source.words != null ? `${source.words.toLocaleString()} words` : "none yet"}
          />
          <p className="text-[10px] text-text-tertiary break-all line-clamp-2" title={source.path}>
            {source.path}
          </p>
        </Card>
      )}

      {proposal && (
        <Card title="Match" aside={refused ? "refused" : undefined}>
          {refused ? (
            <p className="text-[11px] text-warning-text">
              {proposal.note || "The guess refused this passage."}
            </p>
          ) : (
            <>
              <div className="flex items-baseline gap-2">
                <span
                  className={`text-2xl font-medium tabular-nums ${confidenceTone(proposal.confidence)}`}
                >
                  {percent(proposal.confidence)}%
                </span>
                <span className="text-[10px] text-text-tertiary">
                  {proposal.matched_words} of {proposal.reference_words} words matched
                </span>
              </div>
              <div className="h-1.5 rounded-full bg-bg-muted overflow-hidden">
                <div
                  className={`h-full ${
                    proposal.confidence >= 0.7 ? "bg-accent-bg" : "bg-warning-text"
                  }`}
                  style={{ width: `${percent(proposal.confidence)}%` }}
                />
              </div>
              <Row
                label="Span"
                value={`${clock(proposal.start)} → ${clock(proposal.end)}`}
                title={`${(proposal.end - proposal.start).toFixed(3)} s`}
              />
              {proposal.note && (
                <p className="text-[11px] text-text-tertiary">{proposal.note}</p>
              )}
            </>
          )}
        </Card>
      )}

      <Card title="Files" aside={`${sources.filter((s) => s.words != null).length}/${sources.length}`}>
        {sources.length === 0 ? (
          <p className="text-[11px] text-text-tertiary">No recordings registered yet.</p>
        ) : (
          sources.map((file) => {
            const listened = file.words != null;
            const current = source?.uuid === file.uuid;
            const placed = firstBySource[file.uuid];
            return (
              <div
                key={file.uuid}
                className={`flex items-center gap-1 rounded-md px-1 py-1 ${
                  current ? "bg-bg-muted" : ""
                }`}
              >
                <button
                  type="button"
                  onClick={() => onSelectSource(file.uuid)}
                  disabled={!placed}
                  className="min-w-0 flex-1 text-left cursor-pointer disabled:cursor-default"
                  title={placed ? "Go to the first passage of this file" : "Nothing placed here yet"}
                >
                  <span className="block text-[11px] text-text-primary truncate" title={file.path}>
                    {file.file_name}
                  </span>
                  <span className="block text-[10px] text-text-tertiary tabular-nums">
                    {listened
                      ? `${(file.words ?? 0).toLocaleString()} words · ${clockShort(file.duration ?? 0)}`
                      : "no transcript"}
                  </span>
                </button>
                {listened && (
                  <span className="shrink-0 text-accent-bg/70" title="Transcript cached">
                    <Check size={11} />
                  </span>
                )}
                <button
                  type="button"
                  onClick={() => onTranscribeFile(file.uuid)}
                  disabled={mobile || busy != null}
                  aria-label={`Transcribe ${file.file_name}`}
                  title={
                    mobile
                      ? "Transcribing is a desktop action"
                      : listened
                        ? "Listen again — replaces this file's transcript, so re-guess afterwards"
                        : "Listen to this file without leaving the page"
                  }
                  className="shrink-0 inline-flex items-center justify-center w-5 h-5 rounded-md bg-bg-muted text-text-secondary hover:bg-bg-hover cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed"
                >
                  <ScanText size={11} />
                </button>
              </div>
            );
          })
        )}
      </Card>

      <Card title="Keys">
        {KEY_ROWS.map((row) => (
          <div key={row.label} className="flex items-center gap-2 text-[10px] text-text-tertiary">
            <span className="flex items-center gap-0.5 shrink-0">
              {row.keys.map((key) => (
                <kbd
                  key={key}
                  className="px-1 py-0.5 rounded border border-border-default bg-bg-muted text-text-secondary text-[9px] tabular-nums"
                >
                  {key}
                </kbd>
              ))}
            </span>
            <span className="min-w-0 truncate">{row.label}</span>
          </div>
        ))}
      </Card>
    </aside>
  );
}
