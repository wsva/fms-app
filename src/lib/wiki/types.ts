// Shared wiki types mirroring the Rust serde shapes (snake_case on the wire).
// Wiki entries use dataset-relative paths (`rel_path`) so the same value works
// on follower copies and in the hub's REST browse responses.

// Mirrors datasets::wiki::WikiDatasetSummary
export interface WikiDatasetSummary {
  uuid: string;
  name: string;
  updated: string;
  file_count: number;
  path: string;
  location: string;
}

// Mirrors datasets::wiki::WikiFileEntry
export interface WikiFileEntry {
  name: string;
  rel_path: string;
  is_dir: boolean;
  modified: string | null;
}

// Mirrors datasets::wiki::WikiFileContent — one wiki file read, classified for
// preview: extension decides the renderer, a UTF-8 sniff vetoes it for unknown
// files. `content` is decoded text only for markdown/text (max 2 MB, see
// `truncated`); `path` is an absolute path on the local machine and always
// null for hub-served files (the hub never leaks its own paths).
export type WikiFileKind = "markdown" | "text" | "image" | "media" | "binary";

export interface WikiFileContent {
  kind: WikiFileKind;
  content: string | null;
  size: number;
  truncated: boolean;
  path: string | null;
}

// GET /api/v1/wiki/datasets (wiki_hub_list)
export interface HubWikiList {
  datasets: WikiDatasetSummary[];
}

// Where a selected file lives. The wiki page serves two kinds of roots:
//   * locally stored wiki datasets — downloaded or hub-owned (editable);
//   * hub datasets in read-only browse mode (needs download before editing).
export type WikiSource =
  | { kind: "dataset"; uuid: string; rel: string }
  | { kind: "hub-dataset"; uuid: string; rel: string; name: string };

export type Selection = { source: WikiSource; label: string };

export function selectionKey(source: WikiSource): string {
  switch (source.kind) {
    case "dataset":
      return `dataset:${source.uuid}:${source.rel}`;
    case "hub-dataset":
      return `hub:${source.uuid}:${source.rel}`;
  }
}

export function selectionFileName(source: WikiSource): string {
  return source.rel.split("/").pop() || source.rel;
}

export function makeDatasetSelection(ds: WikiDatasetSummary, rel: string): Selection {
  return { source: { kind: "dataset", uuid: ds.uuid, rel }, label: rel.split("/").pop() || ds.name };
}

export function makeHubSelection(ds: WikiDatasetSummary, rel: string): Selection {
  return { source: { kind: "hub-dataset", uuid: ds.uuid, rel, name: ds.name }, label: rel.split("/").pop() || ds.name };
}
