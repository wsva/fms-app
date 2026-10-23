"use client";

import { useMemo, useState } from "react";
import { ChevronDown, ChevronUp, Download, Loader2 } from "lucide-react";
import { tbBase } from "@/lib/ui/toolbar";
import type { OcrDictionary, OcrHyphen } from "./useOcr";

/** Human wording for the resolver's evidence tags. */
const REASON_LABELS: Record<string, string> = {
  dictionary: "in dictionary",
  "unknown-word": "not in dictionary",
  lowercase: "lowercase",
  uppercase: "capitalised",
  digit: "after a digit",
  policy: "forced",
  pending: "undecided",
};

const REASON_TITLES: Record<string, string> = {
  dictionary: "The joined form is a word in one of the selected languages' dictionaries.",
  "unknown-word": "None of the selected dictionaries knows the joined form, so the hyphen is kept.",
  lowercase: "No dictionary installed for the selected languages — a lowercase continuation is treated as a broken word.",
  uppercase: "No dictionary installed for the selected languages — a capitalised continuation is treated as a real compound.",
  digit: "A hyphen after a digit is never a broken word.",
  policy: "Forced by the hyphen policy this page was recognised with.",
};

/**
 * Review list for the line-break hyphens of the last TSV recognition.
 *
 * The Rust rebuild decides each hyphen (any installed dictionary of the
 * recognised language set first, then a shape heuristic) and reports it here,
 * so a wrong decision is one click away from being fixed instead of silently
 * corrupting the text. The header is also where the missing dictionaries of the
 * selected languages can be installed.
 */
