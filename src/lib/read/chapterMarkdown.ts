/**
 * Round-trip Markdown for a book's chapter structure.
 *
 * Each chapter is one ATX heading line carrying its identity as a trailing
 * attribute:
 *
 *     ## Some Chapter {#b279d1b0-5c93-4f4b-8f1e-2a7f9c0d1e23}
 *
 * The embedded uuid is what makes editing safe: a line keeps its identity (and
 * its sentences, order-relative content and completion status) across Preview +
 * Apply. A line without an attribute is a brand-new chapter and gets a fresh
 * uuid. Reordering is cut/paste; re-parenting is adding/removing `#`.
 *
 * Non-heading lines are ignored (free-form notes are allowed). Completion
 * status is *not* encoded here — it is owned by the read-only tree checkbox and
 * preserved per-uuid by the backend sync.
 */

import type { BookChapter } from "./types";
import { flattenChapters } from "./types";
import { getUUID } from "@/components/books/utils";

/** Wire node for the `book_sync_chapters` command (mirrors the Rust SyncNode). */
export interface SyncNode {
  uuid: string;
  parent_uuid: string | null;
  order_num: number;
  title: string;
}

/** A parsed heading line, before parent/order resolution. */
export interface ParsedNode {
  level: number;
  title: string;
  uuid?: string;
}

/** A deletion root and the size of the subtree it takes with it (mirrors Rust). */
export interface DeleteImpact {
  uuid: string;
  title: string;
  chapters: number;
  sentences: number;
}

/** Change counts from a sync (mirrors Rust ChapterSyncSummary). */
export interface ChapterSyncSummary {
  created: number;
  updated: number;
  moved: number;
  reordered: number;
  deleted: number;
  deletions: DeleteImpact[];
}

/** Result of `book_sync_chapters` (mirrors Rust SyncResult). */
export interface SyncResult {
  dry_run: boolean;
  summary: ChapterSyncSummary;
  chapters: BookChapter[];
}

