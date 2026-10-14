"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "@/lib/tauri";
import { Download, RefreshCw, ArrowUpDown, CloudOff, Trash2, Loader2, ServerCog, Upload, LogIn, Archive } from "lucide-react";
import { logError } from "@/lib/logger";

// ---------------------------------------------------------------------------
// Types (mirror the PC REST API + sync commands)
// ---------------------------------------------------------------------------

interface PcDataset {
  uuid: string;
  name: string;
  updated: string;
  /** One of "dictation" | "card" | "book" | "read_aloud" | "wiki" — drives section grouping. */
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
  /** Provenance of the host this copy came from (§6.1 / §19.2): "desktop_hub" |
      "registry" | "cloud_hub"; empty when catalog-only (never pulled here). */
  upstream_kind?: string;
  upstream_origin?: string;
  /** Registry availability: "listed" | "withdrawn" (INV-22); empty when unknown. */
  availability?: string;
  /** The registry version adopted; null for a LAN mirror or a catalog-only row. */
  published_version?: number | null;
  /** Why this copy owes a resync (§19.2), present only when state is needs_resync:
      "retention_gap" | "journal_regressed" | "provenance_reset" | "no_cursor". */
  resync_reason?: string | null;
}

/** One journal scope's writeback backlog (§19.2). A nonzero `depth` against a
    host that does not accept the scope is the "queue keeps growing" signal;
    `oldest_edit_time` says how long the head has been stuck. */
interface ScopeQueue {
  depth: number;
  oldest_edit_time: string | null;
}

/** The §19.4 five-cause "queue keeps growing" answer. `cause_index` maps a fault
    to its §19.4 number (1-5); null for the non-fault states. `severity` drives
    color so the UI need not re-derive it from `cause`. */
interface QueueDiagnosis {
  cause: string;
  cause_index?: number | null;
  severity?: string;
  detail?: string;
  action?: string;
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
  /** Which hub the configured origin resolves to: "website" (OAuth Bearer,
      file + card only) or "lan" (device-HMAC, full protocol). Gates the
      pairing/cluster/app-data affordances and unlocks the Upload surface. */
  hub_kind: string;
  role: string;
  cluster_id: string;
  protocol_version: number;
  device_id: string;
  hub: HubStatus;
  datasets: DatasetStatus[];
  queued_count: number;
  chat_pending: number;
  devices: PairedDevice[];
  /** Cursor into the hub's per-user app-data journal (dictation progress + XP);
      null until the first round has pulled it. */
  app_cursor: number | null;
  /** When this device imported the hub's whole app-data history (a state read,
      since the journal above is pruned); null = never, which for a follower means
      its older practice/XP is still not here. */
  history_backfilled_at: string | null;
  /** When this device imported the hub's whole practice-event history (reviews +
      attempts); null = never, or the hub predates protocol v6. */
  events_backfilled_at?: string | null;
  /** Authority the cluster is serving (§7 / §19.2), from the capability page;
      null when the host serves no /cluster (unreachable, or a website hub). */
  authority_kind?: string | null;
  /** The authority's election epoch (null alongside authority_kind). */
  epoch?: number | null;
  /** Scopes the authority advertises it can serve; empty when none reachable. */
  features?: string[];
  /** The `queued_count` total split per journal scope (§19.2). */
  queue_app?: ScopeQueue;
  queue_dataset?: ScopeQueue;
  /** Why the queue is not draining (§19.4), named rather than left as a spinner. */
  queue_diagnosis?: QueueDiagnosis;
  /** The hub advertises an app plane this device has never folded history/events
      from — a visible gap rather than a silent zero. */
  history_gap?: boolean;
  events_gap?: boolean;
  /** The last completed round's persisted result (§19.2) — status, pushed/held/
      applied/resynced/pruned/errors — so the surface reports it without running
      a new round; null until the first round on this build. */
  last_round?: Record<string, unknown> | null;
  last_round_at?: string | null;
}

interface SyncProgress {
  uuid: string;
  received: number;
  total: number;
  phase: "download" | "extract" | "done";
}

// ---- Website-hub upload (mirrors sync/upload.rs, T2) -----------------------

/** `dataset-upload-progress` payload. `pack` has no total (streaming gzip);
    `upload` carries sent/total bytes; `done` clears the row. */
