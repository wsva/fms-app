"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ImagePlus,
  MonitorUp,
  Loader2,
  Sparkles,
  X,
  Languages,
  Wand2,
  ChevronDown,
  ChevronUp,
  Check,
  Table2,
  Type,
  Copy,
  BookOpen,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/tauri";
import { tbBase, tbGhost } from "@/lib/ui/toolbar";
import { PageHeader, Toolbar, ToolbarButton, ToolbarIconButton, ToolbarMenuItem, ToolbarOverflow } from "@/components/ui/toolbar";
import { useIsMobile } from "@/hooks/useIsMobile";
import { useOcr } from "./useOcr";
import ImageEditor from "./ImageEditor";
import HyphenPanel from "./HyphenPanel";
import HyphenDictionariesDialog from "./HyphenDictionariesDialog";

// Friendly labels for common languages; unknown codes are shown as-is.
const LANG_LABELS: Record<string, string> = {
  eng: "English",
  deu: "Deutsch",
  fra: "Français",
  spa: "Español",
  ita: "Italiano",
  por: "Português",
  nld: "Nederlands",
  chi_sim: "中文简体",
  chi_tra: "中文繁體",
  jpn: "日本語",
  kor: "한국어",
  rus: "Русский",
  ara: "العربية",
};

// Below `lg` the two panes stack and the image collapses, so this is the height
// the viewer gets when expanded. From `lg` up it fills its pane instead.
const NARROW_IMAGE_HEIGHT = "h-[45vh]";

const FONT_MIN = 12;
const FONT_MAX = 40;

