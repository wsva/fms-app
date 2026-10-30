"use client";

import { useState, type FormEvent } from "react";
import { Loader2, X } from "lucide-react";

/** The settable slice of `info.json` a card's back face edits. */
export type DatasetPatch = {
  name: string;
  description: string;
  /** Only present when the card was given a language to edit. */
  language?: string;
  /** Only present when the card was given a visibility to edit. */
  visibility?: string;
};

type Props = {
  variant: "row" | "card";
  name: string;
  description: string;
  language?: string;
  visibility?: string;
  /** A downloaded base under the download root: saving forks a writable overlay. */
  readOnly?: boolean;
  onCancel: () => void;
  /** Rejecting keeps the form open with the message inline; resolving flips back. */
  onSave: (patch: DatasetPatch) => Promise<void>;
};

const label = "text-[10px] font-medium uppercase tracking-wide text-text-tertiary";
const field =
  "w-full rounded-md border border-border-default bg-bg-input px-2 py-1 text-sm text-text-primary outline-none focus:border-accent";

/**
 * The back face of a {@link DatasetCard}: the four settable `info.json` fields.
 *
 * It is mounted only while the card is flipped, so flipping back discards the
 * draft and flipping again starts from what is on disk — no reset effect, no
 * "stale draft" state to reason about. The save itself belongs to the card,
 * which is what flips on success.
 */
export default function DatasetEditForm({
  variant,
  name,
  description,
  language,
  visibility,
  readOnly = false,
  onCancel,
  onSave,
}: Props) {
  const [draftName, setDraftName] = useState(name);
  const [draftDescription, setDraftDescription] = useState(description);
  const [draftLanguage, setDraftLanguage] = useState(language ?? "");
  const [draftVisibility, setDraftVisibility] = useState(visibility ?? "private");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy) return;
    if (!draftName.trim()) {
      setError("Name is required.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onSave({
        name: draftName.trim(),
        description: draftDescription.trim(),
        // Absent means "this page has no such field", which is different from
        // empty ("clear it") — the command reads an absent value as unchanged.
        ...(language !== undefined ? { language: draftLanguage.trim() } : {}),
        ...(visibility !== undefined ? { visibility: draftVisibility } : {}),
      });
    } catch (err) {
      setError(String(err));
      setBusy(false);
    }
  };

  return (
    <form
      onSubmit={submit}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          onCancel();
        }
      }}
      className="flex h-full flex-col gap-1.5"
    >
      <div className="flex items-center justify-between gap-2">
        <span className="truncate text-xs font-semibold text-text-primary">Edit dataset</span>
        <button
          type="button"
          onClick={onCancel}
          disabled={busy}
          aria-label="Cancel editing"
          className="shrink-0 rounded p-0.5 text-text-tertiary hover:bg-bg-hover hover:text-text-primary cursor-pointer disabled:opacity-50"
        >
          <X size={13} />
        </button>
      </div>

      <label className="flex flex-col gap-0.5">
        <span className={label}>Name</span>
        <input
          className={field}
          value={draftName}
          onChange={(e) => setDraftName(e.target.value)}
          autoFocus
          spellCheck={false}
        />
      </label>

      <label className="flex min-h-0 flex-1 flex-col gap-0.5">
        <span className={label}>Description</span>
        <textarea
          className={`${field} min-h-0 flex-1 resize-none`}
          value={draftDescription}
          onChange={(e) => setDraftDescription(e.target.value)}
        />
      </label>

      {(language !== undefined || visibility !== undefined) && (
        <div className="flex items-end gap-1.5">
          {language !== undefined && (
            <label className="flex min-w-0 flex-1 flex-col gap-0.5">
              <span className={label}>Language</span>
              <input
                className={field}
                value={draftLanguage}
                onChange={(e) => setDraftLanguage(e.target.value)}
                placeholder="de"
                spellCheck={false}
              />
            </label>
          )}
          {visibility !== undefined && (
            <label className="flex min-w-0 flex-1 flex-col gap-0.5">
              <span className={label}>Visibility</span>
              <select
                className={field}
                value={draftVisibility}
                onChange={(e) => setDraftVisibility(e.target.value)}
              >
                <option value="private">private</option>
                <option value="shared">shared</option>
                <option value="public">public</option>
              </select>
            </label>
          )}
        </div>
      )}

      {/* A downloaded base is read-only on disk; the save forks a writable overlay
          (dataset-layers.md §6) rather than failing, so say so before it happens. */}
      {readOnly && (
        <p className="text-[10px] leading-tight text-text-tertiary">
          Downloaded copy — saving creates your own editable copy.
        </p>
      )}

      {error && (
        <p
          className={`text-[10px] leading-tight break-words text-error-text ${variant === "card" ? "line-clamp-2" : "truncate"}`}
          title={error}
        >
          {error}
        </p>
      )}

      <div className="flex items-center justify-end gap-1.5">
        <button
          type="button"
          onClick={onCancel}
          disabled={busy}
          className="rounded-md px-2 py-1 text-xs text-text-secondary hover:bg-bg-hover cursor-pointer disabled:opacity-50"
        >
          Cancel
        </button>
        <button
          type="submit"
          disabled={busy}
          className="inline-flex items-center gap-1 rounded-md bg-accent-bg px-2.5 py-1 text-xs font-medium text-white hover:bg-accent-bg-hover cursor-pointer disabled:opacity-60"
        >
          {busy && <Loader2 size={12} className="animate-spin" />}
          {busy ? "Saving…" : "Save"}
        </button>
      </div>
    </form>
  );
}
