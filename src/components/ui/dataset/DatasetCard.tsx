"use client";

import { useState, type ReactNode } from "react";
import { Pencil } from "lucide-react";
import type { LucideIcon } from "lucide-react";
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
  row: { front: "h-[76px]", back: "h-[220px]" },
  card: { front: "h-[148px]", back: "h-[268px]" },
};

type Props = {
  /** `row` for dense lists, `card` for a grid. */
  variant: "row" | "card";
  /** The dataset type's glyph (Headphones, Layers, BookOpen, Mic), decorative. */
  icon: LucideIcon;
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
 * one `<button>` covering the glyph and text, and the pencil is its sibling —
 * never its child, which would be a button inside a button.
 */
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

  const main = (
    <>
      <Icon size={16} className="shrink-0 text-text-tertiary" aria-hidden />
      <span className="flex min-w-0 flex-col gap-0.5">
        <span className={TITLE}>{name}</span>
        {description && (
          <span className={cn(DESC, variant === "row" ? "line-clamp-1" : "line-clamp-2")}>
            {description}
          </span>
        )}
        {meta && <span className={META}>{meta}</span>}
      </span>
    </>
  );

  return (
    <Card
      interactive={!!onOpen && !disabled}
      disabled={disabled}
      className={cn(
        "flip-perspective relative overflow-hidden p-0",
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
            "flip-face-side flip-front absolute inset-0",
            variant === "row" ? "flex items-center gap-2 p-3" : "flex flex-col gap-1.5 p-4"
          )}
        >
          {variant === "row" ? (
            <>
              <button
                type="button"
                onClick={onOpen}
                disabled={!onOpen || disabled}
                className="flex min-w-0 flex-1 items-center gap-2 text-left disabled:cursor-not-allowed"
              >
                {main}
              </button>
              {chips && <div className="flex shrink-0 items-center gap-1.5">{chips}</div>}
              {editable && <PencilButton name={name} onClick={() => setFlipped(true)} />}
            </>
          ) : (
            <>
              <div className="flex min-w-0 items-start gap-2">
                <button
                  type="button"
                  onClick={onOpen}
                  disabled={!onOpen || disabled}
                  className="flex min-w-0 flex-1 items-start gap-2 text-left disabled:cursor-not-allowed"
                >
                  {main}
                </button>
                {editable && <PencilButton name={name} onClick={() => setFlipped(true)} />}
              </div>
              {chips && <div className="mt-auto flex flex-wrap items-center gap-1.5">{chips}</div>}
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
      className="shrink-0 rounded p-1 text-text-tertiary hover:bg-bg-hover hover:text-text-primary cursor-pointer"
    >
      <Pencil size={14} />
    </button>
  );
}
