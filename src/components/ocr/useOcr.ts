"use client";

import { useCallback, useEffect, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import type { HubStage } from "@/components/read_aloud/HubStatusOverlay";

const IMAGE_FILTERS = [
  {
    name: "Images",
    extensions: ["png", "jpg", "jpeg", "bmp", "webp", "tif", "tiff"],
  },
];

/**
 * What produces the text:
 * - `tsv` — Tesseract TSV layout, rebuilt into paragraphs in Rust (no model).
 * - `plain` — Tesseract plain text, re-joined afterwards by an LLM on request.
 * - `ollama` — a vision model served by Ollama (`glm-ocr` and friends): what
 *   reads a photographed or ornate page Tesseract cannot, with its markdown and
 *   LaTeX reference marks stripped in Rust afterwards.
 */
export type OcrMode = "tsv" | "plain" | "ollama";

/** What each engine is called when a run fails. */
const ENGINE_LABEL: Record<OcrMode, string> = {
  tsv: "TSV recognition",
  plain: "Recognition",
  ollama: "Ollama OCR",
};

/**
 * Names that read as an OCR or vision model, so the engine defaults to one that
 * can see instead of to whatever sorts first in the installed list.
 */
const VISION_MODEL_HINT = /(ocr|vl|vision|llava|minicpm|moondream)/i;

function preferredOcrModel(names: string[]): string {
  return names.find((name) => VISION_MODEL_HINT.test(name)) ?? names[0];
}

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

/** Tauri command result of `ocr_recognize_ollama`. */
interface OcrOllamaResult {
  /** The answer after the Rust cleanup: what the user keeps. */
  text: string;
  /** The model's own answer, before the cleanup. */
  raw: string;
  /** Reference/footnote marks the cleanup removed. */
  references: number;
  /** Other markup the cleanup unwrapped or dropped. */
  markup: number;
  model: string;
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
 * screenshot, camera), recognition (local on the desktop, the Hub's TSV rebuild
 * for the Android client), the hand-off of an image to the Hub's own OCR page,
 * and copy. Keeps `OcrPage` presentational.
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
    if (saved === "plain" || saved === "tsv" || saved === "ollama") setModeState(saved);
  }, []);
  const setMode = useCallback((next: OcrMode) => {
    setModeState(next);
    localStorage.setItem("ocr_mode", next);
  }, []);
  const [plainText, setPlainText] = useState("");
  const [tsvText, setTsvText] = useState("");
  // The vision model's text, and what the cleanup stripped from its answer.
  const [ollamaText, setOllamaText] = useState("");
  const [ollamaReport, setOllamaReport] = useState<{
    references: number;
    markup: number;
  } | null>(null);
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

  // ---- Hub send (Android "Send to Hub") ----
  // A send is a hub_mcp_call in flight; `sentToHub` is the transient success
  // flash on the button.
  const [sending, setSending] = useState(false);
  const [sentToHub, setSentToHub] = useState(false);

  // ---- Hub call staging (Android) ----
  // The phase the remote call is in, on the same contract the read-aloud panels
  // use: `sending` while this device is still connecting and uploading, `waiting`
  // once the backend reports the tool call dispatched, `done` on the reply,
  // `error` holding the Hub's own text. Without it a cold Tesseract on the Hub
  // reads as a slow phone.
  const [hubStage, setHubStage] = useState<HubStage | null>(null);
  const [hubError, setHubError] = useState<string | null>(null);
  // The last Hub call that failed, with the arguments it was given, so the
  // retry the error banner offers is that same call and nothing vaguer.
  const [lastFailed, setLastFailed] = useState<
    { kind: "recognize" | "send"; imageBase64: string; lang: string } | null
  >(null);

  // ---- Ollama models ----
  // One list, two jobs: the OCR engine's model and the plain path's text fixer.
  // The fixer's pick is cached under `llm_selected_model`, which the
  // text-selection menu's Ask-the-model actions also read (lib/llm/ask.ts), so
  // one choice serves every LLM feature. The OCR model keeps its own key: an
  // OCR model is not a chat model, and swapping one must not swap the other.
  const [models, setModels] = useState<string[]>([]);
  const [llmModel, setLlmModelState] = useState("");
  const [ollamaModel, setOllamaModelState] = useState("");

  const setLlmModel = useCallback((model: string) => {
    setLlmModelState(model);
    if (model) localStorage.setItem("llm_selected_model", model);
  }, []);

  const setOllamaModel = useCallback((model: string) => {
    setOllamaModelState(model);
    if (model) localStorage.setItem("ocr_ollama_model", model);
  }, []);

  useEffect(() => {
    if (!isTauri()) return;
    // Read the cache here rather than relying on the state the mount effect
    // restores, so this needs no dependency on either selection.
    const cachedFix = localStorage.getItem("llm_selected_model");
    const cachedOcr = localStorage.getItem("ocr_ollama_model");
    invoke<{ installed: { name: string }[] }>("llm_list_models")
      .then((res) => {
        const names = res.installed.map((m) => m.name).sort();
        setModels(names);
        if (names.length === 0) return;
        setLlmModel(names.find((name) => name === cachedFix) ?? names[0]);
        setOllamaModel(names.find((name) => name === cachedOcr) ?? preferredOcrModel(names));
      })
      .catch(() => {
        // Ollama not running — the engine selector then has nothing to offer.
      });
  }, [setLlmModel, setOllamaModel]);

  // ---- Shared UI state ----
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);

  /**
   * Run one Hub call while reporting its phases to `hubStage`, so the caller's
   * overlay can name the stage instead of holding one spinner over a call that
   * a cold model on the Hub can stretch into minutes. The `waiting` milestone is
   * the backend's own (`hub-mcp-phase`, emitted once the tool call is
   * dispatched), not a guess from this side. A failure leaves the overlay on
   * `error` holding the Hub's text and rethrows, so each caller still records
   * its own outcome.
   */
  const withHubStage = useCallback(
    async <T>(run: () => Promise<T>): Promise<T> => {
      setHubStage("sending");
      setHubError(null);
      const subscribed = listen<string>("hub-mcp-phase", (e) => {
        if (e.payload === "waiting") setHubStage((s) => (s === "sending" ? "waiting" : s));
      }).catch(() => null);
      try {
        const reply = await run();
        setHubStage("done");
        // A beat of confirmation, then out of the way — the result is what the
        // user came for, and the overlay must not linger over it.
        setTimeout(() => setHubStage((s) => (s === "done" ? null : s)), 600);
        return reply;
      } catch (e) {
        setHubStage("error");
        setHubError(errorMessage(e));
        throw e;
      } finally {
        // Resolved before the listener is dropped, so a `waiting` that raced the
        // reply is never delivered to an already-unsubscribed call.
        const unlisten = await subscribed;
        unlisten?.();
      }
    },
    []
  );

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

  // A photo the Android webview produced: a camera capture or a gallery pick.
  // Both arrive as a `File` — the webview owns the intent and its permission —
  // so the hook only turns it into the data URL the editor already accepts.
  //
  // This is the mobile twin of `selectFile`, and it is the reason the phone does
  // not use `selectFile`: `plugin-dialog`'s `open` answers with a `content://`
  // URI there, and `convertFileSrc` can only serve a filesystem path, so the
  // picker would return an image that never renders.
  const loadImageFile = useCallback((file: File | null | undefined) => {
    if (!file) return;
    setError("");
    const reader = new FileReader();
    reader.onload = () =>
      setSourceImage(typeof reader.result === "string" ? reader.result : null);
    reader.onerror = () => setError("Could not read the photo.");
    reader.readAsDataURL(file);
  }, []);

  // Available Tesseract languages. Desktop reads its own install; the Android
  // client has no local Tesseract, so it asks the Hub over `hub_mcp_call`. The
  // codes come back as the Hub installed them, which is exactly the set that
  // Hub's `ocr_recognize` accepts.
  const loadLanguages = useCallback(async (): Promise<string[]> => {
    if (!isTauri()) return [];
    if (isMobileApp()) {
      const reply = await invoke<{ status: string; languages?: string[] }>("hub_mcp_call", {
        tool: "ocr_list_languages",
        args: {},
      });
      return reply.languages ?? [];
    }
    return await invoke<string[]>("ocr_list_languages");
  }, []);

  // Hand the current image to the paired Hub's own OCR page: the Hub opens its
  // page and loads it, where the full desktop TSV and hyphen tooling runs.
  const sendToHub = useCallback(
    async (imageBase64: string) => {
      if (!isTauri() || !imageBase64) return;
      setSending(true);
      setError("");
      try {
        await withHubStage(() =>
          invoke("hub_mcp_call", {
            tool: "ocr_open_image",
            args: { image_base64: imageBase64 },
          })
        );
        setLastFailed(null);
        setSentToHub(true);
        setTimeout(() => setSentToHub(false), 2000);
      } catch (e) {
        setLastFailed({ kind: "send", imageBase64, lang: "" });
        setError(
          `Send to the Hub failed: ${errorMessage(e)}. The Hub is unreachable or MCP is off — wake it and send again.`
        );
      } finally {
        setSending(false);
      }
    },
    [withHubStage]
  );

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

  const resultText =
    mode === "tsv" ? tsvText : mode === "plain" ? plainText : ollamaText;

  const setResultText = useCallback(
    (text: string) => {
      if (mode === "tsv") {
        setTsvText(text);
        setEdited(true);
      } else if (mode === "plain") setPlainText(text);
      else setOllamaText(text);
    },
    [mode]
  );

  const recognize = useCallback(
    async (imageBase64: string, lang?: string) => {
      if (!isTauri()) return;
      // The Android client has no local Tesseract: recognition is a Hub call,
      // and it asks for the same TSV rebuild the desktop page runs, because the
      // Rust paragraph join beats a line-by-line dump on a phone screen.
      const mobile = isMobileApp();
      // Capture the mode: the user may switch tabs while Tesseract runs.
      const target = mode;
      setRecognizing(true);
      setError("");
      try {
        if (mobile) {
          // Only the decided text is kept. The hyphen review, its dictionaries
          // and the raw dump are desktop surfaces — the panel that offers them
          // is not rendered here, and installing one calls a command that is not
          // even compiled into this build.
          const reply = await withHubStage(() =>
            invoke<{ status: string; text?: string }>("hub_mcp_call", {
              tool: "ocr_recognize_tsv",
              args: { image_base64: imageBase64, lang: lang || "" },
            })
          );
          setLastFailed(null);
          setTsvText(reply.text ?? "");
        } else if (target === "ollama") {
          // The cleanup (reference marks, markdown) runs in Rust, so `text` is
          // already the page and `raw` is only kept for the log.
          const res = await invoke<OcrOllamaResult>("ocr_recognize_ollama", {
            imageBase64,
            model: ollamaModel,
          });
          setOllamaText(res.text);
          setOllamaReport({ references: res.references, markup: res.markup });
        } else if (target === "tsv") {
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
        const msg = errorMessage(e);
        if (mobile) setLastFailed({ kind: "recognize", imageBase64, lang: lang || "" });
        setError(
          mobile
            ? `Recognition on the Hub failed: ${msg}. The Hub is unreachable or MCP is off — wake it and retry.`
            : `${ENGINE_LABEL[target]} failed: ${msg}`
        );
      } finally {
        setRecognizing(false);
      }
    },
    [mode, ollamaModel, withHubStage]
  );

  /** Re-run the Hub call the last error came from, with the arguments it was
   *  given. The retry the offline message asks for — the image and the language
   *  are still exactly as the user left them. */
  const retryLast = useCallback(() => {
    const failed = lastFailed;
    if (!failed) return;
    setLastFailed(null);
    setError("");
    if (failed.kind === "send") void sendToHub(failed.imageBase64);
    else void recognize(failed.imageBase64, failed.lang);
  }, [lastFailed, recognize, sendToHub]);

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
    setOllamaText("");
    setOllamaReport(null);
    setRawTsv("");
    setHyphens([]);
    setBaseline("");
    setDictionaries([]);
    setUncovered([]);
    setEdited(false);
    setLastFailed(null);
    setError("");
  }, []);

  /** Drop the staging overlay. The one exit it offers, in every phase: the
   *  call itself is not cancellable here, but nothing is lost by closing the
   *  window on it — a send that lands afterwards still reaches the Hub. */
  const dismissHubStatus = useCallback(() => {
    setHubStage(null);
    setHubError(null);
  }, []);

  return {
    // image source
    sourceImage,
    setSourceImage,
    selectFile,
    captureScreenshot,
    loadImageFile,
    // languages (Hub-backed on the Android client)
    loadLanguages,
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
    // Ollama: the installed models, the OCR engine's model, and what the
    // cleanup stripped from the last answer.
    models,
    ollamaModel,
    setOllamaModel,
    llmModel,
    setLlmModel,
    ollamaReport,
    // Hub send (Android "Send to Hub")
    sendToHub,
    sending,
    sentToHub,
    // Hub call staging (Android): the phase the remote call is in, the text to
    // show when it fails, and the retry that re-runs it.
    hubStage,
    hubError,
    dismissHubStatus,
    lastFailed,
    retryLast,
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
