"use client";

import { useCallback, useEffect, useState } from "react";

/**
 * Reading font for markdown prose (`.md-body`: wiki documents and the card
 * viewer's text), chosen in Settings → Fonts.
 *
 * The Schulschriften faces are declared as @font-face in app/globals.css and
 * ship from public/fonts/. Their binaries are maintained in the separate
 * wsva/fms-font repo (the local `font/` checkout is gitignored), so adding a
 * family is three steps: copy its `entenmann-<id>-*.woff2` into public/fonts/,
 * declare the four RIBBI faces there, and add an entry to MD_FONT_OPTIONS.
 *
 * The choice persists in localStorage under `mdFont` and is expressed as two
 * custom properties on `<html>` (`--md-body-font`, `--md-body-leading`) instead
 * of one CSS rule per family, so this registry is the only place that knows the
 * family names. For "default" both properties are removed, and
 * `font-family: var(--md-body-font)` with an undefined property is invalid at
 * computed-value time — the declaration falls back to `unset`, i.e. inherit, so
 * prose keeps the UI font from `:root` with no per-family selector at all.
 *
 * Applied from providers.tsx on mount, so the stored pick is in effect even if
 * Settings is never opened. It is written in an effect rather than during
 * render: the prerendered HTML carries no inline style, so there is nothing for
 * the theme/zoom head script in layout.tsx to mirror and no hydration mismatch.
 */

const MD_FONT_KEY = "mdFont";

/** Custom properties consumed by .md-body in components/wiki/markdown/markdown.css. */
const FONT_PROP = "--md-body-font";
const LEADING_PROP = "--md-body-leading";

export type MdFontId = "default" | "su" | "dn" | "la" | "sas" | "va";

export interface MdFontOption {
  id: MdFontId;
  /** Name shown in Settings. */
  label: string;
  /** Provenance line: script name + the CTAN font that produced it. */
  hint: string;
  /**
   * font-family value for .md-body, or null to leave the UI font in place.
   * The four RIBBI faces share one family name, so <b>/<i> and the markdown
   * headings resolve to the real bold/slanted renders rather than synthetic
   * approximations (globals.css also sets `font-synthesis: none` on :root).
   */
  css: string | null;
  /**
   * Line spacing multiplier. Joined cursive carries ascenders and descenders
   * into the next line, so the scripts want more leading than the UI font's
   * default; null keeps the existing stylesheet values.
   */
  leading: number | null;
}

export const MD_FONT_OPTIONS: MdFontOption[] = [
  { id: "default", label: "UI font", hint: "Interface default", css: null, leading: null },
  {
    id: "va",
    label: "Vereinfachte Ausgangsschrift",
    hint: "VA, 1968 — simplest of the five, easiest to read on screen",
    css: '"Vereinfachte Ausgangsschrift Entenmann", cursive',
    leading: 1.9,
  },
  {
    id: "la",
    label: "Lateinische Ausgangsschrift",
    hint: "LA, 1953 — wela",
    css: '"Lateinische Ausgangsschrift Entenmann", cursive',
    leading: 1.9,
  },
  {
    id: "sas",
    label: "Schulausgangsschrift",
    hint: "SAS, ex-GDR — wesa",
    css: '"Schulausgangsschrift Entenmann", cursive',
    leading: 1.9,
  },
  {
    id: "dn",
    label: "Deutsche Normalschrift",
    hint: "DN, 1951 — wedn",
    css: '"Deutsche Normalschrift Entenmann", cursive',
    leading: 1.9,
  },
  {
    id: "su",
    label: "Sütterlinschrift",
    hint: "Sütterlin, 1919 — wesu",
    css: '"Suetterlinschrift Entenmann", cursive',
    leading: 2,
  },
];

/** Sample shown on each option card: capitals, joins and the sharp S. */
export const MD_FONT_SAMPLE = "Fremdsprachen machen Spaß";

function findOption(id: MdFontId): MdFontOption {
  return MD_FONT_OPTIONS.find((o) => o.id === id) ?? MD_FONT_OPTIONS[0];
}

export function getStoredMdFont(): MdFontId {
  if (typeof window === "undefined") return "default";
  const stored = localStorage.getItem(MD_FONT_KEY) as MdFontId | null;
  // An id with no option behind it (a family later removed from public/fonts/)
  // must not leave prose pointing at a font that cannot load.
  return stored && MD_FONT_OPTIONS.some((o) => o.id === stored) ? stored : "default";
}

/** Write (or clear) the custom properties the markdown body reads. */
export function applyMdFont(id: MdFontId) {
  const root = document.documentElement;
  const option = findOption(id);
  localStorage.setItem(MD_FONT_KEY, id);
  if (option.css) root.style.setProperty(FONT_PROP, option.css);
  else root.style.removeProperty(FONT_PROP);
  if (option.leading) root.style.setProperty(LEADING_PROP, String(option.leading));
  else root.style.removeProperty(LEADING_PROP);
}

export function useMdFont() {
  const [mdFont, setMdFontState] = useState<MdFontId>(getStoredMdFont);

  // Converge on the stored value on mount as well as on every change, so the
  // properties match localStorage even if another tab or an earlier render set
  // them differently.
  useEffect(() => {
    applyMdFont(mdFont);
  }, [mdFont]);

  const setMdFont = useCallback((id: MdFontId) => setMdFontState(id), []);

  return { mdFont, setMdFont, options: MD_FONT_OPTIONS };
}
