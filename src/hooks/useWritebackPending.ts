"use client";

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";

// Matches how long a local edit can sit un-flushed before the user would
// act on it manually — cheap enough to poll from every mounted page.
const POLL_MS = 5000;

/**
 * Depth of the thin-client writeback queue: local edits (cards, book
 * chapters/sentences/words, wiki files, read-aloud takes, dictation progress,
 * reviews) awaiting upload to the hub. Pages surface it through
 * `<PendingWritebackBanner>` so an edit made on a phone never looks
 * immediately saved to the PC.
 *
 * Resolves to 0 everywhere but the Android thin client: `isMobileApp()` is
 * false during prerender (no `navigator`, same posture as `useIsMobile`) and
 * only turns true inside the webview, so the poll starts one effect later —
 * desktop is the hub and never queues, staying silent without a platform gate
 * at the call site. Pages that already resolve the flag (ReadBookPage takes
 * `mobile` from `useBookData`) pass it in as `externalMobile` to skip the
 * re-detection; the flag also gates the poll, so hiding the banner never
 * leaves a timer running.
 *
 * Every mounted page polls on its own — cheap and independent, because pages
 * stay mounted and toggled via `display` (a flush elsewhere must still be
 * seen within the next tick).
 */
export function useWritebackPending(externalMobile?: boolean): number {
  const [selfMobile, setSelfMobile] = useState(false);
  useEffect(() => {
    if (externalMobile !== undefined) return;
    setSelfMobile(isMobileApp());
  }, [externalMobile]);
  const mobile = externalMobile ?? selfMobile;

  const [pending, setPending] = useState(0);
  useEffect(() => {
    if (!mobile || !isTauri()) return;
    let alive = true;
    const refresh = () =>
      invoke<number>("writeback_pending_count")
        .then((n) => alive && setPending(n))
        .catch(() => {});
    refresh();
    const timer = setInterval(refresh, POLL_MS);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, [mobile]);
  return pending;
}

export default useWritebackPending;
