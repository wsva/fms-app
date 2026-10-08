// Shared wiki types mirroring the Rust serde shapes (snake_case on the wire).
// Legacy wiki entries use absolute paths; wiki *dataset* entries use
// dataset-relative paths (`rel_path`) so the same value works on follower
// copies and in the hub's REST browse responses.

export interface WikiEntry {
  name: string;
  path: string;
  is_dir: boolean;
  is_linked: boolean;
  modified: string | null;
}

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

// GET /api/v1/wiki/datasets (wiki_hub_list)
export interface HubWikiList {
  datasets: WikiDatasetSummary[];
  legacy_roots: WikiEntry[];
}

// Where a selected file lives. The wiki page serves three kinds of roots:
//   * legacy wiki directories (absolute paths, unchanged behavior);
//   * locally stored wiki datasets — downloaded or hub-owned (editable);
//   * hub datasets in read-only browse mode (needs download before editing).
export type WikiSource =
  | { kind: "legacy"; path: string }
  | { kind: "dataset"; uuid: string; rel: string }
  | { kind: "hub-dataset"; uuid: string; rel: string; name: string };

export type Selection = { source: WikiSource; label: string };

export function selectionKey(source: WikiSource): string {
  switch (source.kind) {
    case "legacy":
      return `legacy:${source.path}`;
    case "dataset":
      return `dataset:${source.uuid}:${source.rel}`;
    case "hub-dataset":
      return `hub:${source.uuid}:${source.rel}`;
  }
}

export function selectionFileName(source: WikiSource): string {
  const p = source.kind === "legacy" ? source.path : source.rel;
  return p.split(/[\\/]/).pop() || p;
}

export function makeLegacySelection(path: string): Selection {
  return { source: { kind: "legacy", path }, label: path.split(/[\\/]/).pop() || path };
}

export function makeDatasetSelection(ds: WikiDatasetSummary, rel: string): Selection {
  return { source: { kind: "dataset", uuid: ds.uuid, rel }, label: rel.split("/").pop() || ds.name };
}

export function makeHubSelection(ds: WikiDatasetSummary, rel: string): Selection {
  return { source: { kind: "hub-dataset", uuid: ds.uuid, rel, name: ds.name }, label: rel.split("/").pop() || ds.name };
}
