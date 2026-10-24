"use client";

import type { AsrChunk, AsrScoreSummary } from "@/lib/read_aloud/types";
import { WEAK_WORD_SCORE } from "@/lib/read_aloud/types";

/**
 * Render the ASR alignment chunks computed by the backend (`asr_metric`), so
 * the highlighted diff and the score come from the same alignment. Equal runs
 * are plain text; substituted/inserted words get a red background with the
 * reference words in the tooltip; deleted (skipped) reference words are shown
 * struck through.
 *
 * `pron` optionally adds pronunciation emphasis: reference-word index → GOP
 * score in (0, 1]. An equal run is contiguous in both sequences, so the index of
 * its n-th transcript word is `ref_index + n` and no diff logic changes.
 */
export default function AsrDiff({
  summary,
  pron,
}: {
  summary: AsrScoreSummary;
  pron?: Record<number, number>;
}) {
  return (
    <span className="whitespace-pre-wrap break-words">
      {summary.chunks.map((chunk, i) => {
        const lead = i > 0 ? " " : "";
        switch (chunk.op) {
          case "equal":
            return (
              <span key={i}>
                {lead}
                {markWeakWords(chunk, pron)}
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

/**
 * A correctly-read word whose pronunciation was weak gets an amber underline.
 * Everything else in an equal run stays plain: the score already says the word
 * was heard right, so only the "said it, but not clearly" case is marked.
 */
function markWeakWords(chunk: AsrChunk, pron?: Record<number, number>) {
  if (!pron || chunk.ref_index == null) return chunk.hyp_word;
  const words = chunk.hyp_word.split(" ");
  return words.map((word, offset) => {
    const score = pron[chunk.ref_index! + offset];
    if (score == null || score >= WEAK_WORD_SCORE) return <span key={offset}>{offset > 0 ? " " : ""}{word}</span>;
    return (
      <span
        key={offset}
        className="underline decoration-amber-500 decoration-2 underline-offset-2"
        title={`Pronounced weakly (${Math.round(score * 100)}%)`}
      >
        {offset > 0 ? " " : ""}
        {word}
      </span>
    );
  });
}
