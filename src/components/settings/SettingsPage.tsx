"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isMobileApp } from "@/lib/platform";
import { logInfo, logError } from "@/lib/logger";

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// A discovered PC returned by the `pc_discover` command.
interface PcCandidate {
  url: string;
  source: string; // "lan" | "tailscale" | "saved"
  name: string;
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface GlobalSettings {
  ollama_url: string;
  pc_url: string;
  pc_token: string;
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

  // ---- Mobile PC discovery state ----
  const [mobile, setMobile] = useState(false);
  const [candidates, setCandidates] = useState<PcCandidate[]>([]);
  const [discovering, setDiscovering] = useState(false);
  const [discoverMsg, setDiscoverMsg] = useState("");
  const [checking, setChecking] = useState(false);

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
    setMobile(isMobileApp());
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

  // ---- Mobile: PC connection ----

  async function savePc(url: string) {
    if (!globalSettings) return;
    const next = { ...globalSettings, pc_url: url };
    setGlobalSettings(next);
    try {
      await invoke("settings_set_global", { global: next });
      setSavedGlobal(true);
      setTimeout(() => setSavedGlobal(false), 2000);
    } catch (e) {
      console.error("Failed to save PC url:", e);
    }
  }

  async function handleDiscover() {
    if (!isTauri()) return;
    setDiscovering(true);
    setDiscoverMsg("");
    setCandidates([]);
    try {
      const res = await invoke<PcCandidate[]>("pc_discover", { timeoutMs: 2500 });
      setCandidates(res);
      if (res.length === 0) setDiscoverMsg("No FmS PC found on this network.");
    } catch (e) {
      setDiscoverMsg(`Discovery failed: ${String(e)}`);
    } finally {
      setDiscovering(false);
    }
  }

  async function handleRecheck() {
    const url = (globalSettings?.pc_url ?? "").trim();
    if (!url) {
      logInfo("Re-check skipped: pc_url is empty (set or discover a PC first).", "settings");
      setDiscoverMsg("No PC address set. Enter or discover a PC first.");
      return;
    }
    setChecking(true);
    setDiscoverMsg("");
    logInfo(`Re-check: querying ${url} ...`, "settings");
    try {
      // Native command hits the PC with reqwest (CORS-free). We pass the current
      // field values so Re-check tests the typed address even before Save.
      const v = await invoke<{ ok?: boolean; app_name?: string; dataset_count?: number }>(
        "pc_check_status",
        { pcUrl: url, pcToken: globalSettings?.pc_token ?? "" },
      );
      if (v?.ok) {
        logInfo(`Re-check: connected (${v.app_name ?? "fms-app"}, ${v.dataset_count ?? 0} datasets).`, "settings");
        setDiscoverMsg(`Connected · ${v.app_name ?? "fms-app"} · ${v.dataset_count ?? 0} dataset(s).`);
      } else {
        logError("Re-check: PC responded but is not ready.", "settings");
        setDiscoverMsg("PC responded but is not ready.");
      }
    } catch (e) {
      logError(`Re-check failed: ${String(e)}`, "settings");
      setDiscoverMsg(`PC not reachable: ${String(e)}`);
    } finally {
      setChecking(false);
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

        {/* ── PC Connection section (mobile only) ────────────────── */}
        {mobile && (
          <section className="mb-8">
            <h2 className="text-[1.3em] font-semibold mb-2">PC Connection</h2>
            <p className="text-text-secondary text-sm mb-4">
              Connect this phone to your computer to download datasets and upload
              your progress. The two must be on the same Wi-Fi network or tailnet.
            </p>

            <button
              className={btnPrimary}
              onClick={handleDiscover}
              disabled={discovering || !isTauri()}
            >
              {discovering ? "Searching…" : "Discover PC"}
            </button>

            {candidates.length > 0 && (
              <div className="mt-3 flex flex-col gap-2 max-w-md">
                {candidates.map((c) => (
                  <button
                    key={c.url}
                    onClick={() => {
                      savePc(c.url);
                      setDiscoverMsg(`Connected to ${c.name}.`);
                    }}
                    className={`flex items-center justify-between gap-2 px-3 py-2 rounded-lg border text-left transition-colors cursor-pointer ${
                      globalSettings?.pc_url === c.url
                        ? "border-accent bg-bg-hover"
                        : "border-border-default bg-bg-card hover:border-accent-hover"
                    }`}
                  >
                    <span className="flex flex-col min-w-0">
                      <span className="font-medium truncate">{c.name}</span>
                      <span className="text-xs text-text-tertiary truncate">{c.url}</span>
                    </span>
                    <span className="text-[10px] uppercase px-1.5 py-0.5 rounded bg-bg-hover text-text-secondary shrink-0">
                      {c.source}
                    </span>
                  </button>
                ))}
              </div>
            )}

            {/* Current + manual fallback */}
            <div className="mt-4 mb-4 max-w-md">
              <label className="block font-medium mb-1">PC Address</label>
              <div className="flex items-center gap-2">
                <input
                  type="text"
                  className="flex-1 px-3 py-2 border border-border-light rounded-md bg-bg-input text-text-primary"
                  value={globalSettings?.pc_url ?? ""}
                  onChange={(e) => updateGlobalField("pc_url", e.target.value)}
                  placeholder="http://192.168.1.20:35711"
                />
                <button
                  className={`${btnBase} bg-bg-body border border-border-light hover:bg-bg-hover`}
                  onClick={handleRecheck}
                  disabled={checking || !(globalSettings?.pc_url ?? "").trim()}
                >
                  {checking ? "Checking…" : "Re-check"}
                </button>
              </div>
            </div>
            <div className="mb-4 max-w-md">
              <label className="block font-medium mb-1">Access Token (optional)</label>
              <p className="text-text-secondary text-sm mb-2">
                Only needed if the PC was configured with a shared token. Leave
                empty on a trusted network.
              </p>
              <input
                type="text"
                className="w-full px-3 py-2 border border-border-light rounded-md bg-bg-input text-text-primary"
                value={globalSettings?.pc_token ?? ""}
                onChange={(e) => updateGlobalField("pc_token", e.target.value)}
                placeholder="(none)"
              />
            </div>

            <div className="flex items-center gap-3 max-w-md">
              <button
                className={btnPrimary}
                onClick={handleSaveGlobal}
                disabled={savingGlobal}
              >
                {savingGlobal ? "Saving…" : "Save"}
              </button>
              {discoverMsg && (
                <span className="text-sm text-text-secondary">{discoverMsg}</span>
              )}
            </div>
          </section>
        )}

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
