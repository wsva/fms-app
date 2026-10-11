"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { message } from "@tauri-apps/plugin-dialog";
import {
  ArrowUpDown,
  Loader2,
  Download,
  Copy,
  Check,
  Scissors,
  X,
} from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { logError } from "@/lib/logger";
import { type DownloadProgress } from "@/lib/models/types";

// ---------------------------------------------------------------------------
// Types (mirror the Rust `Serialize` shapes; serde keeps snake_case field names)
// ---------------------------------------------------------------------------

interface TextSplitModelInfo {
  id: string;
  name: string;
  description: string;
  downloaded: boolean;
  loaded: boolean;
  size_mb: number;
}

interface TextSplitStatus {
  downloaded: boolean;
  loaded: boolean;
  model_id: string;
  models: TextSplitModelInfo[];
}

const DEFAULT_MODEL_ID = "segment-any-text/sat-3l-sm";

function formatBytes(n: number): string {
  if (n <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const i = Math.min(units.length - 1, Math.floor(Math.log(n) / Math.log(1024)));
  return `${(n / Math.pow(1024, i)).toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function SplitTextPage() {
  const [status, setStatus] = useState<TextSplitStatus | null>(null);
  const [selectedId, setSelectedId] = useState<string>(DEFAULT_MODEL_ID);
  const [input, setInput] = useState("");
  const [sentences, setSentences] = useState<string[]>([]);
  const [threshold, setThreshold] = useState(0.01);
  const [stripWhitespace, setStripWhitespace] = useState(true);
  const [splitOnNewlines, setSplitOnNewlines] = useState(true);

  const [downloading, setDownloading] = useState(false);
  const [splitting, setSplitting] = useState(false);
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  const copyTimer = useRef<number | null>(null);
  // Deferred platform check: calling isTauri() during render diverges between
  // the prerendered (false) and client (true) trees and trips a hydration
  // mismatch. Resolve it once in an effect instead.
  const [inTauri, setInTauri] = useState(false);

  const refreshStatus = useCallback(async () => {
    if (!isTauri()) return;
    try {
      setStatus(await invoke<TextSplitStatus>("text_split_status"));
    } catch (e) {
      logError(`text_split_status failed: ${String(e)}`);
    }
  }, []);

  // Fetch status on mount and keep it in sync with the shared download events.
  useEffect(() => {
    if (!isTauri()) return;
    refreshStatus();
    const unlistenProgress = listen<DownloadProgress>("model-download-progress", (event) => {
      setProgress(event.payload);
    });
    const unlistenChanged = listen("model-status-changed", () => {
      refreshStatus();
    });
    return () => {
      unlistenProgress.then((fn) => fn());
      unlistenChanged.then((fn) => fn());
    };
  }, [refreshStatus]);

  useEffect(() => {
    setInTauri(isTauri());
    return () => {
      if (copyTimer.current) window.clearTimeout(copyTimer.current);
    };
  }, []);

  const model = useMemo(
    () => status?.models.find((m) => m.id === selectedId) ?? null,
    [status, selectedId],
  );

  // Keep the selection anchored to a catalog entry: adopt the backend's current
  // model once status first arrives, then fall back to the default if the
  // selected model ever disappears from the catalog.
  useEffect(() => {
    if (!status) return;
    setSelectedId((cur) => {
      if (status.models.some((m) => m.id === cur)) return cur;
      const current = status.models.find((m) => m.id === status.model_id);
      return current ? status.model_id : (status.models[0]?.id ?? DEFAULT_MODEL_ID);
    });
  }, [status]);

  async function handleDownload() {
    if (!isTauri()) return;
    setError("");
    setDownloading(true);
    setProgress({
      files: [],
      overall_bytes_downloaded: 0,
      overall_total_bytes: 0,
      speed: 0,
      eta_seconds: null,
    });
    try {
      await invoke("model_download_hf", { id: selectedId });
      await refreshStatus();
      await message(`${model?.name ?? selectedId} downloaded. It loads automatically on the first split.`, {
        title: "Split Text",
        kind: "info",
      });
    } catch (e) {
      const msg = String(e);
      setError(msg);
      logError(`model_download_hf failed: ${msg}`);
    } finally {
      setDownloading(false);
      setProgress(null);
      await refreshStatus();
    }
  }

  async function handleSplit() {
    if (!isTauri()) return;
    if (!input.trim()) {
      setSentences([]);
      return;
    }
    setError("");
    setSplitting(true);
    try {
      const result = await invoke<string[]>("text_split", {
        text: input,
        model: selectedId,
        threshold,
        stripWhitespace,
        splitOnInputNewlines: splitOnNewlines,
      });
      setSentences(result);
      await refreshStatus();
    } catch (e) {
      const msg = String(e);
      setError(msg);
      setSentences([]);
      logError(`text_split failed: ${msg}`);
      await message(msg, { title: "Split Text", kind: "error" });
    } finally {
      setSplitting(false);
    }
  }

  async function handleCopyAll() {
    if (sentences.length === 0) return;
    try {
      await navigator.clipboard.writeText(sentences.join("\n"));
      setCopied(true);
      if (copyTimer.current) window.clearTimeout(copyTimer.current);
      copyTimer.current = window.setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      logError(`clipboard write failed: ${String(e)}`);
    }
  }

  const downloaded = model?.downloaded ?? false;
  const loaded = model?.loaded ?? false;
  const canSplit = inTauri && !!input.trim() && !splitting && !downloading;

  const progressPct = useMemo(() => {
    if (!progress || progress.overall_total_bytes <= 0) return null;
    return Math.min(100, Math.round((progress.overall_bytes_downloaded / progress.overall_total_bytes) * 100));
  }, [progress]);

  return (
    <main className="flex-1 flex flex-col h-full min-w-0 min-h-0">
      {/* Header */}
      <div className="p-4 border-b border-border-default flex items-center gap-3 flex-wrap">
        <ArrowUpDown size={20} className="text-text-secondary shrink-0" />
        <h1 className="text-lg font-semibold flex-1 min-w-0">Split Text</h1>

        {/* Model status badge */}
        <div className="flex items-center gap-2 text-xs">
          <select
            value={selectedId}
            onChange={(e) => setSelectedId(e.target.value)}
            className="max-w-[220px] px-2 py-1 rounded-lg bg-bg-muted border border-border-default text-text-primary focus:outline-none focus:border-accent"
            title={model?.description}
          >
            {(status?.models.length ? status.models : [{ id: DEFAULT_MODEL_ID, name: "SaT 3L Small" } as TextSplitModelInfo]).map((m) => (
              <option key={m.id} value={m.id}>
                {m.name}{m.downloaded ? "" : " (not downloaded)"}
              </option>
            ))}
          </select>
          <span
            className={`px-2 py-0.5 rounded-full font-medium ${
              loaded
                ? "bg-green-500/15 text-green-500"
                : downloaded
                  ? "bg-accent-bg/20 text-accent"
                  : "bg-bg-muted text-text-tertiary"
            }`}
          >
            {loaded ? "Loaded" : downloaded ? "Downloaded" : "Not downloaded"}
          </span>
          {!downloaded && (
            <button
              type="button"
              onClick={handleDownload}
              disabled={downloading}
              className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg bg-accent text-white font-medium disabled:opacity-40 disabled:cursor-not-allowed hover:opacity-90 transition-opacity"
            >
              {downloading ? <Loader2 size={14} className="animate-spin" /> : <Download size={14} />}
              {downloading ? "Downloading…" : "Download"}
            </button>
          )}
        </div>
      </div>

      <div className="flex-1 overflow-y-auto p-4 min-h-0">
        <div className="flex flex-col gap-4 h-full min-h-0">
          {/* Download progress */}
          {downloading && progress && (
            <div className="px-4 py-3 rounded-lg bg-bg-card border border-border-default text-sm">
              <div className="flex items-center justify-between mb-2">
                <span className="text-text-primary font-medium">Downloading model…</span>
                <span className="text-text-tertiary">
                  {formatBytes(progress.overall_bytes_downloaded)}
                  {progress.overall_total_bytes > 0 && ` / ${formatBytes(progress.overall_total_bytes)}`}
                  {progressPct !== null && ` (${progressPct}%)`}
                </span>
              </div>
              <div className="h-1.5 rounded-full bg-bg-muted overflow-hidden">
                <div
                  className="h-full bg-accent transition-[width] duration-200"
                  style={{ width: `${progressPct ?? 0}%` }}
                />
              </div>
            </div>
          )}

          {/* Error banner */}
          {error && (
            <div className="px-4 py-3 rounded-lg bg-error-bg text-error-text text-sm flex items-start gap-2">
              <span className="flex-1 whitespace-pre-wrap break-words">{error}</span>
              <button
                type="button"
                onClick={() => setError("")}
                className="shrink-0 opacity-70 hover:opacity-100"
                title="Dismiss"
              >
                <X size={16} />
              </button>
            </div>
          )}

          {/* Options toolbar */}
          <div className="flex flex-wrap items-center gap-x-5 gap-y-3 rounded-xl border border-border-default bg-bg-card px-4 py-3">
            <label className="flex items-center gap-2 text-sm text-text-primary">
              <span className="text-text-secondary">Threshold</span>
              <input
                type="number"
                min={0}
                max={1}
                step={0.01}
                value={threshold}
                onChange={(e) => setThreshold(Math.max(0, Math.min(1, parseFloat(e.target.value) || 0)))}
                className="w-20 px-2 py-1 rounded-lg bg-bg-muted border border-border-default text-text-primary focus:outline-none focus:border-accent"
                title="Sentence-boundary probability threshold (default 0.01)"
              />
            </label>

            <Toggle label="Strip whitespace" checked={stripWhitespace} onChange={setStripWhitespace} />
            <Toggle label="Split on newlines" checked={splitOnNewlines} onChange={setSplitOnNewlines} />

            <div className="flex-1" />

            <button
              type="button"
              onClick={handleSplit}
              disabled={!canSplit}
              className="inline-flex items-center gap-2 px-4 py-2 rounded-lg bg-accent text-white text-sm font-medium disabled:opacity-40 disabled:cursor-not-allowed hover:opacity-90 transition-opacity"
              title={!downloaded ? "Download the model first" : "Split text into sentences"}
            >
              {splitting ? <Loader2 size={16} className="animate-spin" /> : <Scissors size={16} />}
              {splitting ? "Splitting…" : "Split Text"}
            </button>
          </div>

          {/* Two-pane body */}
          <div className="flex-1 grid grid-cols-1 md:grid-cols-2 gap-4 min-h-[320px]">
            {/* Input */}
            <section className="flex flex-col rounded-xl border border-border-default bg-bg-card p-4 min-w-0 min-h-0">
              <div className="flex items-center justify-between mb-2">
                <h2 className="text-sm font-semibold text-text-primary">Input</h2>
                <span className="text-xs text-text-tertiary">{input.length} characters</span>
              </div>
              <textarea
                value={input}
                onChange={(e) => setInput(e.target.value)}
                placeholder="Paste text to split into sentences…"
                className="flex-1 min-h-[200px] w-full px-3 py-2 rounded-lg bg-bg-muted border border-border-default text-text-primary resize-y focus:outline-none focus:border-accent"
              />
            </section>

            {/* Results */}
            <section className="flex flex-col rounded-xl border border-border-default bg-bg-card p-4 min-w-0 min-h-0">
              <div className="flex items-center justify-between mb-2">
                <h2 className="text-sm font-semibold text-text-primary">
                  Sentences
                  {sentences.length > 0 && (
                    <span className="ml-2 text-xs font-normal text-text-tertiary">{sentences.length}</span>
                  )}
                </h2>
                {sentences.length > 0 && (
                  <button
                    type="button"
                    onClick={handleCopyAll}
                    className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg bg-bg-muted border border-border-default text-xs font-medium text-text-primary hover:bg-bg-hover transition-colors"
                    title="Copy all sentences"
                  >
                    {copied ? <Check size={14} /> : <Copy size={14} />}
                    {copied ? "Copied" : "Copy all"}
                  </button>
                )}
              </div>

              <div className="flex-1 min-h-[200px] overflow-y-auto rounded-lg bg-bg-muted border border-border-default p-2">
                {sentences.length === 0 ? (
                  <div className="h-full flex items-center justify-center text-sm text-text-tertiary px-4 text-center">
                    {inTauri
                      ? "Split sentences appear here."
                      : "Available in the desktop app."}
                  </div>
                ) : (
                  <ol className="flex flex-col gap-1">
                    {sentences.map((s, i) => (
                      <li
                        key={i}
                        className="flex gap-2 px-2 py-1.5 rounded-md bg-bg-card border border-border-default text-sm text-text-primary"
                      >
                        <span className="shrink-0 tabular-nums text-text-tertiary w-7 text-right">{i + 1}.</span>
                        <span className="flex-1 min-w-0 break-words">{s}</span>
                      </li>
                    ))}
                  </ol>
                )}
              </div>
            </section>
          </div>
        </div>
      </div>
    </main>
  );
}

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

function Toggle({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <label className="flex items-center gap-2 text-sm text-text-primary cursor-pointer select-none">
      <span
        className={`w-8 h-5 rounded-full relative transition-colors shrink-0 ${
          checked ? "bg-accent" : "bg-bg-muted border border-border-default"
        }`}
      >
        <span
          className={`absolute top-0.5 left-0.5 w-4 h-4 rounded-full bg-white transition-transform ${
            checked ? "translate-x-3" : ""
          }`}
        />
      </span>
      <input
        type="checkbox"
        className="sr-only"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
      {label}
    </label>
  );
}
