export type Cue = {
  uuid: string;
  subtitle_uuid: string;
  order_num: number;
  start_ms: number;
  end_ms: number;
  content: string;
  reference: string | null;
  // UI-only fields
  active?: boolean;
  content_original?: string;
  modified?: boolean;
  deleted?: boolean;
};

export type ListenMedia = {
  uuid: string;
  title: string;
  source: string;
  note: string;
  created_at: string;
  updated_at: string;
};

export type ListenSubtitle = {
  uuid: string;
  media_uuid: string;
  name: string;
  note: string;
  created_at: string;
  updated_at: string;
};

export type ListenDictation = {
  media_uuid: string;
  subtitle_uuid: string;
  status: string;
  completed: string;
};

// ── Card types ──────────────────────────────────────────────────────────────

export type CardDatasetInfo = {
  uuid: string;
  name: string;
  description: string;
  parent_uuid: string;
  version: number;
  structure: 'cards-v1';
  updated: string;
  sync_url: string;
  visibility: 'private' | 'shared' | 'public';
  owner_id: string;
  subscribers: string[];
};

export type CardDatasetSummary = {
  info: CardDatasetInfo;
  card_count: number;
  path: string;
  location: string;
};

export type Card = {
  uuid: string;
  question: string;
  suggestion: string;
  answer: string;
  note: string;
  familiarity: number;
  question_hash: string | null;
  source_card_uuid: string | null;
  source_dataset_uuid: string | null;
  deleted_at: string | null;
  created_at: string;
  updated_at: string;
};

export type CardReview = {
  uuid: string;
  card_uuid: string;
  familiarity: number;
  interval_days: number;
  ease_factor: number;
  repetitions: number;
  last_review_at: string | null;
  next_review_at: string | null;
};

/** Review queue counts for one dataset, mirroring how `card_test_get` picks cards. */
export type CardTestStats = {
  /** Overdue cards — served before anything else. */
  due: number;
  /** Never-reviewed cards, waiting behind `due`. */
  fresh: number;
  /** Familiarity 6: out of the review rotation. */
  mature: number;
  /** Missing question or answer, so review skips them. */
  incomplete: number;
  total: number;
  /** Which pool the next card comes from. */
  serving: "due" | "fresh" | "none";
};

export type CardTag = {
  uuid: string;
  name: string;
  color: string | null;
  deleted_at: string | null;
  created_at: string;
  updated_at: string;
};

export type CardSyncStatus = {
  dataset_uuid: string;
  last_synced_at: string;
  clock_offset_ms: number;
  pending_push: number;
};
