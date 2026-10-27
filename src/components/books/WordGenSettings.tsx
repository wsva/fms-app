"use client";

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { OllamaModelInfo } from "@/lib/llm/types";
import {
  DEFAULT_WORD_GEN_MODEL,
  TEMPERATURE_OPTIONS,
  WORD_GEN_LANGUAGES,
  loadWordGenSettings,
  saveWordGenSetting,
  getDefaultPrompt,
} from "./wordGen";
import type { WordGenSettings as Settings, WordGenLang } from "./wordGen";

type Props = {
  /** If true, the component is disabled (e.g., during generation). */
  disabled?: boolean;
};

/**
 * Reusable word generation settings panel.
 * Manages model, temperature, and system prompt settings with localStorage persistence.
 * Provides settings via render prop pattern.
 */
export default function WordGenSettings({
  disabled,
  children,
}: Props & {
  children: (props: {
    settings: Settings;
    availableModels: OllamaModelInfo[];
    showSettings: boolean;
    setShowSettings: (v: boolean) => void;
  }) => React.ReactNode;
}) {
  const [settings, setSettings] = useState<Settings>({
    model: DEFAULT_WORD_GEN_MODEL,
    prompt: getDefaultPrompt("de"),
    temperature: 0,
    lang: "de",
  });
  const [showSettings, setShowSettings] = useState(false);
  const [availableModels, setAvailableModels] = useState<OllamaModelInfo[]>([]);

  // Load settings from localStorage on mount
  useEffect(() => {
    setSettings(loadWordGenSettings());
    invoke<{ installed: OllamaModelInfo[] }>("llm_list_models")
      .then((res) => setAvailableModels(res.installed))
      .catch(() => {});
  }, []);

  const updateSetting = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    setSettings((prev) => ({ ...prev, [key]: value }));
    saveWordGenSetting(key, value);
  };

  const resetPrompt = () => {
    updateSetting("prompt", getDefaultPrompt(settings.lang));
  };

  const handleLangChange = (newLang: WordGenLang) => {
    setSettings((prev) => ({
      ...prev,
      lang: newLang,
      prompt: getDefaultPrompt(newLang),
    }));
    saveWordGenSetting("lang", newLang);
    saveWordGenSetting("prompt", getDefaultPrompt(newLang));
  };

  return (
    <>
      {children({ settings, availableModels, showSettings, setShowSettings })}

      {/* Collapsible settings panel */}
      {showSettings && (
        <div className="flex flex-col gap-2 p-3 rounded-lg bg-bg-muted border border-border-default text-xs">
          <div className="flex flex-row gap-2">
            <div className="flex flex-col gap-1 w-24">
              <label className="text-text-secondary font-medium">Language</label>
              <select
                className="px-2 py-1 rounded bg-bg-input border border-border-default text-xs text-text-primary focus:outline-none focus:border-accent cursor-pointer"
                value={settings.lang}
                onChange={(e) => handleLangChange(e.target.value as WordGenLang)}
                disabled={disabled}
              >
                {WORD_GEN_LANGUAGES.map((l) => (
                  <option key={l.value} value={l.value}>
                    {l.label}
                  </option>
                ))}
              </select>
            </div>
            <div className="flex flex-col gap-1 max-w-48">
              <label className="text-text-secondary font-medium">Model</label>
              <select
                className="px-2 py-1 rounded bg-bg-input border border-border-default text-xs text-text-primary focus:outline-none focus:border-accent cursor-pointer"
                value={settings.model}
                onChange={(e) => updateSetting("model", e.target.value)}
                disabled={disabled}
              >
                {availableModels.length === 0 && (
                  <option value="">No models found</option>
                )}
                {availableModels.map((m) => (
                  <option key={m.name} value={m.name}>
                    {m.name}
                  </option>
                ))}
              </select>
            </div>
            <div className="flex flex-col gap-1 w-20">
              <label className="text-text-secondary font-medium">Temp</label>
              <select
                value={settings.temperature}
                onChange={(e) => updateSetting("temperature", parseFloat(e.target.value))}
                className="px-2 py-1 rounded-md bg-bg-muted border border-border-default text-text-primary text-xs cursor-pointer"
                disabled={disabled}
              >
                {TEMPERATURE_OPTIONS.map((v) => (
                  <option key={v} value={v}>
                    {v.toFixed(1)}
                  </option>
                ))}
              </select>
            </div>
          </div>
          <div className="flex flex-col gap-1">
            <div className="flex items-center justify-between">
              <label className="text-text-secondary font-medium">System Prompt</label>
              <button
                className="text-text-tertiary hover:text-accent cursor-pointer"
                onClick={resetPrompt}
                disabled={disabled}
              >
                Reset
              </button>
            </div>
            <textarea
              className="w-full h-32 px-2 py-1 rounded bg-bg-input border border-border-default text-sm text-text-primary resize-y focus:outline-none focus:border-accent"
              value={settings.prompt}
              onChange={(e) => updateSetting("prompt", e.target.value)}
              disabled={disabled}
            />
          </div>
        </div>
      )}
    </>
  );
}
