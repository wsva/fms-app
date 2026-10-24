"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AlertTriangle, Check, Copy, Loader2, RefreshCw, Ruler, Sparkles } from "lucide-react";
import type { LlmInstalledModelsResponse, OllamaModelInfo } from "@/lib/llm/types";
import {
  LS_NORMALIZE_MODEL,
  type NormalizeMeasure,
  type NormalizeResult,
} from "@/lib/read_aloud/normalize";
import { isTauri } from "@/lib/tauri";
import { logError } from "@/lib/logger";

const LOG_MODULE = "read-aloud";

type Props = {
  /** The passage the prompt quotes. Empty disables both routes. */
  content: string;
  /** What scoring reads for the *stored* text today — the gate's baseline. */
  baseline: string;
  datasetUuid: string;
  /** Null for a text that has never been saved: there are no attempts to measure. */
  textUuid: string | null;
  /** The box as it stands now; a candidate equal to it has nothing to measure. */
  candidate: string;
  /** Apply a generated candidate to the draft box. Nothing is stored here. */
  onCandidate: (text: string) => void;
  /** Paused while the page is saving. */
  disabled?: boolean;
  /** Bumped by the page after a save or a delete: the baseline the note talked
   *  about is gone, so the note goes with it. */
  refreshKey?: number;
};

type Note = { kind: "info" | "good" | "warn" | "error"; text: string };

const NOTE_CLASS: Record<Note["kind"], string> = {
  info: "text-text-tertiary",
  good: "text-success-text",
  warn: "text-warning-text",
  error: "text-error-text",
};

function score(value: number): string {
  return value.toFixed(1);
}

/**
 * Which model to keep after a (re-)listing: the one already chosen, else the one
 * remembered from last time, else the first installed. A model that was just
 * pulled or deleted must not leave a stale name in the picker.
 */
function keepModel(list: OllamaModelInfo[], previous: string): string {
  const remembered =
    typeof localStorage !== "undefined" ? (localStorage.getItem(LS_NORMALIZE_MODEL) ?? "") : "";
  for (const name of [previous, remembered]) {
    if (name && list.some((m) => m.name === name)) return name;
  }
  return list[0]?.name ?? "";
}

/**
 * The two ways to obtain a normalized scoring reference, side by side.
 *
 * Route 1 runs the app's own prompt through a local model. Route 2 hands that
 * same prompt to the clipboard, for a provider the app cannot reach (ChatGPT and
 * friends) — the answer comes back by hand, which is why the box next to this
 * strip is an ordinary editable draft.
 *
 * Both routes end in the same place: a candidate in the box, measured — when the
 * text has recordings — against the reference scoring uses today. Nothing here
 * writes: the draft is saved by the page, deliberately.
 */
