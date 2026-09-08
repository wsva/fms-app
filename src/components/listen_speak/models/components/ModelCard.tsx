"use client";

import { type ModelVersionInfo, type DownloadProgress, type ModelStatus, formatBytes, formatSpeed, formatEta, statusLabel, statusBadgeClass, btnSmall } from "@/lib/models/types";

// ---------------------------------------------------------------------------
// Download progress bar
// ---------------------------------------------------------------------------

function DownloadProgressBar({ progress }: { progress: DownloadProgress }) {
  const overallPct =
    progress.overall_total_bytes > 0
      ? Math.min(100, (progress.overall_bytes_downloaded / progress.overall_total_bytes) * 100)
      : 0;

  const currentFile = progress.files.find(
    (f) => f.total_bytes == null || f.bytes_downloaded < f.total_bytes,
  );
  const completedCount = progress.files.filter(
    (f) => f.total_bytes != null && f.bytes_downloaded >= f.total_bytes,
  ).length;
  const totalCount = progress.files.length;

  return (
    <div className="mb-3 mt-2">
      <div className="w-full h-2 bg-bg-progress rounded-full overflow-hidden">
        <div
          className="h-full bg-accent-bg rounded-full transition-[width] duration-300 ease-in-out"
          style={{ width: `${overallPct}%` }}
        />
      </div>
      <div className="flex items-center justify-between mt-1.5 text-xs text-text-tertiary">
        <span className="font-medium">{overallPct.toFixed(1)}%</span>
        <span>{formatBytes(progress.overall_bytes_downloaded)} / {formatBytes(progress.overall_total_bytes)}</span>
      </div>
      <div className="flex items-center justify-between mt-1 text-[11px] text-text-tertiary">
        <span className="truncate max-w-[55%]">
          {currentFile
            ? `Downloading ${currentFile.file} (${completedCount + 1}/${totalCount})`
            : completedCount === totalCount
              ? "All files downloaded"
              : `File ${completedCount + 1}/${totalCount}`}
        </span>
        <span className="flex gap-3 shrink-0">
          {progress.speed > 0 && <span>{formatSpeed(progress.speed)}</span>}
          {progress.eta_seconds != null && progress.eta_seconds > 0 && (
            <span>ETA {formatEta(progress.eta_seconds)}</span>
          )}
        </span>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// ModelCard
// ---------------------------------------------------------------------------

interface ModelCardProps {
  model: ModelVersionInfo;
  status: ModelStatus;
  isDownloading: boolean;
  isDownloadInProgress: boolean;
  downloadProgress: DownloadProgress | null;
  modelLoading: boolean;
  confirmDelete: boolean;
  onDownload: () => void;
  onCancel: () => void;
  onLoad: () => void;
  onStop: () => void;
  onDelete: () => void;
  onCancelDelete: () => void;
  onShowDetails: () => void;
}

export default function ModelCard({
  model,
  status,
  isDownloading,
  isDownloadInProgress,
  downloadProgress,
  modelLoading,
  confirmDelete,
  onDownload,
  onCancel,
  onLoad,
  onStop,
  onDelete,
  onCancelDelete,
  onShowDetails,
}: ModelCardProps) {
  const isRunning = status === "Running";
  const isActive = model.downloaded && !isRunning && status === "Stopped";
  const isDownloaded = model.downloaded;

  const borderClass = isRunning
    ? "border-accent bg-success-bg"
    : isDownloading
      ? "border-info-text bg-info-bg"
      : isDownloaded
        ? "border-accent bg-success-bg/50"
        : "border-border-default";

  return (
    <div className={`p-3 border rounded-lg transition-colors ${borderClass}`}>
      {/* Header row */}
      <div className="flex items-start justify-between mb-2">
        <div className="flex flex-col">
          <div className="flex items-center gap-2">
            <span className="font-medium text-[1.05em]">{model.label}</span>
            <span className="text-[10px] px-1.5 py-0.5 rounded bg-bg-muted text-text-secondary">{model.engine_type}</span>
            <span className="text-[10px] text-text-tertiary">{model.size_mb} MB</span>
          </div>
          <span className="text-xs text-text-secondary mt-0.5">{model.description}</span>
        </div>
        <div className="flex items-center gap-1.5">
          <button
            className="p-0.5 rounded hover:bg-bg-hover text-text-tertiary cursor-pointer"
            onClick={onShowDetails}
            title="Model details"
          >
            <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/></svg>
          </button>
          {status !== "NotDownloaded" && status !== "Downloaded" && (
            <span className={statusBadgeClass(status)}>{statusLabel(status)}</span>
          )}
          {isDownloaded && !isDownloading && (
            <button
              className={`p-0.5 rounded cursor-pointer transition-colors ${
                confirmDelete
                  ? "bg-error-text text-white hover:bg-error-hover"
                  : "text-error-text hover:bg-error-bg"
              }`}
              onClick={onDelete}
              disabled={isDownloadInProgress}
              title={confirmDelete ? "Confirm delete" : "Delete model"}
            >
              <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
            </button>
          )}
          {status === "NotDownloaded" && (
            <button
              className="p-0.5 rounded hover:bg-bg-hover text-text-tertiary cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
              onClick={onDownload}
              disabled={isDownloadInProgress || modelLoading}
              title="Download model"
            >
              <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>
            </button>
          )}
        </div>
      </div>

      {/* Download progress */}
      {isDownloading && downloadProgress && (
        <DownloadProgressBar progress={downloadProgress} />
      )}

      {/* Action buttons */}
      <div className="flex items-center gap-1.5 mt-2">
        {isDownloading && (
          <button className={`${btnSmall} bg-error-text text-white hover:bg-error-hover`} onClick={onCancel}>
            Cancel
          </button>
        )}
        {isDownloaded && !isRunning && !isActive && (
          <button
            className={`${btnSmall} bg-success-text text-white hover:opacity-90`}
            onClick={onLoad}
            disabled={modelLoading || isDownloadInProgress}
          >
            Load
          </button>
        )}
        {isRunning && (
          <button className={`${btnSmall} bg-error-text text-white hover:bg-error-hover`} onClick={onStop} disabled={modelLoading}>
            Stop
          </button>
        )}
        {isActive && (
          <span className="text-xs text-text-secondary italic">Ready</span>
        )}
        {confirmDelete && (
          <button className={`${btnSmall} bg-transparent text-text-secondary hover:bg-bg-hover`} onClick={onCancelDelete}>
            Cancel
          </button>
        )}
      </div>
    </div>
  );
}
