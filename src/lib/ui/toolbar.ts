// ---------------------------------------------------------------------------
// Shared toolbar button styles.
//
// The wiki page's sub-toolbar defined the visual target (compact, normal-weight
// text, accent when active). These helpers are now the single source of truth
// for that look so toolbars across pages stay consistent instead of drifting —
// e.g. the dictation toolbar previously used HeroUI `<Button>`, whose base
// styles carry a medium font-weight and read slightly bolder.
//
// Pair these with a plain `<button type="button">` (not HeroUI `<Button>`), so
// nothing reintroduces the component's own padding / font-weight. Prefer the
// shared React kit in `src/components/ui/toolbar/` (ToolbarButton, etc.), which
// wraps these tokens and enforces the conventions below.
//
// Conventions (single source of truth for every toolbar in the app):
// - Icon size: labeled button = 14 (TB_ICON), icon-only button = 16
//   (TB_ICON_SIZE), menu item = 15 (TB_MENU_ICON). The only exception is
//   Dictation's mobile cue-step arrows (20, strokeWidth 3), passed explicitly.
// - Toolbar container gap: `gap-2` on every page.
// - Save-of-a-dirty-editor: `toggle` variant with active={dirty}, disabled when
//   clean. Reserve `solid` for genuine one-shot CTAs (Download, Upload).
// - Destructive toolbar action: `danger` (ghost-red). Solid-red is only the
//   active recording state (RecordButton).
// - Label spacing comes from `gap-1.5` only; no leading-space-in-label idiom.
// - Mobile label collapsing defaults ON for labeled buttons.
// - `aria-label` is auto-derived from label/title; icon-only buttons require one.
// ---------------------------------------------------------------------------

/** Icon size for a labeled toolbar button. */
export const TB_ICON = 14;
/** Icon size for an icon-only toolbar button (nav / search / overflow). */
export const TB_ICON_SIZE = 16;
/** Icon size for an overflow-menu item. */
export const TB_MENU_ICON = 15;

// Base box + typography shared by every labeled toolbar action button.
const base =
  "flex items-center gap-1.5 px-3 py-1 text-sm rounded-md transition-colors disabled:opacity-50 disabled:cursor-not-allowed";

/** The bare labeled-button box, for one-off states that need their own colors
 *  (e.g. RecordButton's solid-red recording state). Prefer the composed tokens
 *  above unless you must supply a custom color. */
export const tbBase = base;

// Base box for a square, icon-only toolbar button (Back / Forward / arrows).
const iconBase = "p-1.5 rounded-md transition-colors";
const iconGhost = "text-text-secondary hover:text-text-primary hover:bg-bg-hover";

/** Neutral (ghost) action: Refresh, Reload, Edit, New File… */
export const tbGhost = `${base} ${iconGhost}`;

/** Destructive action, tinted on hover: Delete, Delete Dataset… */
export const tbDanger = `${base} text-text-secondary hover:text-error-text hover:bg-bg-hover`;

/** Solid accent call-to-action, always "on": Download… */
export const tbSolid = `${base} bg-accent text-white hover:bg-accent/90`;

/** Toggle action — solid accent when active, ghost when inactive. */
export const tbToggle = (active: boolean) =>
  `${base} ${active ? "bg-accent text-white hover:bg-accent/90" : iconGhost}`;

/** Toggle action — subtle highlighted background when active (Tools / View…). */
export const tbToggleSoft = (active: boolean) =>
  `${base} ${active ? "bg-bg-hover text-text-primary" : iconGhost}`;

/** Icon-only navigation button, ghost in both states. */
export const tbIcon = `${iconBase} ${iconGhost} disabled:opacity-30 disabled:cursor-default`;

/** Icon-only toggle, solid accent when active (mobile collapse toggle). */
export const tbIconToggle = (active: boolean) =>
  `${iconBase} ${active ? "bg-accent text-white hover:bg-accent/90" : iconGhost}`;

/** Icon-only toggle, subtle highlighted background when active (search). */
export const tbIconToggleSoft = (active: boolean) =>
  `${iconBase} ${active ? "bg-bg-hover text-text-primary" : iconGhost}`;
