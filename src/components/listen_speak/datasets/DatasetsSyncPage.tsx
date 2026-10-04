"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "@/lib/tauri";
import { Download, RefreshCw, Upload, CloudOff, Search, ChevronDown } from "lucide-react";
import { logInfo, logError } from "@/lib/logger";

// ---------------------------------------------------------------------------
// Types (mirror the PC REST API + sync commands)
// ---------------------------------------------------------------------------

interface PcDataset {
  uuid: string;
  name: string;
  updated: string;
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
  selected_model: string;
  model_dir: string;
  model_unload_timeout: unknown;
  onboarding_completed: boolean;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function DatasetsSyncPage() {
  const [mounted, setMounted] = useState(false);
  const [global, setGlobal] = useState<GlobalSettings | null>(null);

  // PC connection / discovery state.
  const [candidates, setCandidates] = useState<PcCandidate[]>([]);
  const [discovering, setDiscovering] = useState(false);
  const [showManual, setShowManual] = useState(false);
  const [manualUrl, setManualUrl] = useState("");
  const [manualToken, setManualToken] = useState("");

  // Dataset + sync state.
  const [pcDatasets, setPcDatasets] = useState<PcDataset[]>([]);
  const [syncState, setSyncState] = useState<Record<string, SyncStateEntry>>({});
  const [pending, setPending] = useState(0);
  const [loadingList, setLoadingList] = useState(false);
  const [listError, setListError] = useState<string>("");
  const [busyUuid, setBusyUuid] = useState<string>("");
  const [uploading, setUploading] = useState(false);
  const [progress, setProgress] = useState<Record<string, SyncProgress>>({});
  const [message, setMessage] = useState<string>("");

  const pcUrl = (global?.pc_url ?? "").trim();
  const pcToken = global?.pc_token ?? "";

  // ---- Loaders ----------------------------------------------------------

  const loadSettings = useCallback(async () => {
    if (!isTauri()) return null;
    try {
      const g = await invoke<GlobalSettings>("settings_get_global");
      setGlobal(g);
      setManualUrl(g.pc_url ?? "");
      setManualToken(g.pc_token ?? "");
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
      const data = await invoke<PcDataset[]>("pc_list_datasets", { pcUrl, pcToken });
      setPcDatasets(data);
    } catch (e) {
      setListError(`Cannot reach PC at ${pcUrl}.`);
      setPcDatasets([]);
    } finally {
      setLoadingList(false);
    }
  }, [pcUrl, pcToken]);

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

  // Persist pc_url (and optionally pc_token) to global settings so the sync
  // commands (which read settings on the backend) can resolve the target PC.
  const savePc = useCallback(
    async (url: string, token?: string) => {
      if (!global) return;
      const next: GlobalSettings = { ...global, pc_url: url };
      if (token !== undefined) next.pc_token = token;
      setGlobal(next);
      setManualUrl(url);
      if (token !== undefined) setManualToken(token);
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

  return (
    <div className="flex flex-col w-full h-full min-h-0 p-4 gap-3">
      {/* Header */}
      <div className="flex items-center gap-3 shrink-0 flex-wrap">
        <h1 className="text-[1.3em] font-bold">Datasets</h1>
        <button
          className="ml-auto inline-flex items-center gap-1 px-3 py-1.5 text-sm rounded-md bg-bg-body border border-border-light hover:bg-bg-hover disabled:opacity-50"
          onClick={handleUpload}
          disabled={uploading || pending === 0 || !pcUrl}
        >
          <Upload size={14} />
          {uploading ? "Uploading…" : `Upload changes${pending > 0 ? ` (${pending})` : ""}`}
        </button>
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

      {/* PC connection selector */}
      <div className="shrink-0 flex flex-col gap-2 p-3 rounded-lg border border-border-default bg-bg-card">
        <div className="flex items-center gap-2 flex-wrap">
          <span className="text-sm font-medium shrink-0">PC</span>
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
            title="Scan for nearby PCs"
          >
            <Search size={14} className={discovering ? "animate-spin" : undefined} />
            {discovering ? "Scanning…" : "Scan"}
          </button>
          <button
            className="inline-flex items-center gap-1 px-3 py-1.5 text-sm rounded-md bg-bg-body border border-border-light hover:bg-bg-hover shrink-0"
            onClick={() => setShowManual((s) => !s)}
            title="Enter PC address manually"
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
            <input
              type="text"
              className="w-32 px-3 py-1.5 text-sm border border-border-light rounded-md bg-bg-input text-text-primary"
              value={manualToken}
              onChange={(e) => setManualToken(e.target.value)}
              placeholder="token (opt.)"
            />
            <button
              className="inline-flex items-center px-3 py-1.5 text-sm rounded-md bg-accent-bg text-white hover:bg-accent-bg-hover disabled:opacity-50 shrink-0"
              onClick={() => savePc(manualUrl.trim(), manualToken)}
              disabled={!manualUrl.trim()}
            >
              Connect
            </button>
          </div>
        )}
      </div>

      {!mounted ? null : !isTauri() ? (
        <p className="text-text-secondary">Datasets are only available in the app.</p>
      ) : !pcUrl ? (
        <div className="flex flex-col items-center gap-2 py-12 text-text-secondary">
          <CloudOff size={32} />
          <p>{discovering ? "Scanning for nearby PCs…" : "Not connected to a PC."}</p>
          <p className="text-sm">Pick a PC from the selector above, tap Scan, or use Manual address.</p>
        </div>
      ) : (
        <>
          {listError && (
            <div className="px-3 py-2 text-sm rounded-md bg-red-100 text-red-700 dark:bg-red-900/30 dark:text-red-300">
              {listError}
            </div>
          )}
          {message && (
            <div className="px-3 py-2 text-sm rounded-md bg-bg-hover text-text-primary">
              {message}
            </div>
          )}

          <div className="flex-1 min-h-0 overflow-y-auto flex flex-col gap-2">
            {pcDatasets.length === 0 && !loadingList && (
              <p className="text-sm text-text-tertiary">No datasets available on the PC.</p>
            )}
            {pcDatasets.map((ds, i) => {
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
                        {ds.media_count} media ·{" "}
                        {st ? `synced ${st.synced_at}` : "not synced"}
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
            })}
          </div>
        </>
      )}
    </div>
  );
}
