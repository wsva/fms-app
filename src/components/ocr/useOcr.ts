"use client";

import { useCallback, useEffect, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { isTauri } from "@/lib/tauri";

const IMAGE_FILTERS = [
  {
    name: "Images",
    extensions: ["png", "jpg", "jpeg", "bmp", "webp", "tif", "tiff"],
  },
];

/**
 * Result post-processing mode:
 * - `plain` — Tesseract plain text, re-joined afterwards by an LLM on request.
 * - `tsv` — Tesseract TSV layout, rebuilt into paragraphs in Rust (no model).
 */
export type OcrMode = "plain" | "tsv";

/** Tauri command result of `ocr_recognize_tsv`. */
interface OcrTsvResult {
  text: string;
  tsv: string;
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * All OCR page logic: image acquisition (file dialog, native drag-drop,
 * screenshot), recognition, and copy. Keeps `OcrPage` presentational.
 */
export function useOcr() {
  // ---- Image source (input to the editor) ----
  const [sourceImage, setSourceImage] = useState<string | null>(null);

  // ---- Recognition ----
  const [recognizing, setRecognizing] = useState(false);
  const [fixingText, setFixingText] = useState(false);
  // One result per mode: switching tabs never overwrites the other result.
  // TSV is the default — the Rust rebuild resolves paragraphs, line joins and
  // hyphenation with no model, so it is the better starting point; Plain stays
  // for multi-column and right-to-left pages, where reading order is safer left
  // to Tesseract.
  const [mode, setModeState] = useState<OcrMode>("tsv");

  // Remember a deliberate switch to Plain, the same way the font size is kept.
  // The saved value is applied in an effect, not as lazy initial state, so the
  // prerendered HTML and the first client paint agree.
  useEffect(() => {
    const saved = localStorage.getItem("ocr_mode");
    if (saved === "plain" || saved === "tsv") setModeState(saved);
  }, []);
  const setMode = useCallback((next: OcrMode) => {
    setModeState(next);
    localStorage.setItem("ocr_mode", next);
  }, []);
  const [plainText, setPlainText] = useState("");
  const [tsvText, setTsvText] = useState("");
  // Raw Tesseract TSV behind the last TSV-mode recognition.
  const [rawTsv, setRawTsv] = useState("");

  // ---- Shared UI state ----
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);

  // ---- Image acquisition ----

  const selectFile = useCallback(async () => {
    if (!isTauri()) return;
    setError("");
    try {
      const selected = await open({ multiple: false, filters: IMAGE_FILTERS });
      const path = Array.isArray(selected) ? selected[0] : selected;
      if (path) setSourceImage(convertFileSrc(path));
    } catch (e) {
      setError(`Failed to open image: ${errorMessage(e)}`);
    }
  }, []);

  const captureScreenshot = useCallback(async () => {
    if (!isTauri()) return;
    setError("");
    try {
      const dataUrl = await invoke<string>("capture_screenshot");
      setSourceImage(dataUrl);
    } catch (e) {
      const msg = errorMessage(e);
      // A dismissed snip overlay is a normal user action, not an error.
      if (!msg.toLowerCase().includes("cancelled")) {
        setError(`Screenshot failed: ${msg}`);
      }
    }
  }, []);

  // Native Tauri drag-drop (avoids the HTML5 DnD conflict in the webview).
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: UnlistenFn | null = null;
    let cancelled = false;

    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "drop") {
          const path = event.payload.paths[0];
          if (path) {
            setError("");
            setSourceImage(convertFileSrc(path));
          }
        }
      })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {
        /* drag-drop unavailable */
      });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // ---- Recognition ----

  const resultText = mode === "plain" ? plainText : tsvText;

  const setResultText = useCallback(
    (text: string) => {
      if (mode === "plain") setPlainText(text);
      else setTsvText(text);
    },
    [mode]
  );

  const recognize = useCallback(
    async (imageBase64: string, lang?: string) => {
      if (!isTauri()) return;
      // Capture the mode: the user may switch tabs while Tesseract runs.
      const target = mode;
      setRecognizing(true);
      setError("");
      try {
        if (target === "tsv") {
          const res = await invoke<OcrTsvResult>("ocr_recognize_tsv", {
            imageBase64,
            lang: lang || null,
          });
          setTsvText(res.text);
          setRawTsv(res.tsv);
        } else {
          const text = await invoke<string>("ocr_recognize", {
            imageBase64,
            lang: lang || null,
          });
          setPlainText(text);
        }
      } catch (e) {
        setError(
          `${target === "tsv" ? "TSV recognition" : "Recognition"} failed: ${errorMessage(e)}`
        );
      } finally {
        setRecognizing(false);
      }
    },
    [mode]
  );

  const copyResult = useCallback(async () => {
    if (!resultText) return;
    try {
      await navigator.clipboard.writeText(resultText);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable */
    }
  }, [resultText]);

  // Use LLM to intelligently fix OCR text (merge lines, fix hyphens, etc.)
  const fixWithLlm = useCallback(async (model: string) => {
    if (!isTauri() || !resultText) return;
    setFixingText(true);
    setError("");
    try {
      const response = await invoke<{ content: string }>("llm_chat", {
        model,
        messages: [
          {
            role: "system",
            content: `You are an OCR text post-processor. Fix the following OCR output:
1. Merge lines that were incorrectly split (join words that should be on the same line)
2. Fix hyphenated words split across lines (e.g., "exam-\\nple" → "example")
3. Preserve intentional paragraph breaks (double newlines)
4. Fix obvious OCR errors if context makes them clear
5. Do NOT change the meaning or add content
6. Do NOT use LaTeX, MathJax, or any markup (no $, \\text{}, \\mathrm{}, etc.)
7. Output plain text only — no formatting of any kind

Return ONLY the corrected plain text, no explanations.`,
          },
          {
            role: "user",
            content: resultText,
          },
        ],
      });
      setResultText(response.content);
    } catch (e) {
      setError(`LLM fix failed: ${errorMessage(e)}`);
    } finally {
      setFixingText(false);
    }
  }, [resultText, setResultText, setError]);

  const reset = useCallback(() => {
    setSourceImage(null);
    setPlainText("");
    setTsvText("");
    setRawTsv("");
    setError("");
  }, []);

  return {
    // image source
    sourceImage,
    setSourceImage,
    selectFile,
    captureScreenshot,
    // recognition
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
    // shared
    error,
    setError,
    reset,
  };
}
