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

/** One line-break hyphen of a TSV result, with the decision the rebuild made. */
export interface OcrHyphen {
  /** Fragment before the hyphen, hyphen already stripped. */
  left: string;
  /** The hyphen character that ended the line (`-`, `‐`, `‑`). */
  hyphen: string;
  /** First word of the following line. */
  right: string;
  /** What "Join" produces: `left` + `right`. */
  joined: string;
  decision: "join" | "keep";
  /** Evidence: `dictionary`, `unknown-word`, `lowercase`, `uppercase`, `digit`, `policy`. */
  reason: string;
}

/** One dictionary the page's language set maps to, with its install state. */
export interface OcrDictionary {
  /** Hunspell/npm code, e.g. `de`. */
  code: string;
  label: string;
  /** The Tesseract code of the selection that mapped to it (`deu` for `de`). */
  language: string;
  installed: boolean;
}

/** A catalog entry as reported by `ocr_hyphen_dictionary_status`. */
export interface OcrDictionaryStatus {
  code: string;
  label: string;
  version: string;
  license: string;
  /** Tesseract codes this dictionary serves. */
  tesseract: string[];
  installed: boolean;
  /** Bytes on disk — 0 when not installed. */
  size_bytes: number;
  /** What the download costs. */
  approx_bytes: number;
}

/** Tauri command result of `ocr_recognize_tsv`. */
interface OcrTsvResult {
  /** Text with the hyphen decisions applied. */
  text: string;
  /** The same page with every line-break hyphen kept (`Sprach- verarbeitung`). */
  baseline: string;
  tsv: string;
  hyphens: OcrHyphen[];
  /** Every dictionary of the recognised language set, installed or not. */
  dictionaries: OcrDictionary[];
  /** Selected Tesseract codes with no dictionary at all (CJK, Arabic, …). */
  uncovered: string[];
}

/**
 * Index just past `hyphen.left` at or after `from`, or -1.
 *
 * Mirrors `ocr::hyphen::locate`: an earlier occurrence of the fragment that is
 * not followed by the hyphen and a space (the same letters inside a longer
 * word, say) must not end the search, or that decision would look broken — the
 * click is registered but the text never moves.
 */
function locateHyphen(baseline: string, hyphen: OcrHyphen, from: number): number {
  if (!hyphen.left) return -1;

  let search = from;

  for (;;) {
    const at = baseline.indexOf(hyphen.left, search);
    if (at < 0) return -1;

    const afterLeft = at + hyphen.left.length;
    const afterHyphen = afterLeft + hyphen.hyphen.length;

    if (baseline.startsWith(hyphen.hyphen, afterLeft) && baseline[afterHyphen] === " ") {
      return afterLeft;
    }

    search = afterLeft;
  }
}

/**
 * Render a page from its kept-hyphen baseline with the current decisions.
 *
 * Mirrors `ocr::hyphen::apply` in the backend: the rebuild leaves every
 * candidate as `left` + hyphen + one space + `right`, so a single forward scan
 * locates them in reading order. A join drops exactly the hyphen and that
 * space — nothing else is rewritten, so every other word survives verbatim.
 */
function renderHyphens(baseline: string, hyphens: OcrHyphen[]): string {
  const removals: Array<{ at: number; length: number }> = [];
  let cursor = 0;

  for (const hyphen of hyphens) {
    const afterLeft = locateHyphen(baseline, hyphen, cursor);
    if (afterLeft < 0) continue;

    if (hyphen.decision === "join") {
      removals.push({ at: afterLeft, length: hyphen.hyphen.length + 1 });
    }

    // Resume after the next word so a repeated fragment cannot match the same
    // region twice.
    cursor = afterLeft + hyphen.hyphen.length + 1;
    if (baseline.startsWith(hyphen.right, cursor)) cursor += hyphen.right.length;
  }

  if (removals.length === 0) return baseline;

  let out = "";
  let last = 0;
  for (const removal of removals) {
    out += baseline.slice(last, removal.at);
    last = removal.at + removal.length;
  }

  return out + baseline.slice(last);
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
  // Line-break hyphens of the last TSV result. `baseline` is the same page with
  // every hyphen kept, so toggling one re-renders from it instead of patching
  // the visible text.
  const [hyphens, setHyphens] = useState<OcrHyphen[]>([]);
  const [baseline, setBaseline] = useState("");
  // The dictionaries the page's languages map to, and the languages no
  // dictionary covers — together they explain every hyphen decision.
  const [dictionaries, setDictionaries] = useState<OcrDictionary[]>([]);
  const [uncovered, setUncovered] = useState<string[]>([]);
  // Set once the result is edited by hand or by the LLM: the candidates can no
  // longer be located in the text, so the review list turns read-only.
  const [edited, setEdited] = useState(false);
  // Code of the dictionary being downloaded right now.
  const [installingDictionary, setInstallingDictionary] = useState<string | null>(null);

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
      // minimize: the user is snipping a region of whatever is under the app, so
      // the window has to get out of the way. (The command defaults to false —
      // that default belongs to the unattended MCP path, not to this button.)
      const dataUrl = await invoke<string>("capture_screenshot", { minimize: true });
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
      else {
        setTsvText(text);
        setEdited(true);
      }
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
          setBaseline(res.baseline);
          setHyphens(res.hyphens);
          setDictionaries(res.dictionaries);
          setUncovered(res.uncovered);
          setEdited(false);
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

  // ---- Hyphen review ----

  // Re-render from the baseline: the source of truth is the decision list, and
  // the visible text is derived, so "Join all" and single toggles agree.
  const applyHyphens = useCallback(
    (next: OcrHyphen[]) => {
      setHyphens(next);
      setTsvText(renderHyphens(baseline, next));
    },
    [baseline]
  );

  const setHyphenDecision = useCallback(
    (index: number, decision: "join" | "keep") => {
      if (edited) return;
      applyHyphens(
        hyphens.map((hyphen, i) => (i === index ? { ...hyphen, decision } : hyphen))
      );
    },
    [applyHyphens, edited, hyphens]
  );

  const setAllHyphens = useCallback(
    (decision: "join" | "keep") => {
      if (edited) return;
      applyHyphens(hyphens.map((hyphen) => ({ ...hyphen, decision })));
    },
    [applyHyphens, edited, hyphens]
  );

  // One-time download of one language's Hunspell dictionary. The caller
  // re-runs recognition afterwards, which is what picks the dictionary up.
  const installDictionary = useCallback(async (code: string) => {
    if (!isTauri()) return false;
    setInstallingDictionary(code);
    setError("");
    try {
      await invoke("ocr_hyphen_dictionary_download", { code });
      setDictionaries((prev) =>
        prev.map((entry) => (entry.code === code ? { ...entry, installed: true } : entry))
      );
      return true;
    } catch (e) {
      setError(`Dictionary download failed: ${errorMessage(e)}`);
      return false;
    } finally {
      setInstallingDictionary(null);
    }
  }, []);

  const reset = useCallback(() => {
    setSourceImage(null);
    setPlainText("");
    setTsvText("");
    setRawTsv("");
    setHyphens([]);
    setBaseline("");
    setDictionaries([]);
    setUncovered([]);
    setEdited(false);
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
    // hyphen review (TSV mode)
    hyphens,
    dictionaries,
    uncovered,
    edited,
    installingDictionary,
    setHyphenDecision,
    setAllHyphens,
    installDictionary,
    // shared
    error,
    setError,
    reset,
  };
}
