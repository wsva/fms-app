// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type ModelStatus =
  | "NotDownloaded"
  | "Downloading"
  | "Downloaded"
  | "Running"
  | "Stopped"
  | { Error: string };

export interface FileDownloadInfo {
  file: string;
  bytes_downloaded: number;
  total_bytes: number | null;
  speed: number;
  eta_seconds: number | null;
}

export interface DownloadProgress {
  files: FileDownloadInfo[];
  overall_bytes_downloaded: number;
  overall_total_bytes: number;
  speed: number;
  eta_seconds: number | null;
}

export interface ModelVersionInfo {
  id: string;
  label: string;
  description: string;
  downloaded: boolean;
  languages: string[];
  size_mb: number;
  engine_type: string;
  blob_url: string;
  hf_repo_url: string;
  accuracy_score: number;
  speed_score: number;
  supports_translation: boolean;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

export function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  return `${(bytes / Math.pow(1024, i)).toFixed(1)} ${units[i]}`;
}

export function formatSpeed(bytesPerSec: number): string {
  return `${formatBytes(bytesPerSec)}/s`;
}

export function formatEta(seconds: number | null): string {
  if (seconds === null || seconds === 0) return "--";
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return `${m}m ${s}s`;
}

export function statusLabel(status: ModelStatus): string {
  if (typeof status === "string") {
    const labels: Record<string, string> = {
      NotDownloaded: "Not downloaded",
      Downloading: "Downloading...",
      Downloaded: "Downloaded",
      Running: "Running",
      Stopped: "Stopped",
    };
    return labels[status] ?? status;
  }
  return `Error: ${status.Error}`;
}

export function statusBadgeClass(status: ModelStatus): string {
  const base = "px-2 py-[0.2em] rounded-full text-xs font-semibold";
  if (status === "Running") return `${base} bg-success-bg text-success-text`;
  if (status === "Downloading") return `${base} bg-info-bg text-info-text`;
  if (status === "Downloaded") return `${base} bg-bg-muted text-text-secondary`;
  if (typeof status === "object" && "Error" in status) return `${base} bg-error-bg text-error-text`;
  return `${base} bg-bg-muted text-text-secondary`;
}

// ---------------------------------------------------------------------------
// Shared style fragments
// ---------------------------------------------------------------------------

export const btnBase = "px-4 py-2 rounded-md font-medium cursor-pointer transition-colors disabled:opacity-50 disabled:cursor-not-allowed";
export const btnSmall = "px-2.5 py-1 text-xs rounded font-medium cursor-pointer transition-colors disabled:opacity-50 disabled:cursor-not-allowed";
