"use client";

import { useCallback, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  readAlignTranscript,
  registerAlignSources,
  transcribeAlignSource,
  type AlignReconcile,
  type AlignSource,
  type AlignSourceIndex,
  type AlignTranscript,
} from "@/lib/read_aloud/sourceAlign";
import type { ConfirmRequest } from "@/components/books/ConfirmDialog";
import { clock, errorText } from "./format";

/** One empty list for the "no index yet" case, so `sources` keeps its identity. */
const NO_SOURCES: AlignSource[] = [];

/**
 * What a folder pick amounts to, in one line.
 *
 * The useful number is not how many files there are — it is how many of them
 * this dataset had already heard. A pick that shares nothing with the folder it
 * replaced is worth a warning before someone presses Transcribe all on the
 * wrong book; a pick that kept everything is worth saying out loud.
 */
function reconcileNote(
  reconcile: AlignReconcile | null,
  directory: string,
  replaced: boolean
): { kind: "ok" | "warn"; text: string } {
  if (!reconcile) {
    return { kind: "ok", text: `Connected to ${directory}.` };
  }
  const parts = [`${reconcile.total} recording${reconcile.total === 1 ? "" : "s"}`];
  if (reconcile.reused > 0) parts.push(`${reconcile.reused} kept`);
  if (reconcile.adopted > 0) parts.push(`${reconcile.adopted} adopted by content`);
  if (reconcile.new > 0) parts.push(`${reconcile.new} to listen to`);
  if (reconcile.missing > 0) parts.push(`${reconcile.missing} no longer here`);
  const summary = parts.join(" · ");

  const recovered = reconcile.reused + reconcile.adopted;
  if (reconcile.reordered > 0) {
    return {
      kind: "warn",
      text: `${summary} — the playing order changed, so place the texts again.`,
    };
  }
  if (replaced && recovered === 0) {
    return {
      kind: "warn",
      text: `${summary}. Nothing here was heard before, so every file needs listening — the old cache is kept, so you can go back.`,
    };
  }
  return { kind: "ok", text: `${summary}.` };
}

/**
 * The Source Audios half of the page: the folder of recordings, their
 * transcripts, and the one model call that produces one.
 *
 * Nothing here touches a span. A recording is heard **once** — its words and
 * their times are what every span is placed against — so the only writes are
 * the transcript cache and the folder listing, and the only long call is one
 * file at a time, because a chapter is minutes of model time and no request
 * should be held open that long.
 */
export function useAlignSources({
  datasetUuid,
  pipeline,
  setError,
  setNote,
  setConfirm,
  setSelected,
}: {
  datasetUuid: string;
  /** Run one locked step, writing pending nudges before it starts. */
  pipeline: (label: string, task: () => Promise<void>) => void;
  setError: (message: string | null) => void;
  setNote: (note: { kind: "ok" | "warn"; text: string } | null) => void;
  /** The "listen again?" question: a re-listen replaces the timings. */
  setConfirm: (request: ConfirmRequest | null) => void;
  setSelected: (textUuid: string | null) => void;
}) {
  const [index, setIndex] = useState<AlignSourceIndex | null>(null);
  /** The folder the current one replaced, so going back is one click. */
  const [previousDirectory, setPreviousDirectory] = useState("");
  /** The recording whose result is in the right pane. */
  const [sourceSel, setSourceSel] = useState<string | null>(null);
  /** That recording's cached transcript, read on demand. */
  const [transcript, setTranscript] = useState<AlignTranscript | null>(null);
  const [transcriptBusy, setTranscriptBusy] = useState(false);
  const [transcriptError, setTranscriptError] = useState<string | null>(null);
  /** The file being listened to right now; its row shows the spinner. */
  const [listeningUuid, setListeningUuid] = useState<string | null>(null);
  /** Which transcript request is the current one, so a slow answer can be dropped. */
  const transcriptSeq = useRef(0);
  /** Set by the Stop button; the transcription loop checks it between files. */
  const stopRef = useRef(false);

  const sources = index?.sources ?? NO_SOURCES;
  const transcribed = index?.transcribed ?? 0;
  const words = useMemo(() => sources.reduce((sum, s) => sum + (s.words ?? 0), 0), [sources]);

  /** Re-read what is registered — listing only, no model call. */
  const reload = useCallback(async () => {
    const registered = await registerAlignSources(datasetUuid);
    setIndex(registered);
    return registered;
  }, [datasetUuid]);

  /**
   * Connect the dataset to `directory` and say what the connection found.
   *
   * A pick is a *connect*, not a rebuild: the backend reuses every cache whose
   * bytes match, so the answer a person needs is how much of this folder was
   * already heard. The folder it replaced is remembered because nothing was
   * deleted — going back is free.
   */
  const connect = async (directory: string) => {
    const was = index?.directory ?? "";
    const registered = await registerAlignSources(datasetUuid, directory);
    setIndex(registered);
    setSelected(null);
    // Another folder is another set of files: the result on the right would
    // be from a file this dataset no longer has.
    setSourceSel(null);
    setTranscript(null);
    setTranscriptError(null);
    const replaced = was !== "" && was !== registered.directory;
    setPreviousDirectory(replaced ? was : "");
    setNote(reconcileNote(registered.reconcile, registered.directory, replaced));
    return registered;
  };

  const pickFolder = () =>
    pipeline("picking", async () => {
      const picked = await open({
        directory: true,
        multiple: false,
        title: "Folder that holds the recordings",
      });
      if (typeof picked !== "string") return;
      await connect(picked);
    });

  /** Back to the folder this one replaced — free, because nothing was deleted. */
  const reconnectToPrevious = () => {
    const previous = previousDirectory;
    if (!previous) return;
    pipeline("picking", async () => {
      await connect(previous);
    });
  };

  /**
   * One file per call, so the Stop button is honoured before the next
   * transcription starts.
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

  /**
   * One file per call. The result is opened on the right as soon as it lands:
   * the answer to "listen" is the words, not a toast.
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
          )})${repeat ? " — re-guess to re-place this file's texts." : "."}`,
        });
      } finally {
        setListeningUuid(null);
      }
    });
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
            texts already placed from this file keep the spans they were given — place them
            again afterwards.
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

  /** The Stop button: the loop reads this between files. */
  const stopTranscribing = () => {
    stopRef.current = true;
  };

  /** A different dataset is a different folder and a different cache. */
  const reset = useCallback(() => {
    setIndex(null);
    setPreviousDirectory("");
    setSourceSel(null);
    setTranscript(null);
    setTranscriptError(null);
    setListeningUuid(null);
    stopRef.current = false;
  }, []);

  return {
    index,
    sources,
    transcribed,
    words,
    sourceSel,
    previousDirectory,
    transcript,
    transcriptBusy,
    transcriptError,
    listeningUuid,
    reload,
    pickFolder,
    reconnectToPrevious,
    transcribeAll,
    transcribeOne,
    openTranscript,
    listenTo,
    stopTranscribing,
    reset,
  };
}
