"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import {
  ChevronDown,
  ChevronUp,
  Globe,
  Info,
  LayoutGrid,
  Palette,
  Server,
  Smartphone,
} from "lucide-react";
import { MicGlyph } from "@/components/voice/MicGlyph";
import {
  CollapsibleSidebar,
  useCollapsibleSidebar,
} from "@/components/layout/CollapsibleSidebar";
import MicTestPanel from "@/components/voice/MicTestPanel";
import { isMobileApp } from "@/lib/platform";
import { logError, logInfo } from "@/lib/logger";
import { THEMES } from "@/lib/theme";
import { useTheme } from "@/hooks/useTheme";
import { AGENT_SURFACE_SAVED } from "@/lib/agent/control";
import {
  DEFAULT_NAV_CONFIG,
  MAX_BAR_MENUS,
  NAV_CROSS_DEVICE_LABEL,
  NAV_MENUS,
  NAV_TOOLS_LABEL,
  saveNavConfig,
  useNavConfig,
} from "@/lib/mobile-nav";
import type { TabId } from "@/components/layout/Sidebar";
import {
  getPreferredMicDeviceId,
  listInputDevices,
  setPreferredMicDeviceId,
  type MicDevice,
} from "@/lib/voice-input";
import { PageHeader, Toolbar, ToolbarButton } from "@/components/ui/toolbar";
function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// Compare two dotted numeric versions ("0.1.4" vs "0.1.10"); missing parts count as 0.
function cmpSemver(a: string, b: string): number {
  const pa = a.split(".").map((n) => parseInt(n, 10) || 0);
  const pb = b.split(".").map((n) => parseInt(n, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d !== 0) return d;
  }
  return 0;
}

