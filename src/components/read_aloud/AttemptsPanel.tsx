"use client";

import { useCallback, useEffect, useState } from "react";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { Play, Trash2, History, ChevronRight } from "lucide-react";
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
  /** The attempt whose detail is shown, controlled by the page so the
   *  breadcrumb can reflect it as a level. null → show the list. */
  selectedUuid: string | null;
  onSelectUuid: (uuid: string | null) => void;
  /** Reports the resolved selected attempt (or null) upward so the page can
   *  label the attempt crumb. */
  onSelectAttempt?: (attempt: ReadAttempt | null) => void;
  /** Bumped by the page after it deletes the open attempt (the toolbar owns
   *  that action) so the list reloads. */
  refreshKey?: number;
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
 * A dedicated page listing every recorded attempt at one text. Clicking an
 * attempt opens a detail panel that shows the full recognized text at reading
 * size. Reached from the practice view's toolbar ("Attempts").
 */
export default function AttemptsPanel({
  datasetUuid,
  text,
  onChanged,
  selectedUuid,
  onSelectUuid,
  onSelectAttempt,
  refreshKey,
}: Props) {
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
  }, [loadAttempts, refreshKey]);

  const deleteAttempt = (a: ReadAttempt) => {
    setConfirm({
      title: "Delete attempt",
      message: "Delete this recording and its score? This cannot be undone.",
      confirmLabel: "Delete",
      onConfirm: async () => {
        try {
          await invoke("read_aloud_delete_attempt", { datasetUuid, uuid: a.uuid });
          // Leaving the detail panel if the attempt being viewed was deleted.
          if (selectedUuid === a.uuid) onSelectUuid(null);
          await loadAttempts();
          onChanged?.();
        } catch (e) {
          setError(String(e));
        }
      },
    });
  };

  const selectedAttempt = attempts.find((a) => a.uuid === selectedUuid) ?? null;

  // Keep the page's breadcrumb crumb in sync with the resolved attempt.
  useEffect(() => {
    onSelectAttempt?.(selectedAttempt);
  }, [selectedAttempt, onSelectAttempt]);

  return (
    <div className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden p-4 flex flex-col gap-3 min-w-0">
      {selectedAttempt ? (
        <>
          {/* Detail panel — the recognized text at normal reading size.
              Navigation back to the list is via the breadcrumb's "Attempts" crumb;
              Play / Delete for the open attempt live in the page toolbar. */}
          <div className="flex items-center gap-3 min-w-0">
            <span
              className={`text-2xl font-bold tabular-nums ${scoreBadgeClasses(selectedAttempt.score)}`}
            >
              {Math.round(selectedAttempt.score)}%
            </span>
            <span className="text-xs text-text-tertiary tabular-nums">
              {formatWhen(selectedAttempt.created_at)}
            </span>
          </div>

          <section className="flex flex-col gap-1 min-w-0">
            <h3 className="text-xs font-medium text-text-secondary">Recognized text</h3>
            <div className="p-4 rounded-xl bg-bg-muted text-text-primary text-base leading-relaxed whitespace-pre-wrap break-words">
              {selectedAttempt.recognized || "(no speech recognized)"}
            </div>
          </section>
        </>
      ) : (
        <>
          <section className="flex flex-col gap-1 min-w-0">
            <h2 className="text-lg font-semibold text-text-primary break-words flex items-center gap-2">
              <History size={18} className="text-text-tertiary shrink-0" />
              Attempts
            </h2>
            <p className="text-xs text-text-tertiary break-words">
              {text.title || "Untitled"} · {attempts.length} attempt{attempts.length === 1 ? "" : "s"}
            </p>
          </section>

          {attempts.length === 0 ? (
            <p className="text-xs text-text-tertiary">
              No attempts yet. Record yourself reading in the practice view.
            </p>
          ) : (
            <ul className="flex flex-col gap-1.5">
              {attempts.map((a) => (
                <li
                  key={a.uuid}
                  onClick={() => onSelectUuid(a.uuid)}
                  className="flex items-center gap-2 p-2 rounded-lg border border-border-default bg-bg-card hover:bg-bg-hover cursor-pointer transition-colors"
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
                      onClick={(e) => {
                        e.stopPropagation();
                        playUrl(a.audio_url);
                      }}
                      title="Play recording"
                      className="p-1 rounded text-text-tertiary hover:text-text-primary hover:bg-bg-hover cursor-pointer shrink-0"
                    >
                      <Play size={15} />
                    </button>
                  )}
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      deleteAttempt(a);
                    }}
                    title="Delete attempt"
                    className="p-1 rounded text-text-tertiary hover:text-error-text hover:bg-bg-hover cursor-pointer shrink-0"
                  >
                    <Trash2 size={15} />
                  </button>
                  <ChevronRight size={16} className="text-text-tertiary shrink-0" />
                </li>
              ))}
            </ul>
          )}
        </>
      )}

      {error && (
        <div className="p-3 rounded-lg bg-error-bg text-error-text text-sm break-words">{error}</div>
      )}

      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}
