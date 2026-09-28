"use client";

import type { ReactNode } from "react";
import { Loader2, Play } from "lucide-react";
import { btnSmPrimary } from "@/lib/datasets/types";

interface StageRowProps {
  title: string;
  description?: string;
  /** Disable the Run button (e.g. while another stage is running). */
  disabled?: boolean;
  /** This stage is currently running. */
  running?: boolean;
  /** When set, the stage is gated: show this hint instead of an enabled button. */
  hint?: string;
  runLabel?: string;
  onRun?: () => void;
  /** Optional content rendered before the Run button. */
  beforeRun?: ReactNode;
  /** Parameter fields rendered below the header. */
  children?: ReactNode;
}

export default function StageRow({
  title,
  description,
  disabled,
  running,
  hint,
  runLabel = "Run",
  onRun,
  beforeRun,
  children,
}: StageRowProps) {
  const gated = !!hint;
  return (
    <div className="border border-border-default rounded-lg p-3 flex flex-col gap-2">
      <div className="flex items-center justify-between gap-3">
        <div className="flex flex-col min-w-0">
          <span className="text-sm font-medium text-text-primary truncate">{title}</span>
          {description && (
            <span className="text-xs text-text-tertiary truncate" title={description}>
              {description}
            </span>
          )}
        </div>
        <div className="flex items-center gap-2 shrink-0">
          {beforeRun}
          <button
            className={`${btnSmPrimary} inline-flex items-center gap-1`}
            disabled={gated || disabled || running || !onRun}
            onClick={onRun}
            title={gated ? hint : undefined}
          >
            {running ? <Loader2 size={14} className="animate-spin" /> : <Play size={14} />}
            {runLabel}
          </button>
        </div>
      </div>
      {gated && <p className="text-xs text-text-tertiary">{hint}</p>}
      {children && <div className="flex flex-col gap-2">{children}</div>}
    </div>
  );
}
