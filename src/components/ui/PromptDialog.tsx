"use client";

import { useState } from "react";
import { X } from "lucide-react";

type Props = {
  title: string;
  /** Label above the input; defaults to the title. */
  label?: string;
  placeholder?: string;
  /** Giving this adds the second, multiline field — omit it for a one-field prompt. */
  description?: { label: string; placeholder?: string };
  submitLabel?: string;
  onCancel: () => void;
  /** Called with the trimmed values; `description` is "" when the field isn't shown. */
  onSubmit: (name: string, description: string) => void | Promise<void>;
};

/**
 * Themed modal for the "ask for a name" flows — the in-app replacement for the
 * browser's native prompt(), which shows an OS-styled dialog the theme cannot
 * reach.
 *
 * Deliberately value-only: the caller owns what the submitted strings do, so one
 * component serves both the single-field prompts (new tag) and the name-plus-
 * description ones (new dataset). Same overlay/panel styling as the read-aloud
 * DatasetDialog.
 */
export default function PromptDialog({
  title,
  label,
  placeholder,
  description,
  submitLabel = "Create",
  onCancel,
  onSubmit,
}: Props) {
  const [value, setValue] = useState("");
  const [descriptionValue, setDescriptionValue] = useState("");
  const [busy, setBusy] = useState(false);

  const trimmed = value.trim();
  const submit = async () => {
    if (!trimmed || busy) return;
    setBusy(true);
    try {
      await onSubmit(trimmed, descriptionValue.trim());
    } finally {
      setBusy(false);
    }
  };

  const field =
    "w-full p-2 rounded-md bg-bg-input border border-border-default text-text-primary text-sm focus:outline-none focus:border-accent-bg";

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4" onClick={onCancel}>
      <div
        className="bg-bg-card rounded-2xl shadow-2xl w-full max-w-md flex flex-col gap-3 p-5 border border-border-default"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between">
          <h2 className="text-base font-semibold text-text-primary">{title}</h2>
          <button
            onClick={onCancel}
            className="p-1 rounded text-text-tertiary hover:text-text-primary hover:bg-bg-hover cursor-pointer"
            aria-label="Close"
          >
            <X size={18} />
          </button>
        </div>

        <form
          onSubmit={(e) => {
            e.preventDefault();
            submit();
          }}
          className="flex flex-col gap-3"
        >
          <div>
            <label className="text-xs font-medium text-text-secondary mb-1 block">{label ?? title}</label>
            <input
              className={field}
              value={value}
              onChange={(e) => setValue(e.target.value)}
              placeholder={placeholder}
              autoFocus
            />
          </div>

          {description && (
            <div>
              <label className="text-xs font-medium text-text-secondary mb-1 block">
                {description.label} (optional)
              </label>
              <textarea
                className={`${field} min-h-[70px] resize-y`}
                value={descriptionValue}
                onChange={(e) => setDescriptionValue(e.target.value)}
                placeholder={description.placeholder}
              />
            </div>
          )}

          <div className="flex justify-end gap-2 mt-1">
            <button
              type="button"
              onClick={onCancel}
              className="px-3 py-2 rounded-md text-sm text-text-secondary hover:bg-bg-hover cursor-pointer"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={!trimmed || busy}
              className="px-4 py-2 rounded-md text-sm bg-accent-bg text-white hover:bg-accent-bg-hover cursor-pointer disabled:opacity-60 disabled:cursor-not-allowed"
            >
              {submitLabel}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
