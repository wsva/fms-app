"use client";

import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { X } from "lucide-react";
import type { ReadText } from "@/lib/read_aloud/types";
import { isTauri } from "@/lib/tauri";
import { logError } from "@/lib/logger";

const LOG_MODULE = "read-aloud";

function getUUID(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) return crypto.randomUUID();
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    const v = c === "x" ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
}

function nowIso(): string {
  return new Date().toISOString();
}

type Props = {
  datasetUuid: string;
  /** Existing text to edit, or null to create a new one. */
  text: ReadText | null;
  /** order_num assigned to a newly created text. */
  newOrderNum: number;
  onClose: () => void;
  onSaved: () => void;
};

/** Modal editor for a single read-aloud text (title, note, content). */
export default function TextEditor({ datasetUuid, text, newOrderNum, onClose, onSaved }: Props) {
  const [title, setTitle] = useState(text?.title ?? "");
  const [note, setNote] = useState(text?.note ?? "");
  const [content, setContent] = useState(text?.content ?? "");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const field =
    "w-full p-2 rounded-md bg-bg-muted border border-border-default text-text-primary text-sm focus:outline-none focus:border-accent-bg";
  const label = "text-xs font-medium text-text-secondary mb-1 block";

  const save = async () => {
    if (!isTauri()) return;
    if (!content.trim()) {
      setError("The text content must not be empty.");
      return;
    }
    setSaving(true);
    setError(null);
    const now = nowIso();
    const payload: ReadText = {
      uuid: text?.uuid ?? getUUID(),
      dataset_uuid: datasetUuid,
      order_num: text?.order_num ?? newOrderNum,
      title: title.trim(),
      note: note.trim(),
      content: content.trim(),
      best_score: text?.best_score ?? null,
      attempt_count: text?.attempt_count ?? 0,
      created_at: text?.created_at ?? now,
      updated_at: now,
    };
    try {
      await invoke("read_aloud_save_text", { datasetUuid, text: payload });
      onSaved();
      onClose();
    } catch (e) {
      const msg = String(e);
      logError(`save text failed: ${msg}`, LOG_MODULE);
      setError(msg);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4" onClick={onClose}>
      <div
        className="bg-bg-card rounded-2xl shadow-2xl w-full max-w-lg flex flex-col gap-3 p-5 border border-border-default max-h-[90vh] overflow-y-auto"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between">
          <h2 className="text-base font-semibold text-text-primary">
            {text ? "Edit text" : "New text"}
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
          <label className={label}>Title</label>
          <input className={field} value={title} onChange={(e) => setTitle(e.target.value)} placeholder="e.g. Der Erlkönig — first stanza" />
        </div>

        <div>
          <label className={label}>Note (source, location, …)</label>
          <input className={field} value={note} onChange={(e) => setNote(e.target.value)} placeholder="e.g. Goethe, from a textbook p.42" />
        </div>

        <div className="flex flex-col min-h-0">
          <label className={label}>Text to read</label>
          <textarea
            className={`${field} min-h-[140px] resize-y leading-relaxed`}
            value={content}
            onChange={(e) => setContent(e.target.value)}
            placeholder="Paste the passage the user will read aloud…"
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
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
}