// Strip a leading "v" so a git tag and a stamped package version compare equal.
function normVer(v: string): string {
  return v.trim().replace(/^v/, "");
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface GlobalSettings {
  ollama_url: string;
  llm_provider: string;
  llm_api_key: string;
  llm_model: string;
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
  /** "permissive" | "enforce" — see `mcp/policy.rs`. */
  mcp_policy: string;
  /** Whether `/mcp` and the generated manual are served at all. Chosen when the
   *  router is built, so saving this restarts the local web service. */
  mcp_enabled: boolean;
  /** Whether the agent dock's floating button is drawn. Carried through
   *  unchanged: it is owned by the Agent page, and this page writes the whole
   *  object back. */
  agent_fab_visible: boolean;
}

type ModelUnloadTimeout =
  | "immediately"
  | "never"
  | { minutes: number };

interface WorkspaceSettings {
  recordings_dir: string;
  datasets_dir: string;
  books_dir: string;
  // Sync role + cluster identity (docs/design/sync.md §3.1). Workspace-
  // scoped: `role` is "hub"|"follower" (empty ⇒ follower); `cluster_id` is a
  // UUID issued once when a workspace is first designated hub.
  role: string;
  cluster_id: string;
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
  // Owned by lib/theme so the title bar's View menu and this picker can never
  // disagree about which theme is in force.
  const { theme: currentTheme, setTheme } = useTheme();

  // ---- Platform (mobile vs desktop) ----
  // `isMobileApp()` is false during prerendering (no navigator) and only turns
  // true inside the Android WebView, so reading it while rendering would make
  // the server HTML and the first client render disagree (hydration error).
  // Defer it to an effect, like Sidebar / DictationPage do.
  const [mobile, setMobile] = useState(false);

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

  // Build identity for the About section. `app_build_version` reports the git tag
  // baked at build time (dev builds) or the CI-stamped package version (released
  // builds); falls back to getVersion() if the command is unavailable, and stays
  // empty in the web dev preview (isTauri() false).
  const [installedVersion, setInstalledVersion] = useState("");
  useEffect(() => {
    if (!isTauri()) return;
    invoke<{ git_describe: string; package_version: string }>("app_build_version")
      .then(({ git_describe, package_version }) => setInstalledVersion(git_describe || package_version))
      .catch(() => {
        getVersion().then(setInstalledVersion).catch(() => {});
      });
  }, []);

  // Latest published GitHub release, fetched at runtime. Offline / blocked -> stays null.
  const [latestRelease, setLatestRelease] = useState<{ tag: string; url: string } | null>(null);
  const [releaseState, setReleaseState] = useState<"checking" | "done" | "error">("checking");
  useEffect(() => {
    let alive = true;
    fetch("https://api.github.com/repos/wsva/fms-app/releases/latest", {
      headers: { Accept: "application/vnd.github+json" },
    })
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status)))))
      .then((d: { tag_name?: string; html_url?: string }) => {
        if (!alive) return;
        if (d?.tag_name) setLatestRelease({ tag: d.tag_name, url: d.html_url ?? "" });
        setReleaseState("done");
      })
      .catch(() => {
        if (alive) setReleaseState("error");
      });
    return () => {
      alive = false;
    };
  }, []);

  // Update status: compare the installed tag's base against the latest release.
  const installedBase = normVer(installedVersion.split("-")[0]);
  const latestBase = latestRelease ? normVer(latestRelease.tag) : "";
  const isDevBuild = installedVersion.includes("-g") || installedVersion.endsWith("-dirty");
  let updateState: "none" | "up-to-date" | "update" | "ahead" = "none";
  if (latestRelease && installedBase && latestBase) {
    const c = cmpSemver(installedBase, latestBase);
    updateState = c < 0 ? "update" : c > 0 ? "ahead" : isDevBuild ? "ahead" : "up-to-date";
  }

  function selectSection(id: string) {
    setActiveSection(id);
    // Closes the mobile drawer only — the desktop column stays open so the
    // user can keep switching sections.
    sidebar.closeOnSelect();
  }

  const sections = [
    { id: "all", label: "All", icon: LayoutGrid },
    { id: "theme", label: "Theme", icon: Palette },
    // Mobile only: this customizes the Android bottom bar, which the desktop
    // sidebar shell never renders.
    ...(mobile ? [{ id: "bottom-nav", label: "Bottom Nav", icon: Smartphone }] : []),
    { id: "global", label: "Global Settings", icon: Globe },
    { id: "workspace", label: "Workspace Settings", icon: Server },
    { id: "microphone", label: "Microphone", icon: MicGlyph },
    { id: "about", label: "About", icon: Info },
  ];

  // ---- Bottom nav customization (mobile only) ----
  // Shared live config with BottomNav (lib/mobile-nav): every edit saves to
  // localStorage and re-renders the bar immediately through the change event.
  // One ordered list drives both surfaces — checked rows go to the bar in
  // list order, the rest to the "More" drawer, which keeps the PC sidebar's
  // grouping (root menus, Tools, Settings); order positions a menu inside its
  // group.
  const { config: navConfig } = useNavConfig();

  const toggleNavInBar = (id: TabId) => {
    const inBar = navConfig.bar.includes(id);
    if (!inBar && navConfig.bar.length >= MAX_BAR_MENUS) return;
    saveNavConfig({
      ...navConfig,
      bar: inBar ? navConfig.bar.filter((b) => b !== id) : [...navConfig.bar, id],
    });
  };

  const moveNavItem = (index: number, dir: -1 | 1) => {
    const to = index + dir;
    if (to < 0 || to >= navConfig.order.length) return;
    const order = [...navConfig.order];
    [order[index], order[to]] = [order[to], order[index]];
    saveNavConfig({ ...navConfig, order });
  };

  // Section hidden while another one is selected? Visibility is toggled with
  // a class instead of unmounting, so form state and "Saved!" toasts survive
  // switching between sections.
  const showSection = (id: string) => activeSection === "all" || activeSection === id;

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
  }, [fetchSettings]);

  // The Agent page owns `mcp_policy`, `mcp_enabled` and `agent_fab_visible` now.
  // Reload when it saves: this page is always mounted, so its copy would
  // otherwise keep the values read at boot and write them back over the agent
  // page's edit on the next Save here.
  useEffect(() => {
    const onAgentSurface = () => void fetchSettings();
    window.addEventListener(AGENT_SURFACE_SAVED, onAgentSurface);
    return () => window.removeEventListener(AGENT_SURFACE_SAVED, onAgentSurface);
  }, [fetchSettings]);

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
    <main className="flex-1 flex flex-col h-full min-h-0 min-w-0">
      <PageHeader title="Settings" />
      <Toolbar>
        <div className="flex-1 min-w-0" />
        <ToolbarButton
          icon={LayoutGrid}
          label="Sections"
          variant="toggleSoft"
          active={sidebar.visible}
          onClick={sidebar.toggle}
          title={sidebar.visible ? "Hide sections" : "Show sections"}
        />
      </Toolbar>

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
            {THEMES.map((t) => (
              <button
                key={t.id}
                className={`flex flex-col items-center gap-2 p-3 rounded-lg border-2 transition-colors cursor-pointer w-28 ${
                  currentTheme === t.id
                    ? "border-accent bg-bg-hover"
                    : "border-border-default bg-bg-card hover:border-accent-hover"
                }`}
                onClick={() => setTheme(t.id)}
              >
                <div
                  className={`w-full h-10 rounded-md border ${t.preview}`}
                />
                <span className="text-sm font-medium">{t.label}</span>
              </button>
            ))}
          </div>
        </section>

        {/* ── Bottom Nav section (mobile only) ─────────────────────── */}
        {mobile && (
          <section className={`mb-8${showSection("bottom-nav") ? "" : " hidden"}`}>
            <h2 className="text-[1.3em] font-semibold mb-2">Bottom Nav</h2>
            <p className="text-text-secondary text-sm mb-4">
              Pick which menus sit in the bottom bar (up to {MAX_BAR_MENUS}); checked
              rows appear there left-to-right in this order. The rest go into the
              “More” drawer, which keeps the PC sidebar’s structure — plain menus,
              the Cross Device and Tools groups, then Settings at the bottom — and
              your order places each menu inside its group. Changes apply immediately.
            </p>
            <div className="flex flex-col max-w-md">
              {navConfig.order.map((id, i) => {
                const menu = NAV_MENUS.find((m) => m.id === id)!;
                const Icon = menu.icon;
                const inBar = navConfig.bar.includes(id);
                const barFull = navConfig.bar.length >= MAX_BAR_MENUS;
                return (
                  <div
                    key={id}
                    className="flex items-center gap-2 py-2 border-b border-border-light last:border-b-0"
                  >
                    <Icon width={18} height={18} className="text-text-tertiary shrink-0" />
                    <span className="flex-1 text-sm text-text-primary truncate">{menu.label}</span>
                    {/* Where the drawer will put this menu, since it groups by zone. */}
                    {menu.group === "cross-device" && (
                      <span className="text-[10px] px-1.5 py-0.5 rounded bg-mid-gray/20 text-text-tertiary shrink-0">
                        {NAV_CROSS_DEVICE_LABEL}
                      </span>
                    )}
                    {menu.group === "tools" && (
                      <span className="text-[10px] px-1.5 py-0.5 rounded bg-mid-gray/20 text-text-tertiary shrink-0">
                        {NAV_TOOLS_LABEL}
                      </span>
                    )}
                    {menu.group === "bottom" && (
                      <span className="text-[10px] px-1.5 py-0.5 rounded bg-mid-gray/20 text-text-tertiary shrink-0">
                        Bottom
                      </span>
                    )}
                    <label
                      className={`flex items-center gap-1.5 text-xs shrink-0 ${
                        inBar || !barFull
                          ? "cursor-pointer text-text-secondary"
                          : "cursor-not-allowed text-text-tertiary/60"
                      }`}
                      title={inBar || !barFull ? "Show in the bottom bar" : `The bar holds at most ${MAX_BAR_MENUS} menus`}
                    >
                      <input
                        type="checkbox"
                        className="w-4 h-4 cursor-pointer disabled:cursor-not-allowed"
                        checked={inBar}
                        disabled={!inBar && barFull}
                        onChange={() => toggleNavInBar(id)}
                      />
                      In bar
                    </label>
                    <button
                      className={`${btnGhost} px-2 py-1`}
                      disabled={i === 0}
                      onClick={() => moveNavItem(i, -1)}
                      title="Move up"
                    >
                      <ChevronUp size={14} />
                    </button>
                    <button
                      className={`${btnGhost} px-2 py-1`}
                      disabled={i === navConfig.order.length - 1}
                      onClick={() => moveNavItem(i, 1)}
                      title="Move down"
                    >
                      <ChevronDown size={14} />
                    </button>
                  </div>
                );
              })}
            </div>
            <div className="mt-3 max-w-md">
              <button className={btnGhost} onClick={() => saveNavConfig(DEFAULT_NAV_CONFIG)}>
                Reset to defaults
              </button>
            </div>
          </section>
        )}

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
            Input device used by voice input and by Books recordings. Leave it on
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

          {/* Zoom-style audio check on the currently selected device. */}
          <MicTestPanel />
        </section>

        {/* ── About section ────────────────────────────────── */}
        <section className={`mb-8${showSection("about") ? "" : " hidden"}`}>
          <h2 className="text-[1.3em] font-semibold mb-2">About</h2>
          <p className="text-text-secondary text-sm mb-4">
            Application build information.
          </p>
          <div className="flex flex-col gap-2 max-w-xl">
            <div className="flex items-center gap-3">
              <span className="text-sm text-text-secondary w-36 shrink-0">Installed version</span>
              <span className="text-sm font-mono text-text-primary break-all">
                {installedVersion || "unknown"}
              </span>
            </div>
            <div className="flex items-center gap-3 flex-wrap">
              <span className="text-sm text-text-secondary w-36 shrink-0">Latest release</span>
              {releaseState === "checking" && (
                <span className="text-sm text-text-tertiary">Checking…</span>
              )}
              {releaseState === "error" && (
                <span className="text-sm text-text-tertiary">Unavailable (offline?)</span>
              )}
              {latestRelease && (
                <a
                  href={latestRelease.url || "#"}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-sm font-mono text-accent hover:underline break-all"
                >
                  {latestRelease.tag}
                </a>
              )}
              {updateState === "up-to-date" && (
                <span className="text-xs px-1.5 py-0.5 rounded bg-green-600/20 text-green-500 shrink-0">Up to date</span>
              )}
              {updateState === "update" && (
                <span className="text-xs px-1.5 py-0.5 rounded bg-yellow-600/20 text-yellow-500 shrink-0">Update available</span>
              )}
              {updateState === "ahead" && (
                <span className="text-xs px-1.5 py-0.5 rounded bg-mid-gray/20 text-text-tertiary shrink-0">Development build</span>
              )}
            </div>
            <div className="flex items-center gap-3">
              <span className="text-sm text-text-secondary w-36 shrink-0">License</span>
              <a
                href="https://github.com/wsva/fms-app/blob/main/LICENSE"
                target="_blank"
                rel="noopener noreferrer"
                className="text-sm font-mono text-accent hover:underline"
              >
                MIT
              </a>
            </div>
          </div>
        </section>
        </div>
      </div>
    </main>
  );
}
