"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { message } from "@tauri-apps/plugin-dialog";
import {
  Loader2,
  Download,
  Copy,
  Check,
  Scissors,
  Upload,
  X,
} from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { logError } from "@/lib/logger";
import { type DownloadProgress } from "@/lib/models/types";
import {
  PageHeader,
  Toolbar,
  ToolbarButton,
  ToolbarIconButton,
} from "@/components/ui/toolbar";

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

/**
 * The split parameters the page exposes. Persisted as one object so a tuned
 * threshold survives a restart, the same way the OCR page keeps its font size.
 */
interface SplitOptions {
  /** Boundary probability above which a sentence break is inserted. */
  threshold: number;
  stripWhitespace: boolean;
  splitOnNewlines: boolean;
}

const DEFAULT_MODEL_ID = "segment-any-text/sat-3l-sm";
const OPTIONS_KEY = "text_split_options";

/** SaT probabilities are near zero around real boundaries; 0.5 is already far
 *  past "every clause is a sentence", so the slider stops there. */
const THRESHOLD_MAX = 0.5;
const THRESHOLD_STEP = 0.005;

const DEFAULT_OPTIONS: SplitOptions = {
  threshold: 0.01,
  stripWhitespace: true,
  splitOnNewlines: true,
};

function formatBytes(n: number): string {
  if (n <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const i = Math.min(units.length - 1, Math.floor(Math.log(n) / Math.log(1024)));
  return `${(n / Math.pow(1024, i)).toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

/**
 * Count word runs without allocating a string per word: a book-sized paste is
 * re-counted on every keystroke, where `split(/\s+/)` would churn the heap.
 */
function countWords(text: string): number {
  let words = 0;
  let inWord = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (ch === " " || ch === "\n" || ch === "\t" || ch === "\r") {
      inWord = false;
    } else if (!inWord) {
      inWord = true;
      words++;
    }
  }
  return words;
}

/** "1 line" / "2 lines" — counts sit in one-word footers, where the dangling
 *  plural is obvious. */
function plural(count: number, one: string, many = `${one}s`): string {
  return `${count.toLocaleString()} ${count === 1 ? one : many}`;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function SplitTextPage() {
  const [status, setStatus] = useState<TextSplitStatus | null>(null);
  const [selectedId, setSelectedId] = useState<string>(DEFAULT_MODEL_ID);
  const [input, setInput] = useState("");
  const [sentences, setSentences] = useState<string[]>([]);
  const [options, setOptions] = useState<SplitOptions>(DEFAULT_OPTIONS);

  const [downloading, setDownloading] = useState(false);
  const [splitting, setSplitting] = useState(false);
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  const [copiedIndex, setCopiedIndex] = useState<number | null>(null);
  const copyTimer = useRef<number | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
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

  // Restore the last tuning (client only, so prerender and first paint agree).
  useEffect(() => {
    try {
      const raw = localStorage.getItem(OPTIONS_KEY);
      if (!raw) return;
      const saved = JSON.parse(raw) as Partial<SplitOptions>;
      setOptions((prev) => ({
        threshold:
          typeof saved.threshold === "number" && Number.isFinite(saved.threshold)
            ? Math.min(THRESHOLD_MAX, Math.max(0, saved.threshold))
            : prev.threshold,
        stripWhitespace:
          typeof saved.stripWhitespace === "boolean" ? saved.stripWhitespace : prev.stripWhitespace,
        splitOnNewlines:
          typeof saved.splitOnNewlines === "boolean" ? saved.splitOnNewlines : prev.splitOnNewlines,
      }));
    } catch {
      // A corrupt entry is not worth an error banner; defaults still apply.
    }
  }, []);

  const updateOptions = useCallback((patch: Partial<SplitOptions>) => {
    setOptions((prev) => {
      const next = { ...prev, ...patch };
      localStorage.setItem(OPTIONS_KEY, JSON.stringify(next));
      return next;
    });
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

  const handleSplit = useCallback(async () => {
    if (!isTauri()) return;
    if (!input.trim()) {
      setSentences([]);
      return;
    }
    setError("");
    setCopied(false);
    setCopiedIndex(null);
    setSplitting(true);
    try {
      const result = await invoke<string[]>("text_split", {
        text: input,
        model: selectedId,
        threshold: options.threshold,
        stripWhitespace: options.stripWhitespace,
        splitOnInputNewlines: options.splitOnNewlines,
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
  }, [input, selectedId, options, refreshStatus]);

  const handlePickFile = useCallback(() => {
    fileInputRef.current?.click();
  }, []);

  async function handleFileChange(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    e.target.value = ""; // allow re-selecting the same file
    if (!file) return;
    setError("");
    try {
      const text = await file.text();
      setInput(text);
    } catch (err) {
      const msg = String(err);
      setError(`Failed to read file: ${msg}`);
      logError(`file read failed: ${msg}`);
    }
  }

  const handleCopyAll = useCallback(async () => {
    if (sentences.length === 0) return;
    try {
      await navigator.clipboard.writeText(sentences.join("\n"));
      setCopied(true);
      setCopiedIndex(null);
      if (copyTimer.current) window.clearTimeout(copyTimer.current);
      copyTimer.current = window.setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      logError(`clipboard write failed: ${String(e)}`);
    }
  }, [sentences]);

  const copySentence = useCallback(async (text: string, index: number) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedIndex(index);
      setCopied(false);
      if (copyTimer.current) window.clearTimeout(copyTimer.current);
      copyTimer.current = window.setTimeout(() => setCopiedIndex(null), 1500);
    } catch (e) {
      logError(`clipboard write failed: ${String(e)}`);
    }
  }, []);

  const clearAll = useCallback(() => {
    setInput("");
    setSentences([]);
    setError("");
    setCopied(false);
    setCopiedIndex(null);
  }, []);

  const downloaded = model?.downloaded ?? false;
  const canSplit = inTauri && !!input.trim() && !splitting && !downloading;

  // Ctrl/Cmd+Enter splits, Ctrl/Cmd+Shift+C copies every sentence — the same
  // shortcuts the OCR page uses.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      if (e.key === "Enter") {
        e.preventDefault();
        if (canSplit) void handleSplit();
      } else if (e.shiftKey && (e.key === "C" || e.key === "c")) {
        e.preventDefault();
        void handleCopyAll();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [canSplit, handleSplit, handleCopyAll]);

  const progressPct = useMemo(() => {
    if (!progress || progress.overall_total_bytes <= 0) return null;
    return Math.min(100, Math.round((progress.overall_bytes_downloaded / progress.overall_total_bytes) * 100));
  }, [progress]);

  const inputStats = useMemo(
    () => ({
      chars: input.length,
      words: countWords(input),
      lines: input ? input.split("\n").length : 0,
    }),
    [input],
  );

  const resultStats = useMemo(
    () => ({
      count: sentences.length,
      chars: sentences.reduce((total, s) => total + s.length, 0),
    }),
    [sentences],
  );

  const splitTitle = !inTauri
    ? "Available in the desktop app"
    : !input.trim()
      ? "Paste or upload text first"
      : !downloaded
        ? "Split text into sentences — download the model first if this fails"
        : "Split text into sentences (Ctrl+Enter)";

  return (
    <main className="flex-1 flex flex-col h-full min-w-0 min-h-0">
      <PageHeader
        title="Split Text"
        badge={
          <span
            className={`px-2 py-0.5 rounded-full text-xs font-medium ${
              model?.loaded
                ? "bg-green-500/15 text-green-500"
                : downloaded
                  ? "bg-accent-bg/20 text-accent"
                  : "bg-bg-muted text-text-tertiary"
            }`}
          >
            {model?.loaded ? "Loaded" : downloaded ? "Downloaded" : "Not downloaded"}
          </span>
        }
      />

      {/* Page-level actions, above the panes, so Split and Copy stay in reach
          whichever pane is scrolling. */}
      <Toolbar className="flex-wrap">
        <ToolbarButton
          icon={Upload}
          label="Upload"
          onClick={handlePickFile}
          title="Read a text file (.txt, .md, .vtt, .srt) into the input"
        />
        <ToolbarButton
          icon={X}
          label="Clear"
          onClick={clearAll}
          disabled={!input && sentences.length === 0}
          title="Clear the input and the sentences"
        />
        <div className="flex-1 min-w-0" />

        <select
          value={selectedId}
          onChange={(e) => setSelectedId(e.target.value)}
          className="min-w-0 max-w-[220px] px-2 py-1 rounded-md bg-bg-muted border border-border-default text-sm text-text-primary focus:outline-none focus:border-accent"
          title={model?.description}
          aria-label="Split model"
        >
          {(status?.models.length
            ? status.models
            : [{ id: DEFAULT_MODEL_ID, name: "SaT 3L Small" } as TextSplitModelInfo]
          ).map((m) => (
            <option key={m.id} value={m.id}>
              {m.name}
              {m.downloaded ? "" : " (not downloaded)"}
            </option>
          ))}
        </select>

        {!downloaded && (
          <ToolbarButton
            icon={downloading ? Loader2 : Download}
            iconProps={downloading ? { className: "animate-spin" } : undefined}
            label={downloading ? "Downloading…" : "Download"}
            onClick={handleDownload}
            disabled={downloading}
            title={`Download ${model?.name ?? "the split model"}`}
          />
        )}

        <ToolbarButton
          variant="solid"
          icon={splitting ? Loader2 : Scissors}
          iconProps={splitting ? { className: "animate-spin" } : undefined}
          label={splitting ? "Splitting…" : "Split"}
          onClick={handleSplit}
          disabled={!canSplit}
          title={splitTitle}
        />
        <ToolbarButton
          icon={copied ? Check : Copy}
          label={copied ? "Copied" : "Copy all"}
          onClick={handleCopyAll}
          disabled={sentences.length === 0}
          title="Copy every sentence, one per line"
        />
      </Toolbar>

      <input
        ref={fileInputRef}
        type="file"
        accept=".txt,.md,.vtt,.srt,text/plain"
        onChange={handleFileChange}
        className="hidden"
      />

      {downloading && progress && (
        <div className="shrink-0 px-4 py-2 border-b border-border-default bg-bg-card">
          <div className="mb-1.5 flex items-center justify-between gap-3 text-xs">
            <span className="font-medium text-text-primary">Downloading model…</span>
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

      {error && (
        <div className="shrink-0 mx-4 mt-3 px-4 py-3 rounded-lg bg-error-bg text-error-text text-sm flex items-start gap-2">
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

      {/* Split options — one slim strip, not a card: they belong to the Split
          action, not to either pane. */}
      <div
        role="group"
        aria-label="Split options"
        className="shrink-0 flex flex-wrap items-center gap-x-5 gap-y-2 px-4 py-2 border-b border-border-default bg-bg-card"
      >
        <label className="flex items-center gap-2 text-sm text-text-primary">
          <span className="text-text-secondary">Threshold</span>
          <input
            type="range"
            min={0}
            max={THRESHOLD_MAX}
            step={THRESHOLD_STEP}
            value={options.threshold}
            onChange={(e) => updateOptions({ threshold: Number(e.target.value) })}
            className="w-28 cursor-pointer accent-accent"
            aria-label="Sentence boundary threshold"
          />
          <span className="w-12 text-xs tabular-nums text-text-tertiary">
            {options.threshold.toFixed(3)}
          </span>
        </label>

        <Toggle
          label="Strip whitespace"
          checked={options.stripWhitespace}
          onChange={(v) => updateOptions({ stripWhitespace: v })}
        />
        <Toggle
          label="Split on newlines"
          checked={options.splitOnNewlines}
          onChange={(v) => updateOptions({ splitOnNewlines: v })}
        />

        <p className="hidden xl:block flex-1 min-w-0 truncate text-right text-xs text-text-tertiary">
          Lower threshold → more sentence breaks
        </p>
      </div>

      {/* Narrow: two stacked panes inside one scroll container. Wide: a fixed
          split whose panes scroll internally. */}
      <div className="flex-1 min-h-0 overflow-y-auto p-4 lg:overflow-hidden">
        <div className="grid gap-4 lg:h-full lg:min-h-0 lg:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
          {/* ---- Input pane ---- */}
          <section className="flex flex-col rounded-xl border border-border-default bg-bg-card min-w-0 min-h-0 overflow-hidden">
            <div className="shrink-0 flex items-center gap-2 px-3 py-2 border-b border-border-default">
              <h2 className="flex-1 min-w-0 text-sm font-semibold text-text-primary">Input</h2>
              <span className="whitespace-nowrap text-xs text-text-tertiary">
                {plural(inputStats.chars, "character")} · {plural(inputStats.words, "word")}
              </span>
            </div>
            <textarea
              value={input}
              onChange={(e) => setInput(e.target.value)}
              placeholder="Paste text to split into sentences, or use Upload."
              aria-label="Text to split"
              className="flex-1 min-h-[220px] lg:min-h-0 w-full px-3 py-2 bg-transparent text-sm leading-6 text-text-primary resize-y lg:resize-none focus:outline-none"
            />
            <div className="shrink-0 flex items-center gap-2 px-3 py-1.5 border-t border-border-default text-xs text-text-tertiary">
              <span className="truncate">Paste, upload a text file, or type</span>
              <span className="flex-1" />
              <span className="whitespace-nowrap">{plural(inputStats.lines, "line")}</span>
            </div>
          </section>

          {/* ---- Sentences pane ---- */}
          <section className="flex flex-col rounded-xl border border-border-default bg-bg-card min-w-0 min-h-0 overflow-hidden">
            <div className="shrink-0 flex items-center gap-2 px-3 py-2 border-b border-border-default">
              <h2 className="flex-1 min-w-0 text-sm font-semibold text-text-primary">Sentences</h2>
              {sentences.length > 0 && (
                <span className="px-1.5 rounded bg-bg-muted border border-border-default text-xs leading-5 text-text-secondary tabular-nums">
                  {sentences.length}
                </span>
              )}
            </div>

            <div className="flex-1 min-h-[220px] lg:min-h-0 overflow-y-auto p-2">
              {sentences.length === 0 ? (
                <div className="h-full flex flex-col items-center justify-center gap-3 px-4 text-center text-text-tertiary">
                  <Scissors size={36} className="opacity-40" />
                  <p className="text-sm">
                    {inTauri
                      ? "Split sentences appear here, one per line."
                      : "Available in the desktop app."}
                  </p>
                </div>
              ) : (
                <ol className="flex flex-col gap-0.5">
                  {sentences.map((sentence, index) => (
                    <li
                      key={index}
                      className="group flex items-start gap-2 px-2 py-1.5 rounded-md hover:bg-bg-muted"
                    >
                      <span className="w-7 shrink-0 text-right text-xs leading-6 text-text-tertiary tabular-nums">
                        {index + 1}
                      </span>
                      <span className="flex-1 min-w-0 break-words text-sm leading-6 text-text-primary">
                        {sentence}
                      </span>
                      {/* Always visible on touch: a hover-only control would be
                          unreachable there. */}
                      <button
                        type="button"
                        onClick={() => copySentence(sentence, index)}
                        className="shrink-0 p-1 rounded text-text-tertiary opacity-100 sm:opacity-0 sm:group-hover:opacity-100 focus-visible:opacity-100 hover:text-text-primary hover:bg-bg-hover transition-opacity"
                        title="Copy this sentence"
                        aria-label={`Copy sentence ${index + 1}`}
                      >
                        {copiedIndex === index ? <Check size={14} /> : <Copy size={14} />}
                      </button>
                    </li>
                  ))}
                </ol>
              )}
            </div>

            <div className="shrink-0 flex items-center gap-2 px-3 py-1.5 border-t border-border-default text-xs text-text-tertiary">
              <span className="whitespace-nowrap">
                {plural(resultStats.count, "sentence")} · {plural(resultStats.chars, "character")}
              </span>
              {splitting && <span>· splitting…</span>}
            </div>
          </section>
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
