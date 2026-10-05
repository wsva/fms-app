"use client";

import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { X } from "lucide-react";
import type { ReadAloudMeta } from "@/lib/read_aloud/types";
import { isTauri } from "@/lib/tauri";
import { logError } from "@/lib/logger";

const LOG_MODULE = "read-aloud";

export type DatasetDialogState =
  | { mode: "create" }
  | { mode: "edit"; meta: ReadAloudMeta };

type Props = {
  state: DatasetDialogState;
  onClose: () => void;
  /** Called after a successful save with the affected dataset UUID. */
  onSaved: (uuid?: string) => void;
};

/** Modal to create or rename/describe a read-aloud dataset. */
export default function DatasetDialog({ state, onClose, onSaved }: Props) {
  const editing = state.mode === "edit" ? state.meta : null;
  const [name, setName] = useState(editing?.name ?? "");
  const [description, setDescription] = useState(editing?.description ?? "");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const field =
    "w-full p-2 rounded-md bg-bg-muted border border-border-default text-text-primary text-sm focus:outline-none focus:border-accent-bg";
  const label = "text-xs font-medium text-text-secondary mb-1 block";

  const save = async () => {
    if (!isTauri()) return;
    if (!name.trim()) {
      setError("Name is required.");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      if (editing) {
        await invoke("read_aloud_update", {
          uuid: editing.uuid,
          name: name.trim(),
          description: description.trim(),
        });
        onSaved(editing.uuid);
      } else {
        const created = await invoke<ReadAloudMeta>("read_aloud_create", {
          name: name.trim(),
          description: description.trim(),
        });
        onSaved(created.uuid);
      }
      onClose();
    } catch (e) {
      const msg = String(e);
      logError(`save dataset failed: ${msg}`, LOG_MODULE);
      setError(msg);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4" onClick={onClose}>
      <div
        className="bg-bg-card rounded-2xl shadow-2xl w-full max-w-md flex flex-col gap-3 p-5 border border-border-default"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between">
          <h2 className="text-base font-semibold text-text-primary">
            {editing ? "Edit dataset" : "New read-aloud dataset"}
          </h2>
          <button
            onClick={onClose}
            className="p-1 rounded text-text-tertiary hover:text-text-primary hover:bg-bg-hover cursor-pointer"
            aria-label="Close"
          >
            <X size={18} />
          </button>
        </div>

        <div>
          <label className={label}>Name</label>
          <input
            className={field}
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="e.g. German A2 passages"
            autoFocus
          />
        </div>

        <div>
          <label className={label}>Description (optional)</label>
          <textarea
            className={`${field} min-h-[70px] resize-y`}
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="What is this collection about?"
          />
        </div>

        {error && <div className="p-2 rounded-md bg-error-bg text-error-text text-xs break-words">{error}</div>}

        <div className="flex justify-end gap-2 mt-1">
          <button
            onClick={onClose}
            className="px-3 py-2 rounded-md text-sm text-text-secondary hover:bg-bg-hover cursor-pointer"
          >
            Cancel
          </button>
          <button
            onClick={save}
            disabled={saving}
            className="px-4 py-2 rounded-md text-sm bg-accent-bg text-white hover:bg-accent-bg-hover cursor-pointer disabled:opacity-60 disabled:cursor-not-allowed"
          >
            {saving ? "Saving…" : editing ? "Save" : "Create"}
          </button>
        </div>
      </div>
    </div>
  );
}
