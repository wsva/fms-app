"use client";

// ---------------------------------------------------------------------------
// Agent badge
// ---------------------------------------------------------------------------
// The agent's brand mark: a glowing "AGENT" wordmark trailed by a small cyan
// sparkle. Converted from agent-badge.svg; the `viewBox` is cropped to the
// artwork's ink bounds (see INK_*), not the SVG's padded 46 x 17.6 frame.
//
// Unlike the lucide glyphs used elsewhere, this is *not* a monochrome icon that
// inherits `currentColor` — the blue gradient is deliberate brand artwork, so it
// renders the same in all four themes. Use it where the agent itself is being
// named (the dock, an "ask the agent" affordance); reach for `<Bot />` when you
// need a theme-following glyph that scales with the surrounding text.
//
// The SVG ids (`agent-blue`, `agent-glow`, ...) are namespaced per instance with
// `useId`, because gradients/filters resolve document-wide by id: two badges on
// one page would otherwise fight over a single def and the second would render
// unstyled.
// ---------------------------------------------------------------------------

import { useId } from "react";

/** The ink box, centered on the "AGENT" cap band (5.49..12.11, centre ≈ 8.8)
 *  so the element's own centre *is* the wordmark's visual centre — which is what
 *  lets `items-center` in a button line the caps up with adjacent text instead
 *  of sitting them low. The leading sparkle (3.4..6.6) straddles the top edge:
 *  symmetric ±0.9 glow margin above/below keeps the cap band dead-centre, and
 *  the sparkle spill is drawn via `overflow: visible`. Cropped to the glyph
 *  band, `height` tracks the wordmark's real height, not the SVG's padded
 *  46 x 17.6 frame (where the caps were only ~37% of the box). */
const INK_X = 6.0;
const INK_Y = 4.7;
const INK_W = 34.0;
const INK_H = 8.2;

export interface AgentBadgeProps {
  /** Rendered height in px, measured as the wordmark's own ink height, so it
   *  matches the cap height of neighbouring text. Defaults to 18px. */
  height?: number;
  /** Override the width outright (e.g. to fill a container). Pair with
   *  `preserveAspectRatio` behavior of the fixed viewBox. */
  width?: number;
  className?: string;
  /** Wrap the wordmark in a small black chip. The cyan→indigo gradient is brand
   *  artwork that reads poorly on light themes, so wherever the badge sits on a
   *  theme-following surface (a button, a card) this gives it a consistent dark
   *  plate. Keep the chip as the *outermost* box: pass `className` to style the
   *  bare svg instead, not this wrapper. */
  chip?: boolean;
  /** Accessible name. Pass `null` for a purely decorative badge beside a visible
   *  "Agent" label, which then marks the svg `aria-hidden`. */
  title?: string | null;
}

export default function AgentBadge({
  height = 18,
  width,
  className,
  chip = false,
  title = "Agent",
}: AgentBadgeProps) {
  const raw = useId();
  // `useId` yields `:r0:`-style tokens; colons are awkward inside url(#...)
  // fragments, so strip them and use the remainder as the def prefix.
  const uid = raw.replace(/:/g, "");
  const blue = `agent-blue-${uid}`;
  const glow = `agent-glow-${uid}`;

  const effectiveWidth = width ?? (height * INK_W) / INK_H;
  const decorative = title === null;

  const svg = (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      viewBox={`${INK_X} ${INK_Y} ${INK_W} ${INK_H}`}
      width={effectiveWidth}
      height={height}
      fill="none"
      role={decorative ? undefined : "img"}
      aria-label={decorative ? undefined : title}
      aria-hidden={decorative || undefined}
      className={className}
      // `overflow: visible` so the glow — and the top of the sparkle that now
      // straddles the crop edge — aren't clipped by the tight viewBox.
      // `display: block` so the svg behaves as a plain flex item: as an
      // inline-level box it would sit on the text baseline instead of obeying
      // the button's `items-center`.
      style={{ overflow: "visible", display: "block" }}
    >
      <defs>
        <linearGradient
          id={blue}
          x1={INK_X}
          y1={INK_Y}
          x2={INK_X + INK_W}
          y2={INK_Y + INK_H}
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0" stopColor="#8BE9FF" />
          <stop offset=".45" stopColor="#38BDF8" />
          <stop offset="1" stopColor="#6366F1" />
        </linearGradient>
        <filter id={glow} x="-20%" y="-80%" width="140%" height="260%">
          <feGaussianBlur stdDeviation=".55" result="blur" />
          <feMerge>
            <feMergeNode in="blur" />
            <feMergeNode in="SourceGraphic" />
          </feMerge>
        </filter>
      </defs>

      {/* The "AGENT" wordmark, outlined so it stays crisp when scaled down, with
          the brand gradient carried through a soft glow. */}
      <path
        d="M12.45 12H10.88L10.43 10.57H8.15L7.70 12H6.14L8.47 5.6H10.18ZM10.10 9.46 9.41 7.31Q9.34 7.07 9.30 6.74H9.27Q9.25 7.02 9.16 7.30L8.46 9.46Z M19.36 11.57Q18.42 12.11 17.03 12.11Q15.48 12.11 14.59 11.25Q13.70 10.40 13.70 8.90Q13.70 7.39 14.67 6.44Q15.65 5.49 17.26 5.49Q18.28 5.49 19.06 5.77V7.12Q18.32 6.69 17.25 6.69Q16.35 6.69 15.78 7.28Q15.22 7.86 15.22 8.83Q15.22 9.82 15.73 10.36Q16.23 10.90 17.09 10.90Q17.61 10.90 17.92 10.75V9.50H16.63V8.35H19.36Z M25.29 12H21.45V5.6H25.14V6.77H22.89V8.19H24.99V9.36H22.89V10.83H25.29Z M32.92 12H31.47L28.83 7.97Q28.60 7.62 28.51 7.44H28.49Q28.53 7.78 28.53 8.48V12H27.17V5.6H28.72L31.26 9.49Q31.43 9.75 31.58 10.01H31.60Q31.56 9.79 31.56 9.13V5.6H32.92Z M39.73 6.77H37.91V12H36.46V6.77H34.64V5.6H39.73Z"
        fill={`url(#${blue})`}
        filter={`url(#${glow})`}
      />

      {/* The leading sparkle — a small cyan four-point star. */}
      <path
        d="M6.5 4.5L7 3.4L7.5 4.5L8.6 5L7.5 5.5L7 6.6L6.5 5.5L5.4 5Z"
        fill="#CFFAFE"
        opacity=".8"
      />
    </svg>
  );

  if (!chip) return svg;
  return (
    <span className="flex items-center rounded-sm bg-black px-0.5 py-0.5">{svg}</span>
  );
}