interface UploadProgress {
  uuid: string;
  phase: "pack" | "upload" | "done";
  sent: number;
  total?: number;
}

/** A dataset that lives on this machine (any type) — an upload candidate. */
interface LocalDataset {
  uuid: string;
  name: string;
  dataset_type: string;
}

/** A row of `dataset_upload_state` (this device's upload history). */
interface UploadStateEntry {
  dataset_uuid: string;
  version: number;
  archive_sha256: string;
  archive_size: string;
  uploaded_at: string;
  current_version?: number | null;
  published_at?: string;
}

/** One dataset's cloud publish state, read back by `dataset_publish_state`. */
interface PublishStateEntry {
  state: string; // not_published | live | withdrawn | unknown
  version?: number | null;
  error?: string;
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

  // Website-hub upload surface (T2). Local datasets are the upload candidates;
  // `uploadState` records what this device already pushed ("uploaded vN"); the
  // dialog drives a single in-flight upload with a visibility choice.
  const [localDatasets, setLocalDatasets] = useState<LocalDataset[]>([]);
  const [uploadState, setUploadState] = useState<Record<string, UploadStateEntry>>({});
  const [uploadProgress, setUploadProgress] = useState<Record<string, UploadProgress>>({});
  const [uploadingUuid, setUploadingUuid] = useState<string>("");
  const [uploadTarget, setUploadTarget] = useState<LocalDataset | null>(null);
  const [uploadVisibility, setUploadVisibility] = useState<string>("private");
  const [needsSignIn, setNeedsSignIn] = useState(false);

  // Owner publish/withdraw surface (§8.4/§11.5 mirror). `publishState` holds each
  // uploaded dataset's live cloud state (probed read-back), `publishBusy` the uuid
  // with an in-flight publish/withdraw/restore. Only shown on the website hub.
  const [publishState, setPublishState] = useState<Record<string, PublishStateEntry>>({});
  const [publishBusy, setPublishBusy] = useState<string>("");

  // Aggregate sync status (§3.5) — the single source for role, cluster, hub
  // reachability, per-dataset state, and queued/pending counts. Read-only; the
  // page never mutates it directly, it reloads after each round/action.
  const [status, setStatus] = useState<SyncStatusDetail | null>(null);

  // Which dataset section is shown (Dictation / Cards / Books / Read / Wiki). Exposed
  // as tabs instead of stacked vertically, which keeps the narrow Android screen
  // and the desktop page alike from turning into an endless scroll.
  const [activeSection, setActiveSection] = useState<string>("dictation");

  const pcUrl = (global?.pc_url ?? "").trim();

  // The website hub is the only origin that accepts dataset uploads and speaks
  // OAuth (no pairing/cluster/app-data). Defaults false while status loads.
  const isWebsite = status?.hub_kind === "website";

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

  // Load the website-upload surface: every local dataset (candidate) plus what
  // this device has already pushed. Cheap and read-only; harmless on LAN (the
  // Upload UI itself is gated on `isWebsite`).
  // Probe `dataset_publish_state` for the given uploaded datasets and merge the
  // results into `publishState`. Best-effort: a per-dataset failure (e.g. a
  // logged-out account) records an `unknown` entry rather than throwing.
  const loadPublishStates = useCallback(async (uuids: string[]) => {
    if (!isTauri() || uuids.length === 0) return;
    const next: Record<string, PublishStateEntry> = {};
    await Promise.all(
      uuids.map(async (uuid) => {
        try {
          const r = await invoke<PublishStateEntry & { uuid: string }>("dataset_publish_state", {
            datasetUuid: uuid,
          });
          next[uuid] = { state: r.state, version: r.version, error: r.error };
        } catch (e) {
          next[uuid] = { state: "unknown", error: String(e) };
        }
      }),
    );
    setPublishState((prev) => ({ ...prev, ...next }));
  }, []);

