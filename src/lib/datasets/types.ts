// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

import type { DatasetInfo } from "@/lib/types";

// One descriptor for every dataset type — defined once in `@/lib/types` (which
// mirrors the Rust `datasets::info::DatasetInfo`) and re-exported here so the
// dictation UI keeps importing it from the dataset module.
export type { DatasetInfo };

export type DatasetStatus = "ready" | "not_ready";

export interface DatasetSummary {
  info: DatasetInfo;
  media_count: number;
  path: string;
  /** Root location directory this dataset was found under. */
  location: string;
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

/** One file a check flagged, with the reason shown next to it. */
export interface AuditFinding {
  /** Media-relative source, e.g. `a/b.mp3` — the name every layer agrees on. */
  source: string;
  detail: string;
}

/**
 * One read-only consistency check from `dataset_audit`. `count` is the exact
 * number of offenders; `items` is capped at 100, hence `truncated`.
 */
export interface AuditCheck {
  id: string;
  label: string;
  /** `problem` blocks the pipeline, `warn` is drift, `info` is context. */
  level: "problem" | "warn" | "info" | "clean";
  count: number;
  items: AuditFinding[];
  truncated: boolean;
  advice: string;
  /** Template step id that repairs it, when one does. */
  fix_step?: string | null;
  fix_label?: string | null;
}

/** The six booleans the template's `when:` guards evaluate. */
export interface AuditFacts {
  has_media: boolean;
  has_subtitles: boolean;
  has_waveforms: boolean;
  has_database: boolean;
  has_book: boolean;
  has_transcript: boolean;
}

export interface DatasetAudit {
  dataset_uuid: string;
  path: string;
  info: DatasetInfo;
  facts: AuditFacts;
  media_on_disk: number;
  media_in_db: number;
  subtitles_in_db: number;
  cues_in_db: number;
  checks: AuditCheck[];
}

export interface DatasetProgressEvt {
  uuid: string;
  current_file: string;
  file_index: number;
  total_files: number;
  stage: string;
}

/** Cue-time adjustment mode: write a new subtitle copy, or overwrite in place. */
export type AdjustMode = "new" | "in_place";

/**
 * Book-splitting engine for the "Split Book" stage:
 * - `rust`: built-in splitter, no setup, simple heuristic (lower quality).
 * - `python`: bundled NLTK script, higher quality, requires Python + nltk installed.
 */
export type SplitBookMode = "rust" | "python";

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
