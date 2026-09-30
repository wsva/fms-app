"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import {
  ScanText,
  ImagePlus,
  MonitorUp,
  Loader2,
  Sparkles,
  X,
  Languages,
  ZoomIn,
  ZoomOut,
  Wand2,
  ChevronDown,
  Check,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/tauri";
import { useOcr } from "./useOcr";
import ImageEditor from "./ImageEditor";

export default function OcrPage() {
  const {
    sourceImage,
    selectFile,
    captureScreenshot,
    recognizing,
    recognize,
    resultText,
    setResultText,
    fixWithLlm,
    fixingText,
    error,
    setError,
    reset,
  } = useOcr();

  // The editor's current PNG (after rotate/crop) — what we send to recognize.
  const [editedImage, setEditedImage] = useState<string | null>(null);

  // Selected OCR languages for Tesseract (multi-select, combined with '+').
  // Friendly labels for common languages; unknown codes shown as-is.
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
  const [availableLangs, setAvailableLangs] = useState<string[]>([]);
  const [selectedLangs, setSelectedLangs] = useState<string[]>([]);
  const [langDropdownOpen, setLangDropdownOpen] = useState(false);
  const langDropdownRef = useRef<HTMLDivElement>(null);

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

  // Font size for result textarea (in px).
  const [fontSize, setFontSize] = useState(24);

  // LLM model for text fixing (shared with LLM Chat page via localStorage).
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
  }, []);

  const handleImageReady = useCallback((dataUrl: string | null) => {
    setEditedImage(dataUrl);
  }, []);

  const canRecognize = !!editedImage && !recognizing;

  return (
    <main className="flex-1 flex flex-col h-full">
      {/* Header */}
      <div className="p-4 border-b border-border-default flex items-center gap-3">
        <ScanText size={20} className="text-text-secondary" />
        <h1 className="text-lg font-semibold flex-1">OCR</h1>
        <span className="text-xs text-text-tertiary">Tesseract</span>
      </div>

      <div className="flex-1 overflow-y-auto p-4">
        <div className="space-y-5">
          {/* Error banner */}
          {error && (
            <div className="px-4 py-3 rounded-lg bg-error-bg text-error-text text-sm flex items-start gap-2">
              <span className="flex-1">{error}</span>
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

          {/* Editor + result */}
          <div className="space-y-4">
            <section className="rounded-xl border border-border-default bg-bg-card p-4 min-w-0">
              <div className="flex flex-wrap items-center gap-2 mb-3">
                <h2 className="text-sm font-semibold text-text-primary flex-1">Image</h2>
                <ToolButton onClick={selectFile} icon={<ImagePlus size={16} />} label="Select image" />
                <ToolButton
                  onClick={captureScreenshot}
                  icon={<MonitorUp size={16} />}
                  label="Screenshot"
                />
                {sourceImage && (
                  <ToolButton onClick={reset} icon={<X size={16} />} label="Clear" />
                )}
              </div>
              <ImageEditor
                src={sourceImage}
                onImageReady={handleImageReady}
                onError={setError}
              />
            </section>

            <section className="rounded-xl border border-border-default bg-bg-card p-4 flex flex-col min-w-0">
              {/* Action bar: Recognize + Language + LLM fix */}
              <div className="flex flex-wrap items-center gap-2 mb-3">
                <h2 className="text-sm font-semibold text-text-primary flex-1">Result</h2>
                <div className="relative" ref={langDropdownRef}>
                  <button
                    type="button"
                    onClick={() => setLangDropdownOpen((o) => !o)}
                    className="inline-flex items-center gap-2 pl-3 pr-2 py-2 rounded-lg bg-bg-muted border border-border-default text-sm font-medium text-text-primary hover:bg-bg-hover transition-colors cursor-pointer"
                    title="OCR languages"
                  >
                    <Languages size={16} className="text-text-tertiary shrink-0" />
                    <span className="max-w-[120px] truncate">
                      {selectedLangs.length === 0
                        ? "None"
                        : selectedLangs.join("+")}
                    </span>
                    <ChevronDown size={14} className="text-text-tertiary shrink-0" />
                  </button>
                  {langDropdownOpen && (
                    <div className="absolute right-0 top-full mt-1 z-50 w-48 max-h-64 overflow-y-auto rounded-lg border border-border-default bg-bg-card shadow-lg">
                      {availableLangs.length === 0 && (
                        <div className="px-3 py-2 text-xs text-text-tertiary">No languages found</div>
                      )}
                      {availableLangs.map((code) => {
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
                                checked
                                  ? "bg-accent border-accent"
                                  : "border-border-default"
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
                  )}
                </div>
                <button
                  type="button"
                  onClick={() => editedImage && recognize(editedImage, langString)}
                  disabled={!canRecognize}
                  className="inline-flex items-center gap-2 px-4 py-2 rounded-lg bg-accent text-white text-sm font-medium disabled:opacity-40 disabled:cursor-not-allowed hover:opacity-90 transition-opacity"
                  title={
                    !editedImage
                      ? "Add an image first"
                      : "Recognize text"
                  }
                >
                  {recognizing ? (
                    <Loader2 size={16} className="animate-spin" />
                  ) : (
                    <Sparkles size={16} />
                  )}
                  {recognizing ? "Recognizing…" : "Recognize"}
                </button>
              </div>

              {/* LLM post-processing bar */}
              <div className="flex flex-wrap items-center gap-2 mb-3">
                <button
                  type="button"
                  onClick={() => fixWithLlm(llmModel)}
                  disabled={!resultText || fixingText}
                  className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-purple-500/10 text-purple-500 border border-purple-500/20 text-sm font-medium disabled:opacity-40 disabled:cursor-not-allowed hover:bg-purple-500/20 transition-colors"
                  title="Use LLM to intelligently fix OCR text"
                >
                  {fixingText ? <Loader2 size={14} className="animate-spin" /> : <Wand2 size={14} />}
                  {fixingText ? "Fixing…" : "Fix with LLM"}
                </button>
                <select
                  value={llmModel}
                  onChange={(e) => setLlmModel(e.target.value)}
                  className="px-2 py-1.5 rounded-lg bg-bg-muted border border-border-default text-xs font-medium text-text-primary hover:bg-bg-hover transition-colors appearance-none cursor-pointer"
                  title="LLM model for text fixing"
                >
                  {installedModels.length === 0 && (
                    <option value="">No models found</option>
                  )}
                  {installedModels.map((name) => (
                    <option key={name} value={name}>{name}</option>
                  ))}
                </select>
              </div>

              <textarea
                value={resultText}
                onChange={(e) => setResultText(e.target.value)}
                placeholder="Recognized text appears here. You can edit it before copying."
                style={{ fontSize: `${fontSize}px` }}
                className="flex-1 min-h-[240px] w-full px-3 py-2 rounded-lg bg-bg-muted border border-border-default text-text-primary resize-y focus:outline-none focus:border-accent"
              />
              <div className="flex items-center justify-between mt-2">
                <p className="text-xs text-text-tertiary">
                  {resultText.length} characters
                  {recognizing && " · recognizing…"}
                </p>
                <div className="flex items-center gap-1">
                  <button
                    type="button"
                    onClick={() => setFontSize((s) => Math.max(10, s - 2))}
                    className="p-1 rounded text-text-tertiary hover:text-text-primary hover:bg-bg-hover transition-colors"
                    title="Decrease font size"
                  >
                    <ZoomOut size={14} />
                  </button>
                  <span className="text-xs text-text-tertiary w-8 text-center">{fontSize}</span>
                  <button
                    type="button"
                    onClick={() => setFontSize((s) => Math.min(32, s + 2))}
                    className="p-1 rounded text-text-tertiary hover:text-text-primary hover:bg-bg-hover transition-colors"
                    title="Increase font size"
                  >
                    <ZoomIn size={14} />
                  </button>
                </div>
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

function ToolButton({
  onClick,
  icon,
  label,
}: {
  onClick: () => void;
  icon: React.ReactNode;
  label: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="inline-flex items-center gap-2 px-3 py-2 rounded-lg bg-bg-muted border border-border-default text-sm font-medium text-text-primary hover:bg-bg-hover transition-colors"
    >
      {icon}
      <span>{label}</span>
    </button>
  );
}
