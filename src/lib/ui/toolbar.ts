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
// - Mobile labeled icon buttons STACK (icon over a `text-xs` label) by default —
//   the words are kept, not dropped to icon-only. Pass `collapseOnMobile={false}`
//   to hold the inline horizontal labeled button instead (used by the Sync page's
//   per-card action rows).
// - Stacked (`tb*Stacked`) is the icon-over-`text-xs`-label layout, in a narrower
//   box. It trades ~10px of bar height for keeping the words, which is the right
//   call on a phone's toolbar where a horizontal row of labeled buttons would
//   not fit. A stacked button never hides its label.
// - Because stacking keeps the words, a phone bar fills up fast. Occasional
//   actions — Add / New *, Manage / Locations, and Refresh — therefore tuck into
//   the three-dot `ToolbarOverflow` on mobile (they stay inline on desktop), so
//   the visible stacked row carries only the contextual actions that matter now.
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

// Base box for the stacked layout: icon on top, label underneath.
const baseStacked =
  "flex flex-col items-center gap-0.5 px-2.5 py-1 text-xs rounded-md transition-colors disabled:opacity-50 disabled:cursor-not-allowed";

/** The bare stacked-button box, same role as `tbBase` for the vertical layout. */
export const tbBaseStacked = baseStacked;

// Base box for a square, icon-only toolbar button (Back / Forward / arrows).
const iconBase = "p-1.5 rounded-md transition-colors";

// Color/affordance pairs, orthogonal to the layout so every button exists in
// both orientations without repeating a color string.
const ghostColor = "text-text-secondary hover:text-text-primary hover:bg-bg-hover";
const dangerColor = "text-text-secondary hover:text-error-text hover:bg-bg-hover";
const solidColor = "bg-accent text-white hover:bg-accent/90";
const toggleColor = (active: boolean) => (active ? solidColor : ghostColor);
const toggleSoftColor = (active: boolean) =>
  active ? "bg-bg-hover text-text-primary" : ghostColor;

/** Neutral (ghost) action: Refresh, Reload, Edit, New File… */
export const tbGhost = `${base} ${ghostColor}`;

/** Destructive action, tinted on hover: Delete, Delete Dataset… */
export const tbDanger = `${base} ${dangerColor}`;

/** Solid accent call-to-action, always "on": Download… */
export const tbSolid = `${base} ${solidColor}`;

/** Toggle action — solid accent when active, ghost when inactive. */
export const tbToggle = (active: boolean) => `${base} ${toggleColor(active)}`;

/** Toggle action — subtle highlighted background when active (Tools / View…). */
export const tbToggleSoft = (active: boolean) => `${base} ${toggleSoftColor(active)}`;

/** Stacked (icon-top / label-bottom) counterparts of the five above. */
export const tbGhostStacked = `${baseStacked} ${ghostColor}`;
export const tbDangerStacked = `${baseStacked} ${dangerColor}`;
export const tbSolidStacked = `${baseStacked} ${solidColor}`;
export const tbToggleStacked = (active: boolean) => `${baseStacked} ${toggleColor(active)}`;
export const tbToggleSoftStacked = (active: boolean) => `${baseStacked} ${toggleSoftColor(active)}`;

/** Icon-only navigation button, ghost in both states. */
export const tbIcon = `${iconBase} ${ghostColor} disabled:opacity-30 disabled:cursor-default`;

/** Icon-only toggle, solid accent when active (mobile collapse toggle). */
export const tbIconToggle = (active: boolean) =>
  `${iconBase} ${active ? solidColor : ghostColor}`;

/** Icon-only toggle, subtle highlighted background when active (search). */
export const tbIconToggleSoft = (active: boolean) =>
  `${iconBase} ${active ? "bg-bg-hover text-text-primary" : ghostColor}`;
