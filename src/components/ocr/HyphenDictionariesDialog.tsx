"use client";

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Download, Loader2, Trash2, X } from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { tbBase } from "@/lib/ui/toolbar";
import { ToolbarIconButton } from "@/components/ui/toolbar";
import type { OcrDictionaryStatus } from "./useOcr";

function formatBytes(bytes: number): string {
  if (bytes <= 0) return "—";
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * Manager for the Hunspell dictionaries behind line-break hyphen repair.
 *
 * One dictionary per language: a page recognised as `deu+eng` consults every
 * installed one of that set, so the list shows what is there, what a download
 * would cost, and what the licence is. Deleting is safe — the dictionary can be
 * downloaded again, and the shape rule takes over meanwhile.
 */
export default function HyphenDictionariesDialog({
  onClose,
  onChanged,
}: {
  onClose: () => void;
  /** An install or delete landed, so the caller re-runs recognition. */
  onChanged: () => void;
}) {
  const [entries, setEntries] = useState<OcrDictionaryStatus[] | null>(null);
  // Code being downloaded or removed right now.
  const [busy, setBusy] = useState<string | null>(null);
  // Code awaiting a second click before it is removed.
  const [confirming, setConfirming] = useState<string | null>(null);
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    if (!isTauri()) return;
    try {
      setEntries(await invoke<OcrDictionaryStatus[]>("ocr_hyphen_dictionary_status"));
    } catch (e) {
      setError(`Could not read the dictionary list: ${errorMessage(e)}`);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  const install = useCallback(
    async (code: string) => {
      setBusy(code);
      setError("");
      setConfirming(null);
      try {
        await invoke("ocr_hyphen_dictionary_download", { code });
        await refresh();
        onChanged();
      } catch (e) {
        setError(`Download failed: ${errorMessage(e)}`);
      } finally {
        setBusy(null);
      }
    },
    [onChanged, refresh]
  );

  const remove = useCallback(
    async (code: string) => {
      setBusy(code);
      setError("");
      setConfirming(null);
      try {
        await invoke("ocr_hyphen_dictionary_delete", { code });
        await refresh();
        onChanged();
      } catch (e) {
        setError(`Delete failed: ${errorMessage(e)}`);
      } finally {
        setBusy(null);
      }
    },
    [onChanged, refresh]
  );

  const installedCount = entries?.filter((entry) => entry.installed).length ?? 0;

  return (
    <div
      className="fixed inset-0 z-[80] flex items-center justify-center bg-black/40 p-4"
      onClick={onClose}
      role="presentation"
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Hyphen dictionaries"
        className="flex h-full max-h-[80vh] w-full max-w-2xl flex-col overflow-hidden rounded-xl border border-border-default bg-bg-card shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="shrink-0 flex items-center gap-2 px-4 py-2 border-b border-border-default">
          <h2 className="flex-1 min-w-0 text-sm font-semibold text-text-primary">
            Hyphen dictionaries
          </h2>
          <span className="text-xs text-text-tertiary">
            {installedCount} installed of {entries?.length ?? 0}
          </span>
          <ToolbarIconButton icon={X} ariaLabel="Close dictionaries" onClick={onClose} />
        </div>

        <p className="shrink-0 px-4 py-2 text-xs text-text-tertiary border-b border-border-default">
          Line-break hyphens are decided by the dictionary of the languages you
          recognise with: every installed one of the selected set is consulted
          (<span className="font-mono">Sprach-</span> +{" "}
          <span className="font-mono">verarbeitung</span> →{" "}
          <span className="font-mono">Sprachverarbeitung</span>). A language with
          no dictionary falls back to the shape rule.
        </p>

        {error && (
          <div className="shrink-0 mx-4 mt-3 px-3 py-2 rounded-lg bg-error-bg text-error-text text-xs whitespace-pre-wrap break-words">
            {error}
          </div>
        )}

        <div className="flex-1 min-h-0 overflow-y-auto">
          {entries === null ? (
            <p className="px-4 py-3 text-xs text-text-tertiary">Loading…</p>
          ) : (
            <ul className="py-1">
              {entries.map((entry) => (
                <li
                  key={entry.code}
                  className="flex flex-wrap items-center gap-2 px-4 py-2 border-b border-border-default last:border-b-0"
                >
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <span className="text-sm text-text-primary">{entry.label}</span>
                      <span className="px-1.5 rounded bg-bg-muted border border-border-default text-[11px] text-text-tertiary">
                        {entry.tesseract.join(" / ")}
                      </span>
                      {entry.installed && (
                        <span className="px-1.5 rounded border border-accent/30 bg-accent/10 text-[11px] text-accent">
                          Installed
                        </span>
                      )}
                    </div>
                    <p className="mt-0.5 text-[11px] text-text-tertiary">
                      dictionary-{entry.code}@{entry.version} · {entry.license} ·{" "}
                      {formatBytes(entry.installed ? entry.size_bytes : entry.approx_bytes)}
                    </p>
                  </div>

                  {busy === entry.code ? (
                    <span className="shrink-0 flex items-center gap-1 text-xs text-text-tertiary">
                      <Loader2 size={14} className="animate-spin" />
                      {entry.installed ? "Removing…" : "Downloading…"}
                    </span>
                  ) : entry.installed ? (
                    confirming === entry.code ? (
                      <div className="shrink-0 flex items-center gap-1.5">
                        <button
                          type="button"
                          onClick={() => remove(entry.code)}
                          className={`${tbBase} bg-error-text text-white hover:bg-error-hover whitespace-nowrap`}
                          title={`Delete the ${entry.label} dictionary — it can be downloaded again`}
                        >
                          Remove
                        </button>
                        <button
                          type="button"
                          onClick={() => setConfirming(null)}
                          className={`${tbBase} border border-border-default whitespace-nowrap`}
                        >
                          Cancel
                        </button>
                      </div>
                    ) : (
                      <ToolbarIconButton
                        icon={Trash2}
                        ariaLabel={`Remove the ${entry.label} dictionary`}
                        className="shrink-0 text-error-text hover:bg-error-bg"
                        onClick={() => setConfirming(entry.code)}
                      />
                    )
                  ) : (
                    <button
                      type="button"
                      onClick={() => install(entry.code)}
                      disabled={busy !== null}
                      className={`${tbBase} shrink-0 border border-border-default bg-accent/10 text-accent hover:bg-accent/20 disabled:opacity-50 disabled:cursor-not-allowed whitespace-nowrap`}
                      title={`Download the ${entry.label} dictionary (${formatBytes(entry.approx_bytes)})`}
                    >
                      <Download size={14} />
                      {formatBytes(entry.approx_bytes)}
                    </button>
                  )}
                </li>
              ))}
            </ul>
          )}
        </div>

        <div className="shrink-0 px-4 py-2 border-t border-border-default text-[11px] text-text-tertiary">
          Dictionaries are stored in the app data folder and can be removed at
          any time. Deleting one only costs the dictionary&apos;s evidence, not
          the feature.
        </div>
      </div>
    </div>
  );
}
