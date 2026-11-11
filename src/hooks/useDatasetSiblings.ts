"use client";

// ---------------------------------------------------------------------------
// The *other* datasets of each listed dataset's family, read in one batch.
//
// A page that lists datasets wants them all — a card cannot ask for its own
// without one `dataset_family` per card, and each of those re-walks every root
// for the one id it was given. `dataset_family_siblings` reads the disk once and
// answers every id asked about, including the ones with no family: an empty
// list, never a missing key, so a card needs no special case.
//
// Like `useDatasetFamily`, this is a component-scoped read and subscribes to the
// backend event itself rather than going through the page-refresh registry. A
// family is a relation rebuilt from the ids, so nothing here is a cache that can
// go stale — every answer replaces the last.
// ---------------------------------------------------------------------------

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "@/lib/tauri";
import { logError } from "@/lib/logger";
import type { FamilyMember } from "@/lib/datasets/family";

/**
 * The siblings of every id in `uuids`, keyed by id.
 *
 * @param uuids The datasets on screen. Any order; the answer is keyed, so a
 *   reorder costs nothing. Ids are joined into the effect's dependency because a
 *   page usually rebuilds this array on every render.
 */
export function useDatasetSiblings(uuids: readonly string[]): Record<string, FamilyMember[]> {
  const [siblings, setSiblings] = useState<Record<string, FamilyMember[]>>({});

  // No dataset id contains `|`, so this is a faithful, cheap identity for the
  // list — and the effect re-reads on a change of *set*, not of array object.
  const ids = uuids.join("|");

  const load = useCallback(async () => {
    const list = ids ? ids.split("|") : [];
    if (list.length === 0 || !isTauri()) {
      setSiblings({});
      return;
    }
    try {
      const map = await invoke<Record<string, FamilyMember[]>>("dataset_family_siblings", {
        uuids: list,
      });
      setSiblings(map);
    } catch (err) {
      // A family is a link between datasets, never the page's subject: losing it
      // costs the cards one strip, nothing the user came there for.
      logError(`dataset_family_siblings failed: ${String(err)}`, "family");
    }
  }, [ids]);

  useEffect(() => {
    void load();
  }, [load]);

  // A companion created, joined or deleted from another page — or by an agent
  // over MCP — changes a family without this list changing at all.
  useEffect(() => {
    if (!ids || !isTauri()) return;
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    listen("dataset-list-changed", () => void load())
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {
        /* the bridge refused the subscription; the next list read still lands */
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [ids, load]);

  return siblings;
}

export default useDatasetSiblings;
