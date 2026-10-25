"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
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
import type {
  AgentConfirmRequest,
  AgentHostList,
  AgentStatus,
} from "@/lib/agent/types";

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
  // The dock's ACP address. No longer editable here — the Connect card is
  // read-only and `agent_connect` falls back to the built-in default when the
  // stored value is empty — but they must stay in the payload, because
  // `settings_set_global` replaces the whole object and dropping a key would
  // reset it.
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
  /** "permissive" | "enforce" — see `mcp/policy.rs`. */
  mcp_policy: string;
  /** Whether `/mcp` and the generated manual are served at all. Chosen when the
   *  router is built, so saving this restarts the local web service. */
  mcp_enabled: boolean;
}

// The generated agent manual (`agent_manual_get` → `mcp/manual.rs::render_json`).
// Only the fields the Connect card renders are typed; the body carries much more
// (the full tool index, house rules, workflow templates) for `/agent.json`.
interface AgentManual {
  endpoints: {
    running: boolean;
    port: number;
    mcp: string;
    manual_markdown: string;
    note: string;
  };
  trust: {
    zone: string;
    policy: { mode: string; tiers: string[] };
  };
  tools: { count: number };
  connect: {
    pointer_sentence: string;
    pointer_when: string;
    inline_block: string;
    inline_when: string;
    never: string;
    snippets: { product: string; text: string }[];
  };
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

  // ---- Connect card (desktop only) ----
  // Everything the card shows is read back from the backend rather than
  // assembled here: the port is the one the web service actually bound, the tool
  // count comes from the live router, and the copy payloads are the same strings
  // `/agent.md` serves. One generator, so the card cannot disagree with the
  // manual an agent fetches.
  const [manual, setManual] = useState<AgentManual | null>(null);
  const [manualLoading, setManualLoading] = useState(false);
  const [dockConnected, setDockConnected] = useState(false);
  // The host records themselves (id, kind, endpoint, approval rule), so the
  // switch offers what the backend would actually dial rather than a free-text
  // address.
  const [dockHosts, setDockHosts] = useState<AgentHostList | null>(null);
  // A host the Rust trust rule marked `needs_confirmation`: shown inline until
  // the user answers it, because approving is what unblocks the connect.
  const [confirm, setConfirm] = useState<AgentConfirmRequest | null>(null);
  const [copied, setCopied] = useState<string | null>(null);

