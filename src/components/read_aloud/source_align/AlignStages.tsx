"use client";

import type { ReactNode } from "react";
import { Check, FolderOpen, Loader2, ScanText, Square, Upload, Wand2 } from "lucide-react";
import { percent } from "./format";

/**
 * The four steps of the job, always on screen.
 *
 * The pipeline used to be implicit — buttons that appeared and vanished as
 * their prerequisites were met — so the page never said where you were. These
 * chips name each step, print its state, and carry the one action it takes; a
 * step whose prerequisite is missing is drawn as blocked rather than hidden.
 */

type Tone = "done" | "current" | "blocked";

type StageProps = {
  number: number;
  label: string;
  detail: string;
  tone: Tone;
  action: {
    label: string;
    onClick: () => void;
    disabled?: boolean;
    primary?: boolean;
    icon?: ReactNode;
    title?: string;
  };
};

const TONE_BOX: Record<Tone, string> = {
  done: "border-accent-bg/40",
  current: "border-accent-bg",
  blocked: "border-border-default opacity-60",
};

const TONE_BADGE: Record<Tone, string> = {
  done: "border-accent-bg/40 text-accent-bg",
  current: "border-accent-bg text-accent-bg",
  blocked: "border-border-default text-text-tertiary",
};

function Stage({ number, label, detail, tone, action }: StageProps) {
  return (
    <div
      className={`flex items-center gap-2 min-w-0 flex-1 basis-[190px] rounded-lg border px-2.5 py-1.5 bg-bg-card ${TONE_BOX[tone]}`}
    >
      <span
        aria-hidden
        className={`shrink-0 inline-flex items-center justify-center w-5 h-5 rounded-full border text-[10px] tabular-nums ${TONE_BADGE[tone]}`}
      >
        {tone === "done" ? <Check size={11} /> : number}
      </span>
      <div className="min-w-0 flex-1">
        <div className="text-[11px] font-medium text-text-primary truncate">{label}</div>
        <div className="text-[10px] text-text-tertiary truncate tabular-nums" title={detail}>
          {detail}
        </div>
      </div>
      <button
        type="button"
        onClick={action.onClick}
        disabled={action.disabled}
        title={action.title}
        className={`shrink-0 inline-flex items-center gap-1 px-2 py-1 rounded-md text-[11px] cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed ${
          action.primary
            ? "bg-accent-bg text-white"
            : "bg-bg-muted text-text-secondary hover:bg-bg-hover"
        }`}
      >
        {action.icon}
        {action.label}
      </button>
    </div>
  );
}

type Props = {
  mobile: boolean;
  directory: string;
  sourceCount: number;
  transcribed: number;
  words: number;
  passagesTotal: number;
  placed: number;
  meanConfidence: number | null;
  open: number;
  applied: number;
  busy: string | null;
  error: string | null;
  note: { kind: "ok" | "warn"; text: string } | null;
  onPick: () => void;
  onTranscribe: () => void;
  onStop: () => void;
  onGuess: () => void;
  onApply: () => void;
};

