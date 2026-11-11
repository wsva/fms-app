"use client";

import { useState, type ReactNode } from "react";
import { Pencil } from "lucide-react";
import Card from "@/components/ui/card/Card";
import { cn } from "@/lib/ui/cn";
import { btnSmSecondary, DATASET_TYPE_ICONS, type DatasetTypeIcon } from "@/lib/datasets/types";
import { familyTypeLabel, type FamilyMember } from "@/lib/datasets/family";
import DatasetEditForm, { type DatasetPatch } from "./DatasetEditForm";

// One constant per type slice — the app's card type scale (docs/design/cards.md).
// They live here rather than in `Card` because L1 deliberately owns only the box.
const TITLE = "truncate text-sm font-semibold text-text-primary";
const DESC = "text-xs text-text-secondary";
const META = "truncate text-xs text-text-tertiary";

/** Fixed heights, so the flip animates without measuring anything at runtime.
 *  `family` is the front's height once the card lists a family: one extra strip at
 *  the bottom edge, whose height is the companions sub-toolbar's — the same
 *  paddings, the same chip, one line that scrolls sideways. */
const HEIGHT: Record<Props["variant"], { front: string; back: string; family: string }> = {
  row: { front: "h-[76px]", back: "h-[268px]", family: "h-[116px]" },
  card: { front: "h-[148px]", back: "h-[288px]", family: "h-[188px]" },
};

type Props = {
  /** `row` for dense lists, `card` for a grid. */
  variant: "row" | "card";
  /** The dataset type's glyph — see `DATASET_TYPE_ICONS`; a lucide icon or the
   *  app's own `MicGlyph`. */
  icon: DatasetTypeIcon;
  name: string;
  description: string;
  /** Rendered on the back face only when supplied — pages pass what their DTO has. */
  language?: string;
  visibility?: string;
  /** Page-owned badges: readiness, download state, counts. Rendered as-is. */
  chips?: ReactNode;
  /** A line of tertiary text, page-owned (a count, a path). */
  meta?: string;
  /** The *other* datasets of this one's family, as `dataset_family_siblings`
   *  answers it. Rendered as a strip along the bottom edge: the books page's
   *  companions sub-toolbar, one chip per sibling. Omit it — or pass an empty
   *  list — when there is no family; the front then keeps its short height rather
   *  than reserving a strip for nothing. */
  siblings?: FamilyMember[];
  /** A downloaded base: the back face says saving forks an editable copy. */
  readOnly?: boolean;
  disabled?: boolean;
  /** Tooltip on the front face — page-owned context (a path, why it is disabled). */
  title?: string;
  onOpen?: () => void;
  /** Omit to make the card read-only (no pencil). */
  onSave?: (patch: DatasetPatch) => Promise<void>;
  /** Omit to hide the edit view's "Join family" button. */
  onJoinFamily?: () => void;
};

/**
 * One dataset, on every page that lists datasets.
 *
 * The front is the card; the pencil turns it over to a form bound to `info.json`
 * instead of opening a dialog, so the object being edited stays the object that
 * was clicked. The flip, the geometry and the typography live here; the shell
 * comes from `Card` (src/components/ui/card) — see docs/design/cards.md.
 *
 * Structure rule: the root is a region, not a click target. The open target is
 * one `<button>`, and the pencil is its sibling — never its child, which would
 * be a button inside a button. The button's own box is only the glyph and text,
 * but its `::after` stretches over the whole front face (the stretched-link
 * pattern), so the hit area matches the affordance: the card lifts on hover as
 * a whole, so the whole face opens it. The pencil sits above that overlay with
 * `relative z-10` and keeps its own action. See docs/design/cards.md.
 */