  const loadUploadSurface = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const [locals, states] = await Promise.all([
        invoke<LocalDataset[]>("sync_local_datasets"),
        invoke<UploadStateEntry[]>("dataset_upload_state"),
      ]);
      setLocalDatasets(locals);
      const map: Record<string, UploadStateEntry> = {};
      for (const s of states) map[s.dataset_uuid] = s;
      setUploadState(map);
      // Refresh the cloud publish state of every dataset this device has ever
      // uploaded, in parallel (cheap Bearer GETs).
      loadPublishStates(Object.keys(map));
    } catch (e) {
      logError(`Failed to load upload surface: ${String(e)}`, "datasets");
    }
  }, [loadPublishStates]);

  const fetchPcDatasets = useCallback(async () => {
    if (!pcUrl) {
      setPcDatasets([]);
      setListError("");
      return;
    }
    setLoadingList(true);
    setListError("");
    setNeedsSignIn(false);
    try {
      // Native command hits the PC with reqwest — no WebView cross-origin fetch.
      const data = await invoke<PcDataset[]>("pc_list_datasets", { pcUrl });
      setPcDatasets(data);
      setUnpaired(false);
    } catch (e) {
      // A 401 from the PC means "not paired yet" — surface the Pair button.
      // On the website hub the same slot is a token problem: the shared
      // hub_http_error wording ("sign in again", 401) needs a Sign-in action,
      // not pairing (pairing is a LAN concept).
      const text = String(e);
      const signIn = text.includes("sign in again") || /\b401\b/.test(text) || text.includes("access token");
      const notPaired = !signIn && (text.includes("not paired") || text.includes("pairing denied"));
      setUnpaired(notPaired);
      setNeedsSignIn(signIn);
      setListError(
        signIn
          ? "Sign in to reach the website hub — it rejected or did not receive your access token."
          : notPaired
          ? "This device is not paired with the source PC yet."
          : `Cannot reach the PC at ${pcUrl}.`
      );
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
      loadUploadSurface();
    })();
  }, [loadSettings, loadLocalState, loadStatus, loadUploadSurface]);

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

  // Track upload progress events (T2): packing, then byte-streaming to the hub.
  const uploadReq = useRef<(() => void) | undefined>(undefined);
  useEffect(() => {
    if (!isTauri()) return;
    const un = listen<UploadProgress>("dataset-upload-progress", (evt) => {
      const p = evt.payload;
      if (p.phase === "done") {
        setUploadProgress((prev) => {
          const next = { ...prev };
          delete next[p.uuid];
          return next;
        });
      } else {
        setUploadProgress((prev) => ({ ...prev, [p.uuid]: p }));
      }
    });
    un.then((fn) => {
      uploadReq.current = fn;
    });
    return () => {
      uploadReq.current?.();
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
        history?: {
          progress_applied?: number;
          xp_applied?: number;
          complete?: boolean;
        } | null;
        errors?: { stage: string; error: string }[];
      }>("sync_run_round");
      const bits: string[] = [];
      if (r.pushed) bits.push(`pushed ${r.pushed}`);
      if (r.applied) bits.push(`applied ${r.applied}`);
      if (r.resynced?.length) bits.push(`${r.resynced.length} resynced`);
      if (r.pruned?.length) bits.push(`${r.pruned.length} pruned`);
      // The history read only runs when the journal could not deliver the past, so
      // anything here is worth saying out loud — including an unfinished import.
      if (r.history) {
        const h = `${r.history.progress_applied ?? 0} progress, ${r.history.xp_applied ?? 0} XP`;
        bits.push(r.history.complete === false ? `history partial (${h}, retrying)` : `history ${h}`);
      }
      if (r.errors?.length) bits.push(`${r.errors.length} error(s)`);
      setMessage(bits.length ? `Round done — ${bits.join(", ")}.` : "Round done — already up to date.");
    } catch (e) {
      setMessage(`Sync round failed: ${String(e)}`);
    } finally {
      setSyncing(false);
      loadLocalState();
      loadStatus();
      loadUploadSurface();
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

  // Free the disk space of an adopted (website-downloaded) copy without
  // unsubscribing: the read-only base under download/ is removed and the copy
  // flips to "Not downloaded" (progress/XP in the app DB are untouched, INV-9),
  // so the existing Download button re-pulls it on demand. The backend refuses a
  // copy that lives in a writable location, or one with an edited overlay on top
  // (detach the overlay first) — its actionable message is surfaced verbatim.
  async function handleEvict(uuid: string, name: string) {
    if (busyUuid) return;
    if (
      !confirm(
        `Evict the downloaded copy of "${name}"? This frees the disk space and keeps it re-downloadable; your progress is untouched.`
      )
    ) {
      return;
    }
    setBusyUuid(uuid);
    setMessage("");
    try {
      const res = await invoke<{ evicted: boolean; freed_bytes: number }>("dataset_evict", { uuid });
      const mb = ((res.freed_bytes ?? 0) / (1024 * 1024)).toFixed(1);
      setMessage(`Evicted “${name}” — freed ${mb} MB. Re-download any time.`);
    } catch (e) {
      setMessage(`Evict failed: ${String(e)}`);
    } finally {
      setBusyUuid("");
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

  // ---- Website-hub upload (T2) ------------------------------------------

  // Open the upload dialog for one local dataset. Visibility defaults to
  // private; the hub only honours it on the uuid's first upload anyway.
  function openUploadDialog(ds: LocalDataset) {
    setUploadVisibility("private");
    setUploadTarget(ds);
  }

  // Pack + POST the archive. A 409 tagged `owned-by-other` means the hub already
  // holds this uuid under a different account — offer "save as new copy"
  // (`dataset_remint_uuid`, which mints a fresh id and re-uploads in one step).
  async function runUpload(ds: LocalDataset, visibility: string) {
    setUploadingUuid(ds.uuid);
    setMessage("");
    const finish = () => {
      setUploadingUuid("");
      setUploadProgress((prev) => {
        const next = { ...prev };
        delete next[ds.uuid];
        return next;
      });
      loadUploadSurface();
      loadLocalState();
      loadStatus();
      fetchPcDatasets();
    };
    try {
      const res = await invoke<{ version?: number }>("dataset_export_upload", {
        datasetUuid: ds.uuid,
        visibility,
      });
      setMessage(`Uploaded “${ds.name}” v${res?.version ?? "?"}.`);
      setUploadTarget(null);
    } catch (e) {
      const text = String(e);
      if (text.includes("owned-by-other")) {
        if (
          confirm(
            `The hub already holds “${ds.name}” under a different account. Save it as a new copy under your account and upload? This mints a fresh id and never overwrites the hub's copy.`
          )
        ) {
          try {
            const res = await invoke<{ version?: number; new_uuid?: string }>("dataset_remint_uuid", {
              datasetUuid: ds.uuid,
              visibility,
            });
            setMessage(`Saved as a new copy and uploaded v${res?.version ?? "?"}.`);
            setUploadTarget(null);
          } catch (e2) {
            setMessage(`Upload failed: ${String(e2)}`);
          }
        }
      } else {
        setMessage(`Upload failed: ${text}`);
      }
    } finally {
      finish();
    }
  }

  // ---- Owner publish / withdraw / restore (§8.4 / §11.5) ----------------

  // Promote the newest uploaded version to the live pin (POST /api/v1/publish,
  // version omitted = newest uploaded). Additive, so no confirm gate.
  async function handlePublish(ds: LocalDataset) {
    setPublishBusy(ds.uuid);
    setMessage("");
    try {
      const res = await invoke<{ current_version?: number }>("sync_publish_dataset_version", {
        uuid: ds.uuid,
      });
      setMessage(`Published “${ds.name}” v${res?.current_version ?? "?"}.`);
    } catch (e) {
      setMessage(`Publish failed: ${String(e)}`);
    } finally {
      setPublishBusy("");
      await loadPublishStates([ds.uuid]);
      loadUploadSurface();
    }
  }

  // Take the pinned version down (POST /api/v1/publish/available=false) — a
  // §11.5 takedown: fresh reads 410 without deleting bytes. Confirm-gated (it is
  // destructive to subscribers' next check).
  async function handleWithdraw(ds: LocalDataset, version: number) {
    if (!confirm(`Withdraw v${version} of “${ds.name}” from the cloud? Its downloads will report it withdrawn (the stored bytes are kept).`)) return;
    setPublishBusy(ds.uuid);
    setMessage("");
    try {
      await invoke("sync_set_dataset_availability", { uuid: ds.uuid, version, available: false });
      setMessage(`Withdrew “${ds.name}” v${version}.`);
    } catch (e) {
      setMessage(`Withdraw failed: ${String(e)}`);
    } finally {
      setPublishBusy("");
      await loadPublishStates([ds.uuid]);
      loadUploadSurface();
    }
  }

  // Restore a withdrawn version (available=true). Additive, no confirm.
  async function handleRestore(ds: LocalDataset, version: number) {
    setPublishBusy(ds.uuid);
    setMessage("");
    try {
      await invoke("sync_set_dataset_availability", { uuid: ds.uuid, version, available: true });
      setMessage(`Restored “${ds.name}” v${version}.`);
    } catch (e) {
      setMessage(`Restore failed: ${String(e)}`);
    } finally {
      setPublishBusy("");
      await loadPublishStates([ds.uuid]);
      loadUploadSurface();
    }
  }

  // ---- Rendering helpers ------------------------------------------------

  // Human-readable count label per dataset type.
  function countLabel(ds: PcDataset): string {
    if (ds.dataset_type === "card") return `${ds.media_count} cards`;
    if (ds.dataset_type === "read_aloud") return `${ds.media_count} texts`;
    if (ds.dataset_type === "book") return "book";
    if (ds.dataset_type === "wiki") return `${ds.media_count} files`;
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

  // §19.2 diagnostics surfaced off the last persisted round. `last_round` is an
  // opaque JSON blob, so coerce defensively — a stale/odd shape renders as zeros
  // rather than throwing.
  const num = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) ? v : 0);
  const lastRound = status?.last_round ?? null;
  const lastRoundErrors = Array.isArray(lastRound?.errors)
    ? (lastRound!.errors as unknown[]).length
    : 0;
  // Color the §19.4 diagnosis by its severity so a fault reads at a glance.
  const diagClass = (sev?: string): string =>
    sev === "error"
      ? "text-red-600 dark:text-red-400"
      : sev === "warn"
      ? "text-amber-600 dark:text-amber-400"
      : "text-text-tertiary";

  // Human text for a `needs_resync` cause (§19.2). An unknown/absent code falls
  // back to the generic "one snapshot due" wording rather than rendering blank.
  const resyncReasonLabel = (code?: string | null): string => {
    switch (code) {
      case "retention_gap":
        return "Retention gap — the hub pruned changes older than this copy's cursor.";
      case "journal_regressed":
        return "Hub journal regressed (reset/reinstall) — full re-pull due.";
      case "provenance_reset":
        return "Re-establishing against the currently connected host.";
      default:
        return "Predates incremental sync — one snapshot due.";
    }
  };

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
            {/* Evict frees the disk space of an adopted website copy (read-only
                base under download/) without unsubscribing — it flips to "Not
                downloaded" and the Download button above re-pulls it. Only the
                website origin adopts into the download root; a LAN mirror stays
                writable and is deleted from the Datasets page instead. */}
            {state === "downloaded" && isWebsite && (
              <button
                className="inline-flex items-center gap-1 px-3 py-1.5 text-sm rounded-md bg-bg-body border border-border-light hover:bg-bg-hover disabled:opacity-50 shrink-0"
                onClick={() => handleEvict(ds.uuid, ds.name)}
                disabled={!!busyUuid}
                title="Free disk space — keeps it re-downloadable, progress untouched"
              >
                <Archive size={14} />
                {busyUuid === ds.uuid ? "Evicting…" : "Evict"}
              </button>
            )}
          </div>
        </div>

        {detail?.hub_ahead && state === "downloaded" && (
          <span className="text-xs text-text-tertiary">
            Hub has newer changes — run "Sync now" to pull them.
          </span>
        )}

        {/* Provenance (§19.2): where this copy came from and whether its registry
            listing is still live. Hidden for a catalog-only row (never pulled). */}
        {detail?.upstream_kind && (
          <span className="text-xs text-text-tertiary flex items-center gap-1 flex-wrap">
            <span className="font-mono">{detail.upstream_kind}</span>
            {detail.published_version != null && <span>v{detail.published_version}</span>}
            {detail.availability === "withdrawn" && (
              <span className="text-amber-600 dark:text-amber-400">withdrawn upstream</span>
            )}
          </span>
        )}

        {/* Resync cause (§19.2): why this copy owes one snapshot, named rather
            than a bare "Resync due" badge. Only when a resync is actually owed. */}
        {state === "needs_resync" && (
          <span className="text-xs text-amber-600 dark:text-amber-400">
            {resyncReasonLabel(detail?.resync_reason)}
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

  // Datasets grouped into the sync sections, in display order. A local
  // copy with no hub catalog entry carries an empty `dataset_type`, so it is
  // caught by the Dictation bucket and rendered with its removed-on-hub badge.
  // Wiki datasets are syncable markdown trees — the hub serves them in the same
  // catalog, so they need a bucket of their own rather than a new transport.
  const sections: { key: string; label: string; items: PcDataset[] }[] = [
    { key: "dictation", label: "Dictation", items: pcDatasets.filter((d) => d.dataset_type === "dictation" || d.dataset_type === "") },
    { key: "card", label: "Cards", items: pcDatasets.filter((d) => d.dataset_type === "card") },
    { key: "book", label: "Books", items: pcDatasets.filter((d) => d.dataset_type === "book") },
    { key: "read_aloud", label: "Read", items: pcDatasets.filter((d) => d.dataset_type === "read_aloud") },
    { key: "wiki", label: "Wiki", items: pcDatasets.filter((d) => d.dataset_type === "wiki") },
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
              loadUploadSurface();
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
            ) : isWebsite ? (
              <>
                Website Hub <span className="font-mono">{status.hub.address || "not configured"}</span>
                {status.hub.reachable ? " · reachable" : " · unreachable"}
              </>
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
          {/* Per-user practice data (dictation progress, XP) travels in its own
              journal scope, not in a dataset snapshot — so it converges even when
              nothing is subscribed. `null` = no app-data pull has landed yet. */}
          {!isHub && !isWebsite && (
            <span className="text-text-tertiary">
              app data {status.app_cursor == null ? "not pulled yet" : `cursor ${status.app_cursor}`}
              {" · "}
              history {status.history_backfilled_at ? `imported ${status.history_backfilled_at}` : "not imported"}
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

      {/* §19.2 diagnostics (follower only — a hub serves, it does not pull):
          authority + advertised scopes, per-scope queue depth, backfill gaps,
          and what the last round actually did. All additive, degrades to nothing
          when a stale backend omits the fields. */}
      {status && !isHub && (
        <div className="shrink-0 flex flex-col gap-1.5 p-2.5 rounded-lg border border-border-default bg-bg-card text-xs">
          {status.queue_diagnosis && status.queue_diagnosis.cause !== "idle" && (
            <div className={`flex items-center gap-2 flex-wrap ${diagClass(status.queue_diagnosis.severity)}`}>
              <span className="font-medium">{status.queue_diagnosis.detail}</span>
              {status.queue_diagnosis.action && (
                <span className="text-text-tertiary">→ {status.queue_diagnosis.action}</span>
              )}
            </div>
          )}
          {status.authority_kind && (
            <div className="flex items-center gap-2 flex-wrap text-text-secondary">
              <span>
                authority <span className="font-mono">{status.authority_kind}</span>
                {status.epoch != null && <> · epoch {status.epoch}</>}
              </span>
              {(status.features ?? []).map((f) => (
                <span key={f} className="px-1.5 py-0.5 rounded bg-bg-hover text-text-tertiary font-mono">
                  {f}
                </span>
              ))}
            </div>
          )}
          <div className="flex items-center gap-2 flex-wrap text-text-tertiary">
            <span>
              queue @app {status.queue_app?.depth ?? 0} / dataset {status.queue_dataset?.depth ?? 0}
            </span>
            {(status.queue_app?.depth ?? 0) > 0 && status.queue_app?.oldest_edit_time && (
              <span className="text-amber-600 dark:text-amber-400">
                oldest app {status.queue_app.oldest_edit_time}
              </span>
            )}
            {(status.queue_dataset?.depth ?? 0) > 0 && status.queue_dataset?.oldest_edit_time && (
              <span className="text-amber-600 dark:text-amber-400">
                oldest dataset {status.queue_dataset.oldest_edit_time}
              </span>
            )}
          </div>
          {status.history_gap && (
            <span className="text-amber-600 dark:text-amber-400">
              history not imported — run a round, or force it with sync_history_backfill
            </span>
          )}
          {status.events_gap && (
            <span className="text-amber-600 dark:text-amber-400">
              practice events not imported — a hub upgrade may be owed, then a round
            </span>
          )}
          {lastRound && (
            <div className="flex items-center gap-2 flex-wrap text-text-tertiary">
              <span>
                last round <span className="font-mono">{String(lastRound.status ?? "?")}</span>
              </span>
              <span>
                pushed {num(lastRound.pushed)} · applied {num(lastRound.applied)} · held{" "}
                {num(lastRound.held)}
              </span>
              {/* INV-14 LWW loss: a nonzero count means un-pushed local edits were
                  overwritten by a newer hub entry this round — a red flag of silent
                  loss, not a routine statistic, so it only renders when > 0. */}
              {num(lastRound.dropped_pending) > 0 && (
                <span className="text-red-600 dark:text-red-400">
                  {num(lastRound.dropped_pending)} local edit(s) overwritten (INV-14)
                </span>
              )}
              {lastRoundErrors > 0 && (
                <span className="text-red-600 dark:text-red-400">{lastRoundErrors} error(s)</span>
              )}
              {status.last_round_at && <span>{status.last_round_at}</span>}
            </div>
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
              {/* Website: a token problem is fixed by signing in, never by pairing
                  (pairing is a LAN concept). LAN unpaired: point at Devices & Hub. */}
              {needsSignIn && (
                <button
                  className="ml-2 inline-flex items-center gap-1 underline font-medium"
                  onClick={() => invoke("auth_open_login").catch((err) => setMessage(`Sign-in failed: ${String(err)}`))}
                >
                  <LogIn size={14} />
                  Sign in
                </button>
              )}
              {unpaired && !isWebsite && onNavigate && (
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
                {/* Section tabs: Dictation / Cards / Books / Read / Wiki. Only the
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

            {/* Website-hub upload (T2). Local datasets across every type are the
                upload candidates — including copies the hub catalog doesn't list
                yet. Gated to the website origin; a LAN hub never accepts uploads. */}
            {isWebsite && (
              <div className="flex flex-col gap-2">
                <p className="text-xs font-medium text-text-tertiary uppercase tracking-wide">
                  Upload to website hub
                </p>
                <p className="text-xs text-text-tertiary">
                  Subscribing to a shared dataset is browser-only on the website, so a shared
                  dataset shows up in your catalog here only after you subscribe to it on the
                  website market page.
                </p>
                {localDatasets.length === 0 ? (
                  <p className="text-sm text-text-tertiary px-1">No local datasets to upload.</p>
                ) : (
                  localDatasets.map((ds) => {
                    const up = uploadProgress[ds.uuid];
                    const done = uploadState[ds.uuid];
                    const pct =
                      up && up.phase === "upload" && up.total
                        ? Math.min(100, Math.round((up.sent / up.total) * 100))
                        : 0;
                    return (
                      <div
                        key={ds.uuid}
                        className="flex flex-col gap-2 p-3 rounded-lg border border-border-default bg-bg-card"
                      >
                        <div className="flex items-center gap-3">
                          <div className="flex flex-col min-w-0 flex-1">
                            <span className="font-medium truncate">{ds.name}</span>
                            <span className="text-xs text-text-tertiary">
                              {ds.dataset_type}
                              {done ? ` · uploaded v${done.version} (${done.uploaded_at})` : ""}
                            </span>
                          </div>
                          <button
                            className="inline-flex items-center gap-1 px-3 py-1.5 text-sm rounded-md bg-accent-bg text-white hover:bg-accent-bg-hover disabled:opacity-50 shrink-0"
                            onClick={() => openUploadDialog(ds)}
                            disabled={!!uploadingUuid}
                            title="Pack this local dataset and upload it to the website hub"
                          >
                            <Upload size={14} />
                            {uploadingUuid === ds.uuid ? "Uploading…" : done ? "Re-upload" : "Upload"}
                          </button>
                        </div>
                        {up && up.phase !== "done" && (
                          <div className="flex flex-col gap-1">
                            <div className="h-1.5 w-full rounded bg-bg-hover overflow-hidden">
                              <div
                                className="h-full bg-accent-bg transition-[width]"
                                style={{ width: up.phase === "pack" ? "40%" : `${pct}%` }}
                              />
                            </div>
                            <span className="text-xs text-text-tertiary">
                              {up.phase === "pack" ? "Packing…" : `Uploading… ${pct}%`}
                            </span>
                          </div>
                        )}
                        {done && (() => {
                          const ps = publishState[ds.uuid];
                          const busy = publishBusy === ds.uuid;
                          const state = ps?.state ?? "not_published";
                          const ver = ps?.version ?? null;
                          return (
                            <div className="flex items-center justify-between gap-2 pt-2 mt-1 border-t border-border-default">
                              {state === "live" ? (
                                <span className="text-xs text-green-600 dark:text-green-400 truncate">Live v{ver}</span>
                              ) : state === "withdrawn" ? (
                                <span className="text-xs text-red-500 truncate" title={ps?.error}>v{ver} taken down</span>
                              ) : state === "unknown" ? (
                                <span className="text-xs text-text-tertiary truncate" title={ps?.error}>Cloud state unknown</span>
                              ) : (
                                <span className="text-xs text-text-tertiary truncate">Not published</span>
                              )}
                              <div className="flex items-center gap-2 shrink-0">
                                {(state === "live" || state === "withdrawn") && (
                                  <button
                                    className="inline-flex items-center gap-1 px-2.5 py-1 text-xs rounded-md border border-border-default hover:bg-bg-hover disabled:opacity-50"
                                    onClick={() => (state === "live" ? handleWithdraw(ds, ver as number) : handleRestore(ds, ver as number))}
                                    disabled={busy || ver == null}
                                    title={state === "live" ? "Take this version down (bytes kept)" : "Restore this version"}
                                  >
                                    {busy ? <Loader2 size={12} className="animate-spin" /> : null}
                                    {busy ? "Working…" : state === "live" ? "Withdraw" : "Restore"}
                                  </button>
                                )}
                                {(state === "not_published" || state === "unknown") && (
                                  <button
                                    className="inline-flex items-center gap-1 px-2.5 py-1 text-xs rounded-md bg-accent-bg text-white hover:bg-accent-bg-hover disabled:opacity-50"
                                    onClick={() => handlePublish(ds)}
                                    disabled={busy}
                                    title="Publish the newest uploaded version to the cloud"
                                  >
                                    {busy ? <Loader2 size={12} className="animate-spin" /> : null}
                                    {busy ? "Publishing…" : "Publish"}
                                  </button>
                                )}
                              </div>
                            </div>
                          );
                        })()}
                      </div>
                    );
                  })
                )}
              </div>
            )}
          </div>
        </>
      )}
        </>
      )}

      {/* Upload dialog: pick a visibility, then pack + stream. Blocks closing
          while an upload is in flight so the running task is never orphaned. */}
      {uploadTarget && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
          onClick={() => !uploadingUuid && setUploadTarget(null)}
        >
          <div
            className="w-full max-w-sm rounded-lg border border-border-default bg-bg-card p-4 flex flex-col gap-3"
            onClick={(e) => e.stopPropagation()}
          >
            <h2 className="font-semibold truncate">Upload “{uploadTarget.name}”</h2>
            <label className="flex flex-col gap-1 text-sm">
              <span className="text-text-secondary">Visibility</span>
              <select
                className="px-2 py-1.5 rounded-md border border-border-default bg-bg-body text-text-primary"
                value={uploadVisibility}
                onChange={(e) => setUploadVisibility(e.target.value)}
                disabled={!!uploadingUuid}
              >
                <option value="private">Private</option>
                <option value="shared">Shared</option>
                <option value="public">Public</option>
              </select>
              <span className="text-xs text-text-tertiary">
                The hub applies visibility only on the dataset’s first upload; re-uploads keep
                the recorded setting.
              </span>
            </label>
            <div className="flex justify-end gap-2">
              <button
                className="px-3 py-1.5 text-sm rounded-md bg-bg-body border border-border-light hover:bg-bg-hover disabled:opacity-50"
                onClick={() => setUploadTarget(null)}
                disabled={!!uploadingUuid}
              >
                Cancel
              </button>
              <button
                className="inline-flex items-center gap-1 px-3 py-1.5 text-sm rounded-md bg-accent-bg text-white hover:bg-accent-bg-hover disabled:opacity-50"
                onClick={() => runUpload(uploadTarget, uploadVisibility)}
                disabled={!!uploadingUuid}
              >
                {uploadingUuid ? <Loader2 size={14} className="animate-spin" /> : <Upload size={14} />}
                {uploadingUuid ? "Uploading…" : "Upload"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
