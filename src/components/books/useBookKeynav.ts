"use client";

/**
 * useBookKeynav — keyboard walking for the reading pane (make selecting a
 * sentence/paragraph convenient without pointing at running text). Mounted
 * only on a chapter page while the reading pane is on screen.
 *
 *   ↓ / j   next sentence        ↑ / k   previous sentence
 *   n       next paragraph       p       previous paragraph
 *
 * Plain keys only (no Ctrl/Meta/Alt) and never while an input, textarea or
 * the inline contentEditable has focus, so typing inside a sentence is never
 * stolen into navigation. Movement is driven off the focused sentence/
 * paragraph state, not the DOM.
 */

import { useEffect, useRef } from "react";
import type { Paragraph } from "@/lib/read/types";

type Handlers = {
  /** Jump to a sentence by index in the flattened (pane-order) list. */
  goToSentence: (index: number) => void;
  /** Jump to a paragraph by index. */
  goToParagraph: (index: number) => void;
};

export function useBookKeynav(
  enabled: boolean,
  paragraphs: Paragraph[],
  focusSentence: string | null,
  focusParagraph: string | null,
  handlers: Handlers
) {
  // Handlers arrive as an inline object each render; read them off a ref so
  // the listener is (re)bound only when the pane's data actually changes.
  const handlersRef = useRef(handlers);
  handlersRef.current = handlers;
  useEffect(() => {
    if (!enabled) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      const t = e.target as HTMLElement | null;
      if (
        t &&
        (t.tagName === "INPUT" ||
          t.tagName === "TEXTAREA" ||
          t.tagName === "SELECT" ||
          t.isContentEditable)
      ) {
        return;
      }
      // One flat sentence list in pane order drives the vertical walk.
      const flat = paragraphs.flatMap((p) => p.sentences);
      let si = focusSentence ? flat.findIndex((s) => s.uuid === focusSentence) : -1;
      if (si < 0 && focusParagraph) {
        const pi = paragraphs.findIndex((p) => p.breakSentence?.uuid === focusParagraph);
        if (pi >= 0) {
          const start = paragraphs
            .slice(0, pi)
            .reduce((n, p) => n + p.sentences.length, 0);
          si = start - 1; // so "next" lands on the paragraph's first sentence
        }
      }
      switch (e.key) {
        case "ArrowDown":
        case "j": {
          if (flat.length === 0) return;
          e.preventDefault();
          handlersRef.current.goToSentence(Math.min(si + 1, flat.length - 1));
          break;
        }
        case "ArrowUp":
        case "k": {
          if (flat.length === 0) return;
          e.preventDefault();
          handlersRef.current.goToSentence(Math.max(si - 1, 0));
          break;
        }
        case "n":
        case "p": {
          const pi = focusParagraph
            ? paragraphs.findIndex((x) => x.breakSentence?.uuid === focusParagraph)
            : paragraphs.length - 1;
          if (pi < 0) return;
          e.preventDefault();
          const next =
            e.key === "n" ? Math.min(pi + 1, paragraphs.length - 1) : Math.max(pi - 1, 0);
          handlersRef.current.goToParagraph(next);
          break;
        }
        default:
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [enabled, paragraphs, focusSentence, focusParagraph]);
}
