"use client";

import { type ReactNode } from "react";
import { cn } from "@/lib/ui/cn";

// ---------------------------------------------------------------------------
// The card *shell* — nothing more.
//
// A card in this app is: `rounded-lg border border-border-default bg-bg-card`
// sitting on `bg-bg-body`, with one padding scale and one set of states. That is
// the whole contract (see docs/design/cards.md). Titles, descriptions, icons,
// chips and actions belong to the domain card composed on top (DatasetCard,
// ModelCard, MarketCard…), so they can differ while the box cannot.
//
// The rule this component exists to protect: **a card is a region, not a click
// target.** There is deliberately no `onClick`, `role` or `tabIndex` here. A card
// that opens something puts one `<button>` inside it covering its main area, and
// puts every action (a pencil, a menu, a download) as a *sibling* of that button.
// Nesting an action inside the open button produces a button inside a button,
// which is invalid HTML and traps keyboard users.
// ---------------------------------------------------------------------------

export type CardVariant =
  /** On `bg-bg-body`: the normal card surface. */
  | "surface"
  /** A well sitting inside another card. */
  | "subtle"
  /** No fill — a bordered or floating region (ModelCard's download state, overlays). */
  | "plain";

export type CardProps = {
  variant?: CardVariant;
  /** `sm` = p-3 (dense lists), `md` = p-4 (grid cards, panels). */
  density?: "sm" | "md";
  /** Adds the hover surface and a focus ring; does NOT make the card clickable. */
  interactive?: boolean;
  /** `ring-2 ring-accent` — the one "this is the current one" treatment. */
  selected?: boolean;
  disabled?: boolean;
  /** Semantic element for lists and landmarks. Defaults to `div`. */
  as?: "div" | "li" | "article" | "section";
  className?: string;
  children: ReactNode;
};

const VARIANT_CLASS: Record<CardVariant, string> = {
  surface: "bg-bg-card border border-border-default",
  subtle: "bg-bg-muted border border-border-light",
  plain: "border border-border-default",
};

const DENSITY_CLASS: Record<NonNullable<CardProps["density"]>, string> = {
  sm: "p-3",
  md: "p-4",
};

export default function Card({
  variant = "surface",
  density = "sm",
  interactive = false,
  selected = false,
  disabled = false,
  as: Tag = "div",
  className,
  children,
}: CardProps) {
  return (
    <Tag
      className={cn(
        "rounded-lg transition-colors",
        VARIANT_CLASS[variant],
        DENSITY_CLASS[density],
        // Only the look of interactivity — see the doc comment above.
        interactive && !disabled && "hover:bg-bg-hover focus-within:ring-1 focus-within:ring-accent/40",
        selected && "ring-2 ring-accent",
        disabled && "opacity-70 cursor-not-allowed",
        className
      )}
    >
      {children}
    </Tag>
  );
}
