"use client";

import useWritebackPending from "@/hooks/useWritebackPending";

/**
 * Thin-client "changes pending upload" strip — the shared mobile reminder that
 * a local edit (card, book, wiki file, read-aloud take, dictation progress) only
 * lives on the phone until a sync round flushes it to the hub. Renders nothing
 * on desktop or while the queue is empty; sits above a page's breadcrumb bar as
 * a `shrink-0` sibling so it never scrolls away.
 *
 * `mobile` may be passed in by pages that already resolve the platform flag
 * (ReadBookPage takes it from `useBookData`); omitted, it self-resolves via
 * `useWritebackPending`, which stays SSR-safe by answering 0 until the
 * webview-side effect settles the platform.
 */
export function PendingWritebackBanner({ mobile }: { mobile?: boolean }) {
  const pending = useWritebackPending(mobile);
  if (pending <= 0) return null;
  return (
    <div className="shrink-0 px-4 py-1.5 text-xs font-medium bg-amber-100 text-amber-800 dark:bg-amber-900/30 dark:text-amber-300 border-b border-border-default">
      {pending} change(s) pending upload
    </div>
  );
}

export default PendingWritebackBanner;
