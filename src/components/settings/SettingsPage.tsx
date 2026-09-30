"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface AppSettings {
  model_dir: string;
  recordings_dir: string;
  datasets_dir: string;
  books_dir: string;
  wiki_dir: string;
  ocr_engine: string;
  logics_parsing_repo: string;
  logics_parsing_model: string;
  logics_parsing_env: string;
}

type ThemeId = "light" | "dark" | "solarized" | "gruvbox";

const themes: { id: ThemeId; label: string; preview: string }[] = [
  { id: "light", label: "Light", preview: "bg-[#fbfbfb] border-[#e0e0e0]" },
  { id: "dark", label: "Dark", preview: "bg-[#1a1a1a] border-[#3a3a3a]" },
  { id: "solarized", label: "Solarized", preview: "bg-[#fdf6e3] border-[#d3cbb7]" },
  { id: "gruvbox", label: "Gruvbox", preview: "bg-[#282828] border-[#504945]" },
];

// ---------------------------------------------------------------------------
// Theme helpers
// ---------------------------------------------------------------------------

function getStoredTheme(): ThemeId {
  if (typeof window === "undefined") return "light";
  return (localStorage.getItem("theme") as ThemeId) || "light";
}

function applyTheme(theme: ThemeId) {
  localStorage.setItem("theme", theme);
  if (theme === "light") {
    document.documentElement.removeAttribute("data-theme");
  } else {
    document.documentElement.setAttribute("data-theme", theme);
  }
}

// ---------------------------------------------------------------------------
// Shared Tailwind class fragments
// ---------------------------------------------------------------------------

const btnBase = "px-4 py-2 rounded-md font-medium cursor-pointer transition-colors disabled:opacity-50 disabled:cursor-not-allowed";
const btnPrimary = `${btnBase} bg-accent-bg text-white hover:bg-accent-bg-hover`;

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function SettingsPage() {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [currentTheme, setCurrentTheme] = useState<ThemeId>("light");

  // ---- Load settings ----

  const fetchSettings = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const res = await invoke<AppSettings>("settings_get");
      setSettings(res);
    } catch (e) {
      console.error("Failed to load settings:", e);
    }
  }, []);

  useEffect(() => {
    fetchSettings();
    setCurrentTheme(getStoredTheme());
  }, [fetchSettings]);

  // ---- Theme change ----

  function handleThemeChange(theme: ThemeId) {
    setCurrentTheme(theme);
    applyTheme(theme);
  }

  // ---- Update a field ----

  function updateField(field: keyof AppSettings, value: string | boolean) {
    setSettings((prev) => (prev ? { ...prev, [field]: value } : prev));
    setSaved(false);
  }

  // ---- Pick folder via native dialog ----

  async function pickFolder(field: keyof AppSettings) {
    if (!isTauri()) return;
    try {
      const path = await invoke<string>("settings_pick_folder", {
        field,
      });
      updateField(field, path);
    } catch (e) {
      console.log("Folder picker:", e);
    }
  }

  // ---- Save settings ----

  async function handleSave() {
    if (!isTauri() || !settings) return;
    setSaving(true);
    try {
      await invoke("settings_set", { settings });
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch (e) {
      console.error("Failed to save settings:", e);
    } finally {
      setSaving(false);
    }
  }

  // ---- Folder field renderer ----

  function FolderField({
    label,
    description,
    field,
  }: {
    label: string;
    description: string;
    field: keyof AppSettings;
  }) {
    const value = (settings?.[field] ?? "") as string;
    return (
      <div className="mb-4">
        <label className="block font-medium mb-1">{label}</label>
        <p className="text-text-secondary text-sm mb-2">{description}</p>
        <div className="flex gap-2">
          <input
            type="text"
            className="flex-1 px-3 py-2 border border-border-light rounded-md bg-bg-input text-text-primary"
            value={value}
            onChange={(e) => updateField(field, e.target.value)}
            placeholder="Enter path or browse..."
          />
          <button
            className={`${btnPrimary} px-3`}
            onClick={() => pickFolder(field)}
          >
            Browse
          </button>
        </div>
      </div>
    );
  }

  return (
    <>
      <main className="flex-1 p-8 overflow-y-auto">
        <h1 className="text-[1.8em] font-bold mb-6">Settings</h1>

        {/* ── Theme section ──────────────────────────────────────── */}
        <section className="mb-8">
          <h2 className="text-[1.3em] font-semibold mb-2">Theme</h2>
          <p className="text-text-secondary text-sm mb-4">
            Choose a color theme for the application.
          </p>
          <div className="flex flex-wrap gap-3">
            {themes.map((t) => (
              <button
                key={t.id}
                className={`flex flex-col items-center gap-2 p-3 rounded-lg border-2 transition-colors cursor-pointer w-28 ${
                  currentTheme === t.id
                    ? "border-accent bg-bg-hover"
                    : "border-border-default bg-bg-card hover:border-accent-hover"
                }`}
                onClick={() => handleThemeChange(t.id)}
              >
                <div
                  className={`w-full h-10 rounded-md border ${t.preview}`}
                />
                <span className="text-sm font-medium">{t.label}</span>
              </button>
            ))}
          </div>
        </section>
      </main>
    </>
  );
}