export default function AlignStages({
  mobile,
  directory,
  sourceCount,
  transcribed,
  words,
  passagesTotal,
  placed,
  meanConfidence,
  open,
  applied,
  busy,
  error,
  note,
  onPick,
  onTranscribe,
  onStop,
  onGuess,
  onApply,
}: Props) {
  const busyLabel = busy != null;
  const folderName = directory
    ? directory.split(/[\\/]/).filter(Boolean).pop() ?? directory
    : "not picked";
  const hasFolder = Boolean(directory);
  const transcribing = busy === "transcribing";
  const cutting = busy === "cutting";

  const folderStage: StageProps = {
    number: 1,
    label: "Recordings folder",
    detail: hasFolder ? folderName : "not picked",
    tone: hasFolder ? "done" : "current",
    action: {
      label: hasFolder ? "Change" : "Pick",
      onClick: onPick,
      disabled: mobile || busyLabel,
      icon: <FolderOpen size={12} />,
      title: mobile
        ? "Picking a folder is a desktop action; this device can review spans and play them"
        : "Pick the folder that holds the audiobook's files",
    },
  };

  const transcribeStage: StageProps = {
    number: 2,
    label: "Transcribe",
    detail:
      sourceCount === 0
        ? "no files registered"
        : `${transcribed}/${sourceCount} files · ${words.toLocaleString()} words`,
    tone:
      sourceCount > 0 && transcribed === sourceCount
        ? "done"
        : sourceCount > 0
          ? "current"
          : "blocked",
    action: transcribing
      ? { label: "Stop", onClick: onStop, icon: <Square size={12} /> }
      : {
          label: "Transcribe",
          onClick: onTranscribe,
          disabled: mobile || busyLabel || sourceCount === 0 || transcribed === sourceCount,
          icon: <ScanText size={12} />,
          title:
            transcribed === sourceCount && sourceCount > 0
              ? "Every file has a transcript; redo one from the Files card"
              : "Listen to every file without a transcript yet, one file per request",
        },
  };

  const guessStage: StageProps = {
    number: 3,
    label: "Place passages",
    detail:
      passagesTotal === 0
        ? "no texts yet"
        : `placed ${placed}/${passagesTotal}${
            meanConfidence == null ? "" : ` · mean ${percent(meanConfidence)}%`
          }`,
    tone: placed > 0 && placed === passagesTotal ? "done" : transcribed > 0 ? "current" : "blocked",
    action: {
      label: busy === "guessing" ? "Guessing…" : placed > 0 ? "Re-guess" : "Guess all",
      onClick: onGuess,
      disabled: busyLabel || transcribed === 0,
      icon:
        busy === "guessing" ? (
          <Loader2 size={12} className="animate-spin" />
        ) : (
          <Wand2 size={12} />
        ),
    },
  };

  const cutStage: StageProps = {
    number: 4,
    label: "Cut clips",
    detail:
      open > 0
        ? `${open} accepted to cut${applied > 0 ? ` · ${applied} cut` : ""}`
        : applied > 0
          ? `${applied} cut · nothing waiting`
          : "nothing accepted yet",
    tone: open === 0 && applied > 0 ? "done" : open > 0 ? "current" : "blocked",
    action: {
      label: cutting ? "Cutting…" : open > 0 ? `Apply ${open}` : "Apply",
      onClick: onApply,
      disabled: busyLabel || open === 0,
      primary: open > 0,
      icon: cutting ? <Loader2 size={12} className="animate-spin" /> : <Upload size={12} />,
    },
  };

  const status = error ? (
    <p className="text-[11px] text-error-text break-words">{error}</p>
  ) : note ? (
    <p
      className={`text-[11px] break-words ${
        note.kind === "ok" ? "text-text-tertiary" : "text-warning-text"
      }`}
    >
      {note.text}
    </p>
  ) : busy && busy !== "transcribing" && busy !== "cutting" ? (
    <p className="text-[11px] text-text-tertiary">
      {busy === "picking"
        ? "Picking the folder…"
        : busy === "guessing"
          ? "Placing every passage…"
          : `${busy}…`}
    </p>
  ) : (
    <p className="text-[11px] text-text-tertiary">
      {hasFolder
        ? `Every recording is listened to once, then the passages are placed along them. Nothing is written to the dataset until step 4.`
        : `Pick the folder that holds the audiobook's files. The files stay where they are; only the clips are written into the dataset.`}
    </p>
  );

  return (
    <div className="shrink-0 px-4 py-2 border-b border-border-default bg-bg-card">
      <div className="flex flex-wrap items-stretch gap-2">
        <Stage {...folderStage} />
        <Stage {...transcribeStage} />
        <Stage {...guessStage} />
        <Stage {...cutStage} />
      </div>
      {status}
    </div>
  );
}
