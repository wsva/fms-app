"use client";

import { Rss } from "lucide-react";

type MicGlyphProps = {
  size?: number;
  width?: number;
  height?: number;
  className?: string;
};

/**
 * The app's microphone glyph: lucide's `Rss` icon rotated 45deg clockwise
 * (a positive CSS angle is clockwise) around its center. Used everywhere a
 * microphone is depicted — recording buttons, the Read Aloud nav entry, and
 * the Microphone settings section — so the metaphor stays consistent. It is
 * also the Read Aloud dataset type's glyph, through
 * `DATASET_TYPE_ICONS.read_aloud` in `@/lib/datasets/types`, so a dataset card,
 * a workflow tile, a family chip and the nav entry draw the same picture.
 *
 * Accepts the same `size` / `width` / `height` / `className` props lucide
 * icons do, so it drops into both direct JSX (`<MicGlyph size={16} />`) and
 * icon-position component references (a sidebar/bottom-nav tab's `icon:`
 * field, which renders `<Icon width={22} height={22} />`). Only the props that
 * are actually set are forwarded: lucide spreads caller props last, so passing
 * an explicit `width={undefined}` would clobber its computed size.
 */
export function MicGlyph({ size, width, height, className = "" }: MicGlyphProps) {
  const dims =
    width != null || height != null ? { width, height } : { size };
  return (
    <Rss {...dims} className={`rotate-45 origin-center ${className}`.trim()} />
  );
}

export default MicGlyph;