export default function NormalizeActions({
  content,
  baseline,
  datasetUuid,
  textUuid,
  candidate,
  onCandidate,
  disabled,
  refreshKey,
}: Props) {
  const [models, setModels] = useState<OllamaModelInfo[]>([]);
  const [model, setModel] = useState("");
  const [busy, setBusy] = useState<"" | "generate" | "copy" | "measure">("");
  const [note, setNote] = useState<Note | null>(null);
  /** The prompt, shown only when the clipboard refused it. */
  const [prompt, setPrompt] = useState("");

  // A stored reference has just changed under the note: what it measured is no
  // longer what scoring reads, so saying "worth keeping — save it" would be a lie.
  const firstRun = useRef(true);
  useEffect(() => {
    if (firstRun.current) {
      firstRun.current = false;
      return;
    }
    setNote(null);
    setPrompt("");
  }, [refreshKey]);

  // The model list is Ollama's, because that is the local option the Settings
  // page configures; an unreachable Ollama leaves the generate route disabled
  // and the copy route untouched. It is re-listed on demand — the usual reason
  // the list is empty is that Ollama was started after this page was opened.
  const [listingModels, setListingModels] = useState(false);

  const loadModels = useCallback(async (announce = false) => {
    if (!isTauri()) return;
    setListingModels(true);
    try {
      const res = await invoke<LlmInstalledModelsResponse>("llm_list_models");
      const list = res.installed ?? [];
      setModels(list);
      setModel((previous) => keepModel(list, previous));
      // A click that silently changes nothing reads as a broken button, so an
      // explicit re-check says why it stayed empty. The mount call stays quiet.
      if (announce && list.length === 0) {
        setNote({
          kind: "warn",
          text: "Still no models — Ollama does not look like it is running (Settings → LLM), or none are pulled yet.",
        });
      }
    } catch (e) {
      setModels([]);
      logError(`listing ollama models failed: ${String(e)}`, LOG_MODULE);
      if (announce) {
        setNote({ kind: "warn", text: `Could not reach Ollama: ${String(e)}` });
      }
    } finally {
      setListingModels(false);
    }
  }, []);

  useEffect(() => {
    void loadModels();
  }, [loadModels]);

  const hasText = content.trim().length > 0;
  const worthMeasuring = !!textUuid && candidate.trim().length > 0 && candidate.trim() !== baseline.trim();
  const paused = !!disabled || busy !== "";

  const pickModel = (name: string) => {
    setModel(name);
    if (typeof localStorage !== "undefined") localStorage.setItem(LS_NORMALIZE_MODEL, name);
  };

  /** Turn a measurement into the sentence the user needs to act on. */
  const describe = (m: NormalizeMeasure, prefix = ""): Note => {
    if (m.attempts === 0) {
      return {
        kind: "info",
        text: `${prefix}No recordings of this text yet, so the change cannot be measured — record an attempt and measure again to know whether it is worth keeping.`,
      };
    }
    const n = `${m.attempts} recording${m.attempts === 1 ? "" : "s"}`;
    if (m.baseline == null || m.candidate == null) {
      return { kind: "info", text: `${prefix}Compared with ${n}.` };
    }
    const delta = m.candidate - m.baseline;
    const pair = `${score(m.baseline)} → ${score(m.candidate)}`;
    if (delta > 0.05) {
      return { kind: "good", text: `${prefix}Against your ${n}: ${pair} (+${score(delta)}). Worth keeping — save it.` };
    }
    if (delta < -0.05) {
      return {
        kind: "warn",
        text: `${prefix}Against your ${n}: ${pair} (${score(delta)}). Worse than what scoring reads now, so the wording probably changed. Edit it or clear the box before saving.`,
      };
    }
    return {
      kind: "info",
      text: `${prefix}Against your ${n}: ${pair} (no change). The mismatch was not spelling, so this candidate buys nothing.`,
    };
  };

  /** Measure a candidate. Never throws: the gate is advice, not a blocker. */
  const measureCandidate = async (text: string, prefix = ""): Promise<Note | null> => {
    if (!textUuid) return null;
    try {
      const m = await invoke<NormalizeMeasure>("read_aloud_normalize_measure", {
        datasetUuid,
        textUuid,
        reference: baseline,
        candidate: text,
      });
      return describe(m, prefix);
    } catch (e) {
      logError(`measuring a normalization failed: ${String(e)}`, LOG_MODULE);
      return null;
    }
  };

  const generate = async () => {
    if (!isTauri() || !hasText || !model) return;
    setBusy("generate");
    setPrompt("");
    setNote({ kind: "info", text: `Asking ${model}… a local model can take a minute on a long passage.` });
    try {
      const res = await invoke<NormalizeResult>("read_aloud_normalize", { content, model });
      const text = (res.candidate ?? "").trim();
      if (!text) {
        setNote({ kind: "error", text: "The model returned nothing. Try again, or copy the prompt." });
        return;
      }
      onCandidate(text);
      const gate = await measureCandidate(text, `Generated with ${model}. `);
      setNote(
        gate ?? {
          kind: "good",
          text: `Generated with ${model}. Nothing is stored until you save the text.`,
        },
      );
    } catch (e) {
      logError(`generating a normalization failed: ${String(e)}`, LOG_MODULE);
      setNote({ kind: "error", text: String(e) });
    } finally {
      setBusy("");
    }
  };

  const copyPrompt = async () => {
    if (!isTauri() || !hasText) return;
    setBusy("copy");
    setPrompt("");
    setNote(null);
    try {
      const res = await invoke<NormalizeResult>("read_aloud_normalize", { content });
      try {
        await navigator.clipboard.writeText(res.prompt);
        setNote({
          kind: "good",
          text: "Prompt copied. Paste it into ChatGPT (or any LLM), paste the answer into the box, then measure it before saving.",
        });
      } catch (e) {
        logError(`clipboard write failed: ${String(e)}`, LOG_MODULE);
        setPrompt(res.prompt);
        setNote({ kind: "warn", text: "Could not reach the clipboard — the prompt is below, select and copy it." });
      }
    } catch (e) {
      logError(`building the normalization prompt failed: ${String(e)}`, LOG_MODULE);
      setNote({ kind: "error", text: String(e) });
    } finally {
      setBusy("");
    }
  };

  const measureBox = async () => {
    setBusy("measure");
    const gate = await measureCandidate(candidate);
    setNote(gate ?? { kind: "info", text: "Nothing to compare yet." });
    setBusy("");
  };

  const primaryButton =
    "inline-flex items-center gap-1.5 px-2.5 py-1 rounded-md text-[11px] font-medium bg-accent-bg text-white hover:opacity-90 cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed";
  const smallButton =
    "inline-flex items-center gap-1 px-2 py-1 rounded-md text-[11px] bg-bg-muted hover:bg-bg-hover text-text-secondary cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed";

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          className={primaryButton}
          disabled={paused || !hasText || !model}
          onClick={() => void generate()}
          title={
            !hasText
              ? "Write the text to read first — the prompt quotes it."
              : !model
                ? "No local model found. Start Ollama (Settings → LLM), or copy the prompt instead."
                : `Spell the numbers and abbreviations out with ${model}. The result lands in the box as an unsaved draft.`
          }
        >
          {busy === "generate" ? <Loader2 size={12} className="animate-spin" /> : <Sparkles size={12} />}
          {busy === "generate" ? "Generating…" : "Generate with Ollama"}
        </button>

        <button
          type="button"
          className={smallButton}
          disabled={paused || !hasText}
          onClick={() => void copyPrompt()}
          title="Copies the exact prompt the button above would send. Paste it into ChatGPT or any other LLM, then paste the answer into the box."
        >
          {busy === "copy" ? <Loader2 size={12} className="animate-spin" /> : <Copy size={12} />}
          Copy prompt
        </button>

        {worthMeasuring && (
          <button
            type="button"
            className={smallButton}
            disabled={paused}
            onClick={() => void measureBox()}
            title="Score the box against your own recordings of this text, and against the reference scoring reads today. Nothing is stored."
          >
            {busy === "measure" ? <Loader2 size={12} className="animate-spin" /> : <Ruler size={12} />}
            Measure
          </button>
        )}

        <div className="flex-1 min-w-0" />

        <div className="flex items-center gap-1">
          {models.length > 1 ? (
            <label className="flex items-center gap-1.5 text-[11px] text-text-tertiary">
              model
              <select
                className="px-1.5 py-1 rounded bg-bg-muted border border-border-default text-[11px] text-text-primary focus:outline-none focus:border-accent-bg cursor-pointer"
                value={model}
                onChange={(e) => pickModel(e.target.value)}
                disabled={paused}
              >
                {models.map((m) => (
                  <option key={m.name} value={m.name}>
                    {m.name}
                  </option>
                ))}
              </select>
            </label>
          ) : (
            <span className="text-[11px] text-text-tertiary">
              {model ? `model: ${model}` : "no local model found"}
            </span>
          )}
          {/* Ollama is usually started (or a model pulled) while this page is
              already open, so the list has to be re-askable without leaving. */}
          <button
            type="button"
            className="p-1 rounded text-text-tertiary hover:bg-bg-hover hover:text-text-primary cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
            disabled={paused || listingModels}
            onClick={() => void loadModels(true)}
            title="Re-check the models Ollama has installed"
            aria-label="Re-check Ollama models"
          >
            <RefreshCw size={12} className={listingModels ? "animate-spin" : ""} />
          </button>
        </div>
      </div>

      {note && (
        <p className={`flex items-start gap-1.5 text-[11px] ${NOTE_CLASS[note.kind]}`}>
          {note.kind === "good" ? (
            <Check size={12} className="mt-0.5 shrink-0" />
          ) : (
            <AlertTriangle size={12} className="mt-0.5 shrink-0" />
          )}
          <span className="break-words">{note.text}</span>
        </p>
      )}

      {prompt && (
        <textarea
          readOnly
          value={prompt}
          onFocus={(e) => e.currentTarget.select()}
          className="w-full h-40 p-2 rounded-md bg-bg-muted border border-border-default text-[11px] text-text-primary font-mono resize-y"
        />
      )}
    </div>
  );
}
