"use client";

/**
 * About FmS, as the Help menu opens it: the card on a scrim.
 *
 * The card is the same component Settings → About renders, so the two doors lead
 * to one answer rather than to two renderings that can drift. What the dialog
 * adds is only the frame: a scrim that closes on click, Escape to close, and the
 * shadow and centring that say "this is above the page".
 *
 * It renders inside the title bar's root, which is a `deep` drag region and is
 * `select-none`. Both have to be undone here: without the drag opt-out below, a
 * click on the bare backdrop would start a window drag instead of closing (the
 * exact bug `StatusItem` documents for its own scrim), and without `select-text`
 * on the card the version — the one string on it worth selecting — could not be.
 */

import { useEffect } from "react";
import AboutCard from "./AboutCard";

export default function AboutDialog({
  onClose,
  onOpenLogs,
}: {
  onClose: () => void;
  /** Passed through to the card's Logs link; the dialog closes first, since the
   *  log page is about to replace everything underneath. */
  onOpenLogs?: () => void;
}) {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  return (
    <div
      // Above the title bar's own z-[120], which in turn out-stacks the workspace
      // chooser (z-50) so an undecorated window is never impossible to drag.
      // The drag opt-out is the same one `StatusItem` puts on its wrapper: an
      // opt-out on an ancestor of the clicked node is what keeps Tauri's script
      // from reading the click as "grabbed the window".
      data-tauri-drag-region="false"
      className="fixed inset-0 z-[130] flex items-center justify-center bg-black/40 p-4"
      onClick={onClose}
      role="presentation"
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="About FmS"
        onClick={(e) => e.stopPropagation()}
        className="w-full max-w-xl"
      >
        <AboutCard active className="select-text shadow-2xl" onOpenLogs={onOpenLogs} />
      </div>
    </div>
  );
}
