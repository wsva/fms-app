"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import type { LucideIcon } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "@/lib/tauri";
import { useAuth } from "@/hooks/useAuth";
import { message as showDialog } from "@tauri-apps/plugin-dialog";
import { Download, RefreshCw, ArrowUpDown, CloudOff, Trash2, Loader2, ServerCog, Upload, LogIn, Archive } from "lucide-react";
import { tbGhost, tbSolid, tbToggle, tbToggleStacked, TB_ICON } from "@/lib/ui/toolbar";
import { ToolbarButton, PageHeader } from "@/components/ui/toolbar";
import { useIsMobile } from "@/hooks/useIsMobile";
import { useVisiblePolling } from "@/hooks/useVisiblePolling";
import { usePageRefresh, useIsActiveTab } from "@/lib/refresh/page-refresh";
import { logError, logInfo } from "@/lib/logger";

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

// ---- Follower → hub push (mirrors sync/client/push.rs) ---------------------

/** One push's plan, as `dataset_push_preview` reports it — what the confirm
    dialog reads before anything is sent. */
interface PushPreview {
  dataset_uuid: string;
  name: string;
  dataset_type: string;
  local_dir: string;
  hub_base: string;
  creates: number;
  updates: number;
  deletes: number;
  unchanged: number;
  upload_bytes: number;
  db_bytes: number;
  /** The hub's copy moved since this device last synced it. */
  hub_changed: boolean;
  no_baseline: boolean;
  hub_content_gen: number;
  local_content_gen: number;
}

/** A push's own progress: bytes streamed to the hub, then `done` clears the row.
    Tagged `kind: "push"` on the wire so this never collides with the download
    bar, which shares the event name. */
interface PushProgress {
  uuid: string;
  phase: "push" | "done";
  sent?: number;
  total?: number;
}

/** Every payload that arrives on `dataset-sync-progress`. One channel carries
    three planes — a download's byte bar (`download`/`extract`/`done`), the
    per-dataset notices a round emits (`db`, `files`), and a push (`kind:
    "push"`) — so each listener filters instead of sharing a row, since `sent`
    and `received` mean different things per plane. */
