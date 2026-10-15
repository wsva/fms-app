"use client";

import { useCallback, useEffect, useState } from "react";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { Play, Trash2, History } from "lucide-react";
import type { ReadAttempt, ReadText } from "@/lib/read_aloud/types";
import { scoreBadgeClasses } from "@/lib/read_aloud/types";
import ConfirmDialog, { type ConfirmRequest } from "@/components/read_book/ConfirmDialog";
import { isTauri } from "@/lib/tauri";
import { logError } from "@/lib/logger";

const LOG_MODULE = "read-aloud";

type Props = {
  datasetUuid: string;
  text: ReadText;
  /** Called after a deletion so the caller can refresh best score / counts. */
  onChanged?: () => void;
};

function playUrl(url?: string | null) {
  if (!url) return;
  new Audio(convertFileSrc(url)).play().catch(() => {});
}

function formatWhen(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

/**
 * A dedicated page listing every recorded attempt at one text: score, the STT
 * transcript, playback and deletion. Reached from the practice view's overflow
 * menu ("Show Attempts") so the practice pane itself stays focused on reading.
 */
export default function AttemptsPanel({ datasetUuid, text, onChanged }: Props) {
  const [attempts, setAttempts] = useState<ReadAttempt[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);

  const loadAttempts = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const rows = await invoke<ReadAttempt[]>("read_aloud_list_attempts", {
        datasetUuid,
        textUuid: text.uuid,
      });
      setAttempts(rows);
    } catch (e) {
      logError(`list attempts failed: ${String(e)}`, LOG_MODULE);
    }
  }, [datasetUuid, text.uuid]);

  useEffect(() => {
    setError(null);
    loadAttempts();
  }, [loadAttempts]);

  const deleteAttempt = (a: ReadAttempt) => {
    setConfirm({
      title: "Delete attempt",
      message: "Delete this recording and its score? This cannot be undone.",
      confirmLabel: "Delete",
      onConfirm: async () => {
        try {
          await invoke("read_aloud_delete_attempt", { datasetUuid, uuid: a.uuid });
          await loadAttempts();
          onChanged?.();
        } catch (e) {
          setError(String(e));
        }
      },
    });
  };

  return (
    <div className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden p-4 flex flex-col gap-3 min-w-0">
      <section className="flex flex-col gap-1 min-w-0">
        <h2 className="text-lg font-semibold text-text-primary break-words flex items-center gap-2">
          <History size={18} className="text-text-tertiary shrink-0" />
          Attempts
        </h2>
        <p className="text-xs text-text-tertiary break-words">
          {text.title || "Untitled"} · {attempts.length} attempt{attempts.length === 1 ? "" : "s"}
        </p>
      </section>

      {error && (
        <div className="p-3 rounded-lg bg-error-bg text-error-text text-sm break-words">{error}</div>
      )}

      {attempts.length === 0 ? (
        <p className="text-xs text-text-tertiary">
          No attempts yet. Record yourself reading in the practice view.
        </p>
      ) : (
        <ul className="flex flex-col gap-1.5">
          {attempts.map((a) => (
            <li
              key={a.uuid}
              className="flex items-center gap-2 p-2 rounded-lg border border-border-default bg-bg-card"
            >
              <span className={scoreBadgeClasses(a.score)}>{Math.round(a.score)}%</span>
              <div className="flex flex-col min-w-0 flex-1">
                <span className="text-xs text-text-secondary truncate" title={a.recognized}>
                  {a.recognized || "(no speech recognized)"}
                </span>
                <span className="text-[10px] text-text-tertiary tabular-nums">
                  {formatWhen(a.created_at)}
                </span>
              </div>
              {a.audio_url && (
                <button
                  onClick={() => playUrl(a.audio_url)}
                  title="Play recording"
                  className="p-1 rounded text-text-tertiary hover:text-text-primary hover:bg-bg-hover cursor-pointer shrink-0"
                >
                  <Play size={15} />
                </button>
              )}
              <button
                onClick={() => deleteAttempt(a)}
                title="Delete attempt"
                className="p-1 rounded text-text-tertiary hover:text-error-text hover:bg-bg-hover cursor-pointer shrink-0"
              >
                <Trash2 size={15} />
              </button>
            </li>
          ))}
        </ul>
      )}

      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}
