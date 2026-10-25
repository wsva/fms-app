"use client";

/**
 * UnderstandingModelPicker — a single-line LLM model selector shown above the
 * Understanding tabs. Generation commands fall back to the app default from
 * Settings when nothing is chosen here, so the "App default" option is always
 * present. The chosen model is persisted (via the parent) under the shared
 * `llm_selected_model` localStorage key, the same one the Chat and OCR pages
 * use, so a learner's model preference carries across the app.
 */

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Cpu } from "lucide-react";
import { isTauri } from "@/lib/tauri";
import type { OllamaModelInfo } from "@/lib/llm/types";

type Props = {
  /** Currently selected model ("" = app default). */
  value: string;
  onChange: (model: string) => void;
};

export default function UnderstandingModelPicker({ value, onChange }: Props) {
  const [models, setModels] = useState<OllamaModelInfo[]>([]);

  // Installed Ollama models (the picker is informational for cloud providers,
  // where the list is empty and "App default" is the meaningful choice).
  useEffect(() => {
    if (!isTauri()) return;
    invoke<{ installed: OllamaModelInfo[] }>("llm_list_models")
      .then((res) => setModels(res.installed))
      .catch(() => {});
  }, []);

  const configured = value.trim();
  // Keep a previously-chosen (or Settings-derived) model visible even when it
  // is not in the installed list, so the select never silently resets.
  const missing = !!configured && !models.some((m) => m.name === configured);

  return (
    <div className="shrink-0 flex items-center gap-2 px-3 py-1.5 border-b border-border-default text-xs">
      <Cpu size={14} className="shrink-0 text-text-tertiary" />
      <label className="shrink-0 text-text-tertiary">Model</label>
      <select
        className="flex-1 min-w-0 px-2 py-1 rounded-md bg-bg-input border border-border-default text-text-primary focus:outline-none focus:border-accent cursor-pointer"
        value={configured}
        onChange={(e) => onChange(e.target.value)}
        title="LLM used to generate explanations, quizzes, and glossary terms"
      >
        <option value="">App default (Settings)</option>
        {models.map((m) => (
          <option key={m.name} value={m.name}>
            {m.name}
          </option>
        ))}
        {missing && <option value={configured}>{configured} (not installed)</option>}
      </select>
    </div>
  );
}