export default function HyphenPanel({
  hyphens,
  dictionaries,
  uncovered,
  disabled,
  installing,
  onDecide,
  onDecideAll,
  onInstall,
}: {
  hyphens: OcrHyphen[];
  /** Every dictionary of the recognised language set. */
  dictionaries: OcrDictionary[];
  /** Selected languages with no dictionary at all (CJK, Arabic, …). */
  uncovered: string[];
  /** The result was edited by hand, so candidates can no longer be located. */
  disabled: boolean;
  /** Code of the dictionary being downloaded right now. */
  installing: string | null;
  onDecide: (index: number, decision: "join" | "keep") => void;
  onDecideAll: (decision: "join" | "keep") => void;
  onInstall: (code: string) => void;
}) {
  const [open, setOpen] = useState(true);

  const joined = useMemo(
    () => hyphens.filter((hyphen) => hyphen.decision === "join").length,
    [hyphens]
  );
  const needsReview = useMemo(
    () => hyphens.filter((hyphen) => hyphen.reason === "unknown-word" || hyphen.reason === "uppercase").length,
    [hyphens]
  );

  return (
    <div className="shrink-0 border-t border-border-default">
      <div className="flex flex-wrap items-center gap-2 px-3 py-1.5">
        <button
          type="button"
          onClick={() => setOpen((value) => !value)}
          aria-expanded={open}
          className={`${tbBase} border border-transparent`}
          title={open ? "Hide the hyphen review" : "Review the line-break hyphens"}
        >
          {open ? <ChevronDown size={14} /> : <ChevronUp size={14} />}
          Hyphens
          <span className="px-1.5 rounded bg-bg-muted border border-border-default text-[11px] text-text-secondary">
            {hyphens.length}
          </span>
        </button>

        <span className="text-[11px] text-text-tertiary">
          {joined} joined · {hyphens.length - joined} kept
          {needsReview > 0 && !disabled ? ` · ${needsReview} to check` : ""}
        </span>

        <span className="flex-1" />

        {/* One chip per dictionary of the language set: an installed one decides
            the hyphens, a missing one is one click away. */}
        {dictionaries.map((entry) =>
          entry.installed ? (
            <span
              key={entry.code}
              className="px-1.5 py-0.5 rounded border border-accent/30 bg-accent/10 text-[11px] text-accent whitespace-nowrap"
              title={`Line-break hyphens are decided by the ${entry.label} Hunspell dictionary`}
            >
              {entry.label}
            </span>
          ) : (
            <button
              key={entry.code}
              type="button"
              onClick={() => onInstall(entry.code)}
              disabled={installing !== null}
              className={`${tbBase} border border-border-default whitespace-nowrap disabled:opacity-50 disabled:cursor-not-allowed`}
              title={`Download the ${entry.label} dictionary so this page's hyphens are decided by words instead of guessing`}
            >
              {installing === entry.code ? (
                <Loader2 size={14} className="animate-spin" />
              ) : (
                <Download size={14} />
              )}
              {installing === entry.code ? "Downloading…" : `Get ${entry.label}`}
            </button>
          )
        )}

        {uncovered.length > 0 && (
          <span
            className="px-1.5 py-0.5 rounded border border-border-default bg-bg-muted text-[11px] text-text-tertiary whitespace-nowrap"
            title="No Hunspell dictionary exists for these languages, so their line-break hyphens are decided by the shape rule: a lowercase continuation joins, a capitalised one is kept."
          >
            {uncovered.join(", ")}: shape rule
          </span>
        )}

        <button
          type="button"
          onClick={() => onDecideAll("join")}
          disabled={disabled}
          className={`${tbBase} border border-border-default disabled:opacity-40 disabled:cursor-not-allowed`}
          title="Join every hyphenated word break"
        >
          Join all
        </button>
        <button
          type="button"
          onClick={() => onDecideAll("keep")}
          disabled={disabled}
          className={`${tbBase} border border-border-default disabled:opacity-40 disabled:cursor-not-allowed`}
          title="Keep every hyphen as printed"
        >
          Keep all
        </button>
      </div>

      {open && (
        <div className="border-t border-border-default">
          {disabled && (
            <p className="px-3 py-1.5 text-[11px] text-text-tertiary">
              The text was edited, so these decisions no longer apply. Run Recognize
              again to review them.
            </p>
          )}
          <ul className="max-h-44 overflow-y-auto py-1">
            {hyphens.map((hyphen, index) => (
              <li
                key={`${hyphen.left}${hyphen.hyphen}${hyphen.right}-${index}`}
                className="flex flex-wrap items-center gap-2 px-3 py-1"
              >
                <span className="min-w-0 flex-1 text-xs">
                  <span className="text-text-secondary">
                    {hyphen.left}
                    <span className="text-accent">{hyphen.hyphen}</span> {hyphen.right}
                  </span>
                  <span className="mx-1 text-text-tertiary">→</span>
                  <span
                    className={
                      hyphen.decision === "join"
                        ? "text-text-primary font-medium"
                        : "text-text-tertiary"
                    }
                  >
                    {hyphen.joined}
                  </span>
                </span>

                <span
                  className="shrink-0 px-1.5 py-0.5 rounded border border-border-default bg-bg-muted text-[11px] text-text-tertiary"
                  title={REASON_TITLES[hyphen.reason] ?? ""}
                >
                  {REASON_LABELS[hyphen.reason] ?? hyphen.reason}
                </span>

                <div
                  role="group"
                  aria-label={`Decision for ${hyphen.joined}`}
                  className="shrink-0 flex items-center gap-0.5 p-0.5 rounded-md bg-bg-muted border border-border-default"
                >
                  <button
                    type="button"
                    aria-pressed={hyphen.decision === "join"}
                    disabled={disabled}
                    onClick={() => onDecide(index, "join")}
                    className={`px-1.5 py-0.5 rounded text-[11px] transition-colors disabled:opacity-40 disabled:cursor-not-allowed ${
                      hyphen.decision === "join"
                        ? "bg-bg-card text-text-primary shadow-sm"
                        : "text-text-secondary hover:text-text-primary"
                    }`}
                  >
                    Join
                  </button>
                  <button
                    type="button"
                    aria-pressed={hyphen.decision === "keep"}
                    disabled={disabled}
                    onClick={() => onDecide(index, "keep")}
                    className={`px-1.5 py-0.5 rounded text-[11px] transition-colors disabled:opacity-40 disabled:cursor-not-allowed ${
                      hyphen.decision === "keep"
                        ? "bg-bg-card text-text-primary shadow-sm"
                        : "text-text-secondary hover:text-text-primary"
                    }`}
                  >
                    Keep
                  </button>
                </div>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
