"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Globe,
  LayoutGrid,
  Mic,
  Palette,
  Server,
  Settings as SettingsIcon,
  Users,
} from "lucide-react";
import {
  CollapsibleSidebar,
  SidebarToggleButton,
  useCollapsibleSidebar,
} from "@/components/layout/CollapsibleSidebar";
import { isMobileApp } from "@/lib/platform";
import { logError, logInfo } from "@/lib/logger";
import {
  getPreferredMicDeviceId,
  listInputDevices,
  setPreferredMicDeviceId,
  type MicDevice,
} from "@/lib/voice-input";

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface GlobalSettings {
  ollama_url: string;
  llm_provider: string;
  llm_api_key: string;
  llm_model: string;
  goose_acp_url: string;
  goose_acp_secret: string;
  goose_acp_enabled: boolean;
  pc_url: string;
  pc_token: string;
  // Carried through unchanged on save: the device pairing identity (unused on
  // the PC itself, but must not be wiped when settings are written back).
  device_id: string;
  device_seed: string;
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

// A row of the PC's paired-device registry (pairing.rs `PairedDevice`).
interface PairedDevice {
  device_id: string;
  name: string;
  status: string; // "approved" | "denied"
  created_at: string;
  last_seen_at: string | null;
  // Identity this device is bound to for writeback (null for legacy pairings).
  bound_user_id: string | null;
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
const btnGhost = "px-3 py-1 text-xs rounded-md border border-border-light hover:bg-bg-hover cursor-pointer disabled:cursor-not-allowed disabled:opacity-50";

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function SettingsPage() {
  const [globalSettings, setGlobalSettings] = useState<GlobalSettings | null>(null);
  const [workspaceSettings, setWorkspaceSettings] = useState<WorkspaceSettings | null>(null);
  const [savingGlobal, setSavingGlobal] = useState(false);
  const [savedGlobal, setSavedGlobal] = useState(false);
  const [agentTest, setAgentTest] = useState<{ kind: "ok" | "err" | "busy"; text: string } | null>(null);
  const [currentTheme, setCurrentTheme] = useState<ThemeId>("light");

  // ---- Pairing (desktop only) ----
  // `isMobileApp()` is false during prerendering (no navigator) and only turns
  // true inside the Android WebView, so reading it while rendering would make
  // the server HTML and the first client render disagree (hydration error).
  // Defer it to an effect, like Sidebar / DictationPage do.
  const [mobile, setMobile] = useState(false);
  const [devices, setDevices] = useState<PairedDevice[]>([]);
  const [pairError, setPairError] = useState("");

  useEffect(() => {
    setMobile(isMobileApp());
  }, []);

  // ---- Section navigation sidebar ----
  // Shared slide-in sidebar (desktop split column / mobile drawer). Its
  // entries are the page's sections: picking one shows only that section,
  // "All" restores the stacked view.
  const sidebar = useCollapsibleSidebar({
    storageKey: "settings-sidebar-width",
    defaultWidth: 220,
    minWidth: 160,
  });
  const [activeSection, setActiveSection] = useState("all");

  function selectSection(id: string) {
    setActiveSection(id);
    // Closes the mobile drawer only — the desktop column stays open so the
    // user can keep switching sections.
    sidebar.closeOnSelect();
  }

  const sections = [
    { id: "all", label: "All", icon: LayoutGrid },
    { id: "theme", label: "Theme", icon: Palette },
    { id: "global", label: "Global Settings", icon: Globe },
    { id: "workspace", label: "Workspace Settings", icon: Server },
    { id: "microphone", label: "Microphone", icon: Mic },
    // Pairing is a PC-side feature, so the phone has no such section.
    ...(sidebar.mobile ? [] : [{ id: "pairing", label: "Device Pairing", icon: Users }]),
  ];

  // Section hidden while another one is selected? Visibility is toggled with
  // a class instead of unmounting, so form state and "Saved!" toasts survive
  // switching between sections.
  const showSection = (id: string) => activeSection === "all" || activeSection === id;

  const loadDevices = useCallback(async () => {
    if (!isTauri() || isMobileApp()) return;
    try {
      setDevices(await invoke<PairedDevice[]>("pairing_list"));
    } catch (e) {
      console.error("Failed to load paired devices:", e);
    }
  }, []);

  useEffect(() => {
    loadDevices();
  }, [loadDevices]);

  // ---- Microphone (voice input) ----
  // Stored in localStorage, not in the backend settings file: the input device
  // belongs to the machine the microphone is plugged into, so a paired phone
  // must not inherit this PC's pick. Chrome also hides device *names* until the
  // page has been granted the microphone, hence the explicit Detect button.
  const [micDevices, setMicDevices] = useState<MicDevice[]>([]);
  const [micDeviceId, setMicDeviceId] = useState("");
  const [micNamed, setMicNamed] = useState(false);
  const [micLoading, setMicLoading] = useState(false);
  const [micError, setMicError] = useState("");
  const [micSaved, setMicSaved] = useState(false);

  const loadMics = useCallback(async (requestAccess: boolean) => {
    setMicLoading(true);
    setMicError("");
    try {
      const list = await listInputDevices(requestAccess);
      setMicDevices(list);
      setMicNamed(list.length > 0 && list.every((d) => d.label !== ""));
      const stored = getPreferredMicDeviceId();
      // Keep showing an id whose device was unplugged (the capture path falls
      // back to the default for it), but mark it so the user can re-pick.
      setMicDeviceId(stored);
      if (stored && list.length > 0 && !list.some((d) => d.deviceId === stored)) {
        setMicError("The pinned microphone is not connected right now — voice input is using the system default until you pick another one.");
      }
      logInfo(
        `settings: listed ${list.length} audio input device(s), names ${
          list.length > 0 && list.every((d) => d.label !== "") ? "available" : "hidden"
        }`,
        "settings"
      );
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setMicError(`Could not list microphones: ${msg}`);
      logError(`settings: enumerating microphones failed — ${msg}`, "settings");
    } finally {
      setMicLoading(false);
    }
  }, []);

  useEffect(() => {
    setMicDeviceId(getPreferredMicDeviceId());
    // Read-only pass on mount: enumerateDevices() without a getUserMedia probe
    // never triggers a permission prompt.
    void loadMics(false);
  }, [loadMics]);

  function handleMicChange(deviceId: string) {
    setMicDeviceId(deviceId);
    setPreferredMicDeviceId(deviceId);
    setMicSaved(true);
    setTimeout(() => setMicSaved(false), 2000);
  }

  const selectedMicMissing = micDeviceId !== "" && !micDevices.some((d) => d.deviceId === micDeviceId);

  async function handleRevoke(deviceId: string) {
    try {
      await invoke("pairing_revoke", { deviceId });
      loadDevices();
    } catch (e) {
      setPairError(`Failed to revoke device: ${String(e)}`);
    }
  }

  async function handleRemoveDenied(deviceId: string) {
    try {
      await invoke("pairing_remove_denied", { deviceId });
      loadDevices();
    } catch (e) {
      setPairError(`Failed to remove device: ${String(e)}`);
    }
  }

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

  // Persist the current URL/secret, then attempt an ACP connect and report the
  // outcome inline. `agent_connect` is idempotent, so this doubles as a status
  // probe when already connected.
  async function handleTestAgent() {
    if (!isTauri() || !globalSettings) return;
    setAgentTest({ kind: "busy", text: "Connecting…" });
    try {
      await invoke("settings_set_global", { global: globalSettings });
      const status = await invoke<{
        connected: boolean;
        session_id?: string | null;
        mcp_registered?: boolean;
      }>("agent_connect");
      if (status.connected) {
        setAgentTest({
          kind: "ok",
          text: `Connected (session ${status.session_id ?? "?"})${
            status.mcp_registered ? " · fms-app tools registered" : ""
          }.`,
        });
      } else {
        setAgentTest({ kind: "err", text: "Not connected." });
      }
    } catch (e) {
      setAgentTest({ kind: "err", text: e instanceof Error ? e.message : String(e) });
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
    <main className="flex-1 flex flex-col h-full min-h-0 min-w-0">
      {/* Header — same bar style as Read a Book: icon + title on a bordered
          strip pinned above the scrolling content. */}
      <div className="p-4 border-b border-border-default flex items-center gap-4 shrink-0 min-w-0">
        <h1 className="text-lg font-semibold flex items-center gap-2 text-text-primary">
          <SettingsIcon size={20} /> Settings
        </h1>
        <SidebarToggleButton sidebar={sidebar} title="Show/hide sections" />
      </div>

      {/* Body — flex row so the desktop section list splits the page with the
          content; on mobile the drawer is anchored here (absolute, not fixed
          to the viewport), so it stays below the header and bottom nav. */}
      <div className="relative flex flex-1 min-h-0 overflow-hidden">
        <CollapsibleSidebar sidebar={sidebar}>
          <nav className="flex flex-col gap-1 p-3 overflow-y-auto min-h-0">
            <div className="text-xs font-semibold text-text-tertiary uppercase tracking-wide px-2 pt-1 pb-2 mb-1 border-b border-border-light">
              Sections
            </div>
            {sections.map((s) => {
              const Icon = s.icon;
              return (
                <button
                  key={s.id}
                  className={`flex items-center gap-2 px-2 py-1.5 rounded-md text-sm text-left cursor-pointer transition-colors ${
                    activeSection === s.id
                      ? "bg-bg-hover text-text-primary font-medium"
                      : "text-text-secondary hover:bg-bg-hover hover:text-text-primary"
                  }`}
                  onClick={() => selectSection(s.id)}
                >
                  <Icon size={15} className="shrink-0" />
                  <span className="truncate">{s.label}</span>
                </button>
              );
            })}
          </nav>
        </CollapsibleSidebar>

        {/* Content */}
        <div className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden p-4 min-w-0">

        {/* ── Theme section ──────────────────────────────────────── */}
        <section className={`mb-8${showSection("theme") ? "" : " hidden"}`}>
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
        <section className={`mb-8${showSection("global") ? "" : " hidden"}`}>
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

          {/* LLM provider */}
          <div className="mb-4">
            <label className="block font-medium mb-1">LLM Provider</label>
            <p className="text-text-secondary text-sm mb-2">
              Backend used for the chat page and other LLM features (via the goose
              SDK provider layer). Local Ollama needs no API key.
            </p>
            <select
              className="w-full max-w-md px-3 py-2 border border-border-light rounded-md bg-bg-input text-text-primary"
              value={globalSettings?.llm_provider ?? "ollama"}
              onChange={(e) => updateGlobalField("llm_provider", e.target.value)}
            >
              <option value="ollama">Ollama (local)</option>
              <option value="openai">OpenAI</option>
              <option value="anthropic">Anthropic</option>
              <option value="groq">Groq</option>
              <option value="databricks">Databricks</option>
            </select>
          </div>

          {/* API key (cloud providers only) */}
          {(globalSettings?.llm_provider ?? "ollama") !== "ollama" && (
            <div className="mb-4">
              <label className="block font-medium mb-1">
                {(globalSettings?.llm_provider ?? "") === "databricks" ? "Token" : "API Key"}
              </label>
              <input
                type="password"
                className="w-full max-w-md px-3 py-2 border border-border-light rounded-md bg-bg-input text-text-primary"
                value={globalSettings?.llm_api_key ?? ""}
                onChange={(e) => updateGlobalField("llm_api_key", e.target.value)}
                placeholder="Paste your API key"
                autoComplete="off"
              />
            </div>
          )}

          {/* Default model (cloud providers only) */}
          {(globalSettings?.llm_provider ?? "ollama") !== "ollama" && (
            <div className="mb-4">
              <label className="block font-medium mb-1">Default Model</label>
              <p className="text-text-secondary text-sm mb-2">
                Model name used on the chat page. You can still change it per conversation.
              </p>
              <input
                type="text"
                className="w-full max-w-md px-3 py-2 border border-border-light rounded-md bg-bg-input text-text-primary"
                value={globalSettings?.llm_model ?? ""}
                onChange={(e) => updateGlobalField("llm_model", e.target.value)}
                placeholder="e.g. gpt-4o, claude-sonnet-4-5, llama-3.3-70b-versatile"
              />
            </div>
          )}

          {/* Ollama URL / Databricks host */}
          {["ollama", "databricks"].includes(globalSettings?.llm_provider ?? "ollama") && (
          <div className="mb-4">
            <label className="block font-medium mb-1">
              {(globalSettings?.llm_provider ?? "ollama") === "databricks"
                ? "Databricks Host"
                : "Ollama API URL"}
            </label>
            <p className="text-text-secondary text-sm mb-2">
              {(globalSettings?.llm_provider ?? "ollama") === "databricks"
                ? "The Databricks workspace host used as the provider base URL."
                : "The base URL of your local Ollama instance. Default: http://localhost:11434"}
            </p>
            <input
              type="text"
              className="w-full max-w-md px-3 py-2 border border-border-light rounded-md bg-bg-input text-text-primary"
              value={globalSettings?.ollama_url ?? "http://localhost:11434"}
              onChange={(e) => updateGlobalField("ollama_url", e.target.value)}
              placeholder="http://localhost:11434"
            />
          </div>
          )}

          {/* Model Directory (read-only) */}
          <DirField
            label="STT Model Directory"
            description="Where STT models are stored. Shared across all workspaces."
            value={globalSettings?.model_dir ?? ""}
          />

          {/* ── Agent (Goose ACP) — desktop only ─────────────────── */}
          {!mobile && (
            <div className="mt-6 pt-6 border-t border-border-light">
              <h3 className="text-lg font-semibold mb-1">Agent (Goose ACP)</h3>
              <p className="text-text-secondary text-sm mb-4">
                Connect to a running <code>goose serve</code> over the Agent Client Protocol so the
                agent can drive fms-app with natural messages. Desktop only. The app offers its
                built-in MCP tools to goose automatically on connect when goose supports HTTP MCP;
                otherwise register the fms-app extension in goose pointing at the loopback server.
              </p>

              <label className="flex items-center gap-2 mb-4 cursor-pointer">
                <input
                  type="checkbox"
                  className="w-4 h-4"
                  checked={globalSettings?.goose_acp_enabled ?? false}
                  onChange={(e) => {
                    setGlobalSettings((prev) =>
                      prev ? { ...prev, goose_acp_enabled: e.target.checked } : prev
                    );
                    setSavedGlobal(false);
                  }}
                />
                <span className="font-medium">Enable Agent integration</span>
              </label>

              <div className="mb-4">
                <label className="block font-medium mb-1">ACP WebSocket URL</label>
                <p className="text-text-secondary text-sm mb-2">
                  The <code>goose serve</code> ACP endpoint. Default: ws://127.0.0.1:3284/acp
                </p>
                <input
                  type="text"
                  className="w-full max-w-md px-3 py-2 border border-border-light rounded-md bg-bg-input text-text-primary"
                  value={globalSettings?.goose_acp_url ?? "ws://127.0.0.1:3284/acp"}
                  onChange={(e) => updateGlobalField("goose_acp_url", e.target.value)}
                  placeholder="ws://127.0.0.1:3284/acp"
                />
              </div>

              <div className="mb-4">
                <label className="block font-medium mb-1">Secret Key</label>
                <p className="text-text-secondary text-sm mb-2">
                  Sent as the <code>X-Secret-Key</code> header; must match goose&apos;s
                  <code> GOOSE_SERVER__SECRET_KEY</code>. Leave empty if unauthenticated.
                </p>
                <input
                  type="password"
                  className="w-full max-w-md px-3 py-2 border border-border-light rounded-md bg-bg-input text-text-primary"
                  value={globalSettings?.goose_acp_secret ?? ""}
                  onChange={(e) => updateGlobalField("goose_acp_secret", e.target.value)}
                  placeholder="goose server secret key"
                  autoComplete="off"
                />
              </div>

              <div className="flex items-center gap-3">
                <button
                  type="button"
                  className={btnGhost}
                  onClick={handleTestAgent}
                  disabled={agentTest?.kind === "busy"}
                >
                  {agentTest?.kind === "busy" ? "Connecting…" : "Test connection"}
                </button>
                {agentTest && agentTest.kind !== "busy" && (
                  <span
                    className={`text-sm ${
                      agentTest.kind === "ok" ? "text-green-500" : "text-error-text"
                    }`}
                  >
                    {agentTest.text}
                  </span>
                )}
              </div>
            </div>
          )}
        </section>

        {/* ── Workspace Settings section ─────────────────────────── */}
        <section className={`mb-8${showSection("workspace") ? "" : " hidden"}`}>
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

        {/* ── Microphone section ─────────────────────────────── */}
        <section className={`mb-8${showSection("microphone") ? "" : " hidden"}`}>
          <div className="flex items-center justify-between mb-2">
            <h2 className="text-[1.3em] font-semibold">Microphone</h2>
            <div className="flex items-center gap-2">
              {micSaved && <span className="text-sm text-green-500">Saved!</span>}
              <button
                className={btnGhost}
                onClick={() => loadMics(true)}
                disabled={micLoading}
                title="Reads the device names, which the webview only reveals once microphone access has been granted"
              >
                {micLoading ? "Detecting..." : "Detect microphones"}
              </button>
            </div>
          </div>
          <p className="text-text-secondary text-sm mb-4">
            Input device used by voice input and by Read a Book recordings. Leave it on
            System default to follow the operating system, or pin your real microphone so a
            virtual device (Steam, OBS, Voicemeeter) cannot quietly take over dictation. Stored
            on this device only.
          </p>

          {micError && <p className="text-sm text-red-600 mb-2">{micError}</p>}

          <div className="flex items-center gap-3 flex-wrap">
            <select
              className="px-3 py-2 border border-border-light rounded-md bg-bg-input text-text-primary max-w-md w-full"
              value={micDeviceId}
              onChange={(e) => handleMicChange(e.target.value)}
            >
              <option value="">System default</option>
              {micDevices.map((d, i) => (
                <option key={d.deviceId} value={d.deviceId}>
                  {d.label || `Microphone ${i + 1} (name hidden)`}
                </option>
              ))}
              {selectedMicMissing && <option value={micDeviceId}>Pinned device (not connected)</option>}
            </select>
          </div>

          {!micNamed && !micLoading && (
            <p className="text-xs text-text-tertiary mt-2">
              Device names are hidden until this app has microphone access — press Detect
              microphones (or dictate once) to see which entry is which.
            </p>
          )}
          {micDevices.length === 0 && !micLoading && (
            <p className="text-xs text-text-tertiary mt-2">
              No audio input devices reported. If you expected one, check that it is connected
              and not disabled in the system sound settings.
            </p>
          )}
        </section>

        {/* ── Device pairing section (PC only) ──────────────────── */}
        {!mobile && (
          <section className={`mb-8${showSection("pairing") ? "" : " hidden"}`}>
            <div className="flex items-center justify-between mb-2">
              <h2 className="text-[1.3em] font-semibold">Device Pairing</h2>
              <button
                className="px-3 py-1 text-xs rounded-md border border-border-light hover:bg-bg-hover cursor-pointer"
                onClick={loadDevices}
              >
                Refresh
              </button>
            </div>
            <p className="text-text-secondary text-sm mb-4">
              Devices on the LAN/WLAN must pair before they can read or write data — like
              Bluetooth headphones. A phone that asks to connect pops a confirmation dialog here,
              and it can work once you allow it. Requests from this computer (localhost) and from
              Tailscale are always allowed, so local automations need no pairing.
            </p>

            {pairError && (
              <p className="text-sm text-red-600 mb-3">{pairError}</p>
            )}

            <table className="w-full text-sm border-collapse">
              <thead>
                <tr className="text-left text-text-secondary border-b border-border-light">
                  <th className="py-2 pr-4 font-medium">Device</th>
                  <th className="py-2 pr-4 font-medium">Code</th>
                  <th className="py-2 pr-4 font-medium">Status</th>
                  <th className="py-2 pr-4 font-medium">Syncs as</th>
                  <th className="py-2 pr-4 font-medium">Last seen</th>
                  <th className="py-2 font-medium"></th>
                </tr>
              </thead>
              <tbody>
                {devices.length === 0 && (
                  <tr>
                    <td colSpan={6} className="py-3 text-text-tertiary">
                      No devices paired yet.
                    </td>
                  </tr>
                )}
                {devices.map((d) => (
                  <tr key={d.device_id} className="border-b border-border-light">
                    <td className="py-2 pr-4">
                      <div className="font-medium">{d.name}</div>
                      <div className="text-xs text-text-tertiary font-mono">{d.device_id}</div>
                    </td>
                    {/* Fingerprint = the device_id hash prefix: both sides show the same code. */}
                    <td className="py-2 pr-4 font-mono">{d.device_id.slice(0, 6)}</td>
                    <td className="py-2 pr-4">
                      <span
                        className={`px-2 py-0.5 rounded text-xs ${
                          d.status === "approved"
                            ? "bg-green-100 text-green-700"
                            : "bg-red-100 text-red-700"
                        }`}
                      >
                        {d.status}
                      </span>
                    </td>
                    <td className="py-2 pr-4 text-text-secondary">
                      {d.bound_user_id && d.bound_user_id !== "local"
                        ? d.bound_user_id
                        : "this PC's user"}
                    </td>
                    <td className="py-2 pr-4 text-text-secondary">
                      {d.last_seen_at ?? "never"}
                    </td>
                    <td className="py-2 text-right">
                      {d.status === "approved" ? (
                        <button
                          className="px-3 py-1 text-xs rounded-md border border-border-light hover:bg-bg-hover cursor-pointer"
                          onClick={() => handleRevoke(d.device_id)}
                        >
                          Revoke
                        </button>
                      ) : (
                        <button
                          className="px-3 py-1 text-xs rounded-md border border-border-light hover:bg-bg-hover cursor-pointer"
                          onClick={() => handleRemoveDenied(d.device_id)}
                        >
                          Remove
                        </button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </section>
        )}
        </div>
      </div>
    </main>
  );
}
