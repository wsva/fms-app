"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import { Search, ChevronDown, Link2, KeyRound, Unplug, RefreshCw, Loader2, Users } from "lucide-react";
import { logInfo, logError } from "@/lib/logger";

// ---------------------------------------------------------------------------
// Types (mirror the backend commands this page drives)
// ---------------------------------------------------------------------------

// Subset of the global settings this page needs (mirrors settings.rs). Kept
// whole so `settings_set_global` round-trips without dropping unrelated fields.
interface GlobalSettings {
  ollama_url: string;
  pc_url: string;
  pc_token: string;
  device_id: string;
  device_seed: string;
  selected_model: string;
  model_dir: string;
  model_unload_timeout: unknown;
  onboarding_completed: boolean;
}

// Workspace-scoped role + cluster identity (docs/my_sync_design.md §3.1). We
// only read/display it here; the role is written via `settings_set_role`.
interface WorkspaceSettings {
  role: string;
  cluster_id: string;
}

interface HubStatus {
  address: string;
  reachable: boolean;
  role?: string | null;
  cluster_id?: string | null;
  protocol_version?: number | null;
  dataset_count?: number | null;
  error?: string | null;
}

interface PairedDevice {
  device_id: string;
  name: string;
  status: string;
  created_at: string;
  last_seen_at?: string | null;
  bound_user_id?: string | null;
}

interface SyncStatusDetail {
  role: string;
  cluster_id: string;
  protocol_version: number;
  device_id: string;
  hub: HubStatus;
  datasets: unknown[];
  queued_count: number;
  chat_pending: number;
  devices: PairedDevice[];
}

// A discovered PC returned by the `pc_discover` command.
interface PcCandidate {
  url: string;
  source: string; // "lan" | "tailscale" | "saved"
  name: string;
}