// The open button's stretching overlay: its `::after` covers the front face,
// which is `absolute inset-0`, so the whole card — padding, chips, empty space —
// responds. `relative` makes the button the containing block; `z-0` keeps it
// above the face's static siblings while the face's `isolate` caps the ladder,
// so the overlay never lands on top of a neighbouring card. Actions (the
// pencil) and the badges opt back above it with `relative z-10`.
const OPEN = "relative z-0 after:absolute after:inset-0 cursor-pointer";
export default function DatasetCard({
  variant,
  icon: Icon,
  name,
  description,
  language,
  visibility,
  chips,
  meta,
  siblings,
  readOnly = false,
  disabled = false,
  title,
  onOpen,
  onSave,
  onJoinFamily,
}: Props) {
  const [flipped, setFlipped] = useState(false);
  const height = HEIGHT[variant];
  const editable = !!onSave && !disabled;
  const family = siblings && siblings.length > 0 ? siblings : null;

  // The glyph belongs to the name, not to the block under it: it sits on the
  // title line, and the description and meta keep the full content width — in
  // `row` density that is 24px more of a single clamped line. See
  // docs/design/cards.md.
  const main = (
    <span className="flex min-w-0 flex-col gap-0.5">
      {/* `items-center` on a (default) row centres the 16px glyph on the
          name's 20px line box — the vertical axis. `justify-center` would be the
          horizontal one, and would stop the name being left-aligned and
          truncating. Kept geometric on purpose: the glyph's ink and the text's
          ink disagree by a pixel either way, and nobody can nudge that
          consistently across four pages. */}
      <span className="flex min-w-0 items-center gap-2">
        <Icon size={16} className="shrink-0 text-text-tertiary" aria-hidden />
        <span className={TITLE}>{name}</span>
      </span>
      {description && (
        <span className={cn(DESC, variant === "row" ? "line-clamp-1" : "line-clamp-2")}>
          {description}
        </span>
      )}
      {meta && <span className={META}>{meta}</span>}
    </span>
  );

  return (
    <Card
      interactive={!!onOpen && !disabled}
      disabled={disabled}
      className={cn(
        // `shrink-0` is load-bearing: both faces are absolutely positioned, so
        // the card's min-content height is 0 and a scroll column of them will
        // happily squash every row to a few pixels — which reads as a card with
        // no vertical padding and names clipped top and bottom. The height is
        // fixed per density, so it must never be a flex-shrink candidate.
        "flip-perspective relative shrink-0 overflow-hidden p-0",
        "transition-[height,background-color] duration-200",
        flipped ? height.back : family ? height.family : height.front
      )}
    >
      <div
        className="flip-face absolute inset-0 duration-200"
        data-flipped={flipped}
        style={{ transform: flipped ? "rotateY(180deg)" : "rotateY(0deg)" }}
      >
        {/* Front — a column whose last row is the family strip, when there is a
            family: the strip is full-bleed like the companions sub-toolbar it
            copies, so the padding sits on the content above it. */}
        <div
          inert={flipped}
          title={title}
          className="flip-face-side flip-front absolute inset-0 isolate flex flex-col"
        >
          <div
            className={cn(
              "flex min-w-0 flex-1",
              variant === "row" ? "items-center gap-2 p-3" : "flex-col gap-1.5 p-4"
            )}
          >
            {variant === "row" ? (
              <>
                <button
                  type="button"
                  onClick={onOpen}
                  disabled={!onOpen || disabled}
                  className={cn(
                    OPEN,
                    "flex min-w-0 flex-1 items-center gap-2 text-left disabled:cursor-not-allowed"
                  )}
                >
                  {main}
                </button>
                {chips && (
                  <div className="relative z-10 flex shrink-0 items-center gap-1.5">{chips}</div>
                )}
                {editable && <PencilButton name={name} onClick={() => setFlipped(true)} />}
              </>
            ) : (
              <>
                <div className="flex min-w-0 items-start gap-2">
                  <button
                    type="button"
                    onClick={onOpen}
                    disabled={!onOpen || disabled}
                    className={cn(
                      OPEN,
                      "flex min-w-0 flex-1 items-start gap-2 text-left disabled:cursor-not-allowed"
                    )}
                  >
                    {main}
                  </button>
                  {editable && <PencilButton name={name} onClick={() => setFlipped(true)} />}
                </div>
                {chips && (
                  <div className="relative z-10 mt-auto flex flex-wrap items-center gap-1.5">
                    {chips}
                  </div>
                )}
              </>
            )}
          </div>
          {family && <FamilyStrip members={family} />}
        </div>

        {/* Back — mounted only while flipped, so the draft cannot go stale. */}
        <div
          inert={!flipped}
          className="flip-face-side flip-back absolute inset-0 p-3"
          style={{ transform: "rotateY(180deg)" }}
        >
          {flipped && onSave && (
            <DatasetEditForm
              variant={variant}
              name={name}
              description={description}
              language={language}
              visibility={visibility}
              readOnly={readOnly}
              onCancel={() => setFlipped(false)}
              onJoinFamily={onJoinFamily}
              onSave={async (patch) => {
                await onSave(patch);
                setFlipped(false);
              }}
            />
          )}
        </div>
      </div>
    </Card>
  );
}

/**
 * The other datasets of this one's family, as the books page's companions
 * sub-toolbar draws them: a label and one chip per sibling, on one line that
 * scrolls sideways when a family is wide. The chips carry the type's glyph only
 * (the sub-toolbar does the same), so a family of four still fits.
 *
 * `z-10` puts the strip above the open button's stretching overlay, which is
 * what lets it scroll at all: under the overlay the wheel would land on the
 * button. The price is that the strip is not an open target — it reads as a band
 * of its own, like the sub-toolbar it copies.
 */
function FamilyStrip({ members }: { members: FamilyMember[] }) {
  return (
    <div className="no-scrollbar relative z-10 flex shrink-0 items-center gap-1.5 overflow-x-auto border-t border-border-default px-3 py-1.5">
      <span className="shrink-0 text-xs text-text-tertiary">Family</span>
      {members.map((member) => {
        const Icon = DATASET_TYPE_ICONS[member.dataset_type];
        return (
          <span
            key={member.uuid}
            title={`${familyTypeLabel(member.dataset_type)} · ${member.name}`}
            className={cn(btnSmSecondary, "flex shrink-0 items-center gap-1.5")}
          >
            <Icon size={13} className="shrink-0 opacity-70" aria-hidden />
            <span className="max-w-[140px] truncate">{member.name}</span>
          </span>
        );
      })}
    </div>
  );
}

function PencilButton({ name, onClick }: { name: string; onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={`Edit ${name}`}
      title="Edit name, description, language and visibility"
      className="relative z-10 shrink-0 rounded p-1 text-text-tertiary hover:bg-bg-hover hover:text-text-primary cursor-pointer"
    >
      <Pencil size={14} />
    </button>
  );
}