const HEADING_RE = /^(#{1,6})\s+(.*)$/;
/** Trailing `{#token}` attribute — only taken when it *is* a full uuid. */
const ATTR_RE = /\s*\{#([^{}]+)\}\s*$/;
// A chapter id is a full uuid in either of the two shapes the database can hold:
// the canonical dashed form (crypto.randomUUID) or a bare 32-hex form (older /
// sidecar-authored books). Both are valid identities and must round-trip.
const UUID_STRICT_RE = /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i;
const UUID_BARE_RE = /^[0-9a-f]{32}$/i;
/** A `{#...}` token that looks like a (possibly truncated) uuid. */
const UUID_LIKE_RE = /^[0-9a-f-]{32,}$/i;

/** True when the token is a complete uuid in an accepted shape. */
function isFullUuid(token: string): boolean {
  return UUID_STRICT_RE.test(token) || UUID_BARE_RE.test(token);
}

/** Serialize the chapter forest to Markdown, depth-first in display order. */
export function chaptersToMarkdown(chapters: BookChapter[]): string {
  return flattenChapters(chapters)
    .map((c) => `${"#".repeat(c.depth + 1)} ${c.title} {#${c.uuid}}`)
    .join("\n");
}

/**
 * Parse Markdown into heading nodes, validating structure: levels may only
 * deepen one step at a time, titles must be non-empty, uuids must be full and
 * unique. Returns per-line `errors` (empty = safe to build/apply) plus the
 * 1-based `errorLines` that produced them (so the editor can highlight offending
 * lines live, as the user types).
 */
export function parseChaptersMarkdown(md: string): {
  nodes: ParsedNode[];
  errors: string[];
  errorLines: number[];
} {
  const nodes: ParsedNode[] = [];
  const errors: string[] = [];
  const errorLines: number[] = [];
  const lines = md.split(/\r?\n/);
  const seenUuids = new Set<string>();
  let prevLevel = 0;

  // Record an error message and the (1-based) line it belongs to.
  const addErr = (lineNum: number, msg: string) => {
    errors.push(`Line ${lineNum}: ${msg}`);
    errorLines.push(lineNum);
  };

  lines.forEach((raw, i) => {
    const line = raw.trimEnd();
    if (!line.trim() || !line.trimStart().startsWith("#")) return; // blank / prose

    const m = line.match(HEADING_RE);
    if (!m) {
      addErr(i + 1, `expected "# Title {#uuid}"`);
      return;
    }
    let level = m[1].length;
    let text = m[2].trim();
    let uuid: string | undefined;

    const am = text.match(ATTR_RE);
    if (am) {
      const token = am[1].trim();
      if (isFullUuid(token)) {
        uuid = token.toLowerCase();
        text = text.slice(0, am.index).trimEnd();
      } else if (UUID_LIKE_RE.test(token)) {
        addErr(i + 1, `malformed chapter id "{#${token}}" — restore the full {#uuid} or remove it`);
        text = text.slice(0, am.index).trimEnd();
      }
      // otherwise: a `{#note}` that is not uuid-like stays part of the title
    }

    if (!text) {
      addErr(i + 1, `empty chapter title`);
      return;
    }
    if (level > prevLevel + 1) {
      addErr(i + 1, `"${text}" skips a level (${prevLevel || "root"} → ${level}) — nest directly under the previous chapter`);
      // Clamp so later lines still get a sane parent instead of failing cascade-wide.
      level = prevLevel + 1;
    }
    if (uuid) {
      if (seenUuids.has(uuid)) addErr(i + 1, `duplicate chapter id {#${uuid}}`);
      seenUuids.add(uuid);
    }

    prevLevel = level;
    nodes.push({ level, title: text, uuid });
  });

  return { nodes, errors, errorLines };
}

/**
 * Turn parsed nodes into the flat, parent-resolved forest the backend sync
 * consumes. Nodes without a uuid are minted a fresh one (they show up as
 * "created" in Preview); parents are the nearest preceding node of each level
 * (tracked by a stack), and `order_num` is 1..n among siblings in document
 * order. Returns `errors` (empty = sendable).
 */
export function buildSyncNodes(nodes: ParsedNode[]): { syncNodes: SyncNode[]; errors: string[] } {
  const errors: string[] = [];
  const finalUuids: (string | null)[] = nodes.map((n) => n.uuid ?? null);
  nodes.forEach((n, i) => {
    if (!n.uuid) finalUuids[i] = getUUID();
  });

  // Stack of the nearest preceding node at each level (index into `nodes`).
  const levelStack: number[] = [];
  // Per-node resolved parent uuid and 1..n sibling order (parallel arrays).
  const parentUuids: (string | null)[] = [];
  const orderNums: number[] = [];
  const seenIds = new Set<string>();
  const orderCounter = new Map<string, number>();

  for (let i = 0; i < nodes.length; i++) {
    const n = nodes[i];
    const id = finalUuids[i]!;
    if (seenIds.has(id)) {
      errors.push(`Duplicate chapter id {#${id}}`);
      continue;
    }
    seenIds.add(id);

    levelStack.length = n.level - 1;
    const parentIdx = levelStack[levelStack.length - 1];
    const parent = parentIdx === undefined ? null : finalUuids[parentIdx];
    if (n.level > 1 && parentIdx === undefined) {
      errors.push(`"${n.title}" has no parent chapter above it`);
      continue;
    }
    parentUuids.push(parent);

    const key = parent ?? "";
    const next = (orderCounter.get(key) ?? 0) + 1;
    orderCounter.set(key, next);
    orderNums.push(next);

    levelStack.push(i);
  }

  const syncNodes: SyncNode[] = [];
  for (let i = 0; i < nodes.length && errors.length === 0; i++) {
    syncNodes.push({
      uuid: finalUuids[i]!,
      parent_uuid: parentUuids[i] ?? null,
      order_num: orderNums[i],
      title: nodes[i].title,
    });
  }

  return { syncNodes: errors.length ? [] : syncNodes, errors };
}