// Result of the `pc_pair_start` command.
interface PairResult {
  state: "approved" | "pending" | "denied" | "none";
  fingerprint?: string;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/**
 * Central place to manage the cluster identity and the devices you trust:
 *   1. My role — read-only role/cluster status everywhere; an editable
 *      hub/follower selector on desktop (a phone is always a follower).
 *   2. Hub connection — the follower-side flow: pick a source PC, scan,
 *      connect, pair, and forget/re-pair.
 *   3. Devices paired to me — the hub-side registry of trusted devices.
 *
 * Moved out of Settings (Sync Role, Device Pairing) and Datasets Sync (status
 * overview, scan/connect/pair). No new backend commands — every invoke already
 * exists for those pages.
 */
export default function DevicesHubPage() {
  const [mounted, setMounted] = useState(false);
  // `isMobileApp()` is false during prerender and only turns true inside the
  // Android WebView; reading it while rendering would break hydration, so defer
  // it to an effect (same pattern as SettingsPage / Sidebar).
  const [mobile, setMobile] = useState(false);

  const [global, setGlobal] = useState<GlobalSettings | null>(null);
  const [workspaceSettings, setWorkspaceSettings] = useState<WorkspaceSettings | null>(null);
  const [status, setStatus] = useState<SyncStatusDetail | null>(null);

  // Role designation control (desktop only).
  const [savingRole, setSavingRole] = useState<"" | "hub" | "follower">("");
  const [roleMsg, setRoleMsg] = useState("");

  // PC connection / discovery state.
  const [candidates, setCandidates] = useState<PcCandidate[]>([]);
  const [discovering, setDiscovering] = useState(false);
  const [showManual, setShowManual] = useState(false);
  const [manualUrl, setManualUrl] = useState("");

  // Pairing state (Bluetooth-style device identity, see sync.rs / pairing.rs).
  const [pairing, setPairing] = useState(false);
  const [pairFingerprint, setPairFingerprint] = useState("");
  const [pairMessage, setPairMessage] = useState("");
  const [unpaired, setUnpaired] = useState(false);

  // Hub-side paired-device registry (desktop only).
  const [devices, setDevices] = useState<PairedDevice[]>([]);
  const [devicesLoading, setDevicesLoading] = useState(false);
  const [pairError, setPairError] = useState("");

  const [message, setMessage] = useState("");

  const pcUrl = (global?.pc_url ?? "").trim();
  const deviceId = global?.device_id ?? "";
  // Authoritative role at runtime: prefer the aggregate status, fall back to the
  // workspace setting (status is null before it loads on a hub, etc.).
  const role = status?.role || workspaceSettings?.role || "follower";
  const isHub = role === "hub";

  // ---- Loaders ----------------------------------------------------------

  const loadSettings = useCallback(async () => {
    if (!isTauri()) return null;
    try {
      const g = await invoke<GlobalSettings>("settings_get_global");
      setGlobal(g);
      setManualUrl(g.pc_url ?? "");
      return g;
    } catch (e) {
      logError(`Failed to load settings: ${String(e)}`, "devices");
      return null;
    }
  }, []);

  // Workspace role + cluster id. Read cross-platform so the phone can render a
  // read-only role line; only the desktop writes it.
  const loadWorkspace = useCallback(async () => {
    if (!isTauri()) return;
    try {
      setWorkspaceSettings(await invoke<WorkspaceSettings>("settings_get_workspace"));
    } catch (e) {
      logError(`Failed to load workspace settings: ${String(e)}`, "devices");
    }
  }, []);

  // Aggregate status (§3.5) — role, cluster, hub reachability, paired devices.
  const loadStatus = useCallback(async () => {
    if (!isTauri()) return;
    try {
      setStatus(await invoke<SyncStatusDetail>("sync_status"));
    } catch (e) {
      logError(`Failed to load sync status: ${String(e)}`, "devices");
    }
  }, []);

  // Hub-side device registry. `pairing_list` is a desktop-only command, so this
  // never runs (and the section never renders) on the phone.
  const loadDevices = useCallback(async () => {
    if (!isTauri() || isMobileApp()) return;
    setDevicesLoading(true);
    try {
      setDevices(await invoke<PairedDevice[]>("pairing_list"));
    } catch (e) {
      logError(`Failed to load paired devices: ${String(e)}`, "devices");
    } finally {
      setDevicesLoading(false);
    }
  }, []);

  const discover = useCallback(async () => {
    if (!isTauri()) return;
    setDiscovering(true);
    try {
      const res = await invoke<PcCandidate[]>("pc_discover", { timeoutMs: 2500 });
      setCandidates(res);
      if (res.length === 0) logInfo("No FmS PC found on this network.", "devices");
    } catch (e) {
      logError(`Discovery failed: ${String(e)}`, "devices");
      setCandidates([]);
    } finally {
      setDiscovering(false);
    }
  }, []);

  // ---- Lifecycle --------------------------------------------------------

  useEffect(() => {
    setMounted(true);
    setMobile(isMobileApp());
    (async () => {
      const g = await loadSettings();
      loadWorkspace();
      loadStatus();
      loadDevices();
      // Auto-discover on open when no hub is connected yet.
      if (!g || !(g.pc_url ?? "").trim()) discover();
    })();
  }, [loadSettings, loadWorkspace, loadStatus, loadDevices, discover]);

  // ---- Role designation (desktop only) --------------------------------

  // Promote/demote this workspace. The backend generates the `cluster_id` once
  // on the first promotion to hub; re-read so the fresh id shows up.
  async function handleSetRole(next: "hub" | "follower") {
    if (!isTauri() || savingRole) return;
    setSavingRole(next);
    setRoleMsg("");
    try {
      await invoke("settings_set_role", { role: next });
      const w = await invoke<WorkspaceSettings>("settings_get_workspace");
      setWorkspaceSettings(w);
      setStatus(await invoke<SyncStatusDetail>("sync_status").catch(() => status));
      setRoleMsg(
        next === "hub" ? "This workspace is now the sync hub." : "This workspace is now a follower."
      );
      setTimeout(() => setRoleMsg(""), 4000);
    } catch (e) {
      setRoleMsg(`Failed to set role: ${String(e)}`);
    } finally {
      setSavingRole("");
    }
  }

  // ---- Connection + pairing (follower side) ---------------------------

  // Persist pc_url to global settings so the sync commands (which read settings
  // on the backend) can resolve the target hub.
  const savePc = useCallback(
    async (url: string) => {
      if (!global) return;
      const next: GlobalSettings = { ...global, pc_url: url };
      setGlobal(next);
      setManualUrl(url);
      try {
        await invoke("settings_set_global", { global: next });
        setMessage(url ? `Connected to ${url}.` : "Disconnected.");
        loadStatus();
      } catch (e) {
        logError(`Failed to save PC url: ${String(e)}`, "devices");
        setMessage(`Failed to save PC address: ${String(e)}`);
      }
    },
    [global, loadStatus],
  );

  // Ask the hub to pair this device: the owner gets a confirm dialog and the
  // command polls until they answer.
  const pair = useCallback(
    async (url: string) => {
      if (!isTauri() || pairing) return;
      setPairing(true);
      setPairMessage("");
      setPairFingerprint("");
      try {
        const res = await invoke<PairResult>("pc_pair_start", { pcUrl: url });
        if (res.fingerprint) setPairFingerprint(res.fingerprint);
        if (res.state === "approved") {
          setPairMessage("Paired with the PC.");
          setUnpaired(false);
          loadSettings();
          loadStatus();
        } else if (res.state === "denied") {
          setPairMessage("The PC owner denied this pairing. Ask them to remove the block, or start a new pairing request.");
          setUnpaired(true);
        } else {
          setPairMessage("Pairing did not complete.");
        }
      } catch (e) {
        setPairMessage(`Pairing failed: ${String(e)}`);
      } finally {
        setPairing(false);
      }
    },
    [pairing, loadSettings, loadStatus],
  );

  // Recovery path after a denial: regenerate the device identity so the next
  // request pops a fresh dialog.
  const resetIdentity = useCallback(async () => {
    if (!isTauri()) return;
    try {
      await invoke<string>("pc_pair_reset_identity");
      setPairMessage("Generated a new device identity. Pair again.");
      loadSettings();
    } catch (e) {
      setPairMessage(`Failed to reset device identity: ${String(e)}`);
    }
  }, [loadSettings]);

  // "Forget hub / re-pair" (§7): clear the TOFU cluster binding + hub address so
  // this device can attach to a different hub. Keeps local data untouched.
  async function handleForgetHub() {
    if (!confirm("Forget the current hub? This clears the cluster binding and hub address so you can re-pair. Local data is kept.")) {
      return;
    }
    setMessage("");
    try {
      await invoke("settings_forget_hub");
      const g = await loadSettings();
      setManualUrl(g?.pc_url ?? "");
      setStatus(await invoke<SyncStatusDetail>("sync_status").catch(() => null));
      discover();
      setMessage("Forgot the hub. Scan or enter an address to re-pair.");
    } catch (e) {
      setMessage(`Failed to forget hub: ${String(e)}`);
    }
  }

  // ---- Device registry (hub side, desktop only) -----------------------

  async function handleRevoke(deviceIdArg: string) {
    try {
      await invoke("pairing_revoke", { deviceId: deviceIdArg });
      loadDevices();
    } catch (e) {
      setPairError(`Failed to revoke device: ${String(e)}`);
    }
  }

  async function handleRemoveDenied(deviceIdArg: string) {
    try {
      await invoke("pairing_remove_denied", { deviceId: deviceIdArg });
      loadDevices();
    } catch (e) {
      setPairError(`Failed to remove device: ${String(e)}`);
    }
  }

  const clusterId = status?.cluster_id || workspaceSettings?.cluster_id || "";

  return (
    <div className="flex flex-col w-full h-full min-h-0 overflow-y-auto p-4 gap-4">
      <h1 className="text-[1.3em] font-bold shrink-0">Devices &amp; Hub</h1>

      {/* ── 1. My role ───────────────────────────────────────────── */}
      <section className="shrink-0 flex flex-col gap-3 p-4 rounded-lg border border-border-default bg-bg-card">
        <h2 className="text-base font-semibold">My role</h2>

        {/* Read-only status line — every platform, so a phone still sees where
            it stands even though it cannot change its role. */}
        {mounted && status && (
          <div className="flex items-center gap-2 flex-wrap text-sm">
            <span className="px-2 py-0.5 rounded-full bg-bg-hover text-text-secondary capitalize font-medium">
              {role}
            </span>
            <span className="text-text-secondary">
              cluster{" "}
              <span className="font-mono" title={clusterId || undefined}>
                {clusterId ? clusterId.slice(0, 8) : "unbound"}
              </span>
            </span>
            <span className="text-xs text-text-tertiary ml-auto">protocol v{status.protocol_version}</span>
          </div>
        )}

        {mobile ? (
          <p className="text-sm text-text-tertiary">
            This device is always a <span className="font-medium">follower</span> — it syncs from
            its hub and can't serve datasets to others.
          </p>
        ) : (
          mounted && (
            <div className="flex flex-col gap-3">
              <p className="text-text-secondary text-sm">
                Choose which machine holds the authoritative copy. The <strong>hub</strong> is the
                source of truth that followers sync against; a <strong>follower</strong> pulls from
                the hub and pushes edits back. Promoting to hub issues a cluster id once.
              </p>
              <div className="flex items-center gap-3 flex-wrap">
                <label className="text-sm font-medium shrink-0">Sync Role</label>
                <select
                  className="px-3 py-2 border border-border-light rounded-md bg-bg-input text-text-primary max-w-xs w-full"
                  value={workspaceSettings?.role && workspaceSettings.role !== "" ? workspaceSettings.role : "follower"}
                  disabled={!!savingRole || !workspaceSettings}
                  onChange={(e) => handleSetRole(e.target.value === "hub" ? "hub" : "follower")}
                >
                  <option value="follower">Follower (syncs from the hub)</option>
                  <option value="hub">Hub (authoritative copy)</option>
                </select>
                {savingRole && <span className="text-sm text-text-secondary">Applying…</span>}
              </div>
              {roleMsg && <p className="text-sm text-green-500">{roleMsg}</p>}
            </div>
          )
        )}
      </section>

      {/* ── 2. Hub connection (follower side) ────────────────────── */}
      {!isHub && (
        <section className="shrink-0 flex flex-col gap-3 p-4 rounded-lg border border-border-default bg-bg-card">
          <h2 className="text-base font-semibold">Hub connection</h2>

          {/* Follower→upstream-hub link: reachability, pending work, error. */}
          {status && (
            <div className="flex flex-col gap-1.5 text-sm">
              <div className="flex items-center gap-2 flex-wrap text-xs">
                <span
                  className={`inline-block w-2 h-2 rounded-full ${
                    status.hub.reachable ? "bg-emerald-500" : "bg-red-500"
                  }`}
                />
                <span className="text-text-secondary">
                  hub{" "}
                  <span className="font-mono">{status.hub.address || "not configured"}</span>
                  {" "}
                  {status.hub.reachable
                    ? `· reachable${status.hub.dataset_count != null ? ` · ${status.hub.dataset_count} datasets` : ""}`
                    : "· unreachable"}
                </span>
                {status.queued_count > 0 && (
                  <span className="text-amber-600 dark:text-amber-400">
                    {status.queued_count} change(s) queued
                  </span>
                )}
                {status.chat_pending > 0 && (
                  <span className="text-amber-600 dark:text-amber-400">
                    {status.chat_pending} chat message(s) pending
                  </span>
                )}
              </div>
              {status.hub.error && (
                <div className="text-xs text-red-600 dark:text-red-400">{status.hub.error}</div>
              )}
            </div>
          )}

          {/* Source PC selector + scan + manual connect. */}
          <div className="flex flex-col gap-2 pt-1 border-t border-border-light">
            <div className="flex items-center gap-2 flex-wrap">
              <span className="text-sm font-medium shrink-0">Source PC</span>
              <select
                className="flex-1 min-w-0 px-2 py-1.5 text-sm rounded-md border border-border-light bg-bg-input text-text-primary"
                value={pcUrl}
                onChange={(e) => savePc(e.target.value)}
                disabled={discovering && !pcUrl}
              >
                <option value="">{pcUrl ? "Select a PC…" : "Not connected"}</option>
                {candidates.map((c) => (
                  <option key={c.url} value={c.url}>
                    {c.name} · {c.source}
                  </option>
                ))}
                {pcUrl && !candidates.some((c) => c.url === pcUrl) && (
                  <option value={pcUrl}>{pcUrl} · saved</option>
                )}
              </select>
              <button
                className="inline-flex items-center gap-1 px-3 py-1.5 text-sm rounded-md bg-bg-body border border-border-light hover:bg-bg-hover disabled:opacity-50 shrink-0"
                onClick={discover}
                disabled={discovering}
                title="Scan for nearby FmS machines (requires the web service enabled there)"
              >
                <Search size={14} className={discovering ? "animate-spin" : undefined} />
                {discovering ? "Scanning…" : "Scan"}
              </button>
              <button
                className="inline-flex items-center gap-1 px-3 py-1.5 text-sm rounded-md bg-bg-body border border-border-light hover:bg-bg-hover shrink-0"
                onClick={() => setShowManual((s) => !s)}
                title="Enter the source PC address manually"
              >
                <ChevronDown size={14} className={showManual ? "rotate-180 transition-transform" : "transition-transform"} />
                Manual
              </button>
            </div>

            {showManual && (
              <div className="flex items-center gap-2 flex-wrap pt-1">
                <input
                  type="text"
                  className="flex-1 min-w-[12rem] px-3 py-1.5 text-sm border border-border-light rounded-md bg-bg-input text-text-primary"
                  value={manualUrl}
                  onChange={(e) => setManualUrl(e.target.value)}
                  placeholder="http://192.168.1.20:35711"
                />
                <button
                  className="inline-flex items-center px-3 py-1.5 text-sm rounded-md bg-accent-bg text-white hover:bg-accent-bg-hover disabled:opacity-50 shrink-0"
                  onClick={() => savePc(manualUrl.trim())}
                  disabled={!manualUrl.trim()}
                >
                  Connect
                </button>
              </div>
            )}

            {/* Pairing row: the device identity is generated automatically; the
                only user gesture is asking the hub owner to approve this device. */}
            <div className="flex items-center gap-2 flex-wrap pt-2 border-t border-border-light">
              <span className="text-xs text-text-tertiary flex items-center gap-1 shrink-0">
                <KeyRound size={13} />
                {deviceId ? `code ${deviceId.slice(0, 6)}` : "new device"}
              </span>
              <button
                className="inline-flex items-center gap-1 px-3 py-1.5 text-sm rounded-md bg-accent-bg text-white hover:bg-accent-bg-hover disabled:opacity-50 shrink-0"
                onClick={() => pair(pcUrl)}
                disabled={pairing || !pcUrl}
                title="Ask the PC to pair this device"
              >
                <Link2 size={14} className={pairing ? "animate-spin" : undefined} />
                {pairing ? "Pairing…" : "Pair"}
              </button>
              {(unpaired || status?.hub.error?.includes("not paired")) && (
                <button
                  className="ml-auto text-xs text-text-secondary underline disabled:opacity-50"
                  onClick={resetIdentity}
                  disabled={pairing}
                  title="Regenerate this device's identity (use after the PC denied the pairing)"
                >
                  New identity
                </button>
              )}
              {clusterId && (
                <button
                  className="inline-flex items-center gap-1 text-xs text-text-secondary hover:text-text-primary underline"
                  onClick={handleForgetHub}
                  title="Clear the cluster binding + hub address to attach to a different hub"
                >
                  <Unplug size={13} />
                  Forget hub / re-pair
                </button>
              )}
            </div>

            {pairing && (
              <div className="text-sm text-text-secondary">
                Waiting for the PC owner to confirm. Compare the code{" "}
                <span className="font-mono font-semibold">
                  {pairFingerprint || deviceId.slice(0, 6) || "------"}
                </span>{" "}
                with the one on the PC.
              </div>
            )}
            {pairMessage && (
              <div className="px-3 py-2 text-sm rounded-md bg-bg-hover text-text-primary">{pairMessage}</div>
            )}
            {message && (
              <div className="px-3 py-2 text-sm rounded-md bg-bg-hover text-text-primary">{message}</div>
            )}
          </div>
        </section>
      )}

      {/* ── 3. Devices paired to me (hub side, desktop only) ─────── */}
      {isHub && !mobile && mounted && (
        <section className="shrink-0 flex flex-col gap-3 p-4 rounded-lg border border-border-default bg-bg-card">
          <div className="flex items-center justify-between">
            <h2 className="text-base font-semibold flex items-center gap-2">
              <Users size={16} />
              Devices paired to me
            </h2>
            <button
              className="inline-flex items-center gap-1 px-3 py-1 text-xs rounded-md border border-border-light hover:bg-bg-hover disabled:opacity-50"
              onClick={loadDevices}
              disabled={devicesLoading}
            >
              <RefreshCw size={13} className={devicesLoading ? "animate-spin" : undefined} />
              Refresh
            </button>
          </div>
          <p className="text-text-secondary text-sm">
            Devices on the LAN/WLAN must pair before they can read or write data — like Bluetooth
            headphones. A phone that asks to connect pops a confirmation dialog, and it can work
            once you allow it. Requests from this computer (localhost) and from Tailscale are always
            allowed, so local automations need no pairing.
          </p>

          {pairError && <p className="text-sm text-red-600">{pairError}</p>}

          <div className="overflow-x-auto">
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
                      {devicesLoading ? (
                        <span className="inline-flex items-center gap-2">
                          <Loader2 size={14} className="animate-spin" /> Loading…
                        </span>
                      ) : (
                        "No devices paired yet."
                      )}
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
                      {d.bound_user_id && d.bound_user_id !== "local" ? d.bound_user_id : "this PC's user"}
                    </td>
                    <td className="py-2 pr-4 text-text-secondary">{d.last_seen_at ?? "never"}</td>
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
          </div>
        </section>
      )}
    </div>
  );
}
