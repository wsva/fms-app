//! Cross-machine sync: the PC web service, the paired-device registry, and the
//! Datasets Sync page as tools (scan/connect/status/pull plus the v2 incremental path).

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use serde::Deserialize;
use tauri::{Emitter, Manager};

use crate::settings::SettingsState;
use crate::sync;
use crate::sync::server::{WebServiceConfig, WebServiceState};

use super::{DatasetMcpServer, UuidParam};

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WebServiceStartParam {
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default = "default_true")]
    stt: bool,
    #[serde(default = "default_true")]
    dataset: bool,
    #[serde(default = "default_true")]
    tts: bool,
}

fn default_port() -> u16 { 35711 }

fn default_true() -> bool { true }

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct PairingDeviceParam {
    device_id: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct SyncHandoverParam {
    /// Journal scope to hand over: a dataset uuid, or `"@app"` for the app-wide
    /// per-user scope. Raises this hub's authority epoch for that scope by one.
    scope: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct SyncHandoverCloudParam {
    /// Dataset uuid (the scope) to hand between this desktop hub and the cloud.
    uuid: String,
    /// Target role on the cloud: `"authority"` (default) hands the scope TO the cloud;
    /// `"mirror"` hands it back to this hub. Required-explicit on the website, so an
    /// unknown value is refused rather than defaulted.
    #[serde(default)]
    role: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct PcScanParam {
    /// Discovery budget in milliseconds (0 = the default 2500).
    #[serde(default)]
    timeout_ms: u64,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct PcUrlParam {
    /// Source machine base URL, e.g. "http://192.168.1.20:35711". Empty disconnects.
    pc_url: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct SyncChangesParam {
    /// Dataset uuid whose hub change log to read, or `"@app"` for the app-wide
    /// scope holding per-user dictation progress + XP (`GET /api/v1/app/changes`).
    uuid: String,
    /// Return entries with seq > this value (-1 = from the beginning).
    #[serde(default)]
    after: i64,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct SyncFileParam {
    /// Dataset uuid on the hub.
    uuid: String,
    /// Path of one non-DB file, relative to the dataset dir (e.g. "subtitle/a.vtt").
    path: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct HubUploadParam {
    /// Local dataset uuid to pack and upload.
    dataset_uuid: String,
    /// `private|shared|public` — honoured only on the uuid's first upload.
    #[serde(default)]
    visibility: Option<String>,
    /// Tag uuids to attach on the hub (website-side tags, not local ones).
    #[serde(default)]
    tag_uuids: Option<Vec<String>>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct HubKindParam {
    /// Candidate hub address to classify. Empty falls back to the saved pc_url.
    #[serde(default)]
    pc_url: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct MirrorPushParam {
    /// One dataset uuid to mirror; omitted (or empty) pushes every dataset scope
    /// with journal rows.
    #[serde(default)]
    scope: Option<String>,
    /// Marks an auto-cadence call: skip silently when no account is logged in
    /// instead of erroring. Omitted/false = a normal manual push.
    #[serde(default)]
    auto: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct InboxDrainParam {
    /// One dataset uuid to drain; omitted (or empty) drains every dataset scope
    /// the hub has journaled for (the same set the mirror pusher iterates).
    #[serde(default)]
    scope: Option<String>,
    /// Marks an auto-cadence call: skip silently when no account is logged in
    /// instead of erroring. Omitted/false = a normal manual drain.
    #[serde(default)]
    auto: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct PublishParam {
    /// Local dataset uuid already uploaded to the website.
    uuid: String,
    /// Which uploaded version to pin (positive integer). Omitted = the newest
    /// uploaded one.
    #[serde(default)]
    version: Option<i64>,
    /// `private|shared|public` — applied in the same publish. Omitted = unchanged.
    #[serde(default)]
    visibility: Option<String>,
    /// Tag uuids to attach (website-side tags). Omitted/empty = unchanged.
    #[serde(default)]
    tag_uuids: Option<Vec<String>>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct AvailabilityParam {
    /// Dataset uuid whose published version to mark.
    uuid: String,
    /// The published version to take down or restore (positive integer).
    version: i64,
    /// `false` = withdraw (§11.5 takedown: fresh reads of this version then 410,
    /// bytes kept); `true` = re-list. Required-explicit, never defaulted.
    available: bool,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct PublishStateParam {
    /// Local dataset uuid whose cloud publish state to read back.
    dataset_uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct PushFilesParam {
    /// Local dataset uuid whose live file tree to re-push (must already exist on
    /// the website — bootstrap with hub_upload first).
    dataset_uuid: String,
    /// Marks an auto-cadence call: skip silently when no account is logged in
    /// instead of erroring. Omitted/false = a normal manual push.
    #[serde(default)]
    auto: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct MarketListParam {
    /// Dataset types to filter on (dictation, card, book, read_aloud, wiki).
    #[serde(default)]
    types: Option<Vec<String>>,
    /// Tag uuid to restrict the catalog to (website-side tags).
    #[serde(default)]
    tag: Option<String>,
    /// Free-text search; server-side when the host advertises the market
    /// capability, ignored in degraded lite mode.
    #[serde(default)]
    q: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct MarketVisibilityParam {
    /// Market dataset uuid whose visibility to change (must be yours).
    uuid: String,
    /// `private|shared|public` — the website vocabulary.
    visibility: String,
}

#[tool_router(router = sync_router, vis = "pub(crate)")]
impl DatasetMcpServer {
    #[tool(name = "web_service_get_status", description = "Get the web service status: whether it's running, port, enabled features, and URLs.")]
    async fn web_service_get_status(&self) -> String {
        log::info!("[MCP] web_service_get_status");
        let state = self.app.state::<WebServiceState>();
        let status = state.build_status();
        serde_json::to_string_pretty(&status).unwrap_or_default()
    }

    #[tool(name = "web_service_start", description = "Start the web service (HTTP server) with the given config. Default port is 35711. Enables STT, dataset, and TTS endpoints by default.")]
    async fn web_service_start(&self, Parameters(param): Parameters<WebServiceStartParam>) -> Result<String, String> {
        log::info!("[MCP] web_service_start: port={}", param.port);
        let state = self.app.state::<WebServiceState>();
        let config = WebServiceConfig {
            port: param.port,
            stt: param.stt,
            dataset: param.dataset,
            tts: param.tts,
            api_token: String::new(),
        };
        let status = sync::server::web_service_start(self.app.clone(), state, config).await?;
        Ok(serde_json::to_string_pretty(&status).unwrap_or_default())
    }

    #[tool(name = "web_service_stop", description = "Stop the web service.")]
    async fn web_service_stop(&self) -> Result<String, String> {
        log::info!("[MCP] web_service_stop");
        let state = self.app.state::<WebServiceState>();
        let status = sync::server::web_service_stop(state).await?;
        Ok(serde_json::to_string_pretty(&status).unwrap_or_default())
    }

    #[tool(name = "sync_handover_epoch", description = "Manually hand one journal scope over to a new authority (sync-v2.md INV-13): raise this hub's authority epoch for the scope by one and return it. Every follower still presenting the old epoch is refused on write until it re-pulls and learns the bump. This is the split-brain guard, and it is NEVER automatic — call it only when you intentionally reset/reassign authority for the scope (`scope` = a dataset uuid or `@app`). Hub-only.")]
    async fn sync_handover_epoch(&self, Parameters(param): Parameters<SyncHandoverParam>) -> Result<String, String> {
        log::info!("[MCP] sync_handover_epoch: scope={}", param.scope);
        let settings = self.app.state::<SettingsState>();
        let new_epoch = sync::server::sync_handover_epoch(settings, param.scope.clone())?;
        Ok(serde_json::json!({ "scope": param.scope, "epoch": new_epoch }).to_string())
    }

    #[tool(name = "sync_handover_to_cloud", description = "Hand one dataset scope's journal authority between this desktop hub and the cloud (sync-v2.md §9 flow 8 / INV-13) — the export half of the website's POST /api/v1/handover. `role='authority'` (default) hands the scope TO the cloud; `'mirror'` hands it back. It FIRST fully mirrors the scope's journal to the cloud (aborting an `authority` handover if rows are still refused/unpushed — INV-1 continuity), THEN flips the role, THEN records the epoch the cloud echoes back. NEVER automatic (INV-13 forbids failover) and hub-only; needs a logged-in account. Returns { changed, journal_role, journal_epoch, export }.")]
    async fn sync_handover_to_cloud(&self, Parameters(param): Parameters<SyncHandoverCloudParam>) -> Result<String, String> {
        log::info!("[MCP] sync_handover_to_cloud: uuid={}, role={:?}", param.uuid, param.role);
        let v = sync::mirror::sync_handover_to_cloud(
            self.app.state::<SettingsState>(),
            param.uuid,
            param.role,
        )
        .await?;
        Ok(v.to_string())
    }

    #[tool(name = "pairing_list_devices", description = "List devices paired with this PC's web service (device_id, name, approved/denied status, created/last-seen times). Requests from localhost and Tailscale always bypass pairing.")]
    async fn pairing_list_devices(&self) -> Result<String, String> {
        log::info!("[MCP] pairing_list_devices");
        let settings = self.app.state::<SettingsState>();
        let devices = crate::sync::pairing::list_devices(&settings)?;
        Ok(serde_json::to_string_pretty(&devices).unwrap_or_default())
    }

    #[tool(name = "pairing_revoke_device", description = "Revoke a paired device so its future requests are rejected until it pairs again. Pass the device_id from pairing_list_devices.")]
    async fn pairing_revoke_device(&self, Parameters(param): Parameters<PairingDeviceParam>) -> Result<String, String> {
        log::info!("[MCP] pairing_revoke_device: {}", param.device_id);
        let settings = self.app.state::<SettingsState>();
        crate::sync::pairing::revoke(&settings, &param.device_id)?;
        Ok(serde_json::json!({ "status": "ok", "revoked": param.device_id }).to_string())
    }

    #[tool(name = "pc_sync_scan", description = "Scan the LAN (and tailnet) for other FmS machines that expose the dataset web service. Returns candidates as { url, source, name, machine, role, cluster_id }. `machine` is the peer's hostname — the only name that distinguishes two rows, since `name` is the app package name (identical everywhere) and may be empty for peers predating it; present candidates as `machine` + `url`, falling back to `url` alone. Machines that have no web service running cannot be found.")]
    async fn pc_sync_scan(&self, Parameters(param): Parameters<PcScanParam>) -> Result<String, String> {
        let timeout_ms = if param.timeout_ms == 0 { 2500 } else { param.timeout_ms };
        log::info!("[MCP] pc_sync_scan: timeout_ms={}", timeout_ms);
        let candidates = crate::sync::discover::pc_discover(self.app.state::<SettingsState>(), timeout_ms).await?;
        Ok(serde_json::json!({
            "status": if candidates.is_empty() { "no_peers_found" } else { "ok" },
            "candidates": candidates,
            "next": if candidates.is_empty() {
                "Enable the web service in Settings on the other machine, then scan again."
            } else {
                "Call pc_sync_connect with the url you want to sync from."
            }
        })
        .to_string())
    }

    #[tool(name = "pc_sync_connect", description = "Set the machine this app syncs datasets from (base URL such as http://192.168.1.20:35711); pass an empty string to disconnect. Persisted to global settings — it is the same target as the source selector on the Datasets Sync page. Also probes the hub and adopts its cluster id on first contact (trust-on-first-use), so connecting is what binds a follower to a hub; an already-bound device that disagrees reports it in `warning` instead of silently switching. Confirm with pc_sync_status.")]
    async fn pc_sync_connect(&self, Parameters(param): Parameters<PcUrlParam>) -> Result<String, String> {
        let url = param.pc_url.trim().trim_end_matches('/').to_string();
        log::info!("[MCP] pc_sync_connect: {}", if url.is_empty() { "<disconnect>" } else { &url });
        let settings = self.app.state::<SettingsState>();
        let snapshot = {
            let mut s = settings.settings.lock().unwrap();
            s.pc_url = url.clone();
            s.clone()
        };
        let ws_dir = settings.workspace_dir.lock().unwrap().clone();
        SettingsState::save(&snapshot, ws_dir.as_ref())?;
        let _ = self.app.emit("settings-changed", ());
        // Trust-on-first-use binding rides on the same probe the UI's source
        // selector performs, so an agent connecting over MCP ends up bound exactly
        // like a human who picked the hub in the page. Both sides are reported: an
        // unreachable hub and a genuine cluster disagreement need different fixes.
        let (hub_cluster, warning) = if url.is_empty() {
            (String::new(), None)
        } else {
            match sync::client::pc_check_status(self.app.state::<SettingsState>(), Some(url.clone())).await {
                Ok(v) => (
                    v.get("cluster_id").and_then(|c| c.as_str()).unwrap_or("").to_string(),
                    None,
                ),
                Err(e) => (String::new(), Some(e)),
            }
        };
        Ok(serde_json::json!({
            "status": "ok",
            "pc_url": url,
            "cluster_id": settings.cluster_id(),
            "hub_cluster_id": hub_cluster,
            "warning": warning,
            "next": if url.is_empty() {
                "Call pc_sync_scan to find a source machine."
            } else if !hub_cluster.is_empty() {
                "Call pc_sync_status to see what that machine offers."
            } else {
                "Address saved, but the hub did not answer /status: verify its web service is running (web_service_get_status), then retry. A follower also needs pairing (pc_pair_start) before data calls succeed."
            }
        })
        .to_string())
    }

    #[tool(name = "pc_sync_status", description = "Summarise the cross-machine sync setup: the connected source, the datasets it offers, what this machine has already pulled, and queued writeback changes. Read this before pulling anything.")]
    async fn pc_sync_status(&self) -> Result<String, String> {
        log::info!("[MCP] pc_sync_status");
        let settings = self.app.state::<SettingsState>();
        let pc_url = settings.settings.lock().unwrap().pc_url.clone();
        // This machine's sync role/cluster + wire protocol (§3.1) so an agent
        // can tell hub from follower and diagnose mis-pairing before acting.
        let local = serde_json::json!({
            "role": settings.role(),
            "cluster_id": settings.cluster_id(),
            "protocol_version": crate::sync::PROTOCOL_VERSION,
        });
        if pc_url.trim().is_empty() {
            return Ok(serde_json::json!({
                "status": "not_connected",
                "pc_url": "",
                "local": local,
                "next": "Run pc_sync_scan, then pc_sync_connect with one of the candidate urls."
            })
            .to_string());
        }
        let remote = sync::client::pc_list_datasets(self.app.state::<SettingsState>(), None).await;
        let pulled = sync::client::dataset_sync_state(self.app.state::<SettingsState>())
            .await
            .unwrap_or_default();
        let pending = sync::client::writeback_pending_count(self.app.state::<SettingsState>())
            .await
            .unwrap_or(0);
        match remote {
            Ok(list) => Ok(serde_json::json!({
                "status": "ok",
                "pc_url": pc_url,
                "local": local,
                "remote_datasets": list,
                "pulled": pulled,
                "pending_writeback": pending,
                "next": "Pick a uuid from remote_datasets and call pc_sync_pull, or call pc_sync_pull_all."
            })
            .to_string()),
            Err(e) => Ok(serde_json::json!({
                "status": "error",
                "pc_url": pc_url,
                "error": e,
                "next": "If the error mentions pairing, the owner of that machine must approve the pairing request; if it cannot be reached, check that its web service is running and the address is right."
            })
            .to_string()),
        }
    }

    #[tool(name = "pc_sync_pull", description = "Pull one full dataset (media, subtitles, waveforms, database) from the connected source machine into local storage. Non-destructive: local changes are flushed first and the directory is swapped atomically with rollback. Returns updated=false when the dataset is already in sync. For a whole-repo incremental update in one pass prefer sync_run_round.")]
    async fn pc_sync_pull(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] pc_sync_pull: uuid={}", param.uuid);
        let app = self.app.clone();
        let res = sync::client::dataset_sync_snapshot(app, self.app.state::<SettingsState>(), param.uuid.clone()).await?;
        Ok(serde_json::json!({
            "status": "ok",
            "uuid": param.uuid,
            "updated": res.updated,
            "bytes": res.bytes,
            "file_count": res.file_count,
            "hash": res.hash,
            "next": if res.updated { "The dataset is available on the Dictation / Cards / Wiki pages." } else { "Already up to date — nothing to do." }
        })
        .to_string())
    }

    #[tool(name = "pc_sync_pull_all", description = "Pull every dataset the source machine offers in one pass (dictation, cards and books) and report per-dataset results. Prefer this over looping pc_sync_pull for a 'sync everything from the other machine' request.")]
    async fn pc_sync_pull_all(&self) -> Result<String, String> {
        log::info!("[MCP] pc_sync_pull_all");
        let list = sync::client::pc_list_datasets(self.app.state::<SettingsState>(), None).await?;
        let items = list.as_array().cloned().unwrap_or_default();
        let mut results: Vec<serde_json::Value> = Vec::new();
        let mut failed = 0usize;
        for it in items {
            let uuid = it["uuid"].as_str().unwrap_or_default().to_string();
            if uuid.is_empty() {
                continue;
            }
            let name = it["name"].as_str().unwrap_or("").to_string();
            let app = self.app.clone();
            match sync::client::dataset_sync_snapshot(app, self.app.state::<SettingsState>(), uuid.clone()).await {
                Ok(r) => results.push(serde_json::json!({
                    "uuid": uuid,
                    "name": name,
                    "updated": r.updated,
                    "bytes": r.bytes,
                    "file_count": r.file_count
                })),
                Err(e) => {
                    failed += 1;
                    results.push(serde_json::json!({ "uuid": uuid, "name": name, "error": e }));
                }
            }
        }
        Ok(serde_json::json!({
            "status": if failed == 0 { "ok" } else { "partial" },
            "synced": results.len() - failed,
            "failed": failed,
            "results": results,
            "next": if failed > 0 {
                "Read each error: 'not paired' means that machine's owner must approve pairing, a connect error means its web service is off or the address is wrong."
            } else {
                "Every dataset matches the source."
            }
        })
        .to_string())
    }

    #[tool(name = "sync_changes_since", description = "Read one incremental page of a hub change log (GET /datasets/{uuid}/changes?after=SEQ; pass uuid `@app` for the app-wide scope holding per-user dictation progress + XP, served at GET /app/changes which needs no dataset subscription). Returns { entries, pruned_up_to, hub_seq, resync_required }. Protocol diagnostics for the incremental path — use pc_sync_pull for a real sync, and sync_history_backfill for app-data rows older than this journal retains.")]
    async fn sync_changes_since(&self, Parameters(param): Parameters<SyncChangesParam>) -> Result<String, String> {
        log::info!("[MCP] sync_changes_since: uuid={} after={}", param.uuid, param.after);
        let v = sync::client::pc_changes_since(self.app.state::<SettingsState>(), param.uuid.clone(), param.after).await?;
        Ok(v.to_string())
    }

    #[tool(name = "sync_fetch_file", description = "Fetch one non-DB file from the connected hub (GET /file?dataset=&path=) and report its size + sha256 (bytes are not echoed). Verifies the incremental file-transfer path without pulling a whole snapshot.")]
    async fn sync_fetch_file(&self, Parameters(param): Parameters<SyncFileParam>) -> Result<String, String> {
        log::info!("[MCP] sync_fetch_file: uuid={} path={}", param.uuid, param.path);
        let bytes = sync::client::pc_fetch_file(self.app.state::<SettingsState>(), param.uuid.clone(), param.path.clone(), None).await?;
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(&bytes);
        Ok(serde_json::json!({
            "status": "ok",
            "uuid": param.uuid,
            "path": param.path,
            "bytes": bytes.len(),
            "sha256": format!("{:x}", h.finalize()),
        })
        .to_string())
    }

    #[tool(name = "sync_run_round", description = "Run one full incremental sync round against the connected hub: flush local edits, pull each subscribed dataset's row-log delta (advancing the cursor), pull the app-wide per-user journal once (dictation progress + XP, independent of dataset subscriptions, skipped when the hub predates it), import the hub's whole per-user history if this device has never completed such a read (reported under `history`), re-snapshot only datasets whose files actually changed, and prune datasets the hub removed. This is the normal 'sync now' action; use pc_sync_pull to force one whole dataset, pc_sync_pull_all for a bulk first-time import, and sync_history_backfill to force the history read.")]
    async fn sync_run_round(&self) -> Result<String, String> {
        log::info!("[MCP] sync_run_round");
        let v = sync::client::sync_round_inner(self.app.clone(), self.app.state::<SettingsState>()).await?;
        Ok(v.to_string())
    }

    #[tool(name = "sync_history_backfill", description = "Fetch the connected hub's whole per-user history for this device's identity (GET /app/state) and fold it into the local app DB: the current dictation progress per (media, subtitle) pair plus every XP ledger row, then recompute lifetime XP from the merged ledger. Use it when a device was paired or re-subscribed after history already existed, or when a round reported a pruned app-data journal — the delta path can only deliver changes newer than the cursor. Idempotent (awards dedup on user+source+reference, and an un-pushed local progress edit beats the imported one) and it does not re-push what it imports, so re-running is safe. Returns { progress_rows, progress_applied, xp_rows, xp_applied, skipped, pages, lifetime_xp, hub_lifetime_xp, complete }; the read is paged, so a large history arrives whole over several GETs. complete=false means the import is not settled (a row failed — see `failures` — or the page budget was hit), so the next round retries. Needs a hub on sync protocol v4+; against an older hub it returns status=unavailable.")]
    async fn sync_history_backfill(&self) -> Result<String, String> {
        log::info!("[MCP] sync_history_backfill");
        let v = sync::client::sync_backfill_history(self.app.state::<SettingsState>()).await?;
        Ok(v.to_string())
    }

    #[tool(name = "sync_status_detail", description = "JSON twin of the Datasets Sync status screen (§3.5): this device's role/cluster/protocol, hub address + reachability + whether the hub actually accepts this device's signature (hub.paired: true authorized, false = pairing still owed so every data call 401s, null = unknown because the hub is unreachable or unset), per-dataset sync state (downloaded / needs_resync / not_downloaded / removed_on_hub) with row-log cursors and, when a copy is `needs_resync`, a `resync_reason` (`retention_gap` the hub pruned changes older than this cursor / `journal_regressed` the hub's seq fell below it (reset/reinstall) / `provenance_reset` re-establishing against a new host / `no_cursor` predates incremental sync) naming why one snapshot is due, `app_cursor` (this device's position in the hub's per-user dictation/XP journal, null until the first app-data pull) and `history_backfilled_at` (null on a follower means its older history has never been imported — check the hub's protocol_version, then call sync_history_backfill), queued writeback + pending chat counts, and (on a hub) the paired-device registry. Also carries the §19.2 diagnostics: `authority_kind`/`epoch`/`features[]` (the connected authority and the scopes it advertises it can serve, null/empty when the host serves no /cluster); `queue_app`/`queue_dataset` (the `queued_count` total split per journal scope, each with `depth` + `oldest_edit_time` — a nonzero depth against a host that does not accept that scope is the 'queue keeps growing' signal); per-dataset provenance (`upstream_kind`/`upstream_origin`/`availability`/`published_version`); `history_gap`/`events_gap` (the hub advertises an app plane this device has never folded history/events from — a visible gap, not a silent zero) with `events_backfilled_at`; and `last_round`/`last_round_at` (the previous round's persisted result — status, pushed/held/applied/resynced/pruned, `dropped_pending` (the count of un-pushed local edits a provably newer hub entry overwrote this round, INV-14 LWW — a nonzero value is a red flag of silent loss, not a statistic), and typed `errors[]` — reported without running a new round); and `queue_diagnosis` (the §19.4 five-cause answer to 'why is the queue not draining' — `cause`/`cause_index`/`severity`/`detail`/`action`). Read-only and richer than pc_sync_status; use it to diagnose sync without driving the UI — a reachable hub with paired=false means call pc_pair_start next, not sync_run_round.")]
    async fn sync_status_detail(&self) -> Result<String, String> {
        log::info!("[MCP] sync_status_detail");
        let settings = self.app.state::<SettingsState>();
        let detail = sync::client::sync_status_detail(&settings).await?;
        serde_json::to_string(&detail).map_err(|e| e.to_string())
    }

    #[tool(name = "sync_diagnose_queue", description = "Name why this device's writeback queue is not draining, per sync-v2 §19.4: one of five causes (1 authority does not advertise the scope, 2 account suspended/over quota, 3 auth failing / re-pair, 4 network unreachable, 5 journal reset -> resync required) plus the non-fault states idle/draining/no_hub/unknown. Returns { cause, cause_index, severity, detail, action, queue_app, queue_dataset }. Read-only and cheap for an agent to parse: derived from the persisted last round + one status probe, so it never runs a round. Act on `action` (e.g. pc_pair_start for cause 3, sync_run_round for cause 5); a nonzero depth with cause=scope_not_advertised means the changes are held, not lost (INV-20).")]
    async fn sync_diagnose_queue(&self) -> Result<String, String> {
        log::info!("[MCP] sync_diagnose_queue");
        let settings = self.app.state::<SettingsState>();
        let d = sync::client::sync_status_detail(&settings).await?;
        serde_json::to_string(&serde_json::json!({
            "cause": d.queue_diagnosis.cause,
            "cause_index": d.queue_diagnosis.cause_index,
            "severity": d.queue_diagnosis.severity,
            "detail": d.queue_diagnosis.detail,
            "action": d.queue_diagnosis.action,
            "queue_app": d.queue_app,
            "queue_dataset": d.queue_dataset,
        }))
        .map_err(|e| e.to_string())
    }

    #[tool(name = "hub_dataset_upload", description = "Export one local dataset and upload it to the website hub (POST /api/dataset/upload, OAuth bearer). The DB is frozen via the SQLite backup API and shipped as data.sqlite3; info.json's updated_at is refreshed before packing. Requires pc_url to be the website origin and the user to be signed in. Returns the hub's {dataset, version, archive_sha256, archive_size}. A 409 comes back tagged 'owned-by-other' — recover via hub_remint_uuid (save as new copy), never a silent overwrite. 413 means the archive exceeds the hub's size cap.")]
    async fn hub_dataset_upload(&self, Parameters(param): Parameters<HubUploadParam>) -> Result<String, String> {
        log::info!("[MCP] hub_dataset_upload: uuid={}", param.dataset_uuid);
        let v = sync::upload::dataset_export_upload(
            self.app.clone(),
            self.app.state::<SettingsState>(),
            param.dataset_uuid.clone(),
            param.visibility,
            param.tag_uuids,
        )
        .await?;
        Ok(v.to_string())
    }

    #[tool(name = "hub_upload_state", description = "List what this device has uploaded to the website hub (dataset_upload_state table in the app DB): { dataset_uuid, version, archive_sha256, archive_size, uploaded_at }. Lets an agent tell 'this copy is already uploaded (vN)' from 'it changed locally since'. Read-only.")]
    async fn hub_upload_state(&self) -> Result<String, String> {
        log::info!("[MCP] hub_upload_state");
        let rows = sync::upload::dataset_upload_state(self.app.state::<SettingsState>()).await?;
        Ok(serde_json::to_string(&rows).unwrap_or_default())
    }

    #[tool(name = "hub_local_datasets", description = "Enumerate every LOCAL dataset across all types (dictation, card, book, read_aloud, wiki) as { uuid, name, dataset_type }. Upload is type-agnostic, so this is the list a Website Hub upload targets pick from — including copies not yet in the hub catalog. Read-only.")]
    async fn hub_local_datasets(&self) -> Result<String, String> {
        log::info!("[MCP] hub_local_datasets");
        let rows = sync::upload::sync_local_datasets(self.app.state::<SettingsState>()).await?;
        Ok(serde_json::to_string(&rows).unwrap_or_default())
    }

    #[tool(name = "hub_kind", description = "Classify a hub address as 'website' (OAuth Bearer, file + card only, no pairing/cluster) or 'lan' (device-HMAC, full protocol). Empty pc_url falls back to the saved setting. Lets an agent tell which affordances apply before talking to it.")]
    async fn hub_kind(&self, Parameters(param): Parameters<HubKindParam>) -> Result<String, String> {
        log::info!("[MCP] hub_kind");
        let kind = sync::client::sync_hub_kind(self.app.state::<SettingsState>(), param.pc_url).await?;
        Ok(serde_json::to_string(&kind).unwrap_or_default())
    }

    #[tool(name = "hub_remint_uuid", description = "409 recovery: the hub says this dataset's uuid belongs to another account, so mint a fresh uuid locally (rewrite info.json, rename the directory, repoint local fork lineage + sync state) and upload the copy under the new identity. This is the 'save as new copy' path — it never overwrites the hub's existing row. Returns { old_uuid, new_uuid, upload }.")]
    async fn hub_remint_uuid(&self, Parameters(param): Parameters<HubUploadParam>) -> Result<String, String> {
        log::info!("[MCP] hub_remint_uuid: uuid={}", param.dataset_uuid);
        let v = sync::upload::dataset_remint_uuid(
            self.app.clone(),
            self.app.state::<SettingsState>(),
            param.dataset_uuid.clone(),
            param.visibility,
            param.tag_uuids,
        )
        .await?;
        Ok(v.to_string())
    }

    #[tool(name = "sync_mirror_push", description = "Push this desktop hub's dataset-scope change journal up to the account website's mirror (POST /api/v1/changes, sync-v2 phase 4). The website keeps a read-only mirror of the hub's journal so it can render the owner's real state; this is the app-side pusher that feeds it. Incremental (a per-scope cursor is stored in the app DB) and idempotent (the website dedups on change_id), so re-running is safe. Pass an optional scope (a dataset uuid) to push just one; omitted, it mirrors every dataset scope with journal rows. Target is always the account website (not the LAN pc_url) and needs the user signed in. Meaningful on a role=hub desktop — only a hub journals dataset-scope changes, so other roles normally report zero scopes (surfaced in `note`, not an error). Normally drained automatically at the end of every sync round on a logged-in hub; call manually to force one push or to scope a single dataset. The @app scope is not mirrored here (that is the cloud-authority writeback leg). Returns { target, role, scopes:[{scope, cursor_before, cursor_after, pushed, deduped, refused, gap_skipped, errors[]}], totals, errors[], note }.")]
    async fn sync_mirror_push(&self, Parameters(param): Parameters<MirrorPushParam>) -> Result<String, String> {
        log::info!("[MCP] sync_mirror_push: scope={:?}", param.scope);
        let v = sync::mirror::sync_mirror_push(self.app.state::<SettingsState>(), param.scope, param.auto).await?;
        Ok(v.to_string())
    }

    #[tool(name = "sync_inbox_drain", description = "Drain this desktop hub's website inbox (sync-v2 §8.3 step 2 / phase 5). The website is the HOST of a per-cluster inbox; a browser edit on a `mirror` dataset proposes the change (`POST /api/v1/inbox`) instead of writing the shared journal directly (INV-2). This command pulls one page per scope (`GET /api/v1/inbox?scope&after=<cursor>`), applies every proposal through the SAME per-change kernel the LAN writeback uses (`apply_one_change`: LWW, tombstones, dedup on change_id, INV-13 epoch guard, journal append), acks the verdict (`POST /api/v1/inbox/ack` with `{accepted:[{change_id,seq?}], rejected:[{change_id, winner_seq, winner_payload}]}`), and advances a per-scope cursor in `inbox_drain_state`. A proposal whose replay errored is NOT acked and the cursor refuses to advance past its `proposal_seq`, so the next drain retries it; `already_applied` short-circuits a re-drain. Hub-only (a follower must forward via writeback instead); needs a logged-in account. Normally drained automatically at the end of every sync round on a logged-in hub. Pass an optional scope (a dataset uuid) to drain just one; omitted, drains every dataset scope with journal rows. Returns { target, role, scopes:[{scope, drained, accepted, rejected, pending, cursor_before, cursor_after, note?}], totals:{drained,accepted,rejected,pending}, errors[] }.")]
    async fn sync_inbox_drain(&self, Parameters(param): Parameters<InboxDrainParam>) -> Result<String, String> {
        log::info!("[MCP] sync_inbox_drain: scope={:?}", param.scope);
        let v = sync::inbox::sync_inbox_drain(self.app.state::<SettingsState>(), param.scope, param.auto).await?;
        Ok(v.to_string())
    }

    #[tool(name = "sync_check_availability", description = "Check whether each registry-adopted dataset is still available where it was adopted from (sync-v2 §11.5 / INV-22 takedown). For every copy that pinned a version, re-reads its EXACT version manifest (`GET {upstream_origin}/api/v1/registry/{uuid}/manifest?version=N`) — a 200 marks it 'listed', a 410 Gone (the publisher took that version down) marks it 'withdrawn'. A withdrawn copy is KEPT and stays fully practicable (files + progress); only fresh re-reads are refused. Any other status or an unreachable origin leaves the last value untouched and is reported as 'unknown' rather than guessed. Finds no rows on a LAN-only device (registry reads are a website-host feature). Returns { checked, withdrawn, results:[{uuid, version, http_status?, availability, error?|note?}] }.")]
    async fn sync_check_availability(&self) -> Result<String, String> {
        log::info!("[MCP] sync_check_availability");
        let v = sync::client::sync_check_availability(self.app.state::<SettingsState>()).await?;
        Ok(v.to_string())
    }

    #[tool(name = "sync_publish_dataset_version", description = "Promote one of a dataset's already-uploaded immutable versions to its live pin — the explicit act that crosses the publish boundary (sync-v2 §8.4 / INV-17). Owner-only, account Bearer, POST /api/v1/publish against the account website; carries NO bytes (run hub_upload first). Pass `uuid`; optionally `version` (which uploaded version to pin — omitted = the newest uploaded) and/or `visibility` (private|shared|public) and `tag_uuids`, applied in the same transaction. Returns the website's { dataset, current_version }. A 409 means the uuid belongs to another account — remint via hub_remint_uuid and publish that instead.")]
    async fn sync_publish_dataset_version(&self, Parameters(param): Parameters<PublishParam>) -> Result<String, String> {
        log::info!("[MCP] sync_publish_dataset_version: uuid={}", param.uuid);
        let v = sync::publish::sync_publish_dataset_version(
            self.app.state::<SettingsState>(),
            param.uuid,
            param.version,
            param.visibility,
            param.tag_uuids,
        )
        .await?;
        Ok(v.to_string())
    }

    #[tool(name = "sync_set_dataset_availability", description = "Take a published dataset version down, or restore it (sync-v2 §11.5 / INV-8/INV-17/INV-22). Owner-only, account Bearer, POST /api/v1/publish/availability against the account website. `available=false` marks the version unavailable — a fresh read of that exact version then returns 410 Gone, WITHOUT deleting its bytes; `available=true` restores it. Metadata-only: it never moves `current_version` (INV-17) and subscribers keep their installed copies (INV-22); the downstream effect is that an adopter's next sync_check_availability records it 'withdrawn'. Pass `uuid`, `version` (positive integer), and `available` (an explicit boolean — required, never defaulted, so an accidental takedown is impossible). Returns { uuid, version, available }.")]
    async fn sync_set_dataset_availability(&self, Parameters(param): Parameters<AvailabilityParam>) -> Result<String, String> {
        log::info!("[MCP] sync_set_dataset_availability: uuid={} v={} available={}", param.uuid, param.version, param.available);
        let v = sync::publish::sync_set_dataset_availability(
            self.app.state::<SettingsState>(),
            param.uuid,
            param.version,
            param.available,
        )
        .await?;
        Ok(v.to_string())
    }

    #[tool(name = "dataset_publish_state", description = "Read back one dataset's cloud publish state for the owner surface — the mirror of the website's per-card Publish/Withdraw/Restore (sync-v2 §8.4/§11.5). Owner-only, account Bearer, cross-platform. The publish pin is observable only through the upload/publish echoes this device recorded (no REST route serves the version list), so this reads the stored pin and probes its LIVE availability with GET /api/v1/registry/{uuid}/manifest?version=PIN. Returns { uuid, state, version, uploaded_version }: state is 'not_published' (no pin, or the pin references a missing row — 404), 'live' (200), 'withdrawn' (410, taken down but bytes kept), or 'unknown' (unreachable/other — never guessed, with the error surfaced). Withdraw/restore act on the returned `version`. Pass dataset_uuid.")]
    async fn dataset_publish_state(&self, Parameters(param): Parameters<PublishStateParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_publish_state: uuid={}", param.dataset_uuid);
        let v = sync::publish::dataset_publish_state(
            self.app.state::<SettingsState>(),
            param.dataset_uuid,
        )
        .await?;
        Ok(v.to_string())
    }

    #[tool(name = "hub_dataset_push_files", description = "Re-push one dataset's live file tree to the website's content-addressed blob store, moving ONLY the bytes that changed (sync-v2 §7 — the changed-file transport, now with an app client). Owner-only, account Bearer, POST /api/v1/datasets/{uuid}/objects against the account website. Phase 1 sends the tree's per-file sha256s and the store replies which hashes it lacks; phase 2 uploads just those blobs and the website atomically swaps the dataset's live tree and refcounts. The dataset must already exist there (bootstrap via hub_upload / the whole-archive path). An unchanged tree uploads zero bytes yet still reconciles it, so a local rename or deletion syncs without re-sending content. DB files are excluded (they ride the row-log plane, not blobs). Pass dataset_uuid. Normally drained automatically at the end of every sync round for each dataset this device has uploaded, on a logged-in hub; call manually to force one dataset. Returns { uuid, total, need, uploaded, applied, stored, deduped }. A 409 comes back tagged owned-by-other — remint via hub_remint_uuid.")]
    async fn hub_dataset_push_files(&self, Parameters(param): Parameters<PushFilesParam>) -> Result<String, String> {
        log::info!("[MCP] hub_dataset_push_files: uuid={}", param.dataset_uuid);
        let v = sync::blobpush::dataset_push_changed_files(
            self.app.state::<SettingsState>(),
            param.dataset_uuid,
            param.auto,
        )
        .await?;
        Ok(v.to_string())
    }

    #[tool(name = "market_list", description = "Browse the online dataset market on the website origin (pc_url when it is the website, else the signed-in account's website — role-agnostic, a hub workspace browses too). Against a host advertising the `market` capability this is the rich GET /api/v1/market (description, tags, visibility, is_owner/is_subscribed, published_version); otherwise it DEGRADES to GET /api/v1/datasets?lite=1 (rows carry `lite:true`, no subscribe/visibility controls). Optional filters: types[], tag, q. Every row is enriched with its LOCAL state (`sync_state`: downloaded/needs_resync/not_downloaded, `withdrawn`) and the payload returns { datasets, downloads (this device's registry adoptions, even withdrawn ones), published (what this device uploaded, with the stored pin), origin, lite, market_available, can_subscribe, signed_in, degraded_reason }. Download a row through the existing pc_sync_pull / dataset_sync_snapshot tool (registry adoption, owned-locally guarded); evict through dataset_evict.")]
    async fn market_list(&self, Parameters(param): Parameters<MarketListParam>) -> Result<String, String> {
        log::info!("[MCP] market_list: types={:?} tag={:?} q={:?}", param.types, param.tag, param.q);
        let query = sync::client::MarketQuery {
            types: param.types.unwrap_or_default(),
            tag: param.tag.filter(|s| !s.is_empty()),
            q: param.q.filter(|s| !s.is_empty()),
        };
        let v = sync::client::market_list(self.app.state::<SettingsState>(), Some(query)).await?;
        Ok(v.to_string())
    }

    #[tool(name = "market_subscribe", description = "Subscribe the signed-in account to a `shared` market dataset (POST /api/v1/market/{uuid}/subscribe, account Bearer) so its updates keep surfacing for you. Only against a host advertising the `market` capability — in degraded lite mode there is nothing to subscribe to. Subscribing never downloads bytes (use market_list -> dataset_sync_snapshot for that) and unsubscribing never deletes the local copy. Returns { uuid, subscribed, result }.")]
    async fn market_subscribe(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] market_subscribe: uuid={}", param.uuid);
        let v = sync::client::market_subscribe(self.app.state::<SettingsState>(), param.uuid.clone()).await?;
        Ok(v.to_string())
    }

    #[tool(name = "market_unsubscribe", description = "Undo a market subscription (DELETE /api/v1/market/{uuid}/subscribe, account Bearer). The locally adopted copy is untouched — evict/forget stay separate explicit actions. Returns { uuid, subscribed:false, result }.")]
    async fn market_unsubscribe(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] market_unsubscribe: uuid={}", param.uuid);
        let v = sync::client::market_unsubscribe(self.app.state::<SettingsState>(), param.uuid.clone()).await?;
        Ok(v.to_string())
    }

    #[tool(name = "market_set_visibility", description = "Owner-only visibility toggle for one market dataset (PATCH /api/v1/market/{uuid}, account Bearer): `visibility` is private|shared|public. Mirrors the website manage UI; the host's echo refreshes the locally stored publish pin so the My Published badge stays honest. Only works against a host advertising the `market` capability. The My Published section of market_list reads the result back. Returns { uuid, visibility, echo }.")]
    async fn market_set_visibility(&self, Parameters(param): Parameters<MarketVisibilityParam>) -> Result<String, String> {
        log::info!("[MCP] market_set_visibility: uuid={} visibility={}", param.uuid, param.visibility);
        let v = sync::client::market_set_visibility(
            self.app.state::<SettingsState>(),
            param.uuid.clone(),
            param.visibility.clone(),
        )
        .await?;
        Ok(v.to_string())
    }
}
