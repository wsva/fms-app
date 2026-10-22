/**
 * How the Read Aloud page typesets the text to be read.
 *
 * Two dials over that one line of text: which of the four German school scripts
 * it is written in, and how wide the gap between its words is.
 *
 * German learners are taught one of four school scripts, and the text they read
 * aloud should be written the way they were taught to write it - a learner of VA
 * who has never seen LA will lose more time untangling the letterforms than the
 * exercise is worth. So the page offers the models and remembers the choice.
 *
 * The faces are Playwrite DE (OFL 1.1, see public/fonts/PLAYWRITE-OFL.txt),
 * declared in src/app/globals.css and served from public/fonts/. They are
 * variable wght 100..400 but only the 400 default instance is used: a
 * handwriting face has no bold to ask for, and `font-synthesis: none` on :root
 * already declines to fake one.
 *
 * Device-local view state, like the theme and the page zoom - so localStorage,
 * not the backend settings file, and no MCP twin: Rust cannot write a frontend
 * localStorage.
 */

export type RaFontId = "ui" | "grund" | "va" | "sas" | "la";

export type RaFontOption = {
  id: RaFontId;
  /** Short label for the toolbar trigger. */
  label: string;
  /** What the model is, for the menu row that picks it. */
  hint: string;
  /** The `--ra-font` value; empty leaves the interface font in place. The tail
   *  is the UI stack so a face that fails to load falls back to it rather than
   *  to a system cursive. */
  css: string;
};

/** The UI stack, repeated as the fallback tail of every face below. */
const UI = "Inter, Avenir, Helvetica, Arial, sans-serif";

/** In the order German schools introduce them. */
export const RA_FONT_OPTIONS: RaFontOption[] = [
  { id: "ui", label: "UI font", hint: "The interface font", css: "" },
  {
    id: "grund",
    label: "Grundschrift",
    hint: "Connected print - the first script German children write",
    css: `"Playwrite DE Grund", ${UI}`,
  },
  {
    id: "va",
    label: "VA",
    hint: "Vereinfachte Ausgangsschrift - the simplified cursive",
    css: `"Playwrite DE VA", ${UI}`,
  },
  {
    id: "sas",
    label: "SAS",
    hint: "Schulausgangsschrift - the cursive taught in most of Germany",
    css: `"Playwrite DE SAS", ${UI}`,
  },
  {
    id: "la",
    label: "LA",
    hint: "Lateinische Ausgangsschrift - the classic cursive",
    css: `"Playwrite DE LA", ${UI}`,
  },
];

export const RA_FONT_DEFAULT: RaFontId = "ui";

/** The option for an id, or the default when the stored value names nothing -
 *  a key written by an older build, say. */
export function raFontOption(id: RaFontId): RaFontOption {
  return RA_FONT_OPTIONS.find((o) => o.id === id) ?? RA_FONT_OPTIONS[0];
}

/* ---- word spacing ---------------------------------------------------------
 * The other dial for the same line. In a cursive model the letters of a word run
 * into one another and the gap between words is the only thing marking where one
 * ends, so widening it is a legibility aid - most of all for VA and SAS, whose
 * `t` reads as an `l`.
 *
 * `word-spacing` is an ADDITION to the space the font already has, so "2x" has to
 * add one space advance and "3x" two - and that is a property of the font, not a
 * guess. hmtx gives U+0020 as 300/1000 units per em in all four Playwrite faces,
 * which is also what the browser puts between two words: 5.41px at this page's
 * 18px, so 0.30em is exact for the scripts. The interface font's own space is
 * narrower (4.93px, 0.274em), so 2x reads 2.09x there.
 *
 * Two dead ends behind that number: inferring the gap as width("a a") minus
 * width("aa") is polluted by the a-a kern pair these faces carry in GPOS, and it
 * silently measures a fallback font when the webfont has not finished loading.
 * Percentages are no help either - Chromium resolves `word-spacing: 100%` against
 * the font size, not against the space glyph, so it would add a whole em.
 * -------------------------------------------------------------------------- */

export type RaSpaceId = 1 | 2 | 3;

export type RaSpaceOption = { id: RaSpaceId; label: string; hint: string; css: string };

/** One space advance of the Playwrite faces, in em. See the note above. */
const SPACE_EM = 0.3;

export const RA_SPACE_OPTIONS: RaSpaceOption[] = [
  { id: 1, label: "1x", hint: "The space the font itself has", css: "" },
  { id: 2, label: "2x", hint: "One extra space between words", css: `${SPACE_EM}em` },
  { id: 3, label: "3x", hint: "Two extra spaces between words", css: `${SPACE_EM * 2}em` },
];

export const RA_SPACE_DEFAULT: RaSpaceId = 1;

export function raSpaceOption(id: RaSpaceId): RaSpaceOption {
  return RA_SPACE_OPTIONS.find((o) => o.id === id) ?? RA_SPACE_OPTIONS[0];
}

/* ---- persistence ----------------------------------------------------------
 * The default is stored as no key at all, so "unset" and "default" are one state
 * and a later version can change what the default means without stale rows
 * overriding it.
 * -------------------------------------------------------------------------- */

const FONT_KEY = "read_aloud_font";
const SPACE_KEY = "read_aloud_space";

/** Guarded because these reads run during the first render, which is server-side
 *  in the static prerender and has no localStorage. */
function readStored(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeStored(key: string, value: string | null): void {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    /* private mode / no storage - the choice just does not survive a reload */
  }
}

export function getStoredRaFont(): RaFontId {
  const raw = readStored(FONT_KEY) as RaFontId | null;
  return raw && RA_FONT_OPTIONS.some((o) => o.id === raw) ? raw : RA_FONT_DEFAULT;
}

export function setStoredRaFont(id: RaFontId): void {
  writeStored(FONT_KEY, id === RA_FONT_DEFAULT ? null : id);
}

export function getStoredRaSpace(): RaSpaceId {
  // Written as a string, read back as a number; anything that is not one of the
  // three ids - null, "", "four" - fails the some() and falls to the default.
  const n = Number(readStored(SPACE_KEY));
  return RA_SPACE_OPTIONS.some((o) => o.id === n) ? (n as RaSpaceId) : RA_SPACE_DEFAULT;
}

export function setStoredRaSpace(id: RaSpaceId): void {
  writeStored(SPACE_KEY, id === RA_SPACE_DEFAULT ? null : String(id));
}
