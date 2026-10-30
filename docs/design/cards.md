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
   or `tabIndex`. A card that opens something renders one `<button>` covering its
   main area; the description and chips live inside that button, but nothing
   interactive does.
2. **Every action is a sibling of the open button.** A pencil, a menu, a
   download: outside the button, in the card's own flex row. A control nested
   inside the open button is a button inside a button — invalid HTML, unreachable
   by keyboard, and it fires the open handler as a side effect of clicking it.

## In-place editing (flip)

A card may turn over to a form rather than opening a dialog, so the object the
user edits stays the object they clicked. The mechanism lives with its first
consumer, `DatasetCard`; promote it to `ui/card` when a second family needs it.

- The two faces are absolutely positioned inside a `preserve-3d` wrapper that
  rotates 180°; the wrapper's height transitions between two fixed per-density
  tokens, so nothing is measured at runtime.
- The hidden face is `inert`, so tab order never enters it.
- Entry is a pencil on the front; Esc cancels, Enter submits, a failed save keeps
  the back face open with an inline error.
- `prefers-reduced-motion` drops the rotation and keeps the cross-fade.
