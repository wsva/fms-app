use std::path::PathBuf;

use chrono::Utc;
use flate2::write::GzEncoder;
use flate2::Compression;
use libp2p::PeerId;
use sha2::{Digest, Sha256};
use tauri::State;

use super::{P2PState, SharedFile, SharedFileType, TrustLevel};
use crate::settings::SettingsState;

// ============================================================
// P2P Init
// ============================================================

/// Initialize the P2P node: load/generate keypair, load trust list, start swarm.
#[tauri::command]
pub async fn p2p_init(
    state: State<'_, P2PState>,
    settings: State<'_, SettingsState>,
) -> Result<String, String> {
    // Check if already running.
    {
        let guard = state.cmd_tx.lock().await;
        if guard.is_some() {
            return Err("P2P already initialized".into());
        }
    }

    // Determine P2P data directory.
    let p2p_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("fms-app")
        .join("p2p");
    std::fs::create_dir_all(&p2p_dir).map_err(|e| format!("Failed to create P2P dir: {e}"))?;

    // Load or generate identity keypair.
    let key_path = p2p_dir.join("identity.key");
    let keypair = if key_path.exists() {
        let bytes = std::fs::read(&key_path).map_err(|e| format!("Read key: {e}"))?;
        libp2p::identity::Keypair::from_protobuf_encoding(&bytes)
            .map_err(|e| format!("Decode key: {e}"))?
    } else {
        let keypair = libp2p::identity::Keypair::generate_ed25519();
        let bytes = keypair
            .to_protobuf_encoding()
            .map_err(|e| format!("Encode key: {e}"))?;
        std::fs::write(&key_path, &bytes).map_err(|e| format!("Write key: {e}"))?;
        keypair
    };

    let peer_id = keypair.public().to_peer_id().to_string();

    // Load trust list.
    let trust_path = p2p_dir.join("trust.json");
    let trust_list = if trust_path.exists() {
        let data = std::fs::read_to_string(&trust_path).unwrap_or_default();
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        super::TrustList::default()
    };
    *state.trust_list.lock().await = trust_list;

    // Build and start swarm.
    let relay_setting = settings.settings.lock().unwrap().p2p_relay_addr.clone();
    let relay_addr: Option<&str> = if relay_setting.is_empty() {
        None
    } else {
        Some(&relay_setting)
    };
    let (swarm, parsed_relay) = super::swarm::build_swarm(&keypair, relay_addr)?;
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(256);

    let shared_files = state.shared_files.clone();
    let trust_list_arc = state.trust_list.clone();

    tokio::spawn(async move {
        super::swarm::run_swarm(swarm, parsed_relay, cmd_rx, shared_files, trust_list_arc).await;
    });

    *state.cmd_tx.lock().await = Some(cmd_tx);

    Ok(peer_id)
}

// ============================================================
// Status & Peers
// ============================================================

/// Get current P2P node status (peer ID, listening addresses, peer count).
#[tauri::command]
pub async fn p2p_get_status(state: State<'_, P2PState>) -> Result<super::P2PStatus, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .send_command(super::Command::GetStatus { reply: tx })
        .await?;
    rx.await.map_err(|e| format!("No reply: {e}"))
}

/// List connected peers with addresses and trust levels.
#[tauri::command]
pub async fn p2p_get_peers(state: State<'_, P2PState>) -> Result<Vec<super::PeerInfo>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .send_command(super::Command::GetPeers { reply: tx })
        .await?;
    rx.await.map_err(|e| format!("No reply: {e}"))
}

// ============================================================
// File Sharing
// ============================================================

/// Share a model file over P2P.
#[tauri::command]
pub async fn p2p_share_model(
    state: State<'_, P2PState>,
    file_path: String,
    name: String,
) -> Result<SharedFile, String> {
    let path = PathBuf::from(&file_path);
    if !path.exists() {
        return Err("File not found".into());
    }

    let data = std::fs::read(&path).map_err(|e| format!("Read file: {e}"))?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let hash = format!("{:x}", hasher.finalize());
    let size = data.len() as u64;

    let file = SharedFile {
        content_hash: hash.clone(),
        name,
        created_at: Utc::now().to_rfc3339(),
        file_type: SharedFileType::Model,
        size,
        path,
    };

    // Register locally.
    state.shared_files.lock().await.insert(hash, file.clone());

    // Announce to DHT.
    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .send_command(super::Command::ShareFile {
            file: file.clone(),
            reply: tx,
        })
        .await?;
    rx.await
        .map_err(|e| format!("No reply: {e}"))?
        .map(|_| file)
}

