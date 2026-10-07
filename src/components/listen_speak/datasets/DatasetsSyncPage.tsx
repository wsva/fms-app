"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import { Download, RefreshCw, Upload, CloudOff, Search, ChevronDown, Link2, KeyRound } from "lucide-react";
import { logInfo, logError } from "@/lib/logger";

// ---------------------------------------------------------------------------
// Types (mirror the PC REST API + sync commands)
// ---------------------------------------------------------------------------

interface PcDataset {
  uuid: string;
  name: string;
  updated: string;
  /** One of "dictation" | "card" | "book" — drives section grouping. */
  dataset_type: string;
  media_count: number;
  status: string;
}

interface SyncStateEntry {
  dataset_uuid: string;
  overall_hash: string;
  synced_at: string;
  bytes: number;
  file_count: number;
}

interface SyncProgress {
  uuid: string;
  received: number;
  total: number;
  phase: "download" | "extract" | "done";
}

// A discovered PC returned by the `pc_discover` command.
interface PcCandidate {
  url: string;
  source: string; // "lan" | "tailscale" | "saved"
  name: string;
}

// Subset of the global settings this page needs (mirrors settings.rs).
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

// Result of the `pc_pair_start` command.
interface PairResult {
  state: "approved" | "pending" | "denied" | "none";
  fingerprint?: string;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/**
 * Pull datasets from another FmS machine and push local progress back.
 * Reachable from both shells: the Android thin client syncs with its paired PC,
 * and a desktop syncs with another desktop (the remote side just needs its web
 * service enabled in Settings — see `web_service`).
 */
export default function DatasetsSyncPage() {
  const [mounted, setMounted] = useState(false);
  const [mobile, setMobile] = useState(false);
  const [global, setGlobal] = useState<GlobalSettings | null>(null);

  // PC connection / discovery state.
  const [candidates, setCandidates] = useState<PcCandidate[]>([]);
  const [discovering, setDiscovering] = useState(false);
  const [showManual, setShowManual] = useState(false);
  const [manualUrl, setManualUrl] = useState("");

  // Pairing state (Bluetooth-style device identity, see sync.rs / pairing.rs).
  const [pairing, setPairing] = useState(false);
  const [pairFingerprint, setPairFingerprint] = useState("");
  const [pairMessage, setPairMessage] = useState("");

  // Dataset + sync state.
  const [pcDatasets, setPcDatasets] = useState<PcDataset[]>([]);
  const [syncState, setSyncState] = useState<Record<string, SyncStateEntry>>({});
  const [pending, setPending] = useState(0);
  const [loadingList, setLoadingList] = useState(false);
  const [listError, setListError] = useState<string>("");
  const [unpaired, setUnpaired] = useState(false);
  const [busyUuid, setBusyUuid] = useState<string>("");
  const [uploading, setUploading] = useState(false);
  const [progress, setProgress] = useState<Record<string, SyncProgress>>({});
  const [message, setMessage] = useState<string>("");

  // Which dataset section is shown (Dictation / Cards / Books). Exposed as tabs
  // instead of stacked vertically, which keeps the narrow Android screen and the
  // desktop page alike from turning into an endless scroll.
  const [activeSection, setActiveSection] = useState<string>("dictation");

  const pcUrl = (global?.pc_url ?? "").trim();
  const deviceId = global?.device_id ?? "";

  // ---- Loaders ----------------------------------------------------------

  const loadSettings = useCallback(async () => {
    if (!isTauri()) return null;
    try {
      const g = await invoke<GlobalSettings>("settings_get_global");
      setGlobal(g);
      setManualUrl(g.pc_url ?? "");
      return g;
    } catch (e) {
      logError(`Failed to load settings: ${String(e)}`, "datasets");
      return null;
    }
  }, []);

  const loadLocalState = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const [states, count] = await Promise.all([
        invoke<SyncStateEntry[]>("dataset_sync_state"),
        invoke<number>("writeback_pending_count"),
      ]);
      const map: Record<string, SyncStateEntry> = {};
      for (const s of states) map[s.dataset_uuid] = s;
      setSyncState(map);
      setPending(count);
    } catch (e) {
      logError(`Failed to load sync state: ${String(e)}`, "datasets");
    }
  }, []);

  const fetchPcDatasets = useCallback(async () => {
    if (!pcUrl) {
      setPcDatasets([]);
      setListError("");
      return;
    }
    setLoadingList(true);
    setListError("");
    try {
      // Native command hits the PC with reqwest — no WebView cross-origin fetch.
      const data = await invoke<PcDataset[]>("pc_list_datasets", { pcUrl });
      setPcDatasets(data);
      setUnpaired(false);
    } catch (e) {
      // A 401 from the PC means "not paired yet" — surface the Pair button.
      const text = String(e);
      const notPaired = text.includes("not paired") || text.includes("pairing denied");
      setUnpaired(notPaired);
      setListError(notPaired ? "This device is not paired with the source PC yet." : `Cannot reach the PC at ${pcUrl}.`);
      setPcDatasets([]);
    } finally {
      setLoadingList(false);
    }
  }, [pcUrl]);

  // ---- Pairing ----------------------------------------------------------

  // Ask the PC to pair this device: the owner gets a confirm dialog and the
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
          setListError("");
          loadSettings();
          fetchPcDatasets();
        } else if (res.state === "denied") {
          setPairMessage("The PC owner denied this pairing. Ask them to remove the block, or start a new pairing request.");
        } else {
          setPairMessage("Pairing did not complete.");
        }
      } catch (e) {
        setPairMessage(`Pairing failed: ${String(e)}`);
      } finally {
        setPairing(false);
      }
    },
    [pairing, loadSettings, fetchPcDatasets],
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

  // ---- Discovery + connection ------------------------------------------

  const discover = useCallback(async () => {
    if (!isTauri()) return;
    setDiscovering(true);
    try {
      const res = await invoke<PcCandidate[]>("pc_discover", { timeoutMs: 2500 });
      setCandidates(res);
      if (res.length === 0) logInfo("No FmS PC found on this network.", "datasets");
    } catch (e) {
      logError(`Discovery failed: ${String(e)}`, "datasets");
      setCandidates([]);
    } finally {
      setDiscovering(false);
    }
  }, []);

  // Persist pc_url to global settings so the sync commands (which read
  // settings on the backend) can resolve the target PC.
  const savePc = useCallback(
    async (url: string) => {
      if (!global) return;
      const next: GlobalSettings = { ...global, pc_url: url };
      setGlobal(next);
      setManualUrl(url);
      try {
        await invoke("settings_set_global", { global: next });
        setMessage(url ? `Connected to ${url}.` : "Disconnected.");
      } catch (e) {
        logError(`Failed to save PC url: ${String(e)}`, "datasets");
        setMessage(`Failed to save PC address: ${String(e)}`);
      }
    },
    [global],
  );

  // ---- Lifecycle --------------------------------------------------------

  useEffect(() => {
    setMounted(true);
    setMobile(isMobileApp());
    (async () => {
      const g = await loadSettings();
      loadLocalState();
      // Auto-discover on open when no PC is connected yet.
      if (!g || !(g.pc_url ?? "").trim()) discover();
    })();
  }, [loadSettings, loadLocalState, discover]);

  useEffect(() => {
    fetchPcDatasets();
  }, [fetchPcDatasets]);

  // Track snapshot progress events.
  const progressReq = useRef<(() => void) | undefined>(undefined);
  useEffect(() => {
    if (!isTauri()) return;
    const un = listen<SyncProgress>("dataset-sync-progress", (evt) => {
      const p = evt.payload;
      if (p.phase === "done") {
        setProgress((prev) => {
          const next = { ...prev };
          delete next[p.uuid];
          return next;
        });
        setBusyUuid("");
      } else {
        setProgress((prev) => ({ ...prev, [p.uuid]: p }));
      }
    });
    un.then((fn) => {
      progressReq.current = fn;
    });
    return () => {
      progressReq.current?.();
    };
  }, []);

  // ---- Actions ----------------------------------------------------------

  async function handleSync(uuid: string) {
    if (busyUuid) return;
    setBusyUuid(uuid);
    setMessage("");
    try {
      const res = await invoke<{ updated: boolean; bytes: number; file_count: number }>(
        "dataset_sync_snapshot",
        { uuid }
      );
      setMessage(
        res.updated
          ? `Synced ${res.file_count} file(s).`
          : "Already up to date."
      );
    } catch (e) {
      setMessage(`Sync failed: ${String(e)}`);
    } finally {
      setBusyUuid("");
      setProgress((prev) => {
        const next = { ...prev };
        delete next[uuid];
        return next;
      });
      loadLocalState();
    }
  }

  async function handleUpload() {
    setUploading(true);
    setMessage("");
    try {
      const n = await invoke<number>("writeback_flush");
      setMessage(`Uploaded ${n} change(s).`);
      loadLocalState();
    } catch (e) {
      setMessage(`Upload failed: ${String(e)}`);
    } finally {
      setUploading(false);
    }
  }

  // ---- Rendering helpers ------------------------------------------------

  // Human-readable count label per dataset type.
  function countLabel(ds: PcDataset): string {
    if (ds.dataset_type === "card") return `${ds.media_count} cards`;
    if (ds.dataset_type === "book") return "book";
    return `${ds.media_count} media`;
  }

  function renderDatasetCard(ds: PcDataset, i: number) {
    const st = syncState[ds.uuid];
    const prog = progress[ds.uuid];
    const pct =
      prog && prog.total > 0
        ? Math.min(100, Math.round((prog.received / prog.total) * 100))
        : 0;
    return (
      <div
        key={ds.uuid || `${ds.name}-${i}`}
        className="flex flex-col gap-2 p-3 rounded-lg border border-border-default bg-bg-card"
      >
        <div className="flex items-center gap-3">
          <div className="flex flex-col min-w-0 flex-1">
            <span className="font-medium truncate">{ds.name}</span>
            <span className="text-xs text-text-tertiary">
              {countLabel(ds)} · {st ? `synced ${st.synced_at}` : "not synced"}
            </span>
          </div>
          <button
            className="inline-flex items-center gap-1 px-3 py-1.5 text-sm rounded-md bg-accent-bg text-white hover:bg-accent-bg-hover disabled:opacity-50 shrink-0"
            onClick={() => handleSync(ds.uuid)}
            disabled={!!busyUuid}
          >
            <Download size={14} />
            {busyUuid === ds.uuid ? "Syncing…" : st ? "Re-sync" : "Sync"}
          </button>
        </div>

        {prog && prog.phase !== "done" && (
          <div className="flex flex-col gap-1">
            <div className="h-1.5 w-full rounded bg-bg-hover overflow-hidden">
              <div
                className="h-full bg-accent-bg transition-[width]"
                style={{
                  width: prog.phase === "extract" ? "100%" : `${pct}%`,
                }}
              />
            </div>
            <span className="text-xs text-text-tertiary">
              {prog.phase === "extract"
                ? "Extracting…"
                : `Downloading… ${pct}% (${prog.received} / ${prog.total} bytes)`}
            </span>
          </div>
        )}
      </div>
    );
  }

  // Datasets grouped into the three sync sections, in display order.
  const sections: { key: string; label: string; items: PcDataset[] }[] = [
    { key: "dictation", label: "Dictation", items: pcDatasets.filter((d) => d.dataset_type === "dictation") },
    { key: "card", label: "Cards", items: pcDatasets.filter((d) => d.dataset_type === "card") },
    { key: "book", label: "Books", items: pcDatasets.filter((d) => d.dataset_type === "book") },
  ];

  return (
    <div className="flex flex-col w-full h-full min-h-0 p-4 gap-3">
      {/* Header */}
      <div className="flex items-center gap-3 shrink-0 flex-wrap">
        <h1 className="text-[1.3em] font-bold">Datasets Sync</h1>
        <div className="ml-auto flex items-center gap-3">
          {/* Pushing local progress back relies on the writeback queue, which is
              only fed by the thin-client build (desktop writes straight to its
              own DB), so the action is mobile-only. */}
          {mobile && (
            <button
              className="inline-flex items-center gap-1 px-3 py-1.5 text-sm rounded-md bg-bg-body border border-border-light hover:bg-bg-hover disabled:opacity-50"
              onClick={handleUpload}
              disabled={uploading || pending === 0 || !pcUrl}
            >
              <Upload size={14} />
              {uploading ? "Uploading…" : `Upload changes${pending > 0 ? ` (${pending})` : ""}`}
            </button>
          )}
          <button
            className="inline-flex items-center gap-1 px-3 py-1.5 text-sm rounded-md bg-bg-body border border-border-light hover:bg-bg-hover disabled:opacity-50"
            onClick={() => {
              loadSettings();
              loadLocalState();
              fetchPcDatasets();
            }}
            disabled={loadingList}
          >
            <RefreshCw size={14} className={loadingList ? "animate-spin" : undefined} />
            Refresh
          </button>
        </div>
      </div>

      {/* PC connection selector */}
      <div className="shrink-0 flex flex-col gap-2 p-3 rounded-lg border border-border-default bg-bg-card">
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
            only user gesture is asking the PC owner to approve this device. */}
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
          {unpaired && (
            <button
              className="ml-auto text-xs text-text-secondary underline disabled:opacity-50"
              onClick={resetIdentity}
              disabled={pairing}
              title="Regenerate this device's identity (use after the PC denied the pairing)"
            >
              New identity
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
      </div>

      {!mounted ? null : !isTauri() ? (
        <p className="text-text-secondary">Datasets Sync is only available in the app.</p>
      ) : !pcUrl ? (
        <div className="flex flex-col items-center gap-2 py-12 text-text-secondary">
          <CloudOff size={32} />
          <p>{discovering ? "Scanning for nearby PCs…" : "Not connected to a source PC."}</p>
          <p className="text-sm">Pick a PC from the selector above, tap Scan, or use Manual address. On the other machine, enable the web service in Settings so it can be found.</p>
        </div>
      ) : (
        <>
          {listError && (
            <div className="px-3 py-2 text-sm rounded-md bg-red-100 text-red-700 dark:bg-red-900/30 dark:text-red-300">
              {listError}
              {unpaired && (
                <button
                  className="ml-2 underline font-medium"
                  onClick={() => pair(pcUrl)}
                  disabled={pairing}
                >
                  {pairing ? "Pairing…" : "Pair now"}
                </button>
              )}
            </div>
          )}
          {pairMessage && (
            <div className="px-3 py-2 text-sm rounded-md bg-bg-hover text-text-primary">
              {pairMessage}
            </div>
          )}
          {message && (
            <div className="px-3 py-2 text-sm rounded-md bg-bg-hover text-text-primary">
              {message}
            </div>
          )}

          <div className="flex-1 min-h-0 overflow-y-auto flex flex-col gap-4">
            {pcDatasets.length === 0 && !loadingList && (
              <p className="text-sm text-text-tertiary">No datasets available on the source PC.</p>
            )}
            {pcDatasets.length > 0 && (
              <>
                {/* Section tabs: Dictation / Cards / Books. Only the active
                    section's datasets are listed. */}
                <div className="shrink-0 flex gap-1 border-b border-border-default">
                  {sections.map((section) => (
                    <button
                      key={section.key}
                      onClick={() => setActiveSection(section.key)}
                      className={`px-3 py-2 text-sm font-medium border-b-2 -mb-px transition-colors ${
                        activeSection === section.key
                          ? "border-accent-bg text-text-primary"
                          : "border-transparent text-text-tertiary hover:text-text-secondary"
                      }`}
                    >
                      {section.label}
                      {section.items.length > 0 && (
                        <span className="ml-1 text-xs opacity-70">{section.items.length}</span>
                      )}
                    </button>
                  ))}
                </div>

                {(() => {
                  const section =
                    sections.find((s) => s.key === activeSection) ?? sections[0];
                  return section.items.length === 0 ? (
                    <p className="text-sm text-text-tertiary px-1">None on PC.</p>
                  ) : (
                    <div className="flex flex-col gap-2">
                      {section.items.map((ds, i) => renderDatasetCard(ds, i))}
                    </div>
                  );
                })()}
              </>
            )}
          </div>
        </>
      )}
    </div>
  );
}
