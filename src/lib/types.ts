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
