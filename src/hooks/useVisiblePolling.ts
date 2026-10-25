"use client";

import { useEffect, useRef } from "react";

/**
 * Call `fn` on an interval, but only while this page is the one on screen and
 * the app is not backgrounded.
 *
 * The generalisation of the poll `components/chat/ChatPage.tsx` hand-rolled for
 * the phone: for a handful of screens the data changes with no backend event
 * (`DatasetsSyncPage`'s market listing and last round, `DevicesHubPage`'s
 * device/peer state), so a slow poll is the only way to keep them honest.
 * Everything else refreshes on activation instead — see
 * `lib/refresh/page-refresh.tsx` for why that is the default.
 *
 * No immediate call: every page that polls also registers a page refresh, which
 * already re-reads on becoming active. This hook only keeps it current while
 * the user stays put.
 */
export function useVisiblePolling(
  active: boolean,
  fn: () => void,
  intervalMs: number
): void {
  const latest = useRef(fn);
  latest.current = fn;

  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => {
      // A hidden document is a backgrounded app: timers are throttled by the
      // engine anyway, and the resume trigger re-reads on the way back.
      if (document.visibilityState === "visible") latest.current();
    }, intervalMs);
    return () => window.clearInterval(timer);
  }, [active, intervalMs]);
}

export default useVisiblePolling;