interface SyncProgressEvent {
  uuid: string;
  phase: SyncProgress["phase"] | PushProgress["phase"] | "db" | "files";
  kind?: "push";
  received?: number;
  total?: number;
  sent?: number;
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

// ---- Online market (mirrors sync/client/market.rs, docs/design/market.md) ---

/** One catalog row of `market_list`, enriched with its local sync bucket. */
interface MarketDataset {
  uuid: string;
  name: string;
  description?: string | null;
  dataset_type: string;
  /** `private | shared | public`; null in lite-degraded mode. */
  visibility?: string | null;
  language?: string | null;
  is_owner: boolean;
  is_subscribed: boolean;
  owner?: string | null;
  tags?: { uuid: string; name: string }[] | null;
  media_count?: number | null;
  card_count?: number | null;
  total_bytes?: number | null;
  file_count?: number | null;
  updated: string;
  published_version?: number | null;
  /** Coarse local bucket for this uuid (subset of DatasetSyncState). */
  sync_state: DatasetSyncState;
  withdrawn?: boolean;
  /** True when the host lacks the `market` capability and rows came from the
      lite catalog — no subscribe/visibility controls in that mode. */
  lite?: boolean;
  uploaded_version?: number | null;
}

/** One registry adoption on this device (`market_list` downloads section). */
interface MarketDownload {
  uuid: string;
  name: string;
  on_disk: boolean;
  sync_state: DatasetSyncState;
  availability?: string;
  withdrawn?: boolean;
  published_version?: number | null;
  upstream_origin?: string;
  bytes?: number;
  file_count?: number;
  synced_at?: string;
}

/** What this device uploaded under its account (`market_list` published section). */
interface MarketPublished {
  uuid: string;
  version: number;
  current_version?: number | null;
  published_at?: string | null;
  uploaded_at: string;
  name?: string | null;
  dataset_type?: string | null;
  visibility?: string | null;
  is_subscribed?: boolean | null;
}

interface MarketPayload {
  datasets: MarketDataset[];
  downloads: MarketDownload[];
  published: MarketPublished[];
  origin: string;
  lite: boolean;
  market_available: boolean;
  can_subscribe: boolean;
  signed_in: boolean;
  degraded_reason?: string;
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
 * The surface is split into three zones behind one toolbar-row switcher —
 * Pull (browse/download the hub catalog), Publish (upload local datasets to the
 * cloud account) and Status (connection detail + diagnostics). A designated
 * hub is the authority that *serves* data, so in the Pull zone it sees a
 * notice instead of pull controls; cluster identity, the hub role, and pairing
 * all live on the Devices & Hub page, reachable from the Status zone.
 */
export default function DatasetsSyncPage({ onNavigate }: { onNavigate?: (tab: string) => void }) {
  const [mounted, setMounted] = useState(false);
  const [global, setGlobal] = useState<GlobalSettings | null>(null);

  // Dataset + sync state.
  const [pcDatasets, setPcDatasets] = useState<PcDataset[]>([]);
  const [syncState, setSyncState] = useState<Record<string, SyncStateEntry>>({});
  // Local writeback queue depth (`writeback_pending_count`), the client-side
  // counterpart of the hub's `status.queued_count` journal total.
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

  // Follower → hub push: the dataset card's "Push to hub" and the toolbar's
  // page-level push-all. `pushProgress` is keyed by uuid and cleared on `done`,
  // exactly like the download/upload progress maps above.
  const [pushProgress, setPushProgress] = useState<Record<string, PushProgress>>({});
  const [pushingUuid, setPushingUuid] = useState<string>("");
  const [pushingAll, setPushingAll] = useState(false);

  // Owner publish/withdraw surface (§8.4/§11.5 mirror). `publishState` holds each
  // uploaded dataset's live cloud state (probed read-back), `publishBusy` the uuid
  // with an in-flight publish/withdraw/restore. Only shown on the website hub.
  const [publishState, setPublishState] = useState<Record<string, PublishStateEntry>>({});
  const [publishBusy, setPublishBusy] = useState<string>("");

  // Online market (the Pull tab's website branch). Role-agnostic: a hub browses
  // and downloads from the cloud too. Filters mirror the web market (type chips,
  // one tag, free text); every change re-fetches through `loadMarket`.
  const [market, setMarket] = useState<MarketPayload | null>(null);
  const [marketLoading, setMarketLoading] = useState(false);
  const [marketError, setMarketError] = useState<string>("");
  const [marketSection, setMarketSection] = useState<"browse" | "downloads" | "published">("browse");
  const [marketTypes, setMarketTypes] = useState<string[]>([]);
  const [marketTag, setMarketTag] = useState("");
  const [marketQ, setMarketQ] = useState("");
  const [marketBusyUuid, setMarketBusyUuid] = useState<string>("");

  // Aggregate sync status (§3.5) — the single source for role, cluster, hub
  // reachability, per-dataset state, and queued/pending counts. Read-only; the
  // page never mutates it directly, it reloads after each round/action.
  const [status, setStatus] = useState<SyncStatusDetail | null>(null);

  // Which dataset section is shown (Dictation / Cards / Books / Read / Wiki). Exposed
  // as tabs instead of stacked vertically, which keeps the narrow Android screen
  // and the desktop page alike from turning into an endless scroll.
  const [activeSection, setActiveSection] = useState<string>("dictation");

  // Which zone the toolbar switcher shows. "publish" only exists once an
  // account is signed in; "status" carries the connection + diagnostics that
  // used to sit permanently above the lists.
  const [activeZone, setActiveZone] = useState<"pull" | "publish" | "status">("pull");

  // Phone toolbars have ~360px to spend, so there the bar stacks each control
  // icon-over-label (the kit's stacked tokens) instead of dropping the words.
  const mobile = useIsMobile();

  const pcUrl = (global?.pc_url ?? "").trim();

  // The website hub is the only origin that accepts dataset uploads and speaks
  // OAuth (no pairing/cluster/app-data). Defaults false while status loads.
  const isWebsite = status?.hub_kind === "website";

  // Publishing to the cloud is an OWNER-SIDE operation orthogonal to the
  // workspace's LAN role and its `pc_url` (which describes an upstream to sync
  // FROM, not a destination to publish TO). The backend `send_archive`,
  // `sync_mirror_push`, `sync_publish_dataset_version`, and
  // `dataset_push_changed_files` all target `auth::base_url()` directly, so
  // the UI gate is "is there a logged-in account?" — not `isWebsite`. That
  // lets a Hub with no upstream (LAN-only or none) still publish its own
  // datasets through the Datasets Sync page.
  const { authUser } = useAuth();
  const canPublish = authUser != null;

  // The Pull tab's source split: the website branch (market) renders whenever
  // the effective origin is the cloud — `pc_url` pointing at the website, or no
  // `pc_url` with a signed-in account to fall back to (exactly the backend's
  // `hub_base_or_account` order). It shows for every role: a hub may subscribe
  // and download from the cloud while staying the authority for its LAN
  // followers; only a LAN `pc_url` keeps the classic follower-only pull list.
  const marketBranch = isWebsite || (!pcUrl && canPublish);

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

  // Fetch the market catalog plus this device's downloads/published sections.
  // Only ever runs in the website branch; every filter change re-fetches (the
  // type/tag/q filters are server-side when the host advertises `market`,
  // client-side over the lite rows for q).
  const loadMarket = useCallback(async () => {
    if (!isTauri()) return;
    setMarketLoading(true);
    setMarketError("");
    try {
      const p = await invoke<MarketPayload>("market_list", {
        query: {
          types: marketTypes,
          tag: marketTag || null,
          q: marketQ || null,
        },
      });
      setMarket(p);
    } catch (e) {
      setMarketError(String(e));
      setMarket(null);
    } finally {
      setMarketLoading(false);
    }
  }, [marketTypes, marketTag, marketQ]);

  // ---- Lifecycle --------------------------------------------------------

  // Nothing about a sync round announces itself: the last-round summary, the
  // queue depth and the peer's dataset list all move without an event. So this
  // page re-reads on activation (the shared hook) and then keeps itself honest
  // with a slow poll while it stays on screen.
  const refreshAll = useCallback(() => {
    loadSettings();
    loadLocalState();
    loadStatus();
    loadUploadSurface();
    fetchPcDatasets();
    if (marketBranch) loadMarket();
  }, [loadSettings, loadLocalState, loadStatus, loadUploadSurface, fetchPcDatasets, marketBranch, loadMarket]);

  usePageRefresh("datasets-sync", {
    refresh: refreshAll,
    canRefresh: () => !loadingList,
  });

  const pollActive = useIsActiveTab("datasets-sync");
  useVisiblePolling(
    pollActive,
    () => {
      loadStatus();
      if (marketBranch) loadMarket();
    },
    20_000
  );

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

  // The market branch fetches its own catalog on entry and whenever a filter
  // changes — the LAN branch never triggers the call.
  useEffect(() => {
    if (mounted && isTauri() && marketBranch) loadMarket();
  }, [mounted, marketBranch, loadMarket]);

  // Track snapshot progress events. A push reports on this same event name
  // (`kind: "push"`) and the round's per-dataset `db`/`files` notices carry no
  // byte progress, so both are left to their own listeners below.
  const progressReq = useRef<(() => void) | undefined>(undefined);
  useEffect(() => {
    if (!isTauri()) return;
    const un = listen<SyncProgressEvent>("dataset-sync-progress", (evt) => {
      const p = evt.payload;
      if (p.kind === "push") return;
      // Positive test, so only the two byte phases write a row: `db`/`files` are
      // notices, and `push` would carry `sent` where the bar reads `received`.
      if (p.phase === "download" || p.phase === "extract") {
        const phase = p.phase;
        setProgress((prev) => ({
          ...prev,
          [p.uuid]: { uuid: p.uuid, received: p.received ?? 0, total: p.total ?? 0, phase },
        }));
      } else if (p.phase === "done") {
        setProgress((prev) => {
          const next = { ...prev };
          delete next[p.uuid];
          return next;
        });
        setBusyUuid("");
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

  // Track follower → hub push progress. Same channel as the download bar, told
  // apart by `kind: "push"`: the row holds the running byte total, and `done`
  // clears it (the handler's `finally` clears it too, in case the event is
  // missed when the page unmounts mid-push).
  const pushReq = useRef<(() => void) | undefined>(undefined);
  useEffect(() => {
    if (!isTauri()) return;
    const un = listen<SyncProgressEvent>("dataset-sync-progress", (evt) => {
      const p = evt.payload;
      if (p.kind !== "push") return;
      if (p.phase === "done") {
        setPushProgress((prev) => {
          const next = { ...prev };
          delete next[p.uuid];
          return next;
        });
        return;
      }
      setPushProgress((prev) => ({
        ...prev,
        [p.uuid]: { uuid: p.uuid, phase: "push", sent: p.sent ?? 0, total: p.total ?? 0 },
      }));
    });
    un.then((fn) => {
      pushReq.current = fn;
    });
    return () => {
      pushReq.current?.();
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
      if (marketBranch) loadMarket();
    }
  }

  // ---- Follower → hub push (docs/design/dataset-push.md) ----------------

  // Send one dataset's whole directory up to the LAN hub — the "I downloaded
  // it, built it here, publish it back" action. Only the files whose digest the
  // hub lacks are uploaded (untouched media costs nothing), the frozen DB rides
  // along, and the hub installs the merged tree in two renames. The preview is
  // the confirm gate: it names exactly what moves and where, so a push is never
  // a blind overwrite.
  async function handlePush(ds: { uuid: string; name: string }) {
    if (pushingUuid || pushingAll) return;
    setPushingUuid(ds.uuid);
    setMessage("");
    try {
      const p = await invoke<PushPreview>("dataset_push_preview", { uuid: ds.uuid });
      const what = `${p.creates} new, ${p.updates} changed, ${p.deletes} removed, ${p.unchanged} unchanged`;
      // `upload_bytes` is the whole payload (`db_bytes` included), so it is the
      // total the dialog names; the DB is called out separately only because it
      // is a whole-file replace, not a diff.
      const dbLine = p.db_bytes > 0 ? `\nDatabase: ${fmtBytes(p.db_bytes)} (replaces the hub's copy).` : "";
      // The hub's copy moving since this device last saw it is the one case where
      // a push destroys work nobody here has read; say so before asking.
      const warn = p.hub_changed
        ? `\n\nWARNING: the hub's copy changed since this device last synced it (hub revision ${p.hub_content_gen}, this copy last saw ${p.local_content_gen}). Pushing replaces that work — it goes to the hub's trash, it is not merged.`
        : "";
      const ok = confirm(
        `Push “${p.name}” to the hub?\n\n${what}\nUpload: ${fmtBytes(p.upload_bytes)} to ${p.hub_base}${dbLine}${warn}`
      );
      if (!ok) return;
      logInfo(`push: starting "${p.name}" (${p.dataset_uuid}) ${what}`, "datasets");
      const res = await invoke<{
        pushed?: boolean;
        files_sent?: number;
        files_unchanged?: number;
        deletes?: number;
        bytes_sent?: number;
        content_gen?: number;
      }>("dataset_push_to_hub", { uuid: ds.uuid, allowHubChanges: p.hub_changed });
      setMessage(
        res.pushed
          ? `Pushed “${p.name}” — ${res.files_sent ?? 0} file(s), ${fmtBytes(res.bytes_sent ?? 0)} (hub revision ${res.content_gen ?? "?"}).`
          : `“${p.name}” is already identical on the hub — nothing sent.`
      );
    } catch (e) {
      const text = String(e);
      logError(`push: "${ds.name}" (${ds.uuid}) failed: ${text}`, "datasets");
      setMessage(`Push failed: ${text}`);
      if (isTauri()) void showDialog(text, { title: "Push failed", kind: "error" });
    } finally {
      setPushingUuid("");
      setPushProgress((prev) => {
        const next = { ...prev };
        delete next[ds.uuid];
        return next;
      });
      loadLocalState();
      loadStatus();
      fetchPcDatasets();
    }
  }

  // Push every dataset this device authored. Deliberately no `allowHubChanges`:
  // a batch cannot collect a per-dataset consent, so any hub copy that moved is
  // reported as failed (with the actionable `hub_changed` message) instead of
  // being overwritten. Read-only adoptions are skipped by the backend, and one
  // failure never aborts the rest.
  async function handlePushAll() {
    if (pushingAll || pushingUuid) return;
    if (
      !confirm(
        "Push every dataset built on this device to the hub?\n\nOnly the files the hub is missing are uploaded. Copies downloaded from the hub are skipped. Any dataset whose hub copy changed since this device last synced it is reported as failed rather than overwritten — push those individually to confirm."
      )
    ) {
      return;
    }
    setPushingAll(true);
    setMessage("");
    try {
      const r = await invoke<{
        pushed?: number;
        unchanged?: number;
        failed?: number;
        datasets?: { uuid: string; name?: string; status: string; error?: string }[];
      }>("dataset_push_all", { allowHubChanges: false });
      const bits = [`${r.pushed ?? 0} pushed`, `${r.unchanged ?? 0} unchanged`];
      if (r.failed) bits.push(`${r.failed} failed`);
      setMessage(`Push all done — ${bits.join(", ")}.`);
      const bad = (r.datasets ?? []).filter((d) => d.status === "failed");
      if (bad.length && isTauri()) {
        void showDialog(
          bad.map((d) => `${d.name || d.uuid}: ${d.error || "failed"}`).join("\n\n"),
          { title: "Some datasets could not be pushed", kind: "error" }
        );
      }
    } catch (e) {
      const text = String(e);
      logError(`push all failed: ${text}`, "datasets");
      setMessage(`Push all failed: ${text}`);
      if (isTauri()) void showDialog(text, { title: "Push all failed", kind: "error" });
    } finally {
      setPushingAll(false);
      setPushProgress({});
      loadLocalState();
      loadStatus();
      fetchPcDatasets();
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

  // ---- Online market actions --------------------------------------------

  // Adopt one market dataset through the existing snapshot pipeline: registry
  // read → read-only download/<type>/<uuid> with `upstream_kind='registry'`.
  // No confirm — adoption never overwrites a writable local copy (the backend
  // `owned-locally` guard refuses it and that actionable error is shown
  // verbatim). Progress rides the same `dataset-sync-progress` events.
  async function marketDownload(uuid: string, name: string) {
    if (marketBusyUuid || busyUuid) return;
    setMarketBusyUuid(uuid);
    setMessage("");
    try {
      const res = await invoke<{ updated: boolean; bytes: number; file_count: number }>(
        "dataset_sync_snapshot",
        { uuid }
      );
      setMessage(res.updated ? `Downloaded “${name}” (${res.file_count} file(s)).` : "Already up to date.");
    } catch (e) {
      setMessage(`Download failed: ${String(e)}`);
    } finally {
      setMarketBusyUuid("");
      loadLocalState();
      loadStatus();
      loadMarket();
    }
  }

  // Free the disk space of an adopted market copy (same semantics as
  // handleEvict: read-only base removed, provenance and progress kept, the
  // Download button re-pulls on demand).
  async function marketEvict(uuid: string, name: string) {
    if (marketBusyUuid || busyUuid) return;
    if (
      !confirm(
        `Evict the downloaded copy of “${name}”? This frees the disk space and keeps it re-downloadable; your progress is untouched.`
      )
    ) {
      return;
    }
    setMarketBusyUuid(uuid);
    setMessage("");
    try {
      const res = await invoke<{ evicted: boolean; freed_bytes: number }>("dataset_evict", { uuid });
      const mb = ((res.freed_bytes ?? 0) / (1024 * 1024)).toFixed(1);
      setMessage(`Evicted “${name}” — freed ${mb} MB. Re-download any time.`);
    } catch (e) {
      setMessage(`Evict failed: ${String(e)}`);
    } finally {
      setMarketBusyUuid("");
      loadLocalState();
      loadStatus();
      loadMarket();
    }
  }

  // Subscribe/unsubscribe is a pure website fact (dataset_subscription rows);
  // it moves no bytes and unsubscribing never touches the local copy.
  async function toggleSubscribe(d: MarketDataset) {
    const on = !d.is_subscribed;
    setMarketBusyUuid(d.uuid);
    setMessage("");
    try {
      await invoke(on ? "market_subscribe" : "market_unsubscribe", { uuid: d.uuid });
      setMessage(`${on ? "Subscribed to" : "Unsubscribed from"} “${d.name}”.`);
    } catch (e) {
      setMessage(`${on ? "Subscribe" : "Unsubscribe"} failed: ${String(e)}`);
    } finally {
      setMarketBusyUuid("");
      loadMarket();
    }
  }

  // Owner-only visibility toggle from My Published; the backend also refreshes
  // the stored publish pin from the host's echo, so one re-fetch shows truth.
  async function changeVisibility(
    p: { uuid: string; name?: string | null; visibility?: string | null },
    visibility: string
  ) {
    if (visibility === p.visibility) return;
    setMarketBusyUuid(p.uuid);
    setMessage("");
    try {
      await invoke("market_set_visibility", { uuid: p.uuid, visibility });
      setMessage(`“${p.name || p.uuid}” is now ${visibility}.`);
    } catch (e) {
      setMessage(`Visibility change failed: ${String(e)}`);
    } finally {
      setMarketBusyUuid("");
      loadMarket();
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
    logInfo(`upload: starting "${ds.name}" (${ds.uuid}) type=${ds.dataset_type} visibility=${visibility}`, "datasets");
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
      logError(`upload: "${ds.name}" (${ds.uuid}) failed: ${text}`, "datasets");
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
            const text2 = String(e2);
            logError(`upload: remint "${ds.name}" (${ds.uuid}) failed: ${text2}`, "datasets");
            setMessage(`Upload failed: ${text2}`);
            if (isTauri()) void showDialog(text2, { title: "Upload failed", kind: "error" });
          }
        }
      } else {
        setMessage(`Upload failed: ${text}`);
        // The inline message is transient (and hidden in hub mode) — the dialog
        // guarantees the failure reason is actually seen.
        if (isTauri()) void showDialog(text, { title: "Upload failed", kind: "error" });
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
    // A push's byte bar: same shape as the download bar, its own row so a card
    // can show one while the other is idle.
    const pushRow = pushProgress[ds.uuid];
    const pushPct =
      pushRow && pushRow.total
        ? Math.min(100, Math.round(((pushRow.sent ?? 0) / pushRow.total) * 100))
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
              <ToolbarButton
                variant="solid"
                icon={Download}
                className="shrink-0"
                collapseOnMobile={false}
                onClick={() => handleAdopt(ds.uuid, ds.name)}
                disabled={!!busyUuid}
                title="Pull the hub's full copy (overwrites local)"
                label={busyUuid === ds.uuid ? "Downloading…" : "Download"}
              />
            )}
            {/* Evict frees the disk space of an adopted website copy (read-only
                base under download/) without unsubscribing — it flips to "Not
                downloaded" and the Download button above re-pulls it. Only the
                website origin adopts into the download root; a LAN mirror stays
                writable and is deleted from the Datasets page instead. */}
            {state === "downloaded" && isWebsite && (
              <ToolbarButton
                variant="ghost"
                icon={Archive}
                className="shrink-0"
                collapseOnMobile={false}
                onClick={() => handleEvict(ds.uuid, ds.name)}
                disabled={!!busyUuid}
                title="Free disk space — keeps it re-downloadable, progress untouched"
                label={busyUuid === ds.uuid ? "Evicting…" : "Evict"}
              />
            )}
            {/* Push sends this copy's whole directory up to the hub — the "I
                built it here, put it back on the hub" action. Needs a LAN hub to
                send to, and a writable local copy: the backend refuses a
                read-only download base with an actionable message. */}
            {!!pcUrl && (state === "downloaded" || state === "needs_resync") && (
              <ToolbarButton
                variant="ghost"
                icon={Upload}
                className="shrink-0"
                collapseOnMobile={false}
                onClick={() => handlePush(ds)}
                disabled={!!busyUuid || !!pushingUuid || pushingAll}
                title="Send this dataset's directory to the hub (only the files it is missing)"
                label={pushingUuid === ds.uuid ? "Pushing…" : "Push"}
              />
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

        {pushRow && (
          <div className="flex flex-col gap-1">
            <div className="h-1.5 w-full rounded bg-bg-hover overflow-hidden">
              <div
                className="h-full bg-accent-bg transition-[width]"
                style={{ width: `${pushPct}%` }}
              />
            </div>
            <span className="text-xs text-text-tertiary">
              {`Pushing to hub… ${pushPct}% (${fmtBytes(pushRow.sent ?? 0)} / ${fmtBytes(pushRow.total ?? 0)})`}
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
  // The same keys drive the Publish zone's local-dataset filter, so the section
  // tabs mean one thing across both zones.
  const sections: { key: string; label: string; items: PcDataset[] }[] = [
    { key: "dictation", label: "Dictation", items: pcDatasets.filter((d) => d.dataset_type === "dictation" || d.dataset_type === "") },
    { key: "card", label: "Cards", items: pcDatasets.filter((d) => d.dataset_type === "card") },
    { key: "book", label: "Books", items: pcDatasets.filter((d) => d.dataset_type === "book") },
    { key: "read_aloud", label: "Read", items: pcDatasets.filter((d) => d.dataset_type === "read_aloud") },
    { key: "wiki", label: "Wiki", items: pcDatasets.filter((d) => d.dataset_type === "wiki") },
  ];

  // Local datasets bucketed by type, mirroring the hub catalog. The dictation
  // bucket also catches "" for symmetry with the Pull sections.
  const typeMatches = (t: string, key: string) =>
    key === "dictation" ? t === "dictation" || t === "" : t === key;
  const activeSectionLabel =
    sections.find((s) => s.key === activeSection)?.label ?? sections[0].label;
  const publishLabel: Record<string, string> = {
    dictation: "dictation",
    card: "card",
    book: "book",
    read_aloud: "read-aloud",
    wiki: "wiki",
  };

  // Zone-switcher pills: a count on Pull when catalog copies are owed, an
  // accent dot on Publish while an upload is in flight (so a running upload is
  // visible from other zones), an amber dot on Status when the diagnostics
  // actually say something - including changes still queued, the one signal the
  // toolbar used to spell out before the compact hub status moved into Status.
  // All additive — they degrade to nothing.
  const pullPill = (status?.datasets ?? []).filter(
    (d) => d.state === "not_downloaded" || d.state === "needs_resync"
  ).length;
  const publishPill = !!uploadingUuid || Object.keys(uploadProgress).length > 0;
  const statusPill =
    !!status &&
    ((!!status.queue_diagnosis?.cause && status.queue_diagnosis.cause !== "idle") ||
      !!status.history_gap ||
      !!status.events_gap ||
      (!isHub && Math.max(status.queued_count, queued) > 0) ||
      lastRoundErrors > 0 ||
      num(lastRound?.dropped_pending) > 0);

  // The sync CTA is the page's one primary action, so it keeps a word even on
  // the phone. Stacked, that word is what sets the button's width, and "Push to
  // cloud" at text-xs would be the widest thing in the bar — so mobile gets the
  // short form and the tooltip/title still carries the full sentence.
  const syncLabel = mobile
    ? syncing
      ? "Syncing"
      : isHub
        ? "Push"
        : "Sync"
    : syncing
      ? "Syncing…"
      : isHub
        ? "Push to cloud"
        : "Sync now";

  // The underline section tabs shared by the Pull and Publish zones — the
  // dataset-type dimension, deliberately styled apart from the toolbar's
  // segmented zone switcher above it.
  function renderSectionTabs() {
    return (
      <div className="flex gap-1 border-b border-border-default">
        {sections.map((section) => (
          <button
            key={section.key}
            onClick={() => setActiveSection(section.key)}
            className={`flex flex-col items-center px-3 py-2 text-sm font-medium border-b-2 -mb-px transition-colors ${
              activeSection === section.key
                ? "border-accent-bg text-text-primary"
                : "border-transparent text-text-tertiary hover:text-text-secondary"
            }`}
          >
            {/* Five full-width names already push the phone row to scroll, so
                there the count rides on a line of its own under the label;
                desktop keeps it inline after the word, where it reads as the
                word's count. Same shape as the toolbar's zone switcher. */}
            <span>{section.label}</span>
            {section.items.length > 0 && (
              <span
                className={
                  mobile
                    ? "block text-xs opacity-70 leading-none"
                    : "ml-1 text-xs opacity-70"
                }
              >
                {section.items.length}
              </span>
            )}
          </button>
        ))}
      </div>
    );
  }

  // One label/value line of the Status zone's connection grid. Side by side
  // from `sm` up, but stacked on a phone: the 160px label column there leaves
  // the value barely half the card, which wraps a device id or hub address over
  // three lines. Width-driven (not platform-driven) so a tablet gets the
  // two-column reading and no first-paint flash is involved.
  function statusRow(label: string, value: React.ReactNode) {
    return (
      <div key={label} className="flex flex-col gap-0.5 sm:flex-row sm:items-start sm:gap-3">
        <span className="text-text-tertiary sm:w-40 sm:shrink-0">{label}</span>
        <span className="min-w-0 break-words">{value}</span>
      </div>
    );
  }

  // One tab of the toolbar's zone switcher: icon, word, and the badge that says
  // whether the zone is worth opening. Where the badge sits follows the layout —
  // beside the icon when the tab is stacked (its label is on a line of its own
  // below), after the word in the horizontal desktop row, where it reads as the
  // word's count.
  function zoneTab(opts: {
    zone: "pull" | "publish" | "status";
    icon: LucideIcon;
    label: string;
    title: string;
    badge?: React.ReactNode;
  }) {
    const { zone, icon: Icon, label, title, badge } = opts;
    const active = activeZone === zone;
    return (
      <button
        type="button"
        className={mobile ? tbToggleStacked(active) : tbToggle(active)}
        onClick={() => setActiveZone(zone)}
        title={title}
        aria-label={label}
      >
        <span className="flex items-center gap-1">
          <Icon size={TB_ICON} />
          {mobile && badge}
        </span>
        <span className="flex items-center gap-0.5">
          {label}
          {!mobile && badge}
        </span>
      </button>
    );
  }

  // Status zone: the connection detail + §19.2/§19.4 diagnostics that used to
  // sit permanently above the lists. Everything reads off the read-only
  // SyncStatusDetail; each line degrades to nothing when a stale backend
  // omits the field.
  function renderStatusZone() {
    if (!status) {
      return <p className="text-sm text-text-tertiary">Loading sync status…</p>;
    }
    const rows: React.ReactNode[] = [
      statusRow(
        "Hub",
        <span className="flex items-center gap-2 flex-wrap">
          <span
            className={`inline-block w-2 h-2 rounded-full ${
              status.hub.reachable ? "bg-emerald-500" : "bg-red-500"
            }`}
          />
          <span className="font-mono">{status.hub.address || "not configured"}</span>
          <span>{status.hub.reachable ? "reachable" : "unreachable"}</span>
        </span>
      ),
      statusRow("Hub kind", <span className="font-mono">{status.hub_kind || "—"}</span>),
      statusRow("Role", <span className="font-mono">{status.role}</span>),
      statusRow("Cluster", <span className="font-mono">{status.cluster_id || "—"}</span>),
      statusRow("Protocol", <span>{status.protocol_version}</span>),
      statusRow("Device ID", <span className="font-mono break-all">{status.device_id}</span>),
      statusRow(
        "Serves",
        <span>
          {status.hub.dataset_count != null ? `${status.hub.dataset_count} dataset(s)` : "—"}
          {" · "}
          {status.devices.length} paired device(s)
        </span>
      ),
    ];
    if (status.hub.error) {
      rows.push(statusRow("Last error", <span className="text-red-600 dark:text-red-400">{status.hub.error}</span>));
    }
    if (!isHub) {
      // Per-user practice data (dictation progress, XP) travels in its own
      // journal scope, not in a dataset snapshot — so it converges even when
      // nothing is subscribed. `null` = no app-data pull has landed yet.
      rows.push(
        statusRow(
          "App data",
          <>
            {status.app_cursor == null ? "not pulled yet" : `cursor ${status.app_cursor}`}
            {" · "}
            history {status.history_backfilled_at ? `imported ${status.history_backfilled_at}` : "not imported"}
          </>
        )
      );
    }
    return (
      <div className="flex flex-col gap-3">
        <div className="p-3 rounded-lg border border-border-default bg-bg-card text-xs flex flex-col gap-1.5">
          {rows}
          {onNavigate && (
            <button
              className="self-start mt-1 text-text-secondary hover:text-text-primary underline"
              onClick={() => onNavigate("devices-hub")}
            >
              Devices &amp; Hub →
            </button>
          )}
        </div>

        {/* Follower-only diagnostics — a hub serves, it does not pull. */}
        {!isHub && (
          <div className="flex flex-col gap-1.5 p-3 rounded-lg border border-border-default bg-bg-card text-xs">
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
          </div>
        )}

        {/* The last completed round's persisted result (§19.2) — shown for hubs
            too, whose "Push to cloud" rounds produce it. */}
        {lastRound && (
          <div className="flex items-center gap-2 flex-wrap text-xs text-text-tertiary">
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
    );
  }

  // Publish zone: local datasets of the active section are the upload
  // candidates. Gated on canPublish (a logged-in account), not `isWebsite` —
  // the backend upload/publish commands all target `auth::base_url()`
  // directly, so any signed-in workspace (follower or hub) can push its own
  // datasets to its cloud account.
  function renderPublishZone() {
    const items = localDatasets.filter((ds) => typeMatches(ds.dataset_type, activeSection));
    return (
      <div className="flex flex-col gap-4">
        <p className="text-xs text-text-tertiary">
          Subscribing to a shared dataset happens on this page — sign in and open the Pull
          tab's market (Browse section) — or on the website itself; a shared dataset joins
          your catalog only after you subscribe to it.
        </p>
        {renderSectionTabs()}
        {items.length === 0 ? (
          <p className="text-sm text-text-tertiary px-1">
            No {publishLabel[activeSection] ?? activeSectionLabel.toLowerCase()} datasets on this machine.
          </p>
        ) : (
          items.map((ds) => {
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
                  <ToolbarButton
                    variant="solid"
                    icon={Upload}
                    className="shrink-0"
                    collapseOnMobile={false}
                    onClick={() => openUploadDialog(ds)}
                    disabled={!!uploadingUuid}
                    title="Pack this local dataset and upload it to the website hub"
                    label={uploadingUuid === ds.uuid ? "Uploading…" : done ? "Re-upload" : "Upload"}
                  />
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
                            className={tbGhost}
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
                            className={tbSolid}
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
    );
  }

  // ---- Online market rendering (the Pull tab's website branch) ----------

  // Site-convention colors for the visibility chip: private muted, shared amber
  // (only subscribed accounts can pull), public green (open to everyone).
  const VIS_META: Record<string, { label: string; cls: string }> = {
    private: { label: "Private", cls: "bg-bg-hover text-text-secondary" },
    shared: { label: "Shared", cls: "bg-amber-500/15 text-amber-600 dark:text-amber-400" },
    public: { label: "Public", cls: "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400" },
  };

  const MARKET_TYPE_CHIPS = ["dictation", "card", "book", "read_aloud", "wiki"];

  function fmtBytes(n?: number | null): string {
    if (n == null) return "";
    if (n >= 1048576) return `${(n / 1048576).toFixed(1)} MB`;
    if (n >= 1024) return `${Math.round(n / 1024)} KB`;
    return `${n} B`;
  }

  function marketSizeLabel(d: MarketDataset | MarketDownload): string {
    const bits: string[] = [];
    if ("card_count" in d && d.card_count != null) bits.push(`${d.card_count} cards`);
    else if ("media_count" in d && d.media_count != null) bits.push(`${d.media_count} media`);
    if (d.file_count != null) bits.push(`${d.file_count} file(s)`);
    const b = fmtBytes("total_bytes" in d ? d.total_bytes : "bytes" in d ? d.bytes : null);
    if (b) bits.push(b);
    return bits.join(" · ");
  }

  // One Browse/My Published card. The Download/Evict wiring rides the same
  // commands and progress events as the LAN pull flow — single-sourced.
  function renderMarketCard(d: MarketDataset) {
    const prog = progress[d.uuid];
    const pct =
      prog && prog.total > 0 ? Math.min(100, Math.round((prog.received / prog.total) * 100)) : 0;
    const meta = STATE_META[d.sync_state];
    const busy = marketBusyUuid === d.uuid;
    const vis = d.visibility ? VIS_META[d.visibility] : null;
    return (
      <div key={d.uuid} className="flex flex-col gap-2 p-3 rounded-lg border border-border-default bg-bg-card">
        <div className="flex items-start gap-3">
          <div className="flex flex-col min-w-0 flex-1 gap-1">
            <span className="font-medium truncate">{d.name}</span>
            <span className="text-xs text-text-tertiary flex items-center gap-1.5 flex-wrap">
              {vis && <span className={`px-1.5 py-0.5 rounded-full ${vis.cls}`}>{vis.label}</span>}
              {d.owner && <span className="font-mono max-w-[180px] truncate">{d.owner}</span>}
              {d.language && <span>{d.language}</span>}
              {d.published_version != null && <span className="font-mono">v{d.published_version}</span>}
              {marketSizeLabel(d)}
              {d.updated && <span>· {d.updated}</span>}
            </span>
            {d.description && (
              <span className="text-xs text-text-secondary line-clamp-2">{d.description}</span>
            )}
            {(d.tags ?? []).length > 0 && (
              <span className="flex items-center gap-1 flex-wrap">
                {(d.tags ?? []).slice(0, 6).map((t) => (
                  <span key={t.uuid} className="px-1.5 py-0.5 rounded bg-bg-hover text-text-tertiary text-xs">
                    {t.name}
                  </span>
                ))}
              </span>
            )}
            {d.withdrawn && (
              <span className="text-xs text-amber-600 dark:text-amber-400">withdrawn upstream — your copy stays practicable</span>
            )}
          </div>
          <div className="flex flex-col items-end gap-2 shrink-0">
            <span className={`text-xs px-2 py-0.5 rounded-full shrink-0 ${meta.cls}`}>{meta.label}</span>
            {/* Owner-only visibility toggle (My Published): the website-side
                manage control, mirrored; hidden in lite-degraded mode. */}
            {d.is_owner && market?.market_available && d.visibility && (
              <select
                className="text-xs px-2 py-1 rounded-md border border-border-default bg-bg-body text-text-primary"
                value={d.visibility}
                disabled={busy}
                onChange={(e) => changeVisibility(d, e.target.value)}
                title="Who can see and pull this dataset"
              >
                <option value="private">private</option>
                <option value="shared">shared</option>
                <option value="public">public</option>
              </select>
            )}
            {d.sync_state !== "downloaded" && (
              <ToolbarButton
                variant="solid"
                icon={Download}
                className="shrink-0"
                collapseOnMobile={false}
                onClick={() => marketDownload(d.uuid, d.name)}
                disabled={!!marketBusyUuid || !!busyUuid}
                title="Download into the read-only library — evictable, re-downloadable, progress stays local"
                label={busy ? "Downloading…" : "Download"}
              />
            )}
            {d.sync_state === "downloaded" && (
              <ToolbarButton
                variant="ghost"
                icon={Archive}
                className="shrink-0"
                collapseOnMobile={false}
                onClick={() => marketEvict(d.uuid, d.name)}
                disabled={!!marketBusyUuid || !!busyUuid}
                title="Free disk space — keeps it re-downloadable, progress untouched"
                label={busy ? "Evicting…" : "Evict"}
              />
            )}
            {market?.can_subscribe && !d.is_owner && d.visibility === "shared" && (
              <button className={tbGhost} onClick={() => toggleSubscribe(d)} disabled={busy}>
                {busy ? <Loader2 size={12} className="animate-spin" /> : null}
                {d.is_subscribed ? "Unsubscribe" : "Subscribe"}
              </button>
            )}
          </div>
        </div>
        {prog && prog.phase !== "done" && (
          <div className="flex flex-col gap-1">
            <div className="h-1.5 w-full rounded bg-bg-hover overflow-hidden">
              <div
                className="h-full bg-accent-bg transition-[width]"
                style={{ width: prog.phase === "extract" ? "100%" : `${pct}%` }}
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

  // My Downloads: every locally adopted registry copy (from the app DB, not
  // the catalog), so a hub user sees at a glance what the cloud gave them —
  // including copies whose publisher withdrew them.
  function renderMarketDownloads() {
    const items = market?.downloads ?? [];
    if (items.length === 0) {
      return <p className="text-sm text-text-tertiary px-1">Nothing downloaded from the market yet.</p>;
    }
    return (
      <div className="flex flex-col gap-2">
        {items.map((d) => {
          const meta = STATE_META[d.sync_state];
          const busy = marketBusyUuid === d.uuid;
          return (
            <div key={d.uuid} className="flex items-center gap-3 p-3 rounded-lg border border-border-default bg-bg-card">
              <div className="flex flex-col min-w-0 flex-1">
                <span className="font-medium truncate">{d.name}</span>
                <span className="text-xs text-text-tertiary">
                  {d.published_version != null && <>v{d.published_version} · </>}
                  {d.on_disk ? `${fmtBytes(d.bytes)} · ${d.file_count ?? 0} file(s)` : "not on disk"}
                  {d.synced_at && <> · synced {d.synced_at}</>}
                </span>
                {d.withdrawn && (
                  <span className="text-xs text-amber-600 dark:text-amber-400">withdrawn upstream — kept, still practicable</span>
                )}
              </div>
              <span className={`text-xs px-2 py-0.5 rounded-full shrink-0 ${meta.cls}`}>{meta.label}</span>
              <div className="flex items-center gap-2 shrink-0">
                {d.sync_state === "needs_resync" && d.on_disk && (
                  <ToolbarButton
                    variant="solid"
                    icon={Download}
                    className="shrink-0"
                    collapseOnMobile={false}
                    onClick={() => marketDownload(d.uuid, d.name)}
                    disabled={!!marketBusyUuid || !!busyUuid}
                    title="Pull the newly published version over the installed copy"
                    label={busy ? "Updating…" : "Update"}
                  />
                )}
                {d.on_disk ? (
                  <ToolbarButton
                    variant="ghost"
                    icon={Archive}
                    className="shrink-0"
                    collapseOnMobile={false}
                    onClick={() => marketEvict(d.uuid, d.name)}
                    disabled={!!marketBusyUuid || !!busyUuid}
                    title="Free disk space — keeps it re-downloadable, progress untouched"
                    label="Evict"
                  />
                ) : (
                  <ToolbarButton
                    variant="solid"
                    icon={Download}
                    className="shrink-0"
                    collapseOnMobile={false}
                    onClick={() => marketDownload(d.uuid, d.name)}
                    disabled={!!marketBusyUuid || !!busyUuid}
                    title="Re-download the adopted copy"
                    label={busy ? "Downloading…" : "Download"}
                  />
                )}
              </div>
            </div>
          );
        })}
      </div>
    );
  }

  // My Published: this account's shared datasets (the host's own `is_owner`
  // partition), with the visibility toggle and the subscribe/pull state —
  // plus, below, what this device uploaded that the catalog cannot show. New
  // archives still go through the Publish zone; this is the view of what the
  // cloud already holds.
  function renderMarketPublished(own: MarketDataset[]) {
    const extra = (market?.published ?? []).filter(
      (p) => !own.some((d) => d.uuid === p.uuid)
    );
    if (own.length === 0 && extra.length === 0) {
      return (
        <p className="text-sm text-text-tertiary px-1">
          Nothing shared under this account yet — upload an archive in the Publish zone, then publish it.
        </p>
      );
    }
    return (
      <div className="flex flex-col gap-2">
        {own.map((d) => renderMarketCard(d))}
        {extra.map((p) => {
          const busy = marketBusyUuid === p.uuid;
          // Type/name come from the catalog when it still lists the row and
          // from the local info.json otherwise (`market_list` falls back), so
          // the uuid only shows as a last resort.
          const typeLabel = p.dataset_type ? publishLabel[p.dataset_type] ?? p.dataset_type : null;
          const vis = p.visibility ? VIS_META[p.visibility] : null;
          return (
            <div key={p.uuid} className="flex items-center gap-3 p-3 rounded-lg border border-border-default bg-bg-card">
              <div className="flex flex-col min-w-0 flex-1 gap-1">
                <span className="font-medium truncate">{p.name || "Unnamed dataset"}</span>
                <span className="text-xs text-text-tertiary flex items-center gap-1.5 flex-wrap">
                  {typeLabel && (
                    <span className="px-1.5 py-0.5 rounded-full bg-bg-hover text-text-secondary">{typeLabel}</span>
                  )}
                  {vis && <span className={`px-1.5 py-0.5 rounded-full ${vis.cls}`}>{vis.label}</span>}
                  <span>uploaded v{p.version} ({p.uploaded_at})</span>
                  <span>{p.current_version != null ? `published v${p.current_version}` : "not published"}</span>
                  {!p.name && <span className="font-mono max-w-[180px] truncate">{p.uuid}</span>}
                </span>
              </div>
              {busy && <Loader2 size={14} className="animate-spin shrink-0" />}
            </div>
          );
        })}
      </div>
    );
  }

  // The website branch of the Pull tab: the market surface with three
  // sections, rendered for every role. Download/Evict wiring stays
  // single-sourced with the LAN flow (same commands, same events).
  function renderMarketZone() {
    if (marketError) {
      const tokenish = /sign in|401|access token|not logged in/i.test(marketError);
      return (
        <div className="flex flex-col gap-2">
          <div className="px-3 py-2 text-sm rounded-md bg-red-100 text-red-700 dark:bg-red-900/30 dark:text-red-300">
            {marketError}
            {tokenish && (
              <button
                className="ml-2 inline-flex items-center gap-1 underline font-medium"
                onClick={() => invoke("auth_open_login").catch((err) => setMessage(`Sign-in failed: ${String(err)}`))}
              >
                <LogIn size={14} />
                Sign in
              </button>
            )}
          </div>
          <button type="button" className={tbGhost + " self-start"} onClick={loadMarket}>
            <RefreshCw size={14} /> Retry
          </button>
        </div>
      );
    }
    if (!market) {
      return (
        <p className="text-sm text-text-tertiary flex items-center gap-2">
          {marketLoading && <Loader2 size={14} className="animate-spin" />}
          Loading the market…
        </p>
      );
    }
    // Section partition is purely client-side (`is_owner` carries through
    // unfiltered): Browse = what others share, My Published = this account's
    // own rows. `q` is server-side only in market mode; lite rows filter here.
    const q = marketQ.toLowerCase();
    const browse = market.datasets.filter(
      (d) =>
        !d.is_owner &&
        (!market.lite ||
          !q ||
          (d.name ?? "").toLowerCase().includes(q) ||
          (d.description ?? "").toLowerCase().includes(q))
    );
    const own = market.datasets.filter((d) => d.is_owner);
    const tagPool = Array.from(
      new Map(market.datasets.flatMap((d) => d.tags ?? []).map((t) => [t.uuid, t])).values()
    );
    const zoneSections: { key: "browse" | "downloads" | "published"; label: string; count: number }[] = [
      { key: "browse", label: "Browse", count: browse.length },
      { key: "downloads", label: "My Downloads", count: market.downloads.length },
      // `published` overlaps `own` (an uploaded dataset the catalog also lists),
      // so count only the rows that add a card: `renderMarketPublished` splits
      // the same way.
      {
        key: "published",
        label: "My Published",
        count:
          own.length +
          (market.published ?? []).filter((p) => !own.some((d) => d.uuid === p.uuid)).length,
      },
    ];
    return (
      <div className="flex flex-col gap-4">
        <div className="flex items-center gap-2 flex-wrap text-xs text-text-tertiary">
          <span>
            Market · <span className="font-mono">{market.origin}</span>
          </span>
          {marketLoading && <Loader2 size={14} className="animate-spin" />}
        </div>
        {market.lite && (
          <p className="px-3 py-2 text-xs rounded-md bg-amber-500/15 text-amber-700 dark:text-amber-400">
            Basic mode — {market.degraded_reason || "the website market API is not available yet"}. Descriptions,
            tags, subscriptions and visibility controls need the website to ship its market routes; downloading
            shared/public datasets works already.
          </p>
        )}

        {/* Market sections — the same underline style as the dataset-type
            tabs, but this dimension is the surface, not the type. */}
        <div className="flex gap-1 border-b border-border-default">
          {zoneSections.map((s) => (
            <button
              key={s.key}
              onClick={() => setMarketSection(s.key)}
              className={`px-3 py-2 text-sm font-medium border-b-2 -mb-px transition-colors ${
                marketSection === s.key
                  ? "border-accent-bg text-text-primary"
                  : "border-transparent text-text-tertiary hover:text-text-secondary"
              }`}
            >
              {s.label}
              {s.count > 0 && <span className="ml-1 text-xs opacity-70">{s.count}</span>}
            </button>
          ))}
        </div>

        {marketSection === "browse" && (
          <>
            <div className="flex items-center gap-2 flex-wrap">
              {MARKET_TYPE_CHIPS.map((t) => (
                <button
                  key={t}
                  type="button"
                  className={`px-2 py-1 text-xs rounded-full border transition-colors ${
                    marketTypes.includes(t)
                      ? "border-accent-bg bg-bg-hover text-text-primary"
                      : "border-border-default text-text-tertiary hover:text-text-secondary"
                  }`}
                  onClick={() =>
                    setMarketTypes((prev) =>
                      prev.includes(t) ? prev.filter((x) => x !== t) : [...prev, t]
                    )
                  }
                >
                  {publishLabel[t] ?? t}
                </button>
              ))}
              {tagPool.length > 0 && (
                <select
                  className="text-xs px-2 py-1 rounded-md border border-border-default bg-bg-body text-text-primary"
                  value={marketTag}
                  onChange={(e) => setMarketTag(e.target.value)}
                >
                  <option value="">All tags</option>
                  {tagPool.map((t) => (
                    <option key={t.uuid} value={t.uuid}>{t.name}</option>
                  ))}
                </select>
              )}
              <input
                type="search"
                placeholder={market.lite ? "Search (name)…" : "Search…"}
                className="text-xs px-2 py-1 rounded-md border border-border-default bg-bg-body text-text-primary w-40"
                onKeyDown={(e) => {
                  if (e.key === "Enter") setMarketQ((e.currentTarget.value ?? "").trim());
                }}
              />
              {marketQ && (
                <button className="text-xs underline text-text-tertiary" onClick={() => setMarketQ("")}>
                  clear “{marketQ}”
                </button>
              )}
            </div>
            {browse.length === 0 ? (
              <p className="text-sm text-text-tertiary px-1">
                Nothing shared by others matches these filters.
              </p>
            ) : (
              <div className="flex flex-col gap-2">{browse.map((d) => renderMarketCard(d))}</div>
            )}
          </>
        )}

        {marketSection === "downloads" && renderMarketDownloads()}

        {marketSection === "published" && (
          <>
            {market.market_available && own.length > 0 && (
              <p className="text-xs text-text-tertiary">
                The visibility toggle decides who can pull these — private stays owner-only, shared needs a
                subscription, public is downloadable by everyone.
              </p>
            )}
            {renderMarketPublished(own)}
          </>
        )}
      </div>
    );
  }

  // Pull zone, source-aware. Website origin (or a signed-in account with no
  // pc_url): the online market, rendered for every role — a hub may browse,
  // subscribe and download from the cloud while staying the authority for its
  // LAN followers (the owned-locally guard protects what it serves). A LAN
  // pc_url: today's follower-only hub catalog; there the hub notice means "a
  // hub serves, it does not pull **from a PC**".
  function renderPullZone() {
    if (marketBranch) return renderMarketZone();
    if (isHub) {
      return (
        <div className="flex flex-col items-center justify-center gap-2 p-4 text-center rounded-lg border border-border-default bg-bg-card">
          <ServerCog size={28} className="text-text-tertiary" />
          <p className="text-text-secondary max-w-md">
            This workspace is the sync <span className="font-semibold">hub</span>. It serves
            datasets to follower devices — the pull controls are hidden (only
            followers pull <em>from a PC</em>).
          </p>
          <p className="text-sm text-text-tertiary max-w-md">
            To pull datasets <em>into</em> this machine from the cloud, use your account: sign in
            (Settings › Account) and clear the PC address, and this tab becomes the online market
            — a hub can download shared datasets without giving up its LAN role. To sync from
            another PC, designate this machine as a follower on the Devices &amp; Hub page first.
            Publishing to your cloud account (in the Publish
            zone, when you're signed in) works either way —{canPublish ? " and 'Push to cloud' in the toolbar drains this hub's local edits directly to it." : " sign in via Settings › Account to enable that."}
          </p>
          {onNavigate && (
            <button
              className={tbSolid + " mt-1"}
              onClick={() => onNavigate("devices-hub")}
            >
              Open Devices &amp; Hub
            </button>
          )}
        </div>
      );
    }
    if (!mounted) return null;
    if (!isTauri()) {
      return <p className="text-text-secondary">Datasets Sync is only available in the app.</p>;
    }
    if (!pcUrl) {
      return (
        <div className="flex flex-col items-center gap-2 py-12 text-text-secondary">
          <CloudOff size={32} />
          <p>Not connected to a hub.</p>
          <p className="text-sm">Connect and pair with a hub on the Devices &amp; Hub page. On the other machine, enable the web service in Settings so it can be found.</p>
          {onNavigate && (
            <button
              className={tbSolid + " mt-1"}
              onClick={() => onNavigate("devices-hub")}
            >
              Open Devices &amp; Hub
            </button>
          )}
          {/* No hub configured and no account: the market needs a sign-in —
              say so rather than leaving the option implicit. */}
          <button
            className={tbGhost + " mt-1"}
            onClick={() => invoke("auth_open_login").catch((err) => setMessage(`Sign-in failed: ${String(err)}`))}
          >
            <LogIn size={14} />
            Sign in to browse the online market
          </button>
        </div>
      );
    }
    return (
      <div className="flex flex-col gap-4">
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
        {pcDatasets.length === 0 && !loadingList && (
          <p className="text-sm text-text-tertiary">No datasets available on the source PC.</p>
        )}
        {pcDatasets.length > 0 && (
          <>
            {/* Section tabs: Dictation / Cards / Books / Read / Wiki. Only the
                active section's datasets are listed. */}
            {renderSectionTabs()}
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
                <ToolbarButton
                  variant="ghost"
                  icon={Trash2}
                  className="shrink-0"
                  collapseOnMobile={false}
                  onClick={() => handleForgetDataset(d.dataset_uuid, d.name)}
                  title="Remove this local copy"
                  label="Remove local copy"
                />
              </div>
            ))}
          </div>
        )}
      </div>
    );
  }

  // Zone dispatch. Publish only exists with a signed-in account — if auth
  // drops away while it is open, fall back to Pull rather than an empty page.
  function renderZone() {
    if (activeZone === "status") return renderStatusZone();
    if (activeZone === "publish" && canPublish) return renderPublishZone();
    return renderPullZone();
  }

  return (
    <div className="flex flex-col w-full h-full min-h-0 bg-bg-body">
      <PageHeader title="Datasets Sync" />
      {/* Toolbar row: the segmented zone switcher (mode, not dataset-type — the
          underline section tabs below keep that dimension) and the page-level
          actions. Replaces the old h1 header, the always-on status strip, and
          the inline diagnostics block; connection detail lives in Status. */}
      <div className="shrink-0 flex items-center gap-2 px-4 py-2 border-b border-border-default bg-bg-card overflow-x-auto">
        {/* Zone switcher. On a phone each tab stacks icon-over-label instead of
            putting them side by side: the bar then fits every name in the width
            a desktop row would not, at the cost of a little height. Each tab's
            badge rides with the icon there, and with the word on desktop. */}
        <div className="flex items-center gap-1 shrink-0">
          {zoneTab({
            zone: "pull",
            icon: Download,
            label: "Pull",
            title: "Pull — datasets offered by the hub",
            badge:
              pullPill > 0 ? (
                <span className="px-1.5 rounded-full bg-amber-500/20 text-amber-600 dark:text-amber-400 text-xs">
                  {pullPill}
                </span>
              ) : undefined,
          })}
          {canPublish &&
            zoneTab({
              zone: "publish",
              icon: Upload,
              label: "Publish",
              title: "Publish — this device's own uploads",
              badge: publishPill ? <span className="w-2 h-2 rounded-full bg-white" /> : undefined,
            })}
          {zoneTab({
            zone: "status",
            icon: ServerCog,
            label: "Status",
            title: "Status — connection and last rounds",
            badge: statusPill ? <span className="w-2 h-2 rounded-full bg-amber-500" /> : undefined,
          })}
        </div>

        <div className="ml-auto flex items-center gap-1 shrink-0">
          {/* Page-level "push everything I built": every writable local dataset in
              one batch. A follower-only action — a hub is the destination, and a
              website hub takes the Publish zone's archive upload instead. */}
          {!isHub && !!pcUrl && (
            <ToolbarButton
              variant="ghost"
              icon={pushingAll ? Loader2 : Upload}
              iconProps={{ className: pushingAll ? "animate-spin" : undefined }}
              stacked={mobile}
              label={pushingAll ? "Pushing…" : "Push all"}
              title="Push every dataset built on this device to the hub (only the files it is missing)"
              disabled={pushingAll || !!pushingUuid || syncing}
              onClick={handlePushAll}
            />
          )}
          {/* One round moves everything it can in both directions (writeback
              push + pull deltas + step-7 mirror/file drain). Shown for followers
              always (disabled until a hub is configured), and for any Hub with a
              pc_url OR a logged-in account — an auth-only Hub triggers the
              cloud-only branch of sync_round_inner against auth::base_url(). A
              pure-LAN hub with no upstream and no sign-in is the one case a
              round is meaningless, so we hide it rather than show a dead button.
              Kept labelled on mobile, under its icon — it is the page's one
              primary action, so a bare arrow would leave the word out. */}
          {(!isHub || pcUrl || canPublish) && (
            <ToolbarButton
              variant="solid"
              icon={syncing ? Loader2 : ArrowUpDown}
              iconProps={{ className: syncing ? "animate-spin" : undefined }}
              stacked={mobile}
              label={syncLabel}
              title={isHub ? "Run a sync round: drain this hub's journal + changed files to the cloud" : "Run one incremental sync round (push + pull)"}
              disabled={syncing || (!isHub && !pcUrl)}
              onClick={handleSyncNow}
            />
          )}
        </div>
      </div>

      {/* Transient round/action result — one full-width notice line so it stays
          visible whichever zone is open. */}
      {message && (
        <div className="shrink-0 px-4 py-1.5 text-xs border-b border-border-default bg-bg-card truncate" title={message}>
          {message}
        </div>
      )}

      {/* The active zone's body in one padded scroll area. */}
      <div className="flex-1 min-h-0 overflow-y-auto p-4">
        {renderZone()}
      </div>
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
