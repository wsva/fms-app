"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ImagePlus,
  FolderOpen,
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
  Camera,
  Send,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/tauri";
import { reqStr, useAgentActions } from "@/lib/agent/ui-actions";
import { tbBase, tbGhost } from "@/lib/ui/toolbar";
import { PageHeader, Toolbar, ToolbarButton, ToolbarIconButton } from "@/components/ui/toolbar";
import { useIsMobile } from "@/hooks/useIsMobile";
import HubStatusOverlay from "@/components/read_aloud/HubStatusOverlay";
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
    setSourceImage,
    selectFile,
    captureScreenshot,
    takePhoto,
    loadLanguages,
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
    sendToHub,
    sending,
    sentToHub,
    hubStage,
    hubError,
    dismissHubStatus,
    lastFailed,
    retryLast,
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
  // The Android list is the Hub's, so it costs a remote call. Say so while it is
  // in flight, and say `Auto` when it cannot be had, rather than showing the
  // `None` that reads as a broken picker.
  const [langsLoading, setLangsLoading] = useState(false);
  const [langsUnavailable, setLangsUnavailable] = useState(false);

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

  // Hidden input that raises the native camera on Android. `capture=environment`
  // means the webview handles the camera intent and its permission itself, so
  // there is no Rust camera code and no new capability.
  const cameraInputRef = useRef<HTMLInputElement>(null);

  // ---- Intent (Android only) ----
  // Two screens behind one entry, because where a photo should go only becomes
  // clear once the photo exists: `hub` hands it to the Hub's own OCR page, where
  // the TSV and hyphen tooling lives; `phone` reads the text here. The image is
  // shared state either way, so switching intent never costs a re-shoot.
  const [intent, setIntentState] = useState<"hub" | "phone">("hub");
  // Restored in an effect rather than as lazy initial state, so the prerendered
  // HTML and the first client paint agree — the same reason `ocr_mode` does it.
  useEffect(() => {
    const saved = localStorage.getItem("ocr_intent");
    if (saved === "hub" || saved === "phone") setIntentState(saved);
  }, []);
  const setIntent = useCallback((next: "hub" | "phone") => {
    setIntentState(next);
    localStorage.setItem("ocr_intent", next);
  }, []);

  // ---- Hub reachability (Android only) ----
  // Every action on the Android page is a Hub call, so the page says up front
  // whether there is a Hub to call at all. `sync_status` is the same read the
  // Devices & Hub page makes; only reachability is shown here — which machine
  // answers is that page's business.
  const [hub, setHub] = useState<{ reachable: boolean } | null>(null);
  useEffect(() => {
    if (!mobile || !isTauri()) return;
    let alive = true;
    const load = () =>
      invoke<{ hub?: { reachable?: boolean } | null }>("sync_status")
        .then((s) => {
          if (alive) setHub({ reachable: s.hub?.reachable === true });
        })
        .catch(() => {
          // No sync layer to ask — treat the Hub as absent, not as broken.
          if (alive) setHub({ reachable: false });
        });
    load();
    const timer = setInterval(load, 15000);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, [mobile]);
  const hubOnline = hub?.reachable === true;
  const hubOffline = mobile && hub !== null && !hub.reachable;

  // ---- Stage-driven image pane (Android) ----
  // One surface in focus at a time: the photo while the user judges whether it
  // is worth a Hub call, the text once that call has answered. `On the Hub` has
  // no text to give room to, so its preview stays open (see `imageShown`).
  useEffect(() => {
    if (mobile && sourceImage) setImageOpen(true);
  }, [mobile, sourceImage]);
  useEffect(() => {
    if (mobile && intent === "phone" && resultText) setImageOpen(false);
  }, [mobile, intent, resultText]);

  // The Android client recognizes only via the Hub, which returns plain text —
  // there is no local Tesseract to run the TSV/hyphen rebuild. Lock the mode to
  // plain so `resultText` reads the plain result.
  useEffect(() => {
    if (mobile) setMode("plain");
  }, [mobile, setMode]);

  // Receive end of a follower's "Send to Hub", and of the `ocr_open_image` MCP
  // tool: the shell (or an attached agent) runs this action to load an image
  // straight into the image box. It is an action rather than an event because
  // the page mounts with its tab — an event emitted while the tab was still
  // closed had no listener here, so the image arrived at the Hub and vanished.
  useAgentActions("ocr", [
    {
      id: "ocr.open-image",
      label:
        "Load an image into the OCR page's image box, ready for recognition. Nothing is persisted: the image only lives in the rendered page.",
      kind: "mutate",
      params_schema: {
        image: {
          type: "string",
          description: "The image as a `data:image/...;base64,` URL — typically a photo a follower device captured.",
          required: true,
        },
      },
      run: (params) => {
        const image = reqStr("ocr.open-image", params, "image");
        setError("");
        setSourceImage(image);
        return { loaded: true };
      },
    },
  ]);

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

  // Fetch available Tesseract languages on mount. Desktop reads its own install;
  // the Android client asks the Hub (see `loadLanguages`). Re-run when the Hub
  // arrives: a phone that opened with its Hub asleep would otherwise be left
  // promising `Auto` forever.
  useEffect(() => {
    if (!isTauri()) return;
    setLangsLoading(true);
    loadLanguages()
      .then((langs) => {
        setLangsUnavailable(false);
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
        // No Tesseract here (desktop) or no Hub to ask (Android) — empty list.
        setLangsUnavailable(true);
      })
      .finally(() => setLangsLoading(false));
  }, [loadLanguages, hubOnline]);

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

  // What the Android language chip says: the names, or the honest stand-in for
  // not having them yet.
  const langChipLabel = langsLoading
    ? "Loading…"
    : selectedLangs.length === 0 || langsUnavailable
      ? "Auto"
      : `${selectedLangs
          .slice(0, 2)
          .map((code) => LANG_LABELS[code] ?? code)
          .join("+")}${selectedLangs.length > 2 ? ` +${selectedLangs.length - 2}` : ""}`;

  // Whether the image viewer is drawn. `On the Hub` always shows it; desktop
  // keeps the narrow-layout toggle exactly as it was.
  const imageShown = mobile ? intent === "hub" || imageOpen : imageOpen;

  // The result pane is the phone's half of the screen once there is something in
  // it: `On the Hub` never recognizes here, so the box does not exist on that
  // intent, and before a result arrives the photo (or the invitation) owns the
  // viewport. Dropping the empty box is what makes the two read as different
  // screens rather than as one stripped.
  const showResultPane = !mobile || (intent === "phone" && !!resultText);

  // On a phone the panes own the viewport instead of stacking to a fixed height
  // over dead space: the hand-off screen gives everything to the photo, the
  // recognition screen keeps the photo's strip at its own height and grows the
  // text into the rest. Desktop keeps its two-pane split untouched.
  const paneGrid = mobile
    ? showResultPane
      ? "grid gap-4 flex-1 min-h-0 grid-rows-[auto_minmax(0,1fr)]"
      : "grid gap-4 flex-1 min-h-0 grid-rows-[minmax(0,1fr)]"
    : "grid gap-4 lg:flex-1 lg:min-h-0 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.15fr)]";

  const filteredLangs = useMemo(() => {
    const filter = langFilter.trim().toLowerCase();
    if (!filter) return availableLangs;
    return availableLangs.filter(
      (code) =>
        code.toLowerCase().includes(filter) ||
        (LANG_LABELS[code] ?? "").toLowerCase().includes(filter)
    );
  }, [availableLangs, langFilter]);

  // LLM model for the plain path's text fixing. The pick is cached under
  // `llm_selected_model`, which the text-selection menu's Ask-the-model actions
  // also read (lib/llm/ask.ts), so one choice serves every LLM feature.
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

  // The language picker, held as a value because it has two possible homes: the
  // toolbar on the desktop, the image header on Android. One instance, same
  // state and popover either way — only the anchor moves.
  const languageControl = (
    <div className="relative shrink-0" ref={langDropdownRef}>
      <button
        type="button"
        onClick={() => {
          setLangFilter("");
          setLangDropdownOpen((o) => !o);
        }}
        className={`${tbGhost} ${langDropdownOpen ? "bg-bg-hover text-text-primary" : ""}`}
        title={
          langsLoading
            ? "Asking the Hub for its languages"
            : langString
              ? `OCR languages: ${langString}`
              : "OCR languages"
        }
        aria-label={`OCR languages: ${selectedLangs.length ? selectedLangs.join(", ") : "none"}`}
        aria-expanded={langDropdownOpen}
      >
        <Languages size={14} />
        {mobile ? (
          // Names, or the honest stand-in: `None` read as a broken picker when
          // the list was merely still arriving from the Hub.
          <span className="text-xs text-text-secondary truncate max-w-[150px]">
            {langChipLabel}
          </span>
        ) : (
          <>
            {/* On a narrow desktop window the chips cost the whole bar a row, so
                only the count stays visible; the accessible name carries the list. */}
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
          </>
        )}
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
              <div className="px-3 py-2 text-xs text-text-tertiary">
                {langsLoading ? "Loading…" : langsUnavailable ? "The Hub is not connected" : "No languages found"}
              </div>
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
  );

  return (
    <main className="flex-1 flex flex-col h-full min-w-0 min-h-0">
      <PageHeader
        title="OCR"
        badge={
          // The Android page runs on the Hub rather than on a local engine, and
          // says whether that Hub is reachable under the toolbar instead.
          mobile ? undefined : <span className="text-xs text-text-tertiary">Tesseract</span>
        }
      />

      {/* Page-level actions live here, above the panes, so Recognize stays in
          reach while either pane scrolls. */}
      <Toolbar className="flex-wrap">
        {/* The two ways a photo gets in, then the way out of what is on screen;
            the intent's one verb answers on the far side of the bar. */}
        {!mobile && <ToolbarButton icon={ImagePlus} label="Select image" onClick={selectFile} />}
        {mobile && (
          <>
            <ToolbarButton
              icon={Camera}
              label="Camera"
              onClick={() => cameraInputRef.current?.click()}
              title="Take a photo"
            />
            <ToolbarButton
              icon={FolderOpen}
              label="Open"
              onClick={selectFile}
              title="Open an image file"
            />
          </>
        )}
        {!mobile && (
          <ToolbarButton
            icon={MonitorUp}
            label="Screenshot"
            onClick={captureScreenshot}
          />
        )}
        <ToolbarButton
          icon={X}
          label="Clear"
          onClick={handleClear}
          disabled={!sourceImage && !resultText && !rawTsv}
          title="Clear image and results"
        />
        <div className="flex-1 min-w-0" />

        {/* Language is a parameter of recognition, so on Android it leaves the
            bar altogether and sits with the photo it configures. */}
        {!mobile && languageControl}

        {/* One dictionary per language: installed once, used by every TSV run. */}
        {!mobile && (
          <ToolbarButton
            icon={BookOpen}
            label="Dictionaries"
            onClick={() => setShowDictionaries(true)}
            title="Hunspell dictionaries for line-break hyphen repair (one per language)"
          />
        )}

        {/* `On the Hub` has no recognition to run here — its one verb is the
            send on the far right — so the CTA follows the intent. Every action in
            this bar is a normal button: a phone has no room to make one of them
            louder than the others. */}
        {(!mobile || intent === "phone") && (
          <ToolbarButton
            variant={mobile ? "ghost" : "solid"}
            icon={recognizing ? Loader2 : Sparkles}
            iconProps={recognizing ? { className: "animate-spin" } : undefined}
            label={recognizing ? "Recognizing…" : "Recognize"}
            onClick={() => editedImage && recognize(editedImage, langString)}
            disabled={!canRecognize}
            title={
              !editedImage
                ? "Add an image first"
                : mobile && hubOffline
                  ? "The Hub is not connected — wake it and retry"
                  : mode === "tsv"
                    ? "Recognize and rebuild paragraphs in Rust"
                    : "Recognize text"
            }
          />
        )}
        {/* Copy belongs with the text it copies, so on Android it lives in the
            result header and only exists once there is a result. */}
        {!mobile && (
          <ToolbarButton
            icon={copied ? Check : Copy}
            label={copied ? "Copied" : "Copy"}
            onClick={copyResult}
            disabled={!resultText}
            title="Copy the result text"
          />
        )}
        {mobile && intent === "hub" && (
          <ToolbarButton
            variant="ghost"
            icon={sending ? Loader2 : Send}
            iconProps={sending ? { className: "animate-spin" } : undefined}
            label={sentToHub ? "Sent to the Hub" : sending ? "Sending…" : "Send to Hub"}
            onClick={() => editedImage && sendToHub(editedImage)}
            disabled={!editedImage || sending}
            title={
              hubOffline
                ? "The Hub is not connected — wake it and retry"
                : "Open this image in the Hub's OCR page"
            }
          />
        )}
      </Toolbar>

      {/* The fork the whole screen hangs on — one photo, two destinations — as
          the tab bar under the actions. Named for where the work happens, so the
          button above it never has to repeat the noun. */}
      {mobile && (
        <div
          role="tablist"
          aria-label="Where the work happens"
          className="shrink-0 flex items-stretch bg-bg-card border-b border-border-default"
        >
          <IntentTab
            active={intent === "hub"}
            onClick={() => setIntent("hub")}
            label="On the Hub"
            title="Take a photo and open it on the Hub"
          />
          <IntentTab
            active={intent === "phone"}
            onClick={() => setIntent("phone")}
            label="On this phone"
            title="Take a photo and read the text here"
          />
        </div>
      )}

      {/* The one dependency everything above this line has, stated as the
          question the user actually has: is there a Hub to talk to. Its address
          is the Devices page's business, not something to spend width on here. */}
      {mobile && hub !== null && (
        <div
          className={`shrink-0 flex items-center gap-1.5 px-4 py-1.5 text-xs ${
            hub.reachable ? "text-success-text" : "text-warning-text"
          }`}
        >
          <span
            className={`h-1.5 w-1.5 rounded-full shrink-0 ${
              // A status light, so the colour names the state rather than the
              // theme: `success-text` is olive in Solarized and yellow in
              // Gruvbox, and neither reads as "connected".
              hub.reachable ? "bg-green-500" : "bg-warning-text"
            }`}
          />
          {hub.reachable ? "Hub is connected" : "Hub is NOT connected"}
        </div>
      )}

      {/* The Android camera affordance: a native capture intent, its result fed
          to the same editor pipeline as Select image. */}
      {mobile && (
        <input
          ref={cameraInputRef}
          type="file"
          accept="image/*"
          capture="environment"
          className="hidden"
          onChange={(e) => {
            takePhoto(e.target.files?.[0]);
            // Reset so taking the same photo twice still fires onChange.
            e.target.value = "";
          }}
        />
      )}

      {error && (
        <div className="shrink-0 mx-4 mt-4 px-4 py-3 rounded-lg bg-error-bg text-error-text text-sm flex items-start gap-2">
          <span className="flex-1 whitespace-pre-wrap break-words">{error}</span>
          {/* The one thing the message asks for, as a button: the photo and the
              language are still exactly as the user left them. */}
          {mobile && lastFailed && (
            <button
              type="button"
              onClick={retryLast}
              className="shrink-0 font-medium underline cursor-pointer"
              title="Run the failed Hub call again with the same image"
            >
              Retry
            </button>
          )}
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
        <div className="flex flex-col gap-4 min-h-full lg:h-full lg:min-h-0">
          <div className={paneGrid}>
            {/* ---- Image pane ---- */}
            <section className="rounded-xl border border-border-default bg-bg-card flex flex-col min-w-0 min-h-0 overflow-hidden">
              {/* Nothing captured yet: a collapsed `Image` strip would read as an
                  empty box, so the screen invites the shot instead — and names the
                  one thing that can stop either intent working. */}
              {mobile && !sourceImage ? (
                <EmptyState hubOffline={hubOffline} />
              ) : (
                <>
                  <div className="shrink-0 flex items-center gap-2 px-3 py-2 border-b border-border-default">
                    <h2 className="text-sm font-semibold text-text-primary flex-1 min-w-0 truncate">
                      Image
                    </h2>
                    <span className="hidden xl:inline text-xs text-text-tertiary">
                      Drop a file, or use the toolbar
                    </span>
                    {/* Language is a parameter of the recognition run *here*, so on
                        Android it sits with the photo it configures — and only on
                        the intent that recognizes. */}
                    {mobile && intent === "phone" && languageControl}
                    {/* `On the Hub` never collapses: the photo is the whole screen. */}
                    {!(mobile && intent === "hub") && (
                      <ToolbarIconButton
                        className="lg:hidden"
                        icon={imageOpen ? ChevronUp : ChevronDown}
                        ariaLabel={imageOpen ? "Collapse image" : "Expand image"}
                        onClick={() => setImageOpen((v) => !v)}
                      />
                    )}
                  </div>
                  <div
                    className={`p-3 min-h-0 ${
                      imageShown
                        ? // The photo fills the screen whenever it is the only pane
                          // (the hand-off intent, or here before a result arrives);
                          // once the text is on screen it takes the lower half.
                          showResultPane
                          ? NARROW_IMAGE_HEIGHT
                          : "flex-1 min-h-0"
                        : "hidden"
                    } lg:block lg:h-auto lg:flex-1 lg:min-h-0`}
                  >
                    <ImageEditor
                      src={sourceImage}
                      onImageReady={handleImageReady}
                      onError={setError}
                    />
                  </div>
                </>
              )}
            </section>

            {/* ---- Result pane ---- */}
            {showResultPane && (
            <section className="rounded-xl border border-border-default bg-bg-card flex flex-col min-w-0 min-h-0 overflow-hidden">
              {/* One header row: the mode on the left, that mode's single extra
                  action on the right. Everything else lives in the toolbar. The
                  Android client has none of these (plain-only, Hub-backed), so
                  the whole row stays desktop. */}
              {!mobile && (
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
              )}

              {/* Android has no mode to switch and no LLM pass to run here, so its
                  header carries the one action that belongs with the text itself. */}
              {mobile && resultText && (
                <div className="shrink-0 flex items-center gap-2 px-3 py-2 border-b border-border-default">
                  <h2 className="text-sm font-semibold text-text-primary flex-1 min-w-0 truncate">
                    Text
                  </h2>
                  <button
                    type="button"
                    onClick={copyResult}
                    className={`${tbGhost} shrink-0`}
                    title="Copy the result text"
                  >
                    {copied ? <Check size={14} /> : <Copy size={14} />}
                    {copied ? "Copied" : "Copy"}
                  </button>
                </div>
              )}

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
                className={`flex-1 w-full px-3 py-2 bg-transparent text-text-primary resize-y focus:outline-none ${
                  mobile ? "min-h-0" : "min-h-[240px] lg:min-h-0 lg:resize-none"
                }`}
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
                {/* The size is a reading-desktop concern (and is still remembered);
                    on a phone the row is only worth the proof that text came back. */}
                {!mobile && (
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
                )}
              </div>
            </section>
            )}
          </div>
        </div>
      </div>

      {/* The phase the Hub call is in, on the contract the read-aloud panels use:
          a cold Tesseract over there is slow over here, and a bare spinner on a
          phone reads as a broken device rather than a busy Hub. */}
      {hubStage && (
        <HubStatusOverlay
          stage={hubStage}
          error={hubError}
          title={intent === "hub" ? "Sending to the Hub" : "Working with the Hub"}
          errorHint="Nothing was kept on the Hub — retry sends this photo again."
          onLater={dismissHubStatus}
        />
      )}

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

/** One tab of the Android intent switch — the bar over the content it picks, so
 *  it is underlined like a tab rather than boxed like another button. */
function IntentTab({
  active,
  onClick,
  label,
  title,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
  title: string;
}) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      onClick={onClick}
      title={title}
      className={`flex-1 px-3 py-2 text-sm font-medium border-b-2 transition-colors cursor-pointer ${
        active
          ? "border-accent text-text-primary"
          : "border-transparent text-text-secondary hover:text-text-primary"
      }`}
    >
      {label}
    </button>
  );
}

/**
 * The Android starting screen: no photo yet, so the box says what fills it.
 * The two ways in are in the bar above, where every other action is too.
 */
function EmptyState({ hubOffline }: { hubOffline: boolean }) {
  return (
    <div className="flex-1 min-h-0 flex flex-col items-center justify-center gap-3 p-6 text-center">
      <Camera size={32} className="text-text-tertiary" />
      <p className="text-sm text-text-secondary">Take a photo to start</p>
      {hubOffline && (
        <p className="text-xs text-warning-text">
          The Hub is NOT connected — wake it to send the photo or read the text.
        </p>
      )}
    </div>
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
