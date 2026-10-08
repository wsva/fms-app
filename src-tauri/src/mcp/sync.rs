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
    /// Dataset uuid whose hub change log to read.
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

    #[tool(name = "sync_changes_since", description = "Read one incremental page of a hub dataset's change log (GET /datasets/{uuid}/changes?after=SEQ). Returns { entries, pruned_up_to, hub_seq, resync_required }. Protocol diagnostics for the v2 incremental path — use pc_sync_pull for a real sync.")]
    async fn sync_changes_since(&self, Parameters(param): Parameters<SyncChangesParam>) -> Result<String, String> {
        log::info!("[MCP] sync_changes_since: uuid={} after={}", param.uuid, param.after);
        let v = sync::client::pc_changes_since(self.app.state::<SettingsState>(), param.uuid.clone(), param.after).await?;
        Ok(v.to_string())
    }

    #[tool(name = "sync_fetch_file", description = "Fetch one non-DB file from the connected hub (GET /file?dataset=&path=) and report its size + sha256 (bytes are not echoed). Verifies the incremental file-transfer path without pulling a whole snapshot.")]
    async fn sync_fetch_file(&self, Parameters(param): Parameters<SyncFileParam>) -> Result<String, String> {
        log::info!("[MCP] sync_fetch_file: uuid={} path={}", param.uuid, param.path);
        let bytes = sync::client::pc_fetch_file(self.app.state::<SettingsState>(), param.uuid.clone(), param.path.clone()).await?;
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

    #[tool(name = "sync_run_round", description = "Run one full incremental sync round against the connected hub: flush local edits, pull each subscribed dataset's row-log delta (advancing the cursor), re-snapshot only datasets whose files actually changed, and prune datasets the hub removed. This is the normal 'sync now' action; use pc_sync_pull to force one whole dataset and pc_sync_pull_all for a bulk first-time import.")]
    async fn sync_run_round(&self) -> Result<String, String> {
        log::info!("[MCP] sync_run_round");
        let v = sync::client::sync_round_inner(self.app.clone(), self.app.state::<SettingsState>()).await?;
        Ok(v.to_string())
    }

    #[tool(name = "sync_status_detail", description = "JSON twin of the Datasets Sync status screen (§3.5): this device's role/cluster/protocol, hub address + reachability + whether the hub actually accepts this device's signature (hub.paired: true authorized, false = pairing still owed so every data call 401s, null = unknown because the hub is unreachable or unset), per-dataset sync state (downloaded / needs_resync / not_downloaded / removed_on_hub) with row-log cursors, queued writeback + pending chat counts, and (on a hub) the paired-device registry. Read-only and richer than pc_sync_status; use it to diagnose sync without driving the UI — a reachable hub with paired=false means call pc_pair_start next, not sync_run_round.")]
    async fn sync_status_detail(&self) -> Result<String, String> {
        log::info!("[MCP] sync_status_detail");
        let settings = self.app.state::<SettingsState>();
        let detail = sync::client::sync_status_detail(&settings).await?;
        serde_json::to_string(&detail).map_err(|e| e.to_string())
    }
}
