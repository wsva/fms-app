"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { QRCodeSVG } from "qrcode.react";
import { isMobileApp } from "@/lib/platform";

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface GlobalSettings {
  ollama_url: string;
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
}

// One-time pairing code issued by `pairing_create_otp` (120 s TTL, single use).
interface OtpInfo {
  otp: string;
  expires_at: number; // unix seconds
}

interface WebServiceStatus {
  running: boolean;
  port: number;
  local_url: string | null;
  lan_url: string | null;
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

  // ---- Pairing (desktop only) ----
  // `isMobileApp()` is false during prerendering (no navigator) and only turns
  // true inside the Android WebView, so reading it while rendering would make
  // the server HTML and the first client render disagree (hydration error).
  // Defer it to an effect, like Sidebar / DictationPage do.
  const [mobile, setMobile] = useState(false);
  const [devices, setDevices] = useState<PairedDevice[]>([]);
  const [otp, setOtp] = useState<OtpInfo | null>(null);
  const [pairUrl, setPairUrl] = useState("");
  const [secondsLeft, setSecondsLeft] = useState(0);
  const [otpBusy, setOtpBusy] = useState(false);
  const [pairError, setPairError] = useState("");

  useEffect(() => {
    setMobile(isMobileApp());
  }, []);

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

  // Live countdown of the OTP TTL; the code is dropped when it expires.
  useEffect(() => {
    if (!otp) return;
    const tick = () => {
      const left = Math.max(0, otp.expires_at - Math.floor(Date.now() / 1000));
      setSecondsLeft(left);
      if (left === 0) {
        setOtp(null);
        setPairUrl("");
      }
    };
    tick();
    const id = setInterval(tick, 1000);
    return () => clearInterval(id);
  }, [otp]);

  async function handleNewOtp() {
    setOtpBusy(true);
    setPairError("");
    try {
      const info = await invoke<OtpInfo>("pairing_create_otp");
      setOtp(info);
      // The QR carries the LAN URL so the phone knows where to connect.
      const status = await invoke<WebServiceStatus>("web_service_get_status");
      if (status.lan_url) {
        setPairUrl(`fms-app://pair?u=${encodeURIComponent(status.lan_url)}&o=${info.otp}`);
      } else {
        setPairUrl("");
        setPairError("Web service is not running — start it to show the QR code (the code below still works).");
      }
    } catch (e) {
      setPairError(`Failed to create pairing code: ${String(e)}`);
    } finally {
      setOtpBusy(false);
    }
  }

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

        {/* ── Device pairing section (PC only) ──────────────────── */}
        {!mobile && (
          <section className="mb-8">
            <div className="flex items-center justify-between mb-2">
              <h2 className="text-[1.3em] font-semibold">Device Pairing</h2>
              <button className={btnPrimary} onClick={handleNewOtp} disabled={otpBusy}>
                {otpBusy ? "Preparing…" : otp ? "New Code" : "Pair New Device"}
              </button>
            </div>
            <p className="text-text-secondary text-sm mb-4">
              Devices on the LAN/WLAN must pair before they can read or write data — like
              Bluetooth headphones. Requests from this computer (localhost) and from Tailscale
              are always allowed, so local automations need no pairing.
            </p>

            {pairError && (
              <p className="text-sm text-red-600 mb-3">{pairError}</p>
            )}

            {otp && (
              <div className="flex flex-wrap items-start gap-6 p-4 mb-4 rounded-lg border border-border-default bg-bg-card">
                {pairUrl ? (
                  <div className="flex flex-col items-center gap-2">
                    <div className="p-3 bg-white rounded-md">
                      <QRCodeSVG value={pairUrl} size={180} />
                    </div>
                    <span className="text-xs text-text-secondary">
                      Scan with the phone's camera
                    </span>
                  </div>
                ) : (
                  <div className="text-sm text-text-secondary">No QR available.</div>
                )}
                <div className="flex flex-col gap-1">
                  <span className="text-sm font-medium">Or enter this code on the phone</span>
                  <span className="font-mono text-[1.4em] tracking-wider select-all break-all">
                    {otp.otp}
                  </span>
                  <span className="text-sm text-text-secondary">
                    Expires in {secondsLeft}s · single use
                  </span>
                </div>
              </div>
            )}

            <table className="w-full text-sm border-collapse">
              <thead>
                <tr className="text-left text-text-secondary border-b border-border-light">
                  <th className="py-2 pr-4 font-medium">Device</th>
                  <th className="py-2 pr-4 font-medium">Code</th>
                  <th className="py-2 pr-4 font-medium">Status</th>
                  <th className="py-2 pr-4 font-medium">Last seen</th>
                  <th className="py-2 font-medium"></th>
                </tr>
              </thead>
              <tbody>
                {devices.length === 0 && (
                  <tr>
                    <td colSpan={5} className="py-3 text-text-tertiary">
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
      </main>
    </>
  );
}
