import type { JSX } from "react";
import { lcs } from "@/lib/listen/lcs";

function tokenize(s: string): string[] {
  s = s.replace(/\s+/g, " ").trim();
  if (!s) return [];
  const tokens = s
    .split(/(\s+|[.,!?;:\-—()"'“”„«»…])/)
    .filter((t) => t !== undefined && t !== null && t !== "");
  return tokens.filter((t) => t.trim().length > 0 || t === " ");
}

/**
 * Render the recognized STT text, highlighting differences against the
 * original content (red background for mismatches / redundancies / gaps).
 */
export function highlightDifferences(
  original: string,
  recognized: string
): JSX.Element[] {
  const originalTokens = tokenize(original);
  const recognizedTokens = tokenize(recognized);

  const matchesR2O = lcs(originalTokens, recognizedTokens);
  const matchMapR2O = new Map<number, number>(matchesR2O.map((v) => [v[0], v[1]]));

  const result: JSX.Element[] = [];
  let lastPosO = 0;
  let diffPartR = "";
  let diffPartO = "";

  const flushDiff = (key: string) => {
    if (diffPartR) {
      result.push(
        <span
          key={key}
          title={diffPartO || "redundant"}
          className="whitespace-pre-wrap bg-red-400/40 rounded"
        >
          {diffPartR}
        </span>
      );
    } else if (diffPartO) {
      result.push(
        <span
          key={key}
          title={`missing: ${diffPartO}`}
          className="whitespace-pre-wrap bg-red-400/40 rounded"
        >
          {"      "}
        </span>
      );
    }
    diffPartR = "";
    diffPartO = "";
  };

  for (let rIdx = 0; rIdx < recognizedTokens.length; rIdx++) {
    const oIdx = matchMapR2O.get(rIdx);

    if (oIdx === undefined) {
      diffPartR += recognizedTokens[rIdx];
    } else {
      if (oIdx > lastPosO) {
        for (let i = lastPosO; i < oIdx; i++) {
          diffPartO += originalTokens[i];
        }
      }

      if (/^\s+$/.test(recognizedTokens[rIdx]) && (diffPartR || diffPartO)) {
        diffPartR += recognizedTokens[rIdx];
      } else {
        flushDiff(`diff-${rIdx}`);
        result.push(<span key={`match-${rIdx}`}>{recognizedTokens[rIdx]}</span>);
      }

      lastPosO = oIdx + 1;
    }
  }

  for (let i = lastPosO; i < originalTokens.length; i++) {
    diffPartO += originalTokens[i];
  }
  flushDiff("diff-end");

  return result;
}
