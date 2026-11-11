# Cards

The rule book for every card-shaped surface in the app: the geometry, the type
scale, the states, and the two structural rules that are not negotiable. The
component that implements the box is
[`src/components/ui/card/Card.tsx`](../../src/components/ui/card/Card.tsx).

## Layers

| Tier | What it owns | Where |
|---|---|---|
| L0 | Theme tokens (`--bg-card`, `--border-default`, …) and the decisions in this file | `src/app/globals.css`, `docs/design/cards.md` |
| L1 | `Card` — the box and its states. Nothing else. | `src/components/ui/card/` |
| L2 | Domain cards composed on it: `DatasetCard`, `ModelCard`, `MarketCard`, … | `src/components/ui/*/`, feature dirs |

A new card is a `Card` plus content. A page that writes its own
`border border-border-default bg-bg-card` shell is the drift this exists to stop.

## Geometry

One value each. If a surface needs a different one, it is not a card.

| Part | Value | Notes |
|---|---|---|
| Radius | `rounded-lg` | 8px. `rounded-xl` is reserved for modals, dialogs and hero surfaces (About, Workspaces, the sync status panel). |
| Edge | `border border-border-default` | `border-border-light` is for dividers *inside* a card, never for its edge. |
| Fill | `bg-bg-card` | Only for cards sitting on `bg-bg-body`. A well nested in a card uses `bg-bg-body` or `bg-bg-muted`. |
| Padding `sm` | `p-3` | Dense lists: dataset rows, model cards, status rows. |
| Padding `md` | `p-4` | Grid cards and panels: the card decks, settings sections. |
| Gap between cards | `gap-2` rows, `gap-3` grids | Same container that lists them. |
| Glyph | `size={16}`, `shrink-0 text-text-tertiary`, inside the title row | Decorative (`aria-hidden`), one per card, never two. It classifies the *name*, so it sits on the title line rather than in a left gutter beside the whole text block — a gutter reads as labelling the description, and costs that description 24px of a clamped line. |

## Type scale

| Part | Value |
|---|---|
| Title | `truncate text-sm font-semibold text-text-primary` |
| Description | `text-xs text-text-secondary`, `line-clamp-1` in row density, `line-clamp-2` in card density |
| Meta (counts, paths, timestamps) | `text-xs text-text-tertiary` |
| Chip | Page-owned for now: `text-[11px] px-2 py-0.5 rounded-md` plus a semantic token pair (`bg-success-bg`/`text-success-text`, `bg-bg-hover`/`text-text-secondary`). A shared `Chip` lands when two families need the same one. |

Two properties keep a list calm: the description is **always rendered when
non-empty** (never silently dropped per page), and a title never wraps — it
truncates.

## States

| State | Class |
|---|---|
| Interactive (hoverable) | `hover:bg-bg-hover` + `focus-within:ring-1 focus-within:ring-accent/40` |
| Selected / current | `ring-2 ring-accent` |
| Disabled | `opacity-70 cursor-not-allowed` |
| Busy | Keep the shape, swap one control for a spinner; never resize the card. |

## Two structural rules

1. **A card is a region, not a click target.** `Card` takes no `onClick`, `role`
   or `tabIndex`. A card that opens something renders one `<button>`; the
   description and chips live in its visual area, but nothing interactive does.
   That button's `::after` stretches over the whole face (the stretched-link
   pattern), so the hit area matches the `interactive` hover lift: if the card
   looks like it responds, the whole card responds. The button is still the only
   semantic target.
2. **Every action is a sibling of the open button.** A pencil, a menu, a
   download: outside the button, in the card's own flex row, above the stretched
   overlay via `relative z-10`. A control nested inside the open button is a
   button inside a button — invalid HTML, unreachable by keyboard, and it fires
   the open handler as a side effect of clicking it.

## In-place editing (flip)

A card may turn over to a form rather than opening a dialog, so the object the
user edits stays the object they clicked. The mechanism lives with its first
consumer, `DatasetCard`; promote it to `ui/card` when a second family needs it.

- The two faces are absolutely positioned inside a `preserve-3d` wrapper that
  rotates 180°; the wrapper's height transitions between fixed per-density
  tokens, so nothing is measured at runtime. A third token per density covers a
  front that carries a family strip (see below).
- Because both faces are absolute, the wrapper's min-content height is 0: it
  carries `shrink-0`, or any flex column that lists the cards squashes every row
  to a few pixels (the Read Aloud list lost its padding and clipped its names
  that way). A fixed-height card is never a flex-shrink candidate.
- The hidden face is `inert`, so tab order never enters it.
- Entry is a pencil on the front; Esc cancels, Enter submits, a failed save keeps
  the back face open with an inline error.
- `prefers-reduced-motion` drops the rotation and keeps the cross-fade.

## The Family strip

A dataset card ends with the other datasets of its family — the siblings the
`<type>_<family key>` ids relate — in a strip that is the books page's
companions sub-toolbar, moved to the card's bottom edge: the same `px-3 py-1.5`,
the same `Family` label in `text-xs text-text-tertiary`, the same `btnSmSecondary`
chip with the type's glyph and the sibling's name on one line that scrolls
sideways. Both densities carry it; only the label and the chips live in the card,
because the strip *is* the card's (a page that has a family to show passes the
members, not rows of its own).

The strip sits above the open button's stretching overlay (`z-10`), which is what
lets it scroll at all — under the overlay the wheel lands on the button. The
price is that the strip is not an open target: it reads as a band of its own,
which is how the sub-toolbar it copies reads too.

A page that has no family to show passes nothing, and the card keeps its short
height rather than reserving a strip for an empty family.

## Related

This file governs the **card component** — the box, its geometry and states. It
is not the flashcard *feature*: that lives in the Cards page. Card content
features are documented where they are designed; media attachments (images,
audio, video on a card's fields) are specified in
[`card-media.md`](./card-media.md).
