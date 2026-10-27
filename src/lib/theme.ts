// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------
// The four themes are defined as `--bg-*` / `--text-*` custom properties per
// `[data-theme="…"]` block in app/globals.css; switching one is a single
// attribute on `<html>`. This module owns that write so the settings page and
// the title bar's View menu cannot drift apart, and so the key below stays in
// sync with the pre-paint script in app/layout.tsx (which reads the same
// `theme` key to avoid a flash of the wrong theme).
//
// `light` is represented by *removing* the attribute rather than setting
// `data-theme="light"`, matching both that script and globals.css, where the
// light values live on the bare `:root` selector.

export type ThemeId = "light" | "dark" | "solarized" | "gruvbox";

export interface ThemeDef {
  id: ThemeId;
  label: string;
  /** Swatch classes for the picker, hardcoded to the theme's own body/border
   *  colors — a preview must not follow the theme it is previewing. */
  preview: string;
}

/** Order is the picker order: light first, then the three dark variants. */
export const THEMES: ThemeDef[] = [
  { id: "light", label: "Light", preview: "bg-[#fbfbfb] border-[#e0e0e0]" },
  { id: "dark", label: "Dark", preview: "bg-[#1a1a1a] border-[#3a3a3a]" },
  { id: "solarized", label: "Solarized", preview: "bg-[#fdf6e3] border-[#d3cbb7]" },
  { id: "gruvbox", label: "Gruvbox", preview: "bg-[#282828] border-[#504945]" },
];

const THEME_KEY = "theme";

/** Fired on `window` with the new id in `detail` whenever the applied theme
 *  changes. Every surface that shows a selected theme listens for it (the
 *  settings swatches, the title bar's menu), because pages stay mounted in this
 *  shell — a stale local copy would otherwise keep offering the old theme. */
export const THEME_CHANGED_EVENT = "theme-changed";

/** The theme localStorage asks for. `light` is the default, and an unknown
 *  stored value falls back to it rather than rendering an undefined theme. */
export function getStoredTheme(): ThemeId {
  if (typeof window === "undefined") return "light";
  const stored = localStorage.getItem(THEME_KEY) as ThemeId | null;
  return THEMES.some((t) => t.id === stored) ? (stored as ThemeId) : "light";
}

/** Apply a theme, persist it, and tell every other surface. Idempotent: calling
 *  it with the theme already in force writes the same value and re-dispatches
 *  the event, which listeners fold into their own unchanged state. */
export function applyTheme(theme: ThemeId): void {
  localStorage.setItem(THEME_KEY, theme);
  if (theme === "light") {
    document.documentElement.removeAttribute("data-theme");
  } else {
    document.documentElement.setAttribute("data-theme", theme);
  }
  window.dispatchEvent(new CustomEvent(THEME_CHANGED_EVENT, { detail: theme }));
}
