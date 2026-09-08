pub mod behaviour;
pub mod commands;
pub mod swarm;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use libp2p::PeerId;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex};

// ============================================================
// Types
// ============================================================

/// Type of file being shared.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SharedFileType {
    Model,
    Dataset,
}

/// A file registered for sharing over P2P.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedFile {
    /// SHA-256 hash of the file content — unique fingerprint.
    pub content_hash: String,
    /// Logical name (e.g. "parakeet-ctc-0.6b" or "my-dataset").
    pub name: String,
    /// ISO-8601 timestamp for version ordering.
    pub created_at: String,
    /// Model or Dataset.
    pub file_type: SharedFileType,
    /// File size in bytes.
    pub size: u64,
    /// Local filesystem path to the file.
    #[serde(skip)]
    pub path: PathBuf,
}

/// Trust level for a peer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel {
    Trusted,
    Neutral,
    Blocked,
}

impl Default for TrustLevel {
    fn default() -> Self {
        TrustLevel::Neutral
    }
}

/// Information about a connected peer.
#[derive(Debug, Clone, Serialize)]
pub struct PeerInfo {
    pub peer_id: String,
    pub addresses: Vec<String>,
    pub trust_level: TrustLevel,
}

/// Progress of an active file transfer.
#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)]
pub struct TransferProgress {
    pub file_hash: String,
    pub peer_id: String,
    pub bytes_received: u64,
    pub total_bytes: u64,
}

// ============================================================
// Commands (sent from Tauri commands to swarm task)
// ============================================================

pub enum Command {
    /// Get current node status.
    GetStatus {
        reply: tokio::sync::oneshot::Sender<P2PStatus>,
    },
    /// Get list of connected peers.
    GetPeers {
        reply: tokio::sync::oneshot::Sender<Vec<PeerInfo>>,
    },
    /// Share a file — register and announce to DHT.
    ShareFile {
        file: SharedFile,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    /// Stop sharing a file.
    UnshareFile {
        content_hash: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    /// List all shared files.
    ListShared {
        reply: tokio::sync::oneshot::Sender<Vec<SharedFile>>,
    },
    /// Browse a peer's shared file catalog.
    BrowsePeer {
        peer_id: PeerId,
        reply: tokio::sync::oneshot::Sender<Result<Vec<SharedFile>, String>>,
    },
    /// Download a file by hash.
    DownloadFile {
        content_hash: String,
        peer_id: Option<PeerId>,
        output_dir: PathBuf,
        reply: tokio::sync::oneshot::Sender<Result<PathBuf, String>>,
    },
    /// Connect to a peer by multiaddr.
    Connect {
        addr: String,
        reply: tokio::sync::oneshot::Sender<Result<String, String>>,
    },
    /// Set trust level for a peer.
    SetTrust {
        peer_id: PeerId,
        level: TrustLevel,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
}

// ============================================================
// Status response
// ============================================================

#[derive(Debug, Clone, Serialize)]
pub struct P2PStatus {
    pub peer_id: String,
    pub listening_addresses: Vec<String>,
    pub connected_peer_count: usize,
}

// ============================================================
// Persisted trust list
// ============================================================

/// Trust list persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrustList {
    pub entries: HashMap<String, TrustLevel>,
}

// ============================================================
// P2PState (managed by Tauri)
// ============================================================

pub struct P2PState {
    /// Channel to send commands to the swarm task.
    pub cmd_tx: Arc<Mutex<Option<mpsc::Sender<Command>>>>,
    /// Local shared files registry (also tracked in swarm for network requests).
    pub shared_files: Arc<Mutex<HashMap<String, SharedFile>>>,
    /// Trust list.
    pub trust_list: Arc<Mutex<TrustList>>,
    /// App handle for emitting events.
    #[allow(dead_code)]
    pub app_handle: Arc<Mutex<Option<tauri::AppHandle>>>,
}

impl P2PState {
    pub fn new() -> Self {
        Self {
            cmd_tx: Arc::new(Mutex::new(None)),
            shared_files: Arc::new(Mutex::new(HashMap::new())),
            trust_list: Arc::new(Mutex::new(TrustList::default())),
            app_handle: Arc::new(Mutex::new(None)),
        }
    }

    /// Send a command to the swarm task. Returns error if swarm not started.
    pub async fn send_command(&self, cmd: Command) -> Result<(), String> {
        let guard = self.cmd_tx.lock().await;
        let tx = guard.as_ref().ok_or("P2P swarm not started")?;
        tx.send(cmd).await.map_err(|e| format!("Failed to send command: {e}"))
    }
}

impl Default for P2PState {
    fn default() -> Self {
        Self::new()
    }
}
