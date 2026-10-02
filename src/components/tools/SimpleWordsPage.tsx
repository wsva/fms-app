"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCw, Plus, X, Star, Globe, Play } from "lucide-react";

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

interface CardDatasetSummary {
  info: { uuid: string; name: string; description: string };
  card_count: number;
  path: string;
  location: string;
}

interface LanguageConfig {
  dataset_uuids: string[];
  default_dataset_uuid: string;
}

interface SimpleWordsConfig {
  languages: Record<string, LanguageConfig>;
  active_language: string;
}

interface SimpleWordsConfigInfo {
  languages: Record<string, LanguageConfig>;
  active_language: string;
  word_count: number;
}

export default function SimpleWordsPage() {
  const [allDatasets, setAllDatasets] = useState<CardDatasetSummary[]>([]);
  const [config, setConfig] = useState<SimpleWordsConfig>({ languages: {}, active_language: "" });
  const [wordCount, setWordCount] = useState(0);
  const [loading, setLoading] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [newLang, setNewLang] = useState("");
  const [showAddLang, setShowAddLang] = useState(false);

  const loadData = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const [datasets, cfgInfo] = await Promise.all([
        invoke<CardDatasetSummary[]>("card_dataset_list"),
        invoke<SimpleWordsConfigInfo>("simple_words_get_config"),
      ]);
      setAllDatasets(datasets);
      setConfig({ languages: cfgInfo.languages, active_language: cfgInfo.active_language });
      setWordCount(cfgInfo.word_count);
    } catch (e) {
      console.error("Failed to load:", e);
    }
  }, []);

  useEffect(() => { loadData(); }, [loadData]);

  const languages = Object.keys(config.languages);

  // ── Language CRUD ─────────────────────────────────────────
  const addLanguage = () => {
    const trimmed = newLang.trim().toLowerCase();
    if (!trimmed || config.languages[trimmed]) return;
    setConfig((prev) => ({
      ...prev,
      languages: { ...prev.languages, [trimmed]: { dataset_uuids: [], default_dataset_uuid: "" } },
    }));
    setNewLang("");
    setShowAddLang(false);
  };

  const removeLanguage = (lang: string) => {
    setConfig((prev) => {
      const next = { ...prev.languages };
      delete next[lang];
      const activeLang = prev.active_language === lang ? "" : prev.active_language;
      return { languages: next, active_language: activeLang };
    });
  };

  // ── Dataset management per language ───────────────────────
  const removeDataset = (lang: string, uuid: string) => {
    setConfig((prev) => {
      const langCfg = prev.languages[lang];
      if (!langCfg) return prev;
      const updatedUuids = langCfg.dataset_uuids.filter((u) => u !== uuid);
      return {
        ...prev,
        languages: {
          ...prev.languages,
          [lang]: {
            dataset_uuids: updatedUuids,
            default_dataset_uuid:
              langCfg.default_dataset_uuid === uuid ? updatedUuids[0] ?? "" : langCfg.default_dataset_uuid,
          },
        },
      };
    });
  };

  const addDatasetToLang = (lang: string, uuid: string) => {
    if (!uuid) return;
    setConfig((prev) => {
      const langCfg = prev.languages[lang];
      if (!langCfg || langCfg.dataset_uuids.includes(uuid)) return prev;
      const updatedUuids = [...langCfg.dataset_uuids, uuid];
      return {
        ...prev,
        languages: {
          ...prev.languages,
          [lang]: {
            dataset_uuids: updatedUuids,
            default_dataset_uuid: langCfg.default_dataset_uuid || updatedUuids[0],
          },
        },
      };
    });
  };

  const setDefault = (lang: string, uuid: string) => {
    setConfig((prev) => ({
      ...prev,
      languages: {
        ...prev.languages,
        [lang]: { ...prev.languages[lang], default_dataset_uuid: uuid },
      },
    }));
  };

  // ── Actions ───────────────────────────────────────────────
  const handleSave = async () => {
    if (!isTauri()) return;
    // Validate
    for (const [lang, cfg] of Object.entries(config.languages)) {
      if (cfg.dataset_uuids.length === 0) {
        setError(`'${lang}' must have at least one dataset.`);
        return;
      }
      if (!cfg.default_dataset_uuid) {
        setError(`'${lang}' must have a default dataset (⭐).`);
        return;
      }
    }
    setError(null);
    setLoading(true);
    try {
      await invoke("simple_words_save_config", { config });
      setMessage("Config saved.");
    } catch (e) {
      setError(`Save failed: ${e}`);
    } finally {
      setLoading(false);
    }
  };

  const handleLoad = async (lang: string) => {
    if (!isTauri()) return;
    setError(null);
    setLoading(true);
    try {
      const count = await invoke<number>("simple_words_load_language", { language: lang });
      setWordCount(count);
      setConfig((prev) => ({ ...prev, active_language: lang }));
      setMessage(`Loaded ${count} words for '${lang}'.`);
    } catch (e) {
      setError(`Load failed: ${e}`);
    } finally {
      setLoading(false);
    }
  };

  const handleReload = async () => {
    if (!isTauri()) return;
    setLoading(true);
    try {
      const count = await invoke<number>("simple_words_reload");
      setWordCount(count);
      setMessage(`Reloaded: ${count} words.`);
    } finally {
      setLoading(false);
    }
  };

  // ── Helpers ───────────────────────────────────────────────
  const getDatasetName = (uuid: string) => {
    const ds = allDatasets.find((d) => d.info.uuid === uuid);
    return ds ? ds.info.name : uuid;
  };

  const getAvailableDatasets = (lang: string) => {
    const selected = new Set(config.languages[lang]?.dataset_uuids ?? []);
    return allDatasets.filter((ds) => !selected.has(ds.info.uuid));
  };

  // ── Render ────────────────────────────────────────────────
  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="p-4 border-b border-border-default shrink-0 flex items-center gap-3">
        <div className="flex-1">
          <h1 className="text-lg font-semibold text-text-primary flex items-center gap-2">
            <Globe size={18} /> Simple Words
          </h1>
          <p className="text-xs text-text-secondary mt-0.5">
            Load card datasets into memory as a "known simple" word set. Only one language is active at a time.
            When filtering a word list, all words present in the set are removed. New words you mark as simple
            are added to the <strong>default dataset</strong> (⭐).
          </p>
        </div>
        <button
          onClick={() => setShowAddLang(true)}
          className="flex items-center gap-1.5 px-3 py-1.5 text-sm rounded-lg border border-border-default hover:bg-bg-hover text-text-secondary cursor-pointer"
        >
          <Plus size={14} /> Add Language
        </button>
      </div>

      {/* Status bar */}
      <div className="px-4 py-2 flex items-center gap-4 text-xs text-text-tertiary border-b border-border-default shrink-0">
        <span>
          Active: <strong className="uppercase text-text-primary">{config.active_language || "none"}</strong>
        </span>
        <span>{wordCount} words in memory</span>
        {message && <span className="text-text-secondary">{message}</span>}
        {error && <span className="text-red-500">{error}</span>}
      </div>

      {/* Language sections */}
      <div className="flex-1 overflow-auto p-4 space-y-4">
        {showAddLang && (
          <div className="flex items-center gap-2 p-3 rounded-lg border border-accent bg-accent/5">
            <input
              type="text"
              value={newLang}
              onChange={(e) => setNewLang(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && addLanguage()}
              placeholder="Language code (e.g. de, en, fr)"
              className="flex-1 max-w-xs px-2 py-1 text-sm rounded border border-border-default bg-bg-primary text-text-primary focus:outline-none focus:border-accent"
              autoFocus
            />
            <button onClick={addLanguage} className="px-3 py-1 text-sm rounded bg-accent text-white cursor-pointer">
              Add
            </button>
            <button onClick={() => setShowAddLang(false)} className="p-1 rounded hover:bg-bg-hover text-text-tertiary cursor-pointer">
              <X size={16} />
            </button>
          </div>
        )}

        {languages.length === 0 && !showAddLang && (
          <div className="flex flex-col items-center justify-center py-16 text-text-tertiary">
            <Globe size={48} className="mb-4 opacity-40" />
            <p>No languages configured.</p>
            <p className="text-sm mt-1">Click "Add Language" to get started.</p>
          </div>
        )}

        {languages.map((lang) => {
          const cfg = config.languages[lang];
          const isActive = config.active_language === lang;
          const available = getAvailableDatasets(lang);
          return (
            <div
              key={lang}
              className={`rounded-xl border bg-bg-card shadow-sm p-4 ${
                isActive ? "border-accent ring-1 ring-accent/20" : "border-border-default"
              }`}
            >
              {/* Language header */}
              <div className="flex items-center gap-3 mb-3">
                <span className="text-sm font-semibold uppercase text-text-primary">{lang}</span>
                {isActive && (
                  <span className="text-[10px] px-1.5 py-0.5 rounded bg-accent/10 text-accent font-medium">
                    Active
                  </span>
                )}
                <div className="flex-1" />
                <button
                  onClick={() => removeLanguage(lang)}
                  className="p-1 rounded text-text-tertiary hover:text-red-500 hover:bg-red-500/10 cursor-pointer"
                  title={`Remove '${lang}'`}
                >
                  <X size={14} />
                </button>
              </div>

              {/* Selected dataset pills */}
              {cfg.dataset_uuids.length > 0 ? (
                <div className="flex flex-wrap gap-2 mb-3">
                  {cfg.dataset_uuids.map((uuid) => {
                    const isDefault = cfg.default_dataset_uuid === uuid;
                    return (
                      <div
                        key={uuid}
                        className={`flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg border text-sm ${
                          isDefault ? "border-yellow-400/40 bg-yellow-400/5" : "border-border-default"
                        }`}
                      >
                        <button
                          className={`shrink-0 cursor-pointer ${
                            isDefault ? "text-yellow-500" : "text-text-tertiary hover:text-yellow-400"
                          }`}
                          title={isDefault ? "Default dataset" : "Set as default"}
                          onClick={() => setDefault(lang, uuid)}
                        >
                          <Star size={13} className={isDefault ? "fill-current" : ""} />
                        </button>
                        <span className="text-text-primary">{getDatasetName(uuid)}</span>
                        <button
                          className="shrink-0 text-text-tertiary hover:text-red-500 cursor-pointer ml-0.5"
                          onClick={() => removeDataset(lang, uuid)}
                          title="Remove"
                        >
                          <X size={13} />
                        </button>
                      </div>
                    );
                  })}
                </div>
              ) : (
                <p className="text-xs text-text-tertiary mb-3">No datasets selected.</p>
              )}

              {/* Add dataset selector */}
              {available.length > 0 && (
                <div className="mb-3">
                  <select
                    value=""
                    onChange={(e) => addDatasetToLang(lang, e.target.value)}
                    className="text-sm px-2 py-1 rounded border border-border-default bg-bg-primary text-text-primary focus:outline-none focus:border-accent max-w-xs"
                  >
                    <option value="">+ Add dataset...</option>
                    {available.map((ds) => (
                      <option key={ds.info.uuid} value={ds.info.uuid}>
                        {ds.info.name} ({ds.card_count} cards)
                      </option>
                    ))}
                  </select>
                </div>
              )}

              {/* Per-language actions */}
              <div className="flex items-center gap-2 pt-2 border-t border-border-default">
                <button
                  onClick={handleSave}
                  disabled={loading}
                  className="px-3 py-1 text-xs rounded-lg bg-accent text-white font-medium hover:bg-accent-hover disabled:opacity-50 cursor-pointer"
                >
                  Save
                </button>
                <button
                  onClick={() => handleLoad(lang)}
                  disabled={loading || cfg.dataset_uuids.length === 0}
                  className={`px-3 py-1 text-xs rounded-lg flex items-center gap-1 disabled:opacity-50 cursor-pointer ${
                    isActive
                      ? "border border-border-default text-text-secondary hover:bg-bg-hover"
                      : "bg-emerald-600 text-white hover:bg-emerald-700"
                  }`}
                >
                  <Play size={11} />
                  {isActive ? "Reload" : "Load"}
                </button>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
