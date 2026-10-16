"use client";

import { useMemo } from "react";

/**
 * A plain-text chapter-structure editor that shows syntax highlighting without
 * giving up the native `<textarea>`.
 *
 * How it works: the real `<textarea>` stays the single source of truth (so
 * caret, selection, typing, paste, undo and IME all behave exactly like plain
 * text), but its own glyphs are made `color: transparent`. A read-only overlay
 * painted *behind* it renders the same string with per-token colors. The caret
 * stays visible because `caret-color` is independent of text color (globals.css
 * already themes it with `--accent`).
 *
 * The overlay is laid out in normal flow so it defines the box height and lets
 * the control grow with content; the textarea is absolutely positioned to fill
 * the identical box. Both share byte-for-byte the same font metrics, padding
 * and wrapping rules, which is what keeps the two layers pixel-aligned.
 */

// Keep in sync with the parser's grammar (chapterMarkdown.ts) so what is
// colored matches what is validated.
const HEADING_RE = /^(#{1,6})(\s+)(.*)$/;
const ATTR_RE = /\s*\{#([^{}]+)\}\s*$/;

// Shared box metrics for BOTH the overlay and the textarea. Any difference here
// desyncs the two layers, so keep them literally the same string.
const TEXT_METRICS =
  "font-mono text-sm leading-6 px-3 py-2 whitespace-pre-wrap break-words";

type Props = {
  value: string;
  onChange: (value: string) => void;
  /** 1-based line numbers flagged by the parser; rendered with a red tint. */
  errorLines?: number[];
  disabled?: boolean;
  autoFocus?: boolean;
  placeholder?: string;
  /** Applied to the outer box (sizing, border, background). */
  className?: string;
};

/** Split one source line into colored token spans: `#` / title / `{#uuid}`. */
function renderLine(text: string) {
  if (!text) return <span>{"\u200b"}</span>; // keep empty lines one row tall

  const m = text.match(HEADING_RE);
  if (!m) {
    // Prose / blank line (a non-heading line is allowed but ignored by sync).
    return <span className="text-text-tertiary">{text}</span>;
  }

  const hashes = m[1];
  const gap = m[2];
  const rest = m[3];

  const am = rest.match(ATTR_RE);
  let title = rest;
  let attr = "";
  if (am) {
    attr = rest.slice(am.index ?? 0);
    title = rest.slice(0, am.index ?? rest.length);
  }

  return (
    <>
      <span className="text-text-tertiary">{hashes}</span>
      <span>{gap}</span>
      <span className="text-text-primary font-medium">{title}</span>
      {attr && <span className="italic opacity-60 text-text-tertiary">{attr}</span>}
    </>
  );
}

export default function HighlightedChapterInput({
  value,
  onChange,
  errorLines,
  disabled,
  autoFocus,
  placeholder,
  className = "",
}: Props) {
  const errorSet = useMemo(
    () => new Set(errorLines ?? []),
    [errorLines]
  );

  const lines = value.split("\n");

  return (
    <div className={`relative ${className}`}>
      {/* Highlight overlay: in normal flow, so it sizes the box. */}
      <div aria-hidden className={`pointer-events-none ${TEXT_METRICS}`}>
        {lines.map((line, i) => (
          <div
            key={i}
            className={errorSet.has(i + 1) ? "rounded-sm bg-red-500/10" : undefined}
          >
            {renderLine(line)}
          </div>
        ))}
      </div>

      {/* Real editor: transparent text so only the overlay shows through. */}
      <textarea
        className={`absolute inset-0 h-full w-full resize-none border-0 bg-transparent outline-none text-transparent placeholder:text-text-tertiary ${TEXT_METRICS} ${
          disabled ? "cursor-not-allowed" : ""
        }`}
        style={{ caretColor: "var(--accent)" }}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        disabled={disabled}
        autoFocus={autoFocus}
        placeholder={placeholder}
        spellCheck={false}
        wrap="soft"
      />
    </div>
  );
}
