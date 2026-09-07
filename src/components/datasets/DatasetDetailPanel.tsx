"use client";

import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/tauri";
import {
  type DatasetDetail as DatasetDetailType,
  type DatasetProgressEvt,
  btnSmPrimary,
  btnSmDanger,
  actionLabel,
} from "@/lib/datasets/types";

interface Props {
  detail: DatasetDetailType;
  datasetPath: string;
  progress: DatasetProgressEvt | null;
  generating: boolean;
  setGenerating: (v: boolean) => void;
  setProgress: (v: DatasetProgressEvt | null) => void;
  onRefreshDetail: () => Promise<void>;
  onRefreshList: () => Promise<void>;
}

export default function DatasetDetailPanel({
  detail,
  datasetPath,
  progress,
  generating,
  setGenerating,
  setProgress,
  onRefreshDetail,
  onRefreshList,
}: Props) {
  const [deletingSubtitles, setDeletingSubtitles] = useState(false);
  const [deletingWaveforms, setDeletingWaveforms] = useState(false);
  const [deletingDatabase, setDeletingDatabase] = useState(false);
  const [writingTranscripts, setWritingTranscripts] = useState(false);
  const [parsingBook, setParsingBook] = useState(false);
  const [aligningCues, setAligningCues] = useState(false);
  const [modelLoading, setModelLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // ---- Generic action runner ----

  async function runAction(
    command: string,
    args: Record<string, unknown>,
    opts: {
      setLoading: (v: boolean) => void;
      refreshList?: boolean;
      label: string;
    }
  ) {
    if (!isTauri()) return;
    setError(null);
    opts.setLoading(true);
    try {
      await invoke(command, args);
      await onRefreshDetail();
      if (opts.refreshList) await onRefreshList();
    } catch (e) {
      setError(String(e));
    } finally {
      opts.setLoading(false);
    }
  }

  // ---- Handlers ----

  async function handleGenerateSubtitles() {
    if (!isTauri()) return;
    setError(null);
    try {
      const modelStatus = await invoke<{
        models: { id: string; label: string; downloaded: boolean }[];
        active_version: string | null;
        active_status: string;
      }>("model_get_status");
      const downloaded = modelStatus.models.filter((m) => m.downloaded);
      if (downloaded.length === 0) {
        setError("No models are downloaded. Please go to the Models page to download a model first.");
        return;
      }
      if (modelStatus.active_version && modelStatus.active_status === "Running") {
        await doGenerate();
        return;
      }
      // For now, auto-select the first downloaded model
      setModelLoading(true);
      await invoke("model_select_version", { version: downloaded[0].id });
      await invoke("model_start");
      await doGenerate();
    } catch (e) {
      setError(`Failed to check model status: ${e}`);
    } finally {
      setModelLoading(false);
    }
  }

  async function doGenerate() {
    setError(null);
    setGenerating(true);
    setProgress(null);
    try {
      await invoke("dataset_generate_subtitles", { uuid: detail.info.uuid });
      await onRefreshDetail();
      await onRefreshList();
    } catch (e) {
      setError(String(e));
    } finally {
      setGenerating(false);
      setProgress(null);
    }
  }

  const handleDeleteSubtitles = () =>
    runAction("dataset_delete_subtitles", { uuid: detail.info.uuid }, { setLoading: setDeletingSubtitles, refreshList: true, label: "Delete subtitles" });

  const handleDeleteWaveforms = () =>
    runAction("dataset_delete_waveforms", { uuid: detail.info.uuid }, { setLoading: setDeletingWaveforms, label: "Delete waveforms" });

  const handleDeleteDatabase = () =>
    runAction("dataset_delete_database", { uuid: detail.info.uuid }, { setLoading: setDeletingDatabase, refreshList: true, label: "Delete database" });

  const handleGenerateWaveform = () =>
    runAction("dataset_generate_waveform", { uuid: detail.info.uuid }, { setLoading: setGenerating, refreshList: true, label: "Generate waveform" });

  const handleGenerateDatabase = () =>
    runAction("dataset_generate_database", { uuid: detail.info.uuid }, { setLoading: setGenerating, refreshList: true, label: "Generate database" });

  const handleWriteTranscripts = () =>
    runAction("dataset_write_transcripts", { datasetUuid: detail.info.uuid }, { setLoading: setWritingTranscripts, label: "Write transcripts" });

  const handleParseBook = () =>
    runAction("dataset_parse_book", { datasetUuid: detail.info.uuid }, { setLoading: setParsingBook, label: "Parse book" });

  const handleAlignCues = () =>
    runAction("dataset_align_cues", { datasetUuid: detail.info.uuid }, { setLoading: setAligningCues, label: "Align cues" });

  // ---- Render ----

  const hasSubtitles = detail.has_subtitles;
  const hasWaveforms = detail.has_waveforms;
  const hasDatabase = detail.has_database;

  return (
    <div className="px-4 pb-4 border-t border-border-default">
      <p className="text-xs text-text-tertiary font-mono mb-2">{datasetPath}</p>
      {detail.info.description && (
        <p className="text-sm text-text-secondary mb-3">{detail.info.description}</p>
      )}

      <div className="flex flex-col gap-1.5 mt-3">
        {/* Subtitles */}
        <div className="flex items-center gap-2">
          <span className={actionLabel}>1. generate subtitles</span>
          <button className={btnSmDanger} onClick={handleDeleteSubtitles} disabled={deletingSubtitles || !hasSubtitles}>
            {deletingSubtitles ? "Deleting..." : "Delete"}
          </button>
          <button className={btnSmPrimary} onClick={handleGenerateSubtitles} disabled={generating || modelLoading || hasSubtitles}>
            {generating ? "Generating..." : modelLoading ? "Loading..." : "Generate"}
          </button>
        </div>

        {/* Waveforms */}
        <div className="flex items-center gap-2">
          <span className={actionLabel}>2. generate waveforms</span>
          <button className={btnSmDanger} onClick={handleDeleteWaveforms} disabled={deletingWaveforms || !hasWaveforms}>
            {deletingWaveforms ? "Deleting..." : "Delete"}
          </button>
          <button className={btnSmPrimary} onClick={handleGenerateWaveform} disabled={generating || hasWaveforms}>
            {generating ? "Generating..." : "Generate"}
          </button>
        </div>

        {/* Database */}
        <div className="flex items-center gap-2">
          <span className={actionLabel}>3. generate database</span>
          <button className={btnSmDanger} onClick={handleDeleteDatabase} disabled={deletingDatabase || !hasDatabase}>
            {deletingDatabase ? "Deleting..." : "Delete"}
          </button>
          <button className={btnSmPrimary} onClick={handleGenerateDatabase} disabled={generating || hasDatabase}>
            {generating ? "Generating..." : "Generate"}
          </button>
        </div>

        {/* Transcripts */}
        <div className="flex items-center gap-2">
          <span className={actionLabel}>4. write transcripts</span>
          {hasDatabase ? (
            detail.media.some((m) => m.has_transcript) ? (
              <button className={btnSmPrimary} onClick={handleWriteTranscripts} disabled={writingTranscripts || generating}>
                {writingTranscripts ? "Writing..." : "Write"}
              </button>
            ) : (
              <span className="text-xs text-text-tertiary">No transcripts.</span>
            )
          ) : (
            <span className="text-xs text-text-tertiary">No database.</span>
          )}
        </div>

        {/* Book */}
        <div className="flex items-center gap-2">
          <span className={actionLabel}>5. write book</span>
          {hasDatabase ? (
            detail.has_book ? (
              <button className={btnSmPrimary} onClick={handleParseBook} disabled={parsingBook || generating}>
                {parsingBook ? "Parsing..." : "Split"}
              </button>
            ) : (
              <span className="text-xs text-text-tertiary">No book.</span>
            )
          ) : (
            <span className="text-xs text-text-tertiary">No database.</span>
          )}
        </div>

        {/* Align cues */}
        <div className="flex items-center gap-2">
          <span className={actionLabel}>6. align cues</span>
          {hasDatabase ? (
            detail.has_book ? (
              <button className={btnSmPrimary} onClick={handleAlignCues} disabled={aligningCues || generating}>
                {aligningCues ? "Aligning..." : "Align"}
              </button>
            ) : (
              <span className="text-xs text-text-tertiary">No book.</span>
            )
          ) : (
            <span className="text-xs text-text-tertiary">No database.</span>
          )}
        </div>
      </div>

      {generating && progress && (
        <div className="mt-3">
          <div className="text-sm mb-1">
            {progress.stage === "subtitles" ? "Transcribing" : progress.stage === "waveform" ? "Waveform" : "Database"}:{" "}
            {progress.current_file} ({progress.file_index}/{progress.total_files})
          </div>
          <div className="w-full h-2 bg-bg-progress rounded-full overflow-hidden">
            <div
              className="h-full bg-accent-bg transition-[width] duration-300 ease-in-out"
              style={{ width: `${(progress.file_index / progress.total_files) * 100}%` }}
            />
          </div>
        </div>
      )}
      {error && (
        <div className="p-3 bg-error-bg text-error-text rounded-md mt-3 text-sm">{error}</div>
      )}
    </div>
  );
}
