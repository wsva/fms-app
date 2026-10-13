"use client";

import type { AsrScoreSummary } from "@/lib/read_aloud/types";

/**
 * Render the ASR alignment chunks computed by the backend (`asr_metric`), so
 * the highlighted diff and the score come from the same alignment. Equal runs
 * are plain text; substituted/inserted words get a red background with the
 * reference words in the tooltip; deleted (skipped) reference words are shown
 * struck through.
 */
export default function AsrDiff({ summary }: { summary: AsrScoreSummary }) {
  return (
    <span className="whitespace-pre-wrap break-words">
      {summary.chunks.map((chunk, i) => {
        const lead = i > 0 ? " " : "";
        switch (chunk.op) {
          case "equal":
            return (
              <span key={i}>
                {lead}
                {chunk.hyp_word}
              </span>
            );
          case "substitute":
            return (
              <span key={i} className="bg-red-400/40 rounded" title={`said instead of: ${chunk.ref_word}`}>
                {lead}
                {chunk.hyp_word}
              </span>
            );
          case "insert":
            return (
              <span key={i} className="bg-red-400/40 rounded" title="redundant">
                {lead}
                {chunk.hyp_word}
              </span>
            );
          case "delete":
            return (
              <span key={i} className="text-red-500 line-through" title="missing">
                {lead}
                {chunk.ref_word}
              </span>
            );
          default:
            return null;
        }
      })}
    </span>
  );
}