export default function OcrPage() {
  const {
    sourceImage,
    selectFile,
    captureScreenshot,
    recognizing,
    recognize,
    mode,
    setMode,
    resultText,
    setResultText,
    rawTsv,
    copyResult,
    copied,
    fixWithLlm,
    fixingText,
    hyphens,
    dictionaries,
    uncovered,
    edited: hyphensEdited,
    installingDictionary,
    setHyphenDecision,
    setAllHyphens,
    installDictionary,
    error,
    setError,
    reset,
  } = useOcr();

  // The editor's current PNG (after rotate/crop) — what we send to recognize.
  const [editedImage, setEditedImage] = useState<string | null>(null);

  // ---- Languages ----
  const [availableLangs, setAvailableLangs] = useState<string[]>([]);
  const [selectedLangs, setSelectedLangs] = useState<string[]>([]);
  const [langDropdownOpen, setLangDropdownOpen] = useState(false);
  const [langFilter, setLangFilter] = useState("");
  const langDropdownRef = useRef<HTMLDivElement>(null);

  // ---- Result pane ----
  // Raw TSV opens as an overlay, not inline: a 100+ row dump inline would shove
  // the text down every time it is toggled.
  const [showRawTsv, setShowRawTsv] = useState(false);
  // Dictionary manager: one Hunspell dictionary per language, installed once
  // and then used by every TSV recognition of that language.
  const [showDictionaries, setShowDictionaries] = useState(false);
  // Installs inside the manager are batched: the page is re-recognised once on
  // close, not once per dictionary.
  const dictionaryDirty = useRef(false);

  // ---- Image pane, narrow layouts only (from `lg` the pane is always open) ----
  const [imageOpen, setImageOpen] = useState(false);

  // The Android webview has no room for every action; `Clear` tucks into the
  // overflow there so the bar keeps to two rows.
  const mobile = useIsMobile();

  // ---- Font size for the result text, remembered across sessions ----
  const [fontSize, setFontSize] = useState(24);
  useEffect(() => {
    const saved = Number(localStorage.getItem("ocr_font_size"));
    if (saved >= FONT_MIN && saved <= FONT_MAX) setFontSize(saved);
  }, []);
  const setFont = useCallback((next: number) => {
    const clamped = Math.min(FONT_MAX, Math.max(FONT_MIN, next));
    setFontSize(clamped);
    localStorage.setItem("ocr_font_size", String(clamped));
  }, []);

  // Fetch available Tesseract languages on mount.
  useEffect(() => {
    if (!isTauri()) return;
    invoke<string[]>("ocr_list_languages")
      .then((langs) => {
        setAvailableLangs(langs);
        // Default to eng+deu if available, otherwise first two.
        setSelectedLangs((prev) => {
          if (prev.length > 0) return prev;
          const defaults = ["eng", "deu"].filter((l) => langs.includes(l));
          if (defaults.length > 0) return defaults;
          return langs.slice(0, Math.min(2, langs.length));
        });
      })
      .catch(() => {
        // Tesseract not found — leave list empty.
      });
  }, []);

  // Close language dropdown on outside click.
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (langDropdownRef.current && !langDropdownRef.current.contains(e.target as Node)) {
        setLangDropdownOpen(false);
      }
    };
    if (langDropdownOpen) document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [langDropdownOpen]);

  const toggleLang = useCallback((code: string) => {
    setSelectedLangs((prev) =>
      prev.includes(code) ? prev.filter((l) => l !== code) : [...prev, code]
    );
  }, []);

  const langString = selectedLangs.join("+");

  const filteredLangs = useMemo(() => {
    const filter = langFilter.trim().toLowerCase();
    if (!filter) return availableLangs;
    return availableLangs.filter(
      (code) =>
        code.toLowerCase().includes(filter) ||
        (LANG_LABELS[code] ?? "").toLowerCase().includes(filter)
    );
  }, [availableLangs, langFilter]);

  // LLM model for the plain path's text fixing (shared with the LLM Chat page).
  const [llmModel, setLlmModelState] = useState("");
  const setLlmModel = useCallback((model: string) => {
    setLlmModelState(model);
    if (typeof window !== "undefined" && model) {
      localStorage.setItem("llm_selected_model", model);
    }
  }, []);

  // Restore cached model on mount (client-side only).
  useEffect(() => {
    const cached = localStorage.getItem("llm_selected_model");
    if (cached) setLlmModelState(cached);
  }, []);

  const [installedModels, setInstalledModels] = useState<string[]>([]);

  // Fetch installed Ollama models on mount.
  useEffect(() => {
    if (!isTauri()) return;
    invoke<{ installed: { name: string }[] }>("llm_list_models")
      .then((res) => {
        const names = res.installed.map((m) => m.name).sort();
        setInstalledModels(names);
        // Prefer cached model if still installed; otherwise first model.
        const cached = localStorage.getItem("llm_selected_model");
        if (names.length > 0) {
          const stillInstalled = cached && names.includes(cached);
          if (!cached || !stillInstalled) {
            setLlmModel(names[0]);
          } else if (cached && names.includes(cached)) {
            setLlmModelState(cached);
          }
        }
      })
      .catch(() => {
        // Ollama not running — ignore.
      });
  }, [setLlmModel]);

  const handleImageReady = useCallback((dataUrl: string | null) => {
    setEditedImage(dataUrl);
  }, []);

  const handleClear = useCallback(() => {
    reset();
    setShowRawTsv(false);
  }, [reset]);

  // Installing a dictionary only affects the next run, so refresh the page
  // straight away while the image is still loaded.
  const handleInstallDictionary = useCallback(
    async (code: string) => {
      const installed = await installDictionary(code);
      if (installed && editedImage) recognize(editedImage, langString);
    },
    [editedImage, installDictionary, recognize, langString]
  );

  const canRecognize = !!editedImage && !recognizing;

  // Ctrl/Cmd+Enter runs recognition, Ctrl/Cmd+Shift+C copies the result — the
  // same habit as the dictation page's shortcuts. The raw-TSV overlay owns the
  // keyboard while it is open, so both are ignored then.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || showRawTsv) return;
      if (e.key === "Enter") {
        e.preventDefault();
        if (canRecognize && editedImage) recognize(editedImage, langString);
      } else if (e.shiftKey && (e.key === "C" || e.key === "c")) {
        e.preventDefault();
        void copyResult();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [canRecognize, editedImage, recognize, langString, copyResult, showRawTsv]);

  const words = useMemo(() => {
    const text = resultText.trim();
    return text ? text.split(/\s+/).length : 0;
  }, [resultText]);

  // Row count of the last dump (minus its header). Recomputed only when a new
  // dump arrives, so the footer can report its size without the overlay open.
  const tsvRowCount = useMemo(() => {
    if (!rawTsv) return 0;
    return Math.max(0, rawTsv.trimEnd().split("\n").length - 1);
  }, [rawTsv]);

  return (
    <main className="flex-1 flex flex-col h-full min-w-0 min-h-0">
      <PageHeader
        title="OCR"
        badge={<span className="text-xs text-text-tertiary">Tesseract</span>}
      />

      {/* Page-level actions live here, above the panes, so Recognize stays in
          reach while either pane scrolls. */}
      <Toolbar className="flex-wrap">
        <ToolbarButton icon={ImagePlus} label="Select image" onClick={selectFile} />
        <ToolbarButton
          icon={MonitorUp}
          label="Screenshot"
          onClick={captureScreenshot}
        />
        {!mobile && (
          <ToolbarButton
            icon={X}
            label="Clear"
            onClick={handleClear}
            disabled={!sourceImage && !resultText && !rawTsv}
            title="Clear image and results"
          />
        )}
        {mobile && (
          <ToolbarOverflow label="More actions">
            <ToolbarMenuItem
              icon={X}
              danger
              disabled={!sourceImage && !resultText && !rawTsv}
              onClick={handleClear}
            >
              Clear image and results
            </ToolbarMenuItem>
            <ToolbarMenuItem icon={BookOpen} onClick={() => setShowDictionaries(true)}>
              Hyphen dictionaries
            </ToolbarMenuItem>
          </ToolbarOverflow>
        )}
        <div className="flex-1 min-w-0" />

        {/* Languages: the chip row shows the current selection without opening
            the menu, which the old truncated `eng+deu` string could not. */}
        <div className="relative shrink-0" ref={langDropdownRef}>
          <button
            type="button"
            onClick={() => {
              setLangFilter("");
              setLangDropdownOpen((o) => !o);
            }}
            className={`${tbGhost} ${langDropdownOpen ? "bg-bg-hover text-text-primary" : ""}`}
            title={langString ? `OCR languages: ${langString}` : "OCR languages"}
            aria-label={`OCR languages: ${selectedLangs.length ? selectedLangs.join(", ") : "none"}`}
            aria-expanded={langDropdownOpen}
          >
            <Languages size={14} />
            {/* On a phone the chips cost the whole bar a row, so only the count
                stays visible; the accessible name carries the full list. */}
            <span className="sm:hidden text-xs text-text-secondary">
              {selectedLangs.length || "None"}
            </span>
            <span className="hidden sm:flex items-center gap-1 min-w-0 max-w-[220px] overflow-hidden">
              {selectedLangs.length === 0 ? (
                "None"
              ) : (
                // Labels rather than codes: `deu+eng` says nothing to most users.
                // Two fit the bar; the rest collapse into a count.
                selectedLangs.slice(0, 2).map((code) => (
                  <span
                    key={code}
                    title={code}
                    className="px-1.5 rounded bg-bg-muted border border-border-default text-xs leading-5 whitespace-nowrap"
                  >
                    {LANG_LABELS[code] ?? code}
                  </span>
                ))
              )}
              {selectedLangs.length > 2 && (
                <span className="text-xs text-text-tertiary">
                  +{selectedLangs.length - 2}
                </span>
              )}
            </span>
            <ChevronDown size={14} className="text-text-tertiary shrink-0" />
          </button>

          {langDropdownOpen && (
            <div className="absolute right-0 top-full mt-1 z-50 w-64 rounded-lg border border-border-default bg-bg-card shadow-xl overflow-hidden">
              {availableLangs.length > 8 && (
                <div className="p-1.5 border-b border-border-default">
                  <input
                    autoFocus
                    value={langFilter}
                    onChange={(e) => setLangFilter(e.target.value)}
                    placeholder="Filter languages…"
                    aria-label="Filter languages"
                    className="w-full px-2 py-1 rounded bg-bg-muted border border-border-default text-sm text-text-primary focus:outline-none focus:border-accent"
                  />
                </div>
              )}
              <div className="max-h-64 overflow-y-auto py-1">
                {filteredLangs.length === 0 && (
                  <div className="px-3 py-2 text-xs text-text-tertiary">No languages found</div>
                )}
                {filteredLangs.map((code) => {
                  const checked = selectedLangs.includes(code);
                  return (
                    <button
                      key={code}
                      type="button"
                      onClick={() => toggleLang(code)}
                      className="w-full flex items-center gap-2 px-3 py-1.5 text-sm text-text-primary hover:bg-bg-hover transition-colors cursor-pointer"
                    >
                      <span
                        className={`w-4 h-4 rounded border flex items-center justify-center shrink-0 ${
                          checked ? "bg-accent border-accent" : "border-border-default"
                        }`}
                      >
                        {checked && <Check size={12} className="text-white" />}
                      </span>
                      <span className="flex-1 text-left">{LANG_LABELS[code] ?? code}</span>
                      <span className="text-xs text-text-tertiary">{code}</span>
                    </button>
                  );
                })}
              </div>
              <div className="px-3 py-1.5 border-t border-border-default text-[11px] text-text-tertiary">
                Toggle to combine — e.g. deu+eng
              </div>
            </div>
          )}
        </div>

        {/* One dictionary per language: installed once, used by every TSV run. */}
        {!mobile && (
          <ToolbarButton
            icon={BookOpen}
            label="Dictionaries"
            onClick={() => setShowDictionaries(true)}
            title="Hunspell dictionaries for line-break hyphen repair (one per language)"
          />
        )}

        <ToolbarButton
          variant="solid"
          icon={recognizing ? Loader2 : Sparkles}
          iconProps={recognizing ? { className: "animate-spin" } : undefined}
          label={recognizing ? "Recognizing…" : "Recognize"}
          onClick={() => editedImage && recognize(editedImage, langString)}
          disabled={!canRecognize}
          title={
            !editedImage
              ? "Add an image first"
              : mode === "tsv"
                ? "Recognize and rebuild paragraphs in Rust"
                : "Recognize text"
          }
        />
        <ToolbarButton
          icon={copied ? Check : Copy}
          label={copied ? "Copied" : "Copy"}
          onClick={copyResult}
          disabled={!resultText}
          title="Copy the result text"
        />
      </Toolbar>

      {error && (
        <div className="shrink-0 mx-4 mt-4 px-4 py-3 rounded-lg bg-error-bg text-error-text text-sm flex items-start gap-2">
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

      {/* Narrow: one scrolling column with a collapsible image. Wide: a fixed
          two-pane split whose panes scroll internally, so the scan and the text
          are visible together. */}
      <div className="flex-1 min-h-0 overflow-y-auto p-4 lg:overflow-hidden">
        <div className="flex flex-col gap-4 lg:h-full lg:min-h-0">
          <div className="grid gap-4 lg:flex-1 lg:min-h-0 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.15fr)]">
            {/* ---- Image pane ---- */}
            <section className="rounded-xl border border-border-default bg-bg-card flex flex-col min-w-0 min-h-0 overflow-hidden">
              <div className="shrink-0 flex items-center gap-2 px-3 py-2 border-b border-border-default">
                <h2 className="text-sm font-semibold text-text-primary flex-1 min-w-0 truncate">
                  Image
                </h2>
                <span className="hidden xl:inline text-xs text-text-tertiary">
                  Drop a file, or use the toolbar
                </span>
                <ToolbarIconButton
                  className="lg:hidden"
                  icon={imageOpen ? ChevronUp : ChevronDown}
                  ariaLabel={imageOpen ? "Collapse image" : "Expand image"}
                  onClick={() => setImageOpen((v) => !v)}
                />
              </div>
              <div
                className={`p-3 ${imageOpen ? `${NARROW_IMAGE_HEIGHT} min-h-0` : "hidden"} lg:block lg:h-auto lg:flex-1 lg:min-h-0`}
              >
                <ImageEditor
                  src={sourceImage}
                  onImageReady={handleImageReady}
                  onError={setError}
                />
              </div>
            </section>

            {/* ---- Result pane ---- */}
            <section className="rounded-xl border border-border-default bg-bg-card flex flex-col min-w-0 min-h-0 overflow-hidden">
              {/* One header row: the mode on the left, that mode's single extra
                  action on the right. Everything else lives in the toolbar. */}
              <div className="shrink-0 flex flex-wrap items-center gap-2 px-3 py-2 border-b border-border-default">
                <div
                  role="group"
                  aria-label="Recognition mode"
                  className="flex items-center gap-0.5 p-0.5 rounded-lg bg-bg-muted border border-border-default"
                >
                  {/* TSV leads because it is the default; the active tab is the
                      one that answers a fresh image. */}
                  <ModeTab
                    active={mode === "tsv"}
                    onClick={() => setMode("tsv")}
                    icon={<Table2 size={14} />}
                    label="TSV"
                    title="Tesseract TSV layout — paragraphs rebuilt in Rust, no LLM"
                  />
                  <ModeTab
                    active={mode === "plain"}
                    onClick={() => setMode("plain")}
                    icon={<Type size={14} />}
                    label="Plain"
                    title="Tesseract plain text — better for multi-column and right-to-left pages, lines re-joined by an LLM on request"
                  />
                </div>
                <p className="hidden xl:block flex-1 min-w-0 truncate text-xs text-text-tertiary">
                  {mode === "plain"
                    ? "Tesseract text → optional LLM cleanup"
                    : "Tesseract TSV → paragraphs, line joins and dash spacing in Rust"}
                </p>
                {/* On a narrow pane the actions get their own line instead of
                    squeezing "Fix with LLM" into two words per line. */}
                <div className="ml-auto flex items-center gap-2 min-w-0 w-full sm:w-auto justify-end">
                  {mode === "plain" ? (
                    <>
                      <button
                        type="button"
                        onClick={() => fixWithLlm(llmModel)}
                        disabled={!resultText || fixingText}
                        className={`${tbBase} whitespace-nowrap bg-purple-500/10 text-purple-500 border border-purple-500/20 hover:bg-purple-500/20 disabled:hover:bg-purple-500/10`}
                        title="Use LLM to intelligently fix OCR text"
                      >
                        {fixingText ? (
                          <Loader2 size={14} className="animate-spin" />
                        ) : (
                          <Wand2 size={14} />
                        )}
                        {fixingText ? "Fixing…" : "Fix with LLM"}
                      </button>
                      <select
                        value={llmModel}
                        onChange={(e) => setLlmModel(e.target.value)}
                        className="min-w-0 max-w-[160px] px-2 py-1 rounded-md bg-bg-muted border border-border-default text-xs text-text-primary hover:bg-bg-hover transition-colors appearance-none cursor-pointer"
                        title="LLM model for text fixing"
                      >
                        {installedModels.length === 0 && <option value="">No models found</option>}
                        {installedModels.map((name) => (
                          <option key={name} value={name}>
                            {name}
                          </option>
                        ))}
                      </select>
                    </>
                  ) : (
                    <ToolbarButton
                      icon={Table2}
                      label="Raw TSV"
                      onClick={() => setShowRawTsv(true)}
                      disabled={!rawTsv}
                      title="Show the Tesseract TSV dump this result was built from"
                    />
                  )}
                </div>
              </div>

              <textarea
                value={resultText}
                onChange={(e) => setResultText(e.target.value)}
                aria-label="Recognized text"
                placeholder={
                  mode === "tsv"
                    ? "Recognized text appears here — paragraphs rebuilt from Tesseract's TSV. You can edit it before copying."
                    : "Recognized text appears here. You can edit it before copying."
                }
                style={{ fontSize: `${fontSize}px` }}
                className="flex-1 min-h-[240px] lg:min-h-0 w-full px-3 py-2 bg-transparent text-text-primary resize-y lg:resize-none focus:outline-none"
              />

              {/* Line-break hyphens: what the dictionary/heuristic decided, and
                  one click to overrule any of it. */}
              {mode === "tsv" && hyphens.length > 0 && (
                <HyphenPanel
                  hyphens={hyphens}
                  dictionaries={dictionaries}
                  uncovered={uncovered}
                  disabled={hyphensEdited}
                  installing={installingDictionary}
                  onDecide={setHyphenDecision}
                  onDecideAll={setAllHyphens}
                  onInstall={handleInstallDictionary}
                />
              )}

              <div className="shrink-0 flex flex-wrap items-center gap-2 px-3 py-1.5 border-t border-border-default">
                <p className="text-xs text-text-tertiary">
                  {resultText.length} characters · {words} words
                  {mode === "tsv" && tsvRowCount > 0 ? ` · ${tsvRowCount} TSV rows` : ""}
                  {recognizing && " · recognizing…"}
                </p>
                <span className="flex-1" />
                <div className="flex items-center gap-0.5">
                  <button
                    type="button"
                    onClick={() => setFont(fontSize - 2)}
                    disabled={fontSize <= FONT_MIN}
                    className="px-1.5 rounded text-xs font-semibold text-text-tertiary hover:text-text-primary hover:bg-bg-hover disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                    title="Smaller text"
                    aria-label="Smaller text"
                  >
                    A−
                  </button>
                  <span className="w-6 text-center text-xs text-text-tertiary">{fontSize}</span>
                  <button
                    type="button"
                    onClick={() => setFont(fontSize + 2)}
                    disabled={fontSize >= FONT_MAX}
                    className="px-1.5 rounded text-xs font-semibold text-text-tertiary hover:text-text-primary hover:bg-bg-hover disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                    title="Larger text"
                    aria-label="Larger text"
                  >
                    A+
                  </button>
                </div>
              </div>
            </section>
          </div>
        </div>
      </div>

      {showRawTsv && rawTsv && (
        <RawTsvOverlay tsv={rawTsv} rows={tsvRowCount} onClose={() => setShowRawTsv(false)} />
      )}

      {showDictionaries && (
        <HyphenDictionariesDialog
          onClose={() => {
            setShowDictionaries(false);
            // A dictionary appearing or disappearing changes the decisions of
            // the page on screen, so re-run it while the image is still loaded.
            if (dictionaryDirty.current && editedImage) {
              recognize(editedImage, langString);
            }
            dictionaryDirty.current = false;
          }}
          onChanged={() => {
            dictionaryDirty.current = true;
          }}
        />
      )}
    </main>
  );
}

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

/** One tab of the result-mode switcher (Plain / TSV). */
function ModeTab({
  active,
  onClick,
  icon,
  label,
  title,
}: {
  active: boolean;
  onClick: () => void;
  icon: React.ReactNode;
  label: string;
  title: string;
}) {
  return (
    <button
      type="button"
      aria-pressed={active}
      onClick={onClick}
      title={title}
      className={`${tbBase} border cursor-pointer ${
        active
          ? "bg-bg-card border-border-default text-text-primary"
          : "border-transparent text-text-secondary hover:text-text-primary hover:bg-bg-hover"
      }`}
    >
      {icon}
      {label}
    </button>
  );
}

/**
 * Read-only viewer for the Tesseract TSV dump. Overlaid rather than inlined so
 * opening it never reflows the result text under the user's cursor.
 */
function RawTsvOverlay({
  tsv,
  rows,
  onClose,
}: {
  tsv: string;
  rows: number;
  onClose: () => void;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div
      className="fixed inset-0 z-[80] flex items-center justify-center bg-black/40 p-4"
      onClick={onClose}
      role="presentation"
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Raw Tesseract TSV"
        className="flex h-full max-h-[80vh] w-full max-w-4xl flex-col overflow-hidden rounded-xl border border-border-default bg-bg-card shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="shrink-0 flex items-center gap-2 px-4 py-2 border-b border-border-default">
          <h2 className="flex-1 min-w-0 text-sm font-semibold text-text-primary">
            Raw Tesseract TSV
          </h2>
          <span className="text-xs text-text-tertiary">{rows} word rows</span>
          <ToolbarIconButton icon={X} ariaLabel="Close raw TSV" onClick={onClose} />
        </div>
        <pre className="flex-1 min-h-0 overflow-auto p-3 font-mono text-xs leading-5 text-text-secondary whitespace-pre">
          {tsv}
        </pre>
      </div>
    </div>
  );
}
