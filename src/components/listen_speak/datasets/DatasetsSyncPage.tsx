"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "@/lib/tauri";
import { Download, RefreshCw, Upload, CloudOff } from "lucide-react";

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

interface PcSettings {
  pc_url: string;
  pc_token: string;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function DatasetsSyncPage() {
  const [mounted, setMounted] = useState(false);
  const [pc, setPc] = useState<PcSettings>({ pc_url: "", pc_token: "" });
  const [pcDatasets, setPcDatasets] = useState<PcDataset[]>([]);
  const [syncState, setSyncState] = useState<Record<string, SyncStateEntry>>({});
  const [pending, setPending] = useState(0);
  const [loadingList, setLoadingList] = useState(false);
  const [listError, setListError] = useState<string>("");
  const [busyUuid, setBusyUuid] = useState<string>("");
  const [uploading, setUploading] = useState(false);
  const [progress, setProgress] = useState<Record<string, SyncProgress>>({});
  const [message, setMessage] = useState<string>("");

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
      console.error("Failed to load sync state:", e);
    }
  }, []);

  const loadPcSettings = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const g = await invoke<{ pc_url: string; pc_token: string }>("settings_get_global");
      setPc({ pc_url: g.pc_url ?? "", pc_token: g.pc_token ?? "" });
    } catch (e) {
      console.error("Failed to load PC settings:", e);
    }
  }, []);

  const fetchPcDatasets = useCallback(async () => {
    if (!pc.pc_url.trim()) {
      setPcDatasets([]);
      setListError("");
      return;
    }
    setLoadingList(true);
    setListError("");
    try {
      // Native command hits the PC with reqwest — no WebView cross-origin fetch.
      const data = await invoke<PcDataset[]>("pc_list_datasets", {
        pcUrl: pc.pc_url,
        pcToken: pc.pc_token,
      });
      setPcDatasets(data);
    } catch (e) {
      setListError(`Cannot reach PC at ${pc.pc_url}. Check Settings › Discover PC.`);
      setPcDatasets([]);
    } finally {
      setLoadingList(false);
    }
  }, [pc]);

  useEffect(() => {
    setMounted(true);
    loadPcSettings();
    loadLocalState();
  }, [loadPcSettings, loadLocalState]);

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
          disabled={uploading || pending === 0 || !pc.pc_url}
        >
          <Upload size={14} />
          {uploading ? "Uploading…" : `Upload changes${pending > 0 ? ` (${pending})` : ""}`}
        </button>
        <button
          className="inline-flex items-center gap-1 px-3 py-1.5 text-sm rounded-md bg-bg-body border border-border-light hover:bg-bg-hover disabled:opacity-50"
          onClick={() => {
            loadPcSettings();
            fetchPcDatasets();
            loadLocalState();
          }}
          disabled={loadingList}
        >
          <RefreshCw size={14} className={loadingList ? "animate-spin" : undefined} />
          Refresh
        </button>
      </div>

      {!mounted ? null : !isTauri() ? (
        <p className="text-text-secondary">Datasets are only available in the app.</p>
      ) : !pc.pc_url ? (
        <div className="flex flex-col items-center gap-2 py-12 text-text-secondary">
          <CloudOff size={32} />
          <p>No PC connected.</p>
          <p className="text-sm">Open Settings and tap “Discover PC” to pair with your computer.</p>
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
