"use client";

import { levelProgress } from "@/lib/xp";

/**
 * Circular initial with a level-progress ring around it.
 *
 * The ring is a `conic-gradient` rather than an SVG: `--accent` and
 * `--border-default` are defined in every theme block in `globals.css`, so the
 * ring follows the theme with no new colour variables and no stroke math. The
 * inner disc paints over the middle in `bg-bg-card`, which is what turns the
 * filled cone into a ring.
 *
 * Sizes are the three places an avatar appears: 24 collapsed sidebar, 28 the
 * sidebar footer, 36 the Android drawer.
 */
export default function LevelAvatar({
  name,
  xp,
  size = 28,
  flash = null,
}: {
  name: string;
  xp: number;
  size?: 24 | 28 | 36;
  /** XP just earned; rendered as a bouncing `+N` badge until it clears. */
  flash?: number | null;
}) {
  const { level, nextXp, toNext, pct } = levelProgress(xp);
  const thickness = size >= 36 ? 3 : size >= 28 ? 2.5 : 2;
  const initial = (name.trim().charAt(0) || "?").toUpperCase();

  return (
    <span
      className="relative rounded-full shrink-0 select-none"
      style={{
        width: size,
        height: size,
        // One decimal is plenty for a ring and keeps the gradient string short.
        background: `conic-gradient(var(--accent) ${(pct * 100).toFixed(1)}%, var(--border-default) 0)`,
      }}
      title={`Level ${level} · ${xp} XP${toNext > 0 ? ` (next at ${nextXp})` : ""}`}
    >
      <span
        className="absolute rounded-full bg-bg-card flex items-center justify-center font-bold text-text-primary leading-none"
        style={{ inset: thickness, fontSize: Math.round(size * 0.4) }}
      >
        {initial}
      </span>
      {flash !== null && flash > 0 && (
        <span
          className="absolute -top-2 left-1/2 -translate-x-1/2 rounded-full bg-yellow-500 px-1 text-[9px] font-bold leading-[14px] text-bg-card animate-bounce"
          aria-hidden="true"
        >
          +{flash}
        </span>
      )}
    </span>
  );
}
