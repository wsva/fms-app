// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type DatasetStatus = "ready" | "not_ready";

export interface DatasetInfo {
  name: string;
  uuid: string;
  description: string;
  parent_uuid: string;
  version: number;
  structure: string;
  updated: string;
}

export interface DatasetSummary {
  info: DatasetInfo;
  media_count: number;
  path: string;
  status: DatasetStatus;
}

export interface MediaFile {
  name: string;
  path: string;
  size: number;
  has_transcript: boolean;
}

export interface DatasetDetail {
  info: DatasetInfo;
  media: MediaFile[];
  has_subtitles: boolean;
  has_waveforms: boolean;
  has_database: boolean;
  has_book: boolean;
  status: DatasetStatus;
}

export interface DatasetProgressEvt {
  uuid: string;
  current_file: string;
  file_index: number;
  total_files: number;
  stage: string;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

export function datasetStatusLabel(status: DatasetStatus): string {
  const labels: Record<DatasetStatus, string> = {
    ready: "Ready",
    not_ready: "Not ready",
  };
  return labels[status] ?? status;
}

export function datasetStatusBadgeClasses(status: DatasetStatus): string {
  const base = "text-xs px-2 py-[0.2em] rounded-full font-medium";
  switch (status) {
    case "ready":
      return `${base} bg-success-bg text-success-text`;
    case "not_ready":
      return `${base} bg-bg-muted text-text-secondary`;
    default:
      return base;
  }
}

// ---------------------------------------------------------------------------
// Style fragments
// ---------------------------------------------------------------------------

export const btnSm = "px-2.5 py-1 text-xs rounded-md font-medium cursor-pointer transition-colors disabled:opacity-50 disabled:cursor-not-allowed";
export const btnSmPrimary = `${btnSm} bg-accent-bg text-white hover:bg-accent-bg-hover`;
export const btnSmDanger = `${btnSm} bg-error-text text-white hover:bg-error-hover`;
export const btnSmSecondary = `${btnSm} bg-transparent border border-border-light text-text-primary hover:bg-bg-hover`;
export const actionLabel = "text-sm text-text-tertiary shrink-0 whitespace-nowrap";