/// Share a dataset by creating a tar.gz archive and announcing it.
#[tauri::command]
pub async fn p2p_share_dataset(
    state: State<'_, P2PState>,
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<SharedFile, String> {
    let dataset_dir = crate::dataset::find_dataset_dir(&settings, &dataset_uuid)?;

    // Read info.json for the dataset name.
    let info_path = dataset_dir.join("info.json");
    let info_data =
        std::fs::read_to_string(&info_path).map_err(|e| format!("Read info.json: {e}"))?;
    let info: serde_json::Value =
        serde_json::from_str(&info_data).map_err(|e| format!("Parse info.json: {e}"))?;
    let dataset_name = info["name"]
        .as_str()
        .unwrap_or(&dataset_uuid)
        .to_string();

    // Create tar.gz archive in temp directory.
    let archive_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("fms-app")
        .join("p2p")
        .join("archives");
    std::fs::create_dir_all(&archive_dir).map_err(|e| format!("Create archive dir: {e}"))?;

    let archive_path = archive_dir.join(format!("{dataset_uuid}.tar.gz"));
    let archive_file =
        std::fs::File::create(&archive_path).map_err(|e| format!("Create archive: {e}"))?;
    let encoder = GzEncoder::new(archive_file, Compression::default());
    let mut tar_builder = tar::Builder::new(encoder);

    tar_builder
        .append_dir_all(&dataset_name, &dataset_dir)
        .map_err(|e| format!("Create tar.gz: {e}"))?;
    tar_builder
        .finish()
        .map_err(|e| format!("Finish tar.gz: {e}"))?;

    // Compute hash.
    let data = std::fs::read(&archive_path).map_err(|e| format!("Read archive: {e}"))?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let hash = format!("{:x}", hasher.finalize());
    let size = data.len() as u64;

    let file = SharedFile {
        content_hash: hash.clone(),
        name: dataset_name,
        created_at: Utc::now().to_rfc3339(),
        file_type: SharedFileType::Dataset,
        size,
        path: archive_path,
    };

    state
        .shared_files
        .lock()
        .await
        .insert(hash.clone(), file.clone());

    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .send_command(super::Command::ShareFile {
            file: file.clone(),
            reply: tx,
        })
        .await?;
    rx.await
        .map_err(|e| format!("No reply: {e}"))?
        .map(|_| file)
}

/// Stop sharing a file and remove its DHT announcement.
#[tauri::command]
pub async fn p2p_unshare(
    state: State<'_, P2PState>,
    content_hash: String,
) -> Result<(), String> {
    state.shared_files.lock().await.remove(&content_hash);

    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .send_command(super::Command::UnshareFile {
            content_hash,
            reply: tx,
        })
        .await?;
    rx.await
        .map_err(|e| format!("No reply: {e}"))?
}

/// List all files shared by the local node.
#[tauri::command]
pub async fn p2p_list_shared(state: State<'_, P2PState>) -> Result<Vec<SharedFile>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .send_command(super::Command::ListShared { reply: tx })
        .await?;
    rx.await.map_err(|e| format!("No reply: {e}"))
}

// ============================================================
// Peer Browsing & Download
// ============================================================

/// Browse a peer's shared file catalog.
#[tauri::command]
pub async fn p2p_browse_peer(
    state: State<'_, P2PState>,
    peer_id: String,
) -> Result<Vec<SharedFile>, String> {
    let peer: PeerId = peer_id
        .parse()
        .map_err(|e| format!("Invalid peer ID: {e}"))?;

    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .send_command(super::Command::BrowsePeer {
            peer_id: peer,
            reply: tx,
        })
        .await?;
    rx.await
        .map_err(|e| format!("No reply: {e}"))?
}

/// Download a file by content hash, optionally from a specific peer.
#[tauri::command]
pub async fn p2p_download_file(
    state: State<'_, P2PState>,
    content_hash: String,
    peer_id: Option<String>,
    output_dir: String,
) -> Result<String, String> {
    let target_peer = peer_id
        .map(|p| p.parse::<PeerId>().map_err(|e| format!("Invalid peer ID: {e}")))
        .transpose()?;
    let out_dir = PathBuf::from(&output_dir);

    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .send_command(super::Command::DownloadFile {
            content_hash,
            peer_id: target_peer,
            output_dir: out_dir,
            reply: tx,
        })
        .await?;
    rx.await
        .map_err(|e| format!("No reply: {e}"))?
        .map(|p| p.to_string_lossy().into_owned())
}

// ============================================================
// Connection & Trust
// ============================================================

/// Manually connect to a peer by multiaddr.
#[tauri::command]
pub async fn p2p_connect(state: State<'_, P2PState>, addr: String) -> Result<String, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .send_command(super::Command::Connect {
            addr,
            reply: tx,
        })
        .await?;
    rx.await
        .map_err(|e| format!("No reply: {e}"))?
}

/// Set trust level for a peer (trusted, neutral, or blocked).
#[tauri::command]
pub async fn p2p_trust_peer(
    state: State<'_, P2PState>,
    peer_id: String,
    level: String,
) -> Result<(), String> {
    let peer: PeerId = peer_id
        .parse()
        .map_err(|e| format!("Invalid peer ID: {e}"))?;
    let trust_level = match level.as_str() {
        "trusted" => TrustLevel::Trusted,
        "neutral" => TrustLevel::Neutral,
        "blocked" => TrustLevel::Blocked,
        _ => return Err(format!("Invalid trust level: {level}")),
    };

    // Update persisted trust list.
    {
        let mut trust = state.trust_list.lock().await;
        trust.entries.insert(peer_id.clone(), trust_level);

        // Persist to disk.
        let trust_path = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("fms-app")
            .join("p2p")
            .join("trust.json");
        if let Ok(data) = serde_json::to_string_pretty(&*trust) {
            let _ = std::fs::write(&trust_path, data);
        }
    }

    // Notify swarm (for immediate disconnect if blocked).
    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .send_command(super::Command::SetTrust {
            peer_id: peer,
            level: trust_level,
            reply: tx,
        })
        .await?;
    rx.await
        .map_err(|e| format!("No reply: {e}"))?
}
