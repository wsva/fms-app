"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "@/lib/tauri";
import { Download, RefreshCw, ArrowUpDown, CloudOff, Trash2, Loader2, ServerCog } from "lucide-react";
import { logError } from "@/lib/logger";

// ---------------------------------------------------------------------------
// Types (mirror the PC REST API + sync commands)
// ---------------------------------------------------------------------------

interface PcDataset {
  uuid: string;
  name: string;
  updated: string;
  /** One of "dictation" | "card" | "book" | "read_aloud" — drives section grouping. */
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
  /** Incremental row-log cursor; null = predates incremental sync (resync due). */
  cursor: number | null;
}

// ---- Sync status detail (mirrors sync.rs::SyncStatusDetail, §3.5) ----------

/** Coarse per-dataset sync bucket the Status surface renders directly. */
type DatasetSyncState = "downloaded" | "needs_resync" | "not_downloaded" | "removed_on_hub";

interface DatasetStatus {
  dataset_uuid: string;
  dataset_type: string;
  name: string;
  state: DatasetSyncState;
  cursor: number | null;
  synced_at: string;
  bytes: number;
  file_count: number;
  hub_ahead: boolean;
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
  datasets: DatasetStatus[];
  queued_count: number;
  chat_pending: number;
  devices: PairedDevice[];
}

