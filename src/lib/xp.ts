/**
 * XP level progress.
 *
 * `level = floor(sqrt(xp / 100)) + 1` mirrors the backend curve in
 * `src-tauri/src/xp.rs` (`recompute_user_total`) — the two must stay in step,
 * or the ring would disagree with the `level` the XP ledger reports. The level
 * is re-derived from `lifetime_xp` here rather than read off the row, so the
 * ratio and the label always describe the same number.
 */

/** Kept in one place so the curve is legible against `xp.rs` (xp / 100). */
const XP_UNIT = 100;

export interface LevelProgress {
  /** Level the XP total lands on. */
  level: number;
  /** XP at which this level started. */
  floorXp: number;
  /** XP required for the next level. */
  nextXp: number;
  /** XP still missing for the next level (0 at the top of the curve is finite,
   *  so this only reaches 0 exactly on the boundary). */
  toNext: number;
  /** Fraction of the way through the current level, clamped to 0..1. */
  pct: number;
}

export function levelProgress(lifetimeXp: number): LevelProgress {
  const xp = Math.max(0, Math.floor(lifetimeXp || 0));
  const level = Math.floor(Math.sqrt(xp / XP_UNIT)) + 1;
  const floorXp = XP_UNIT * (level - 1) ** 2;
  const nextXp = XP_UNIT * level ** 2;
  // The span is XP_UNIT * (2 * level - 1), so it is never 0; the guard only
  // protects against a hand-edited XP_UNIT of 0.
  const span = nextXp - floorXp;
  const pct = span > 0 ? Math.min(1, Math.max(0, (xp - floorXp) / span)) : 1;
  return { level, floorXp, nextXp, toNext: Math.max(0, nextXp - xp), pct };
}
