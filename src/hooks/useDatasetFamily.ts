"use client";

// ---------------------------------------------------------------------------
// One read of `dataset_family`, kept current while the component is mounted.
//
// Deliberately *not* built on `lib/refresh/page-refresh.tsx`: that registry is
// keyed one-per-tab, and the pages that show a family already register
// themselves there for their own data. This is a component-scoped read, so it
// subscribes to the same backend event directly.
// ---------------------------------------------------------------------------

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "@/lib/tauri";
import { logError } from "@/lib/logger";
import type { FamilyReport } from "@/lib/datasets/family";

export interface UseDatasetFamily {
  /** `null` until the first answer, and for a dataset with no family — check
   *  `report.family` to tell those apart, since an unaffiliated dataset still
   *  comes back as a family of one. */
  report: FamilyReport | null;
  /** True only while nothing is on screen yet, so a re-read cannot blank the
   *  crumb or strip the user is looking at. */
  loading: boolean;
  /** Re-read now — after creating a companion, for instance. */
  refresh: () => void;
}

/**
 * The family `uuidOrKey` belongs to.
 *
 * @param uuidOrKey A dataset uuid, or a bare family key. Falsy means "nothing
 *   selected", which resolves to `null` rather than an error.
 * @param reloadToken Bump to force a re-read from outside (a page's own refresh
 *   closure, a dialog closing).
 */
export function useDatasetFamily(
  uuidOrKey: string | null | undefined,
  reloadToken: number = 0
): UseDatasetFamily {
  const [report, setReport] = useState<FamilyReport | null>(null);
  const [loading, setLoading] = useState(false);
  /** Which key the current `report` answers, so a late reply from a previous
   *  selection cannot overwrite the new one. */
  const loadedKey = useRef<string | null>(null);

  const key = (uuidOrKey ?? "").trim();

  const load = useCallback(async () => {
    if (!key || !isTauri()) {
      loadedKey.current = null;
      setReport(null);
      setLoading(false);
      return;
    }
    if (loadedKey.current !== key) {
      // A *different* dataset's family must not be shown while the new one
      // loads. A re-read of the same one keeps its rows on screen.
      loadedKey.current = key;
      setReport(null);
      setLoading(true);
    }
    try {
      const next = await invoke<FamilyReport>("dataset_family", { uuidOrKey: key });
      if (loadedKey.current === key) setReport(next);
    } catch (err) {
      // A family is a link between things, never a page's subject: losing it
      // must not cost the user what they came to see, so this is logged, not
      // surfaced.
      logError(`dataset_family('${key}') failed: ${String(err)}`, "family");
      if (loadedKey.current === key) setReport(null);
    } finally {
      if (loadedKey.current === key) setLoading(false);
    }
  }, [key]);

  useEffect(() => {
    void load();
  }, [load, reloadToken]);

  // A companion created from another page — or by an agent over MCP — changes
  // this family without the component doing anything at all.
  useEffect(() => {
    if (!key || !isTauri()) return;
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    listen("dataset-list-changed", () => void load())
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {
        /* the bridge refused the subscription; a re-read on selection still works */
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [key, load]);

  const refresh = useCallback(() => void load(), [load]);
  return { report, loading, refresh };
}

export default useDatasetFamily;