interface SyncProgress {
  uuid: string;
  received: number;
  total: number;
  phase: "download" | "extract" | "done";
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

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/**
 * Pull datasets from another FmS machine and push local progress back.
 * Reachable from both shells: the Android thin client syncs with its paired PC,
 * and a desktop syncs with another desktop (the remote side just needs its web
 * service enabled in Settings — see `web_service`).
 *
 * The whole surface is follower-side: it selects a source hub and pulls from it.
 * A workspace designated the hub is the authority that *serves* data, so when
 * `role === "hub"` we hide every pull control and point the user at the
 * Devices & Hub page to demote or manage paired devices. Cluster identity, the
 * hub role, and pairing all live there now.
 */
export default function DatasetsSyncPage({ onNavigate }: { onNavigate?: (tab: string) => void }) {
  const [mounted, setMounted] = useState(false);
  const [global, setGlobal] = useState<GlobalSettings | null>(null);

  // Dataset + sync state.
  const [pcDatasets, setPcDatasets] = useState<PcDataset[]>([]);
  const [syncState, setSyncState] = useState<Record<string, SyncStateEntry>>({});
  const [queued, setQueued] = useState(0);
  const [loadingList, setLoadingList] = useState(false);
  const [listError, setListError] = useState<string>("");
  const [unpaired, setUnpaired] = useState(false);
  const [busyUuid, setBusyUuid] = useState<string>("");
  const [syncing, setSyncing] = useState(false);
  const [progress, setProgress] = useState<Record<string, SyncProgress>>({});
  const [message, setMessage] = useState<string>("");

  // Aggregate sync status (§3.5) — the single source for role, cluster, hub
  // reachability, per-dataset state, and queued/pending counts. Read-only; the
  // page never mutates it directly, it reloads after each round/action.
  const [status, setStatus] = useState<SyncStatusDetail | null>(null);

  // Which dataset section is shown (Dictation / Cards / Books / Read). Exposed
  // as tabs instead of stacked vertically, which keeps the narrow Android screen
  // and the desktop page alike from turning into an endless scroll.
  const [activeSection, setActiveSection] = useState<string>("dictation");

  const pcUrl = (global?.pc_url ?? "").trim();

  // ---- Loaders ----------------------------------------------------------

  const loadSettings = useCallback(async () => {
    if (!isTauri()) return null;
    try {
      const g = await invoke<GlobalSettings>("settings_get_global");
      setGlobal(g);
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
      setQueued(count);
    } catch (e) {
      logError(`Failed to load sync state: ${String(e)}`, "datasets");
    }
  }, []);

  // Pull the aggregate status (§3.5). Never throws — a partially unreachable
  // hub still yields local role/cluster/dataset state to render.
  const loadStatus = useCallback(async () => {
    if (!isTauri()) return;
    try {
      setStatus(await invoke<SyncStatusDetail>("sync_status"));
    } catch (e) {
      logError(`Failed to load sync status: ${String(e)}`, "datasets");
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

  // ---- Lifecycle --------------------------------------------------------

  useEffect(() => {
    setMounted(true);
    (async () => {
      await loadSettings();
      loadLocalState();
      loadStatus();
    })();
  }, [loadSettings, loadLocalState, loadStatus]);

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

  // One full incremental sync round (§3.3): push queue → per-dataset /changes
  // apply → file-drift fetch → catalog prune. Replaces the old per-dataset
  // Pull / Upload buttons — a round moves everything it can in both directions.
  async function handleSyncNow() {
    if (syncing) return;
    setSyncing(true);
    setMessage("");
    try {
      const r = await invoke<{
        pushed?: number;
        applied?: number;
        resynced?: string[];
        pruned?: string[];
        errors?: { stage: string; error: string }[];
      }>("sync_run_round");
      const bits: string[] = [];
      if (r.pushed) bits.push(`pushed ${r.pushed}`);
      if (r.applied) bits.push(`applied ${r.applied}`);
      if (r.resynced?.length) bits.push(`${r.resynced.length} resynced`);
      if (r.pruned?.length) bits.push(`${r.pruned.length} pruned`);
      if (r.errors?.length) bits.push(`${r.errors.length} error(s)`);
      setMessage(bits.length ? `Round done — ${bits.join(", ")}.` : "Round done — already up to date.");
    } catch (e) {
      setMessage(`Sync round failed: ${String(e)}`);
    } finally {
      setSyncing(false);
      loadLocalState();
      loadStatus();
    }
  }

  // Pull a full hub snapshot for one dataset. On a demoted PC that holds its own
  // copy, this is the explicit "adopt hub copy" action (§7): it overwrites the
  // local dataset, so it is only ever offered deliberately, never automatic.
  async function handleAdopt(uuid: string, name: string) {
    if (busyUuid) return;
    if (!confirm(`Download the hub's copy of "${name}"? This overwrites the local dataset.`)) {
      return;
    }
    setBusyUuid(uuid);
    setMessage("");
    try {
      const res = await invoke<{ updated: boolean; bytes: number; file_count: number }>(
        "dataset_sync_snapshot",
        { uuid }
      );
      setMessage(
        res.updated ? `Downloaded hub copy (${res.file_count} file(s)).` : "Already up to date."
      );
    } catch (e) {
      setMessage(`Download failed: ${String(e)}`);
    } finally {
      setBusyUuid("");
      setProgress((prev) => {
        const next = { ...prev };
        delete next[uuid];
        return next;
      });
      loadLocalState();
      loadStatus();
    }
  }

  // Drop a local copy the hub no longer offers (unsubscribe). Guarded so a
  // dataset still listed by a reachable hub cannot be deleted.
  async function handleForgetDataset(uuid: string, name: string) {
    if (!confirm(`Remove the local copy of "${name || uuid}"? It is no longer on the hub.`)) {
      return;
    }
    setMessage("");
    try {
      await invoke("sync_forget_dataset", { uuid });
      setMessage("Removed local copy.");
      loadLocalState();
      loadStatus();
    } catch (e) {
      setMessage(`Could not remove: ${String(e)}`);
    }
  }

  // ---- Rendering helpers ------------------------------------------------

  // Human-readable count label per dataset type.
  function countLabel(ds: PcDataset): string {
    if (ds.dataset_type === "card") return `${ds.media_count} cards`;
    if (ds.dataset_type === "read_aloud") return `${ds.media_count} texts`;
    if (ds.dataset_type === "book") return "book";
    return `${ds.media_count} media`;
  }

  // Style + label per coarse sync state, so the row and the overview agree.
  const STATE_META: Record<DatasetSyncState, { label: string; cls: string }> = {
    downloaded: { label: "Downloaded", cls: "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400" },
    needs_resync: { label: "Resync due", cls: "bg-amber-500/15 text-amber-600 dark:text-amber-400" },
    not_downloaded: { label: "Not downloaded", cls: "bg-bg-hover text-text-secondary" },
    removed_on_hub: { label: "Removed on hub", cls: "bg-red-500/15 text-red-600 dark:text-red-400" },
  };

  // Index the status detail rows by uuid for per-card lookup.
  const statusByUuid: Record<string, DatasetStatus> = {};
  for (const d of status?.datasets ?? []) statusByUuid[d.dataset_uuid] = d;

  // A designated hub is the authority that serves data — it never pulls, so the
  // entire follower surface (source selector, Sync now, adopt/pull lists) is
  // hidden. Defaults to false while status is still loading or on non-Tauri.
  const isHub = status?.role === "hub";

  function renderDatasetCard(ds: PcDataset, i: number) {
    const st = syncState[ds.uuid];
    const detail = statusByUuid[ds.uuid];
    const prog = progress[ds.uuid];
    const pct =
      prog && prog.total > 0
        ? Math.min(100, Math.round((prog.received / prog.total) * 100))
        : 0;
    const state = detail?.state;
    const meta = state ? STATE_META[state] : null;
    const syncedLine = st
      ? `synced ${st.synced_at}${st.cursor == null ? " · resync due" : ` · cursor ${st.cursor}`}`
      : "not synced";
    return (
      <div
        key={ds.uuid || `${ds.name}-${i}`}
        className="flex flex-col gap-2 p-3 rounded-lg border border-border-default bg-bg-card"
      >
        <div className="flex items-center gap-3">
          <div className="flex flex-col min-w-0 flex-1">
            <span className="font-medium truncate">{ds.name}</span>
            <span className="text-xs text-text-tertiary">
              {countLabel(ds)} · {syncedLine}
            </span>
          </div>
          {/* Status + adopt action. On a narrow (phone) screen the label and the
              "Adopt hub copy" button don't fit beside the name, so stack them
              vertically; widen back to a single row from `sm:` up. */}
          <div className="flex flex-col items-end gap-2 shrink-0 sm:flex-row sm:items-center">
            {meta && (
              <span className={`text-xs px-2 py-0.5 rounded-full shrink-0 ${meta.cls}`}>
                {meta.label}
              </span>
            )}
            {/* "Adopt hub copy" overwrites the local dataset — shown only when a
                local copy is absent or awaiting a resync, never silently applied. */}
            {(state === "not_downloaded" || state === "needs_resync") && (
              <button
                className="inline-flex items-center gap-1 px-3 py-1.5 text-sm rounded-md bg-accent-bg text-white hover:bg-accent-bg-hover disabled:opacity-50 shrink-0"
                onClick={() => handleAdopt(ds.uuid, ds.name)}
                disabled={!!busyUuid}
                title="Pull the hub's full copy (overwrites local)"
              >
                <Download size={14} />
                {busyUuid === ds.uuid ? "Downloading…" : "Download"}
              </button>
            )}
          </div>
        </div>

        {detail?.hub_ahead && state === "downloaded" && (
          <span className="text-xs text-text-tertiary">
            Hub has newer changes — run "Sync now" to pull them.
          </span>
        )}

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

  // A local copy the hub no longer lists, kept separate from the hub-advertised
  // cards so it can offer "remove local copy" without appearing as a source.
  const removedLocal = (status?.datasets ?? []).filter((d) => d.state === "removed_on_hub");

  // Datasets grouped into the four sync sections, in display order. A local
  // copy with no hub catalog entry carries an empty `dataset_type`, so it is
  // caught by the Dictation bucket and rendered with its removed-on-hub badge.
  const sections: { key: string; label: string; items: PcDataset[] }[] = [
    { key: "dictation", label: "Dictation", items: pcDatasets.filter((d) => d.dataset_type === "dictation" || d.dataset_type === "") },
    { key: "card", label: "Cards", items: pcDatasets.filter((d) => d.dataset_type === "card") },
    { key: "book", label: "Books", items: pcDatasets.filter((d) => d.dataset_type === "book") },
    { key: "read_aloud", label: "Read", items: pcDatasets.filter((d) => d.dataset_type === "read_aloud") },
  ];

  return (
    <div className="flex flex-col w-full h-full min-h-0 p-4 gap-3">
      {/* Header */}
      <div className="flex items-center gap-3 shrink-0 flex-wrap">
        <h1 className="text-[1.3em] font-bold">Datasets Sync</h1>
        <div className="ml-auto flex items-center gap-2 flex-wrap">
          {/* One round moves everything it can in both directions (push queue +
              pull deltas), replacing the old per-dataset Pull / Upload buttons.
              Follower-only — a hub serves data, it doesn't pull. */}
          {!isHub && (
            <button
              className="inline-flex items-center gap-1 px-3 py-1.5 text-sm rounded-md bg-accent-bg text-white hover:bg-accent-bg-hover disabled:opacity-50"
              onClick={handleSyncNow}
              disabled={syncing || !pcUrl}
              title="Run one incremental sync round (push + pull)"
            >
              {syncing ? <Loader2 size={14} className="animate-spin" /> : <ArrowUpDown size={14} />}
              {syncing ? "Syncing…" : "Sync now"}
            </button>
          )}
          <button
            className="inline-flex items-center gap-1 px-3 py-1.5 text-sm rounded-md bg-bg-body border border-border-light hover:bg-bg-hover disabled:opacity-50"
            onClick={() => {
              loadSettings();
              loadLocalState();
              loadStatus();
              fetchPcDatasets();
            }}
            disabled={loadingList}
          >
            <RefreshCw size={14} className={loadingList ? "animate-spin" : undefined} />
            Refresh
          </button>
        </div>
      </div>

      {/* Compact connection status strip. Full cluster identity, the hub role,
          and pairing all live on the Devices & Hub page now — this only
          summarizes the link and jumps there. */}
      {status && (
        <div className="shrink-0 flex items-center gap-2 flex-wrap p-2.5 rounded-lg border border-border-default bg-bg-card text-xs">
          <span
            className={`inline-block w-2 h-2 rounded-full ${
              isHub ? "bg-blue-500" : status.hub.reachable ? "bg-emerald-500" : "bg-red-500"
            }`}
          />
          <span className="text-text-secondary">
            {isHub ? (
              "hub · serving datasets to paired devices"
            ) : (
              <>
                hub <span className="font-mono">{status.hub.address || "not configured"}</span>
                {status.hub.reachable ? " · reachable" : " · unreachable"}
              </>
            )}
          </span>
          {!isHub && (status.queued_count > 0 || queued > 0) && (
            <span className="text-amber-600 dark:text-amber-400">
              {Math.max(status.queued_count, queued)} change(s) queued
            </span>
          )}
          {onNavigate && (
            <button
              className="ml-auto inline-flex items-center gap-1 text-text-secondary hover:text-text-primary underline"
              onClick={() => onNavigate("devices-hub")}
            >
              Devices &amp; Hub →
            </button>
          )}
        </div>
      )}

      {isHub ? (
        /* Hub workspace: it serves datasets, it doesn't pull — hide the whole
           follower surface and point the user at Devices & Hub. */
        <div className="flex-1 min-h-0 flex flex-col items-center justify-center gap-3 p-6 text-center">
          <ServerCog size={40} className="text-text-tertiary" />
          <p className="text-text-secondary max-w-md">
            This workspace is the sync <span className="font-semibold">hub</span>. It serves
            datasets to follower devices — only followers can use this page to pull data.
          </p>
          <p className="text-sm text-text-tertiary max-w-md">
            To sync datasets into this machine, designate it as a follower on the Devices &amp; Hub
            page first. Manage paired devices and the hub role there too.
          </p>
          {onNavigate && (
            <button
              className="mt-1 inline-flex items-center gap-1 px-3 py-1.5 text-sm rounded-md bg-accent-bg text-white hover:bg-accent-bg-hover"
              onClick={() => onNavigate("devices-hub")}
            >
              Open Devices &amp; Hub
            </button>
          )}
        </div>
      ) : (
        <>
      {/* The source-PC selector, scan, connect, and pairing all live on the
          Devices & Hub page now — this page only lists and syncs datasets. */}
      {!mounted ? null : !isTauri() ? (
        <p className="text-text-secondary">Datasets Sync is only available in the app.</p>
      ) : !pcUrl ? (
        <div className="flex flex-col items-center gap-2 py-12 text-text-secondary">
          <CloudOff size={32} />
          <p>Not connected to a hub.</p>
          <p className="text-sm">Connect and pair with a hub on the Devices &amp; Hub page. On the other machine, enable the web service in Settings so it can be found.</p>
          {onNavigate && (
            <button
              className="mt-1 inline-flex items-center gap-1 px-3 py-1.5 text-sm rounded-md bg-accent-bg text-white hover:bg-accent-bg-hover"
              onClick={() => onNavigate("devices-hub")}
            >
              Open Devices &amp; Hub
            </button>
          )}
        </div>
      ) : (
        <>
          {listError && (
            <div className="px-3 py-2 text-sm rounded-md bg-red-100 text-red-700 dark:bg-red-900/30 dark:text-red-300">
              {listError}
              {unpaired && onNavigate && (
                <button
                  className="ml-2 underline font-medium"
                  onClick={() => onNavigate("devices-hub")}
                >
                  Pair in Devices &amp; Hub
                </button>
              )}
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
                {/* Section tabs: Dictation / Cards / Books / Read. Only the
                    active section's datasets are listed. */}
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

            {/* Local copies the hub no longer offers. Kept out of the section
                list (they have no hub catalog entry) and surfaced here with an
                explicit remove action (§7 prune-by-absence follow-up). */}
            {removedLocal.length > 0 && (
              <div className="flex flex-col gap-2">
                <p className="text-xs font-medium text-text-tertiary uppercase tracking-wide">
                  Removed on hub
                </p>
                {removedLocal.map((d) => (
                  <div
                    key={d.dataset_uuid}
                    className="flex items-center gap-3 p-3 rounded-lg border border-border-default bg-bg-card"
                  >
                    <div className="flex flex-col min-w-0 flex-1">
                      <span className="font-medium truncate">
                        {d.name || d.dataset_uuid}
                      </span>
                      <span className="text-xs text-text-tertiary">
                        no longer on the hub · {d.file_count} file(s)
                      </span>
                    </div>
                    <span className="text-xs px-2 py-0.5 rounded-full shrink-0 bg-red-500/15 text-red-600 dark:text-red-400">
                      Removed on hub
                    </span>
                    <button
                      className="inline-flex items-center gap-1 px-3 py-1.5 text-sm rounded-md bg-bg-body border border-border-light hover:bg-bg-hover shrink-0"
                      onClick={() => handleForgetDataset(d.dataset_uuid, d.name)}
                      title="Remove this local copy"
                    >
                      <Trash2 size={14} />
                      Remove local copy
                    </button>
                  </div>
                ))}
              </div>
            )}
          </div>
        </>
      )}
        </>
      )}
    </div>
  );
}