  // ---- Platform (mobile vs desktop) ----
  // `isMobileApp()` is false during prerendering (no navigator) and only turns
  // true inside the Android WebView, so reading it while rendering would make
  // the server HTML and the first client render disagree (hydration error).
  // Defer it to an effect, like Sidebar / DictationPage do. The desktop-only
  // "Connect an agent" card is hidden with it — and so is the command that
  // feeds it, `agent_manual_get`, which does not exist in the mobile build.
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
    setCurrentTheme(getStoredTheme());
  }, [fetchSettings]);

  // ---- Connect card data ----
  const loadAgentManual = useCallback(async () => {
    if (!isTauri() || mobile) return;
    setManualLoading(true);
    try {
      const [m, list] = await Promise.all([
        invoke<AgentManual>("agent_manual_get"),
        // The dock half of the status line: which host records exist, which one
        // is active, and whether a session is live right now.
        invoke<AgentHostList>("agent_host_list").catch(() => null),
      ]);
      setManual(m);
      setDockHosts(list);
      setDockConnected(!!list?.connected);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      logError(`settings: reading the agent manual failed — ${msg}`, "settings");
    } finally {
      setManualLoading(false);
    }
  }, [mobile]);

  useEffect(() => {
    loadAgentManual();
  }, [loadAgentManual]);

  // A confirm request can originate outside this card: an agent calling
  // `agent_host_connect` over MCP is refused, and the backend emits the ask so
  // the user has something to answer. Listening is what makes that refusal
  // actionable instead of a dead end — the caller retries after the answer.
  useEffect(() => {
    if (!isTauri() || mobile) return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listen<AgentConfirmRequest>("agent-confirm-request", (e) => {
      setConfirm(e.payload);
      setAgentTest({
        kind: "busy",
        text: `Waiting for your approval of ${e.payload.display_name}.`,
      });
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [mobile]);

  // Copy-only, by design: FmS never writes another product's config file (see
  // `manual.connect.never`). The key names the button so only it flips to
  // "Copied".
  async function copyPayload(key: string, text: string) {
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      setCopied(key);
      setTimeout(() => setCopied((c) => (c === key ? null : c)), 2000);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      logError(`settings: copying to the clipboard failed — ${msg}`, "settings");
      setCopied(null);
    }
  }

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

  // Probe the dock's ACP host and report the outcome inline. `agent_connect` is
  // idempotent, so this doubles as a status refresh when already connected. It no
  // longer saves pending settings first: the address is not editable here, so
  // there is nothing to persist and a "test" button that writes is a surprise.
  async function handleTestAgent() {
    if (!isTauri()) return;
    setAgentTest({ kind: "busy", text: "Connecting…" });
    try {
      const status = await invoke<{
        connected: boolean;
        session_id?: string | null;
        mcp_registered?: boolean;
      }>("agent_connect");
      setDockConnected(status.connected);
      if (status.connected) {
        setAgentTest({
          kind: "ok",
          text: `Connected (session ${status.session_id ?? "?"})${
            status.mcp_registered ? " · FmS tools registered" : ""
          }.`,
        });
      } else {
        setAgentTest({ kind: "err", text: "Not connected." });
      }
    } catch (e) {
      setAgentTest({ kind: "err", text: e instanceof Error ? e.message : String(e) });
    }
  }

  // The switch on the Connect card. Two commands, one decision: a host the Rust
  // trust rule marks `needs_confirmation` is asked about first (the inline
  // `confirm` card), and the approval it records is what unblocks the connect.
  // No address is accepted here — the records are the only dial targets.
  async function handleShowInDock(hostId: string) {
    if (!isTauri()) return;
    setAgentTest({ kind: "busy", text: hostId ? "Connecting…" : "Disconnecting…" });
    try {
      if (!hostId) {
        await invoke("agent_host_disconnect");
        setDockConnected(false);
        setAgentTest({ kind: "ok", text: "No agent hosted — FmS is on its own." });
        await loadAgentManual();
        return;
      }
      const host = dockHosts?.hosts.find((h) => h.id === hostId);
      if (host?.needs_confirmation && !host.approved) {
        const req = await invoke<{ state: string; host?: AgentConfirmRequest }>(
          "agent_confirm_request",
          { id: hostId }
        );
        if (req.state === "pending" && req.host) {
          setConfirm(req.host);
          setAgentTest({ kind: "busy", text: "Waiting for your approval…" });
          return;
        }
      }
      await connectHostInDock(hostId);
    } catch (e) {
      setAgentTest({ kind: "err", text: e instanceof Error ? e.message : String(e) });
    }
  }

  async function connectHostInDock(hostId: string) {
    const status = await invoke<AgentStatus>("agent_host_connect", { id: hostId });
    setDockConnected(status.connected);
    setAgentTest({
      kind: status.connected ? "ok" : "err",
      text: status.connected
        ? `Hosting ${status.display_name ?? hostId} over ${status.transport ?? "?"} — session ${
            status.session_id ?? "?"
          }${status.mcp_transport ? `, FmS tools over ${status.mcp_transport}` : ""}.`
        : "Not connected.",
    });
    await loadAgentManual();
  }

  async function answerConfirm(approve: boolean) {
    if (!confirm) return;
    const host = confirm;
    setConfirm(null);
    setAgentTest({ kind: "busy", text: approve ? "Connecting…" : "Cancelling…" });
    try {
      await invoke("agent_confirm_respond", { hostId: host.host_id, approve });
      if (!approve) {
        setAgentTest({ kind: "err", text: `${host.display_name} stays unhosted.` });
        await loadAgentManual();
        return;
      }
      await connectHostInDock(host.host_id);
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

          {/* ── Connect an agent — desktop only ────────────────────── */}
          {!mobile && (
            <div className="mt-6 pt-6 border-t border-border-light">
              <div className="flex items-start justify-between gap-3 mb-1">
                <h3 className="text-lg font-semibold">Connect an agent</h3>
                <button
                  type="button"
                  className={btnGhost}
                  onClick={loadAgentManual}
                  disabled={manualLoading}
                >
                  {manualLoading ? "Reading…" : "Refresh"}
                </button>
              </div>
              <p className="text-text-secondary text-sm mb-4">
                FmS runs a Model Context Protocol server on the loopback interface.
                Copy one line below into any MCP-capable agent and it configures
                itself: the manual it then reads is generated from this build
                ({manual ? `${manual.tools.count} tools` : "…"}), so it cannot drift
                from the tools that actually exist. The endpoint is not editable —
                it follows the port the service really bound.
              </p>

              {/* Status half: everything read back, nothing inferred. */}
              <div className="mb-5 rounded-md border border-border-light p-3 text-sm space-y-1">
                <div className="flex flex-wrap items-center gap-2">
                  <span
                    className={manual?.endpoints.running ? "text-green-500" : "text-error-text"}
                    aria-hidden
                  >
                    ●
                  </span>
                  <span className="font-medium">
                    {manual
                      ? manual.endpoints.running
                        ? `Web service running on port ${manual.endpoints.port}`
                        : "Web service stopped"
                      : "Web service status unknown"}
                  </span>
                </div>
                <p className="text-text-secondary">
                  MCP endpoint: <code>{manual?.endpoints.mcp ?? "—"}</code>
                </p>
                <p className="text-text-secondary">
                  Manual: <code>{manual?.endpoints.manual_markdown ?? "—"}</code>
                </p>
                <p className="text-text-secondary">
                  This page sits in the <strong>{manual?.trust.zone ?? "unknown"}</strong>{" "}
                  trust zone
                  {manual?.trust.zone === "loopback"
                    ? " — a local agent needs no credentials at all."
                    : "."}
                </p>
                <p className="text-text-secondary">
                  MCP surface:{" "}
                  <strong>
                    {globalSettings?.mcp_enabled ?? true ? "exposed" : "off"}
                  </strong>
                  {(globalSettings?.mcp_enabled ?? true)
                    ? " — the endpoint below answers."
                    : " — /mcp and the manual answer 404 until you switch it back on."}
                </p>
                <p className="text-text-secondary">
                  Agent dock:{" "}
                  {dockConnected ? (
                    <>
                      hosting{" "}
                      <strong>
                        {dockHosts?.session?.display_name ?? dockHosts?.active_id ?? "a host"}
                      </strong>{" "}
                      via <strong>{dockHosts?.session?.transport ?? "?"}</strong>
                      {dockHosts?.session?.mcp_transport ? (
                        <>
                          , FmS tools over <strong>{dockHosts.session.mcp_transport}</strong>
                        </>
                      ) : null}
                      .
                    </>
                  ) : (
                    "no host hosted."
                  )}
                </p>
                {manual && !manual.endpoints.running && (
                  <p className="text-error-text">{manual.endpoints.note}</p>
                )}
              </div>

              {/* The switch: which record the dock shows. Two commands behind
                  one select — `agent_host_disconnect` for "FmS only",
                  `agent_host_connect` for a record — because a free-text address
                  is exactly what the records replaced. */}
              <div className="mb-5 max-w-md">
                <label className="block font-medium mb-1" htmlFor="dock-host">
                  Shown in
                </label>
                <p className="text-text-secondary text-sm mb-2">
                  The dock hosts one agent at a time: either a server you run
                  yourself (<code>ws</code>) or a program FmS starts and stops
                  (<code>stdio</code>). Hosts come from the app&apos;s records — an
                  agent adds them with <code>agent_host_register</code>. Whether
                  dialling one needs your approval is decided in the backend, never
                  here.
                </p>
                <select
                  id="dock-host"
                  className="w-full px-3 py-2 border border-border-light rounded-md bg-bg-input text-text-primary"
                  value={dockConnected ? dockHosts?.session?.host_id ?? "" : ""}
                  onChange={(e) => void handleShowInDock(e.target.value)}
                  disabled={agentTest?.kind === "busy"}
                >
                  <option value="">FmS only — no agent hosted</option>
                  {(dockHosts?.hosts ?? []).map((h) => (
                    <option key={h.id} value={h.id}>
                      {h.display_name} ({h.kind})
                      {h.needs_confirmation && !h.approved ? " — needs your approval" : ""}
                    </option>
                  ))}
                </select>
                {(dockHosts?.hosts.length ?? 0) === 0 && (
                  <p className="text-text-secondary text-sm mt-2">
                    No host records yet. Until one exists the dock has nothing to
                    dial — use the copy-only lines above (Mode 1), or have an agent
                    register one.
                  </p>
                )}
              </div>

              {/* The confirm dialog, inline. Same content the `agent-confirm-request`
                  event carries, shown here because the Connect card is where the
                  user acts on it. */}
              {confirm && (
                <div className="mb-5 rounded-md border border-accent-bg/40 bg-accent-bg/10 p-3 text-sm space-y-2">
                  <p className="font-medium">Host {confirm.display_name}?</p>
                  <p className="text-text-secondary">
                    {confirm.reason}: <code>{confirm.endpoint}</code>
                  </p>
                  <div className="flex flex-wrap gap-2">
                    <button
                      type="button"
                      className={btnGhost}
                      onClick={() => void answerConfirm(true)}
                    >
                      Approve and connect
                    </button>
                    <button
                      type="button"
                      className={btnGhost}
                      onClick={() => void answerConfirm(false)}
                    >
                      Cancel
                    </button>
                  </div>
                </div>
              )}

              {/* Payload A — the pointer sentence. One line, and the manual it
                  points at teaches the rest. */}
              <div className="mb-4">
                <label className="block font-medium mb-1">
                  1 · Paste this sentence (preferred)
                </label>
                <p className="text-text-secondary text-sm mb-2">
                  {manual?.connect.pointer_when ??
                    "For any agent that can fetch a URL."}
                </p>
                <div className="flex items-start gap-2">
                  <code className="flex-1 px-3 py-2 border border-border-light rounded-md bg-bg-input text-text-primary text-sm break-all">
                    {manual?.connect.pointer_sentence ?? "—"}
                  </code>
                  <button
                    type="button"
                    className={btnGhost}
                    onClick={() =>
                      copyPayload("pointer", manual?.connect.pointer_sentence ?? "")
                    }
                    disabled={!manual}
                  >
                    {copied === "pointer" ? "Copied" : "Copy"}
                  </button>
                </div>
              </div>

              {/* Payload B — the inline block. "Can attach MCP" is not the same
                  as "can fetch a URL", and without this the whole second class
                  of agents fails silently. */}
              <div className="mb-4">
                <label className="block font-medium mb-1">
                  2 · Or paste this block
                </label>
                <p className="text-text-secondary text-sm mb-2">
                  {manual?.connect.inline_when ??
                    "For an agent that can attach an MCP server but cannot fetch a URL."}
                </p>
                <div className="flex items-start gap-2">
                  <pre className="flex-1 px-3 py-2 border border-border-light rounded-md bg-bg-input text-text-primary text-xs whitespace-pre-wrap break-all">
                    {manual?.connect.inline_block ?? "—"}
                  </pre>
                  <button
                    type="button"
                    className={btnGhost}
                    onClick={() =>
                      copyPayload("inline", manual?.connect.inline_block ?? "")
                    }
                    disabled={!manual}
                  >
                    {copied === "inline" ? "Copied" : "Copy"}
                  </button>
                </div>
              </div>

              {/* Per-product config lines, still copy-only. */}
              {(manual?.connect.snippets.length ?? 0) > 0 && (
                <div className="mb-4">
                  <label className="block font-medium mb-1">Config lines</label>
                  <p className="text-text-secondary text-sm mb-2">
                    {manual?.connect.never}
                  </p>
                  <ul className="space-y-2">
                    {manual?.connect.snippets.map((s) => (
                      <li key={s.product} className="flex items-start gap-2">
                        <span className="w-24 shrink-0 pt-2 text-sm text-text-secondary">
                          {s.product}
                        </span>
                        <pre className="flex-1 px-3 py-2 border border-border-light rounded-md bg-bg-input text-text-primary text-xs whitespace-pre-wrap break-all">
                          {s.text}
                        </pre>
                        <button
                          type="button"
                          className={btnGhost}
                          onClick={() => copyPayload(s.product, s.text)}
                        >
                          {copied === s.product ? "Copied" : "Copy"}
                        </button>
                      </li>
                    ))}
                  </ul>
                </div>
              )}

              {/* The two things here that are still settings. The policy mode
                  belongs on this page precisely because an agent under
                  "enforce" is refused the ability to change it itself. */}
              <div className="mb-4 max-w-md">
                <label className="block font-medium mb-1" htmlFor="mcp-policy">
                  Agent policy mode
                </label>
                <p className="text-text-secondary text-sm mb-2">
                  <em>permissive</em> classifies and journals every MCP call but
                  refuses nothing. <em>enforce</em> additionally refuses the
                  destructive and human-handoff tiers, and refuses an
                  agent&apos;s attempt to change this setting. Live mode:{" "}
                  <strong>{manual?.trust.policy.mode ?? "unknown"}</strong>.
                  Takes effect when you save.
                </p>
                <select
                  id="mcp-policy"
                  className="w-full px-3 py-2 border border-border-light rounded-md bg-bg-input text-text-primary"
                  value={globalSettings?.mcp_policy ?? "permissive"}
                  onChange={(e) => updateGlobalField("mcp_policy", e.target.value)}
                >
                  <option value="permissive">permissive — journal only</option>
                  <option value="enforce">enforce — refuse destructive tools</option>
                </select>
              </div>

              {/* P3: the MCP surface is a runtime switch, not a build feature.
                  It is baked into the router, so saving this restarts the local
                  web service — said out loud because an attached agent loses its
                  tools for a moment. */}
              <label className="flex items-start gap-2 mb-4 cursor-pointer max-w-2xl">
                <input
                  type="checkbox"
                  className="w-4 h-4 mt-0.5"
                  checked={globalSettings?.mcp_enabled ?? true}
                  onChange={(e) => {
                    setGlobalSettings((prev) =>
                      prev ? { ...prev, mcp_enabled: e.target.checked } : prev
                    );
                    setSavedGlobal(false);
                  }}
                />
                <span>
                  <span className="font-medium">Expose the MCP server (agents)</span>
                  <span className="block text-text-secondary text-sm">
                    Serves <code>/mcp</code> and the generated <code>/agent.md</code>{" "}
                    manual. Saving restarts the local web service, because the
                    routes are chosen when the router is built: an agent attached
                    right now loses its tools until the restart returns, and
                    <code>/api/v1/status</code> keeps answering with{" "}
                    <code>mcp_enabled: false</code> so a peer can tell why{" "}
                    <code>/mcp</code> 404s. Under <em>enforce</em> an agent cannot
                    flip this itself.
                  </span>
                </span>
              </label>

              <label className="flex items-start gap-2 mb-4 cursor-pointer max-w-2xl">
                <input
                  type="checkbox"
                  className="w-4 h-4 mt-0.5"
                  checked={globalSettings?.goose_acp_enabled ?? false}
                  onChange={(e) => {
                    setGlobalSettings((prev) =>
                      prev ? { ...prev, goose_acp_enabled: e.target.checked } : prev
                    );
                    setSavedGlobal(false);
                  }}
                />
                <span>
                  <span className="font-medium">Show the agent dock</span>
                  <span className="block text-text-secondary text-sm">
                    The dock is the second way in: instead of attaching to FmS, it
                    attaches FmS to an agent host — a <code>goose serve</code> you
                    started yourself, or a program FmS spawns
                    {globalSettings?.goose_acp_url
                      ? <> (at <code>{globalSettings.goose_acp_url}</code>)</>
                      : null}
                    . Slower to set up, but it is the only window where FmS can
                    show an agent&apos;s reasoning, tool calls and permission
                    requests.
                  </span>
                </span>
              </label>

              <div className="flex items-center gap-3">
                <button
                  type="button"
                  className={btnGhost}
                  onClick={handleTestAgent}
                  disabled={agentTest?.kind === "busy"}
                >
                  {agentTest?.kind === "busy" ? "Connecting…" : "Test dock connection"}
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
          </div>
        </section>
        </div>
      </div>
    </main>
  );
}
