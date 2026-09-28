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
  const [resultText, setResultText] = useState("");

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

  const recognize = useCallback(async (imageBase64: string, lang?: string) => {
    if (!isTauri()) return;
    setRecognizing(true);
    setError("");
    try {
      const text = await invoke<string>("ocr_recognize", { imageBase64, lang: lang || null });
      setResultText(text);
    } catch (e) {
      setError(`Recognition failed: ${errorMessage(e)}`);
    } finally {
      setRecognizing(false);
    }
  }, []);

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

Return ONLY the corrected text, no explanations.`,
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
  }, [resultText, setError]);

  const reset = useCallback(() => {
    setSourceImage(null);
    setResultText("");
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
    resultText,
    setResultText,
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
