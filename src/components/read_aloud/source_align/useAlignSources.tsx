"use client";

import { useCallback, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  readAlignTranscript,
  registerAlignSources,
  transcribeAlignSource,
  type AlignSource,
  type AlignSourceIndex,
  type AlignTranscript,
} from "@/lib/read_aloud/sourceAlign";
import type { ConfirmRequest } from "@/components/read_book/ConfirmDialog";
import { clock, errorText } from "./format";

/** One empty list for the "no index yet" case, so `sources` keeps its identity. */
const NO_SOURCES: AlignSource[] = [];

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

  const pickFolder = () =>
    pipeline("picking", async () => {
      const picked = await open({
        directory: true,
        multiple: false,
        title: "Folder that holds the recordings",
      });
      if (typeof picked !== "string") return;
      const registered = await registerAlignSources(datasetUuid, picked);
      setIndex(registered);
      setSelected(null);
      // Another folder is another set of files: the result on the right would
      // be from a file this dataset no longer has.
      setSourceSel(null);
      setTranscript(null);
      setTranscriptError(null);
      setNote({
        kind: "ok",
        text: `${registered.source_count} recording${
          registered.source_count === 1 ? "" : "s"
        } from ${registered.directory}`,
      });
    });

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
          )})${repeat ? " — re-guess to re-place this file's passages." : "."}`,
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

  /** The Stop button: the loop reads this between files. */
  const stopTranscribing = () => {
    stopRef.current = true;
  };

  /** A different dataset is a different folder and a different cache. */
  const reset = useCallback(() => {
    setIndex(null);
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
    transcript,
    transcriptBusy,
    transcriptError,
    listeningUuid,
    reload,
    pickFolder,
    transcribeAll,
    transcribeOne,
    openTranscript,
    listenTo,
    stopTranscribing,
    reset,
  };
}
