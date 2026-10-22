/**
 * Which writing the Read Aloud page shows its texts in.
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

const STORAGE_KEY = "read_aloud_font";

/** The option for an id, or the default when the stored value names nothing -
 *  a key written by an older build, say. */
export function raFontOption(id: RaFontId): RaFontOption {
  return RA_FONT_OPTIONS.find((o) => o.id === id) ?? RA_FONT_OPTIONS[0];
}

/** Read the stored choice. Guarded because this runs during the first render,
 *  which is server-side in the static prerender and has no localStorage. */
export function getStoredRaFont(): RaFontId {
  try {
    const raw = localStorage.getItem(STORAGE_KEY) as RaFontId | null;
    return raw && RA_FONT_OPTIONS.some((o) => o.id === raw) ? raw : RA_FONT_DEFAULT;
  } catch {
    return RA_FONT_DEFAULT;
  }
}

export function setStoredRaFont(id: RaFontId): void {
  try {
    if (id === RA_FONT_DEFAULT) localStorage.removeItem(STORAGE_KEY);
    else localStorage.setItem(STORAGE_KEY, id);
  } catch {
    /* private mode / no storage - the choice just does not survive a reload */
  }
}
