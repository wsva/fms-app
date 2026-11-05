"use client";

import { useState, type ComponentType, type ReactNode } from "react";
import { Pencil } from "lucide-react";
import Card from "@/components/ui/card/Card";
import { cn } from "@/lib/ui/cn";
import DatasetEditForm, { type DatasetPatch } from "./DatasetEditForm";

// One constant per type slice — the app's card type scale (docs/design/cards.md).
// They live here rather than in `Card` because L1 deliberately owns only the box.
const TITLE = "truncate text-sm font-semibold text-text-primary";
const DESC = "text-xs text-text-secondary";
const META = "truncate text-xs text-text-tertiary";

/** Fixed heights, so the flip animates without measuring anything at runtime. */
const HEIGHT: Record<Props["variant"], { front: string; back: string }> = {
  row: { front: "h-[76px]", back: "h-[268px]" },
  card: { front: "h-[148px]", back: "h-[288px]" },
};

type Props = {
  /** `row` for dense lists, `card` for a grid. */
  variant: "row" | "card";
  /** The dataset type's glyph, decorative — a lucide icon or the app's MicGlyph.
   *  Typed by what is rendered here (`size`/`className`), not by lucide's wrapper. */
  icon: ComponentType<{ size?: number; className?: string }>;
  name: string;
  description: string;
  /** Rendered on the back face only when supplied — pages pass what their DTO has. */
  language?: string;
  visibility?: string;
  /** Page-owned badges: readiness, download state, counts. Rendered as-is. */
  chips?: ReactNode;
  /** A line of tertiary text, page-owned (a count, a path). */
  meta?: string;
  /** A downloaded base: the back face says saving forks an editable copy. */
  readOnly?: boolean;
  disabled?: boolean;
  /** Tooltip on the front face — page-owned context (a path, why it is disabled). */
  title?: string;
  onOpen?: () => void;
  /** Omit to make the card read-only (no pencil). */
  onSave?: (patch: DatasetPatch) => Promise<void>;
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
  readOnly = false,
  disabled = false,
  title,
  onOpen,
  onSave,
}: Props) {
  const [flipped, setFlipped] = useState(false);
  const height = HEIGHT[variant];
  const editable = !!onSave && !disabled;

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
        flipped ? height.back : height.front
      )}
    >
      <div
        className="flip-face absolute inset-0 duration-200"
        data-flipped={flipped}
        style={{ transform: flipped ? "rotateY(180deg)" : "rotateY(0deg)" }}
      >
        {/* Front */}
        <div
          inert={flipped}
          title={title}
          className={cn(
            "flip-face-side flip-front absolute inset-0 isolate",
            variant === "row" ? "flex items-center gap-2 p-3" : "flex flex-col gap-1.5 p-4"
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
