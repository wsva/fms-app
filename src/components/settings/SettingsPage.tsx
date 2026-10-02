"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface GlobalSettings {
  ollama_url: string;
  selected_model: string;
  model_dir: string;
  model_unload_timeout: ModelUnloadTimeout;
  onboarding_completed: boolean;
}

type ModelUnloadTimeout =
  | "immediately"
  | "never"
  | { minutes: number };

interface WorkspaceSettings {
  recordings_dir: string;
  datasets_dir: string;
  books_dir: string;
  wiki_dir: string;
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
  const [globalSettings, setGlobalSettings] = useState<GlobalSettings | null>(null);
  const [workspaceSettings, setWorkspaceSettings] = useState<WorkspaceSettings | null>(null);
  const [savingGlobal, setSavingGlobal] = useState(false);
  const [savedGlobal, setSavedGlobal] = useState(false);
  const [currentTheme, setCurrentTheme] = useState<ThemeId>("light");

  // ---- Load settings ----

  const fetchSettings = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const [g, w] = await Promise.all([
        invoke<GlobalSettings>("settings_get_global"),
        invoke<WorkspaceSettings>("settings_get_workspace"),
      ]);
      setGlobalSettings(g);
      setWorkspaceSettings(w);
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

  // ---- Update helpers ----

  function updateGlobalField(field: keyof GlobalSettings, value: string) {
    setGlobalSettings((prev) => (prev ? { ...prev, [field]: value } : prev));
    setSavedGlobal(false);
  }

  // ---- Save handlers ----

  async function handleSaveGlobal() {
    if (!isTauri() || !globalSettings) return;
    setSavingGlobal(true);
    try {
      await invoke("settings_set_global", { global: globalSettings });
      setSavedGlobal(true);
      setTimeout(() => setSavedGlobal(false), 2000);
    } catch (e) {
      console.error("Failed to save global settings:", e);
    } finally {
      setSavingGlobal(false);
    }
  }

  // ---- Read-only directory field ----

  function DirField({
    label,
    description,
    value,
  }: {
    label: string;
    description: string;
    value: string;
  }) {
    return (
      <div className="mb-4">
        <label className="block font-medium mb-1">{label}</label>
        <p className="text-text-secondary text-sm mb-2">{description}</p>
        <input
          type="text"
          readOnly
          className="w-full px-3 py-2 border border-border-light rounded-md bg-bg-card text-text-secondary cursor-default"
          value={value}
        />
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

        {/* ── Global Settings section ────────────────────────────── */}
        <section className="mb-8">
          <div className="flex items-center justify-between mb-2">
            <h2 className="text-[1.3em] font-semibold">Global Settings</h2>
            <div className="flex items-center gap-2">
              {savedGlobal && (
                <span className="text-sm text-green-500">Saved!</span>
              )}
              <button
                className={btnPrimary}
                onClick={handleSaveGlobal}
                disabled={savingGlobal}
              >
                {savingGlobal ? "Saving..." : "Save"}
              </button>
            </div>
          </div>
          <p className="text-text-secondary text-sm mb-4">
            These settings apply across all workspaces. Stored in the global config directory.
          </p>

          {/* Ollama URL */}
          <div className="mb-4">
            <label className="block font-medium mb-1">Ollama API URL</label>
            <p className="text-text-secondary text-sm mb-2">
              The base URL of your local Ollama instance. Default: http://localhost:11434
            </p>
            <input
              type="text"
              className="w-full max-w-md px-3 py-2 border border-border-light rounded-md bg-bg-input text-text-primary"
              value={globalSettings?.ollama_url ?? "http://localhost:11434"}
              onChange={(e) => updateGlobalField("ollama_url", e.target.value)}
              placeholder="http://localhost:11434"
            />
          </div>

          {/* Model Directory (read-only) */}
          <DirField
            label="STT Model Directory"
            description="Where STT models are stored. Shared across all workspaces."
            value={globalSettings?.model_dir ?? ""}
          />
        </section>

        {/* ── Workspace Settings section ─────────────────────────── */}
        <section className="mb-8">
          <h2 className="text-[1.3em] font-semibold mb-2">Workspace Settings</h2>
          <p className="text-text-secondary text-sm mb-4">
            These directories are set to default values for the current workspace. Stored in the workspace directory.
          </p>

          <DirField
            label="Datasets Directory"
            description="Where language learning datasets are stored."
            value={workspaceSettings?.datasets_dir ?? ""}
          />

          <DirField
            label="Recordings Directory"
            description="Where audio recordings are stored."
            value={workspaceSettings?.recordings_dir ?? ""}
          />

          <DirField
            label="Books Directory"
            description="Default: <workspace>/datasets/book. Additional locations can be linked from the Read a Book > Manage page."
            value={workspaceSettings?.books_dir ?? ""}
          />

          <DirField
            label="Wiki Directory"
            description="Root directory for wiki markdown documents."
            value={workspaceSettings?.wiki_dir ?? ""}
          />
        </section>
      </main>
    </>
  );
}
