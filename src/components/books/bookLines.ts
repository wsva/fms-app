/**
 * The paragraph-text grammar shared by the Import textarea and the panel's
 * Edit tab: one line per sentence, an empty line is a paragraph break.
 *
 * The rules are the ones `handleImport` always applied — leading empty lines
 * skipped, consecutive empty lines collapse into one break, trailing empties
 * dropped — pulled out so reflow (match-by-text) and import (all-new rows)
 * cannot drift apart.
 */

import { getUUID } from "./utils";

export type ParsedLine = { text: string; isBreak: boolean };

export function parseLineRows(lines: string[]): ParsedLine[] {
  const out: ParsedLine[] = [];
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i].trim();
    if (line !== "") {
      out.push({ text: line, isBreak: false });
      continue;
    }
    // Skip leading empty lines
    if (out.length === 0) continue;
    // Skip trailing empty lines
    const remaining = lines.slice(i + 1).some((l) => l.trim() !== "");
    if (!remaining) break;
    // Collapse consecutive empty lines into a single paragraph break
    if (out[out.length - 1].isBreak) continue;
    out.push({ text: "", isBreak: true });
  }
  return out;
}

/**
 * Re-segment a paragraph's new text against its existing rows: a line equal
 * (trimmed) to an unconsumed existing sentence keeps that row's uuid, so its
 * audio, color and sentence-scoped artifacts survive an edit that did not
 * touch it. Anything left unmatched is reported as removed (a uuid that was
 * not reused — a moved break row is not a deletion).
 */
export function matchRows(
  desired: ParsedLine[],
  existing: { uuid: string; content: string; isBreak: boolean }[]
): { rows: { text: string; isBreak: boolean; uuid: string; reused: boolean }[]; removed: string[] } {
  const pool = existing.map((e) => ({ ...e, used: false }));
  const take = (text: string, isBreak: boolean) =>
    pool.find((e) => !e.used && e.isBreak === isBreak && e.content.trim() === text);
  const rows = desired.map((d) => {
    if (d.isBreak) return { text: "", isBreak: true, uuid: getUUID(), reused: false };
    const hit = take(d.text, false);
    if (hit) {
      hit.used = true;
      return { text: d.text, isBreak: false, uuid: hit.uuid, reused: true };
    }
    return { text: d.text, isBreak: false, uuid: getUUID(), reused: false };
  });
  const consumed = new Set(rows.map((r) => r.uuid));
  const removed = pool.filter((e) => !e.used && !consumed.has(e.uuid)).map((e) => e.uuid);
  return { rows, removed };
}
