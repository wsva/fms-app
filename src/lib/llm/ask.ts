// ---------------------------------------------------------------------------
// One-off "ask the model about this text" — the app's only free-form LLM surface.
//
// FmS has no chat page: a question about a word or a sentence is asked from the
// selection itself (the global text-selection menu), so the answer lands next to
// the text it is about instead of in a conversation that would have to be
// re-pasted. This module is the seam — resolve which model the app should call,
// turn a prompt template into the system prompt, and hand back the answer.
// ---------------------------------------------------------------------------

import { invoke } from "@tauri-apps/api/core";

import { buildSystemPrompt, type PromptLanguage } from "./prompt-templates";
import type { LlmChatResponse, LlmInstalledModelsResponse } from "./types";

/** Shared with the OCR / Books model pickers: the last model the user chose. */
const SELECTED_MODEL_KEY = "llm_selected_model";

/**
 * The model to call.
 *
 * For a cloud provider it is the default model named in settings (that is the
 * only thing the features can call through it). For Ollama it is the user's
 * last pick when that model is still installed, otherwise the first installed
 * one. Returns `null` when nothing is configured, so the caller can say so
 * instead of calling with an empty model name.
 */
export async function resolveLlmModel(): Promise<string | null> {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return null;

  let provider = "ollama";
  let configured = "";
  try {
    const g = await invoke<{ llm_provider?: string; llm_model?: string }>(
      "settings_get_global"
    );
    provider = (g.llm_provider || "ollama").trim();
    configured = (g.llm_model || "").trim();
  } catch {
    // Settings unreadable — leave the Ollama defaults in place.
  }

  if (provider !== "ollama") return configured || null;

  const cached = localStorage.getItem(SELECTED_MODEL_KEY);
  try {
    const res = await invoke<LlmInstalledModelsResponse>("llm_list_models");
    const names = res.installed.map((m) => m.name);
    if (cached && names.includes(cached)) return cached;
    if (names.length > 0) return names[0];
  } catch {
    // Ollama is not running; the caller reports that as "no model".
  }
  return null;
}

/**
 * Run one prompt template over the given text and return the model's answer.
 *
 * The template becomes the system prompt and the text stays the user message —
 * the same split the (removed) chat page's prompt builder used, so a template's
 * text is never repeated inside the material being explained.
 */
export async function askAboutSelection(
  templateId: string,
  text: string,
  language: PromptLanguage = "en"
): Promise<string> {
  const system = buildSystemPrompt(templateId, language);
  if (!system) throw new Error(`Unknown prompt template "${templateId}".`);

  const model = await resolveLlmModel();
  if (!model) {
    throw new Error(
      "No language model is available. Start Ollama with a model installed, or set a model in Settings."
    );
  }

  const res = await invoke<LlmChatResponse>("llm_chat", {
    model,
    messages: [
      { role: "system", content: system },
      { role: "user", content: text },
    ],
  });
  return res.content.trim();
}
