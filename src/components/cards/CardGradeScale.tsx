"use client";

// ---------------------------------------------------------------------------
// The five grades, in one row, wherever a card is rated.
//
// The review panel and the card level ask the same question of the same card,
// so the row must not differ between them: the table lives here once, and a
// grade's name, order and colour cannot drift between the two pages a learner
// rates on. A wrong label on the fourth button is not a styling bug, it is
// data — the rating lands in SM-2 either way.
//
// Two emphases, one geometry:
//   - `solid` (review): the page exists to grade cards, so the buttons should
//     be unmissable.
//   - `quiet` (card level): the reader came to read. Colour moves from the
//     fill to the icon, so the scale is findable without out-shouting the
//     card it is judging.
// ---------------------------------------------------------------------------

import type { ReactNode } from "react";
import { CircleHelp, Frown, Meh, Smile, Laugh } from "lucide-react";

/** Quality → the label the UI uses. `q` is the 0–5 value SM-2 stores. */
export const CARD_GRADES = [
  { q: 1, label: "Forgot", color: "bg-red-500", tone: "text-red-500", Icon: CircleHelp },
  { q: 2, label: "Hard", color: "bg-orange-500", tone: "text-orange-500", Icon: Frown },
  { q: 3, label: "Okay", color: "bg-amber-500", tone: "text-amber-500", Icon: Meh },
  { q: 4, label: "Good", color: "bg-cyan-500", tone: "text-cyan-500", Icon: Smile },
  { q: 5, label: "Easy", color: "bg-green-500", tone: "text-green-500", Icon: Laugh },
] as const;

/**
 * Button caption that disappears below `sm`, so a phone sees the icon alone
 * while the wide layout keeps icon + text. The owning button carries
 * `title`/`aria-label`, so the accessible name and the hover hint do not
 * depend on this span being visible.
 */
function IconLabel({ children }: { children: ReactNode }) {
  return <span className="hidden sm:inline">{children}</span>;
}

type Props = {
  onRate: (quality: number) => void;
  /** `solid` fills each button (review); `quiet` outlines it (card level). */
  variant?: "solid" | "quiet";
  disabled?: boolean;
  /** The grade just given, so the row can show what it did rather than guess. */
  chosen?: number | null;
};

export default function CardGradeScale({
  onRate,
  variant = "solid",
  disabled = false,
  chosen = null,
}: Props) {
  const solid = variant === "solid";
  return (
    <div className="flex flex-wrap items-center gap-2" role="group" aria-label="Rate this card">
      {CARD_GRADES.map((btn) => (
        <button
          key={btn.q}
          type="button"
          onClick={() => onRate(btn.q)}
          disabled={disabled}
          title={`${btn.label} (${btn.q})`}
          aria-label={btn.label}
          aria-pressed={chosen === btn.q}
          className={`flex items-center gap-1.5 px-4 py-2 min-h-11 sm:min-h-0 rounded-lg border border-border-default text-sm disabled:opacity-40 ${
            solid
              ? `${btn.color} hover:opacity-90`
              : `bg-bg-surface text-text-secondary hover:bg-bg-hover ${
                  chosen === btn.q ? "ring-1 ring-accent" : ""
                }`
          }`}
        >
          <btn.Icon size={14} className={solid ? "" : btn.tone} />
          <IconLabel>{btn.label}</IconLabel>
        </button>
      ))}
    </div>
  );
}
