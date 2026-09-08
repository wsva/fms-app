use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use futures::StreamExt;
use libp2p::identity::Keypair;
use libp2p::request_response::{self, Event as RequestResponseEvent};
use libp2p::swarm::SwarmEvent;
use libp2p::{autonat, dcutr, identify, kad, mdns, relay, Multiaddr, PeerId, Swarm};
use log::{info, warn};
use tokio::sync::{mpsc, oneshot, Mutex};

use super::behaviour::{
    build_file_share_behaviour, FileRequest, FileResponse, FileShareBehaviour,
};
use super::{Command, PeerInfo, SharedFile, TrustLevel};

// ============================================================
// Combined network behaviour
// ============================================================

#[derive(libp2p::swarm::NetworkBehaviour)]
pub struct NetworkBehaviour {
    pub relay_client: relay::client::Behaviour,
    pub identify: identify::Behaviour,
    pub mdns: mdns::tokio::Behaviour,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
    pub autonat: autonat::Behaviour,
    pub dcutr: dcutr::Behaviour,
    pub file_share: FileShareBehaviour,
}

// ============================================================
// Pending download tracking (two-phase: DHT lookup → file request)
// ============================================================

enum PendingDownload {
    /// Waiting for DHT provider lookup results.
    FindingProviders {
        peer_id: Option<PeerId>,
        output_dir: std::path::PathBuf,
        reply: oneshot::Sender<Result<std::path::PathBuf, String>>,
    },
    /// Waiting for file data from a specific peer.
    Requesting {
        reply: oneshot::Sender<Result<std::path::PathBuf, String>>,
        content_hash: String,
        output_dir: std::path::PathBuf,
    },
}

// ============================================================
// Swarm construction
// ============================================================

/// Build the libp2p swarm with all network behaviours.
/// If `relay_addr` is provided, connects to a relay server for NAT traversal.
pub fn build_swarm(
    keypair: &Keypair,
    relay_addr: Option<&str>,
) -> Result<(Swarm<NetworkBehaviour>, Option<Multiaddr>), String> {
    let peer_id = keypair.public().to_peer_id();

    let identify_cfg = identify::Config::new(
        "/fms-app/1.0.0".into(),
        keypair.public(),
    )
    .with_agent_version("fms-app/0.1.0".into());

    let mdns_config = mdns::Config {
        ttl: std::time::Duration::from_secs(300),
        ..Default::default()
    };

    // Create relay client (transport + behaviour pair).
    let (_relay_transport, relay_behaviour) = relay::client::new(peer_id);

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair.clone())
        .with_tokio()
        .with_tcp(
            libp2p::tcp::Config::default(),
            libp2p::noise::Config::new,
            libp2p::yamux::Config::default,
        )
        .map_err(|e| format!("TCP transport error: {e}"))?
        .with_behaviour(|_key| {
            let mdns_behaviour = mdns::tokio::Behaviour::new(mdns_config, peer_id)
                .map_err(|e| format!("mDNS error: {e}"))?;
            Ok(NetworkBehaviour {
                relay_client: relay_behaviour,
                identify: identify::Behaviour::new(identify_cfg),
                mdns: mdns_behaviour,
                kademlia: kad::Behaviour::new(
                    peer_id,
                    kad::store::MemoryStore::new(peer_id),
                ),
                autonat: autonat::Behaviour::new(peer_id, autonat::Config::default()),
                dcutr: dcutr::Behaviour::new(peer_id),
                file_share: build_file_share_behaviour(),
            })
        })
        .map_err(|e| format!("Behaviour error: {e}"))?
        .build();

    // Listen on all interfaces, random port.
    swarm
        .listen_on("/ip4/0.0.0.0/tcp/0".parse().unwrap())
        .map_err(|e| format!("Listen error: {e}"))?;

    // Parse relay address if provided.
    let parsed_relay = relay_addr.and_then(|addr| {
        addr.parse::<Multiaddr>().ok().map(|a| {
            info!("Will connect to relay: {a}");
            a
        })
    });

    Ok((swarm, parsed_relay))
}

// ============================================================
// Swarm event loop (runs as a background tokio task)
// ============================================================

/// Run the swarm event loop. Processes network events and commands from
/// Tauri handlers until the command channel is closed.
pub async fn run_swarm(
    mut swarm: Swarm<NetworkBehaviour>,
    relay_addr: Option<Multiaddr>,
    mut cmd_rx: mpsc::Receiver<Command>,
    shared_files: Arc<Mutex<HashMap<String, SharedFile>>>,
    trust_list: Arc<Mutex<super::TrustList>>,
) {
    // Connect to relay server if configured.
    if let Some(addr) = relay_addr {
        match swarm.dial(addr.clone()) {
            Ok(_) => info!("Dialing relay server: {addr}"),
            Err(e) => warn!("Failed to dial relay: {e}"),
        }
    }
    let mut connected_peers: HashMap<PeerId, Vec<Multiaddr>> = HashMap::new();
    let mut pending_downloads: HashMap<request_response::OutboundRequestId, PendingDownload> =
        HashMap::new();
    let mut pending_browse: HashMap<request_response::OutboundRequestId, oneshot::Sender<Result<Vec<SharedFile>, String>>> =
        HashMap::new();
    let mut pending_provider_queries: HashMap<kad::QueryId, PendingDownload> = HashMap::new();

    loop {
        tokio::select! {
            // ---- Network events ----
            event = swarm.select_next_some() => {
                match event {
                    // -- mDNS discovery --
                    SwarmEvent::Behaviour(NetworkBehaviourEvent::Mdns(mdns::Event::Discovered(peers))) => {
                        for (peer_id, addr) in peers {
                            info!("mDNS discovered peer: {peer_id} at {addr}");

                            // Respect block list.
                            let trust = trust_list.lock().await;
                            let level = trust.entries.get(&peer_id.to_string())
                                .copied().unwrap_or(TrustLevel::Neutral);
                            drop(trust);

                            if level == TrustLevel::Blocked {
                                warn!("Ignoring blocked peer: {peer_id}");
                                let _ = swarm.disconnect_peer_id(peer_id);
                                continue;
                            }

                            swarm.behaviour_mut().kademlia.add_address(&peer_id, addr.clone());
                            connected_peers.entry(peer_id).or_default().push(addr);
                        }
                    }
                    SwarmEvent::Behaviour(NetworkBehaviourEvent::Mdns(mdns::Event::Expired(peers))) => {
                        for (peer_id, _addr) in peers {
                            info!("mDNS peer expired: {peer_id}");
                        }
                    }

                    // -- Incoming file-share requests --
                    SwarmEvent::Behaviour(NetworkBehaviourEvent::FileShare(
                        RequestResponseEvent::Message {
                            peer: _peer,
                            message: request_response::Message::Request { request, channel, .. },
                            ..
                        },
                    )) => {
                        let response = match request {
                            FileRequest::GetFile { hash } => {
                                let files = shared_files.lock().await;
                                if let Some(file) = files.get(&hash) {
                                    match std::fs::read(&file.path) {
                                        Ok(data) => {
                                            let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
                                            FileResponse::FileData {
                                                name: file.name.clone(),
                                                size: file.size,
                                                data: encoded,
                                            }
                                        }
                                        Err(e) => FileResponse::Error {
                                            message: format!("Failed to read file: {e}"),
                                        },
                                    }
                                } else {
                                    FileResponse::Error {
                                        message: format!("File not found: {hash}"),
                                    }
                                }
                            }
                            FileRequest::ListFiles => {
                                let files = shared_files.lock().await;
                                FileResponse::FileList {
                                    files: files.values().cloned().collect(),
                                }
                            }
                        };
                        let _ = swarm.behaviour_mut().file_share.send_response(channel, response);
                    }

                    // -- File-share response (for our outgoing requests) --
                    SwarmEvent::Behaviour(NetworkBehaviourEvent::FileShare(
                        RequestResponseEvent::Message {
                            message: request_response::Message::Response { request_id, response },
                            ..
                        },
                    )) => {
                        // Check if this is a pending browse request.
                        if let Some(reply) = pending_browse.remove(&request_id) {
                            let result = match response {
                                FileResponse::FileList { files } => Ok(files),
                                FileResponse::Error { message } => Err(message),
                                _ => Err("Unexpected response type for browse".into()),
                            };
                            let _ = reply.send(result);
                            continue;
                        }

                        // Otherwise, check pending downloads.
                        if let Some(pending) = pending_downloads.remove(&request_id) {
                            match pending {
                                PendingDownload::Requesting { reply, content_hash, output_dir } => {
                                    let result = match response {
                                        FileResponse::FileData { name: _, size: _, data } => {
                                            match base64::engine::general_purpose::STANDARD.decode(&data) {
                                                Ok(bytes) => {
                                                    let out_path = output_dir.join(&content_hash);
                                                    match std::fs::write(&out_path, &bytes) {
                                                        Ok(_) => Ok(out_path),
                                                        Err(e) => Err(format!("Failed to write file: {e}")),
                                                    }
                                                }
                                                Err(e) => Err(format!("Base64 decode error: {e}")),
                                            }
                                        }
                                        FileResponse::Error { message } => Err(message),
                                        _ => Err("Unexpected response type".into()),
                                    };
                                    let _ = reply.send(result);
                                }
                                _ => {}
                            }
                        }
                    }

                    // -- Kademlia events --
                    SwarmEvent::Behaviour(NetworkBehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed { result, id, .. })) => {
                        match result {
                            kad::QueryResult::GetProviders(Ok(kad::GetProvidersOk::FoundProviders { key, providers })) => {
                                info!("Found {} providers for {}", providers.len(), String::from_utf8_lossy(key.as_ref()));
                                // If we have a pending download for this query, request from first provider.
                                if let Some(pending) = pending_provider_queries.remove(&id) {
                                    match pending {
                                        PendingDownload::FindingProviders { peer_id, output_dir, reply } => {
                                            let target = peer_id.or_else(|| {
                                                providers.iter().next().copied()
                                            });
                                            if let Some(target_peer) = target {
                                                let req = FileRequest::GetFile {
                                                    hash: String::from_utf8_lossy(key.as_ref()).to_string(),
                                                };
                                                let req_id = swarm.behaviour_mut().file_share.send_request(&target_peer, req);
                                                pending_downloads.insert(req_id, PendingDownload::Requesting {
                                                    reply,
                                                    content_hash: String::from_utf8_lossy(key.as_ref()).to_string(),
                                                    output_dir,
                                                });
                                            } else {
                                                let _ = reply.send(Err("No providers found".into()));
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            kad::QueryResult::StartProviding(Ok(_)) => {
                                info!("Provider record announced successfully");
                            }
                            kad::QueryResult::StartProviding(Err(e)) => {
                                warn!("Failed to announce provider record: {e:?}");
                            }
                            _ => {}
                        }
                    }

                    // -- Identify: add remote addresses to Kademlia routing table --
                    SwarmEvent::Behaviour(NetworkBehaviourEvent::Identify(identify::Event::Received { peer_id, info: identify_info, .. })) => {
                        for addr in identify_info.listen_addrs {
                            swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                        }
                        // After identify exchange with relay, add it to Kademlia and bootstrap.
                        swarm.behaviour_mut().kademlia.bootstrap().ok();
                    }

                    // -- AutoNAT: NAT detection --
                    SwarmEvent::Behaviour(NetworkBehaviourEvent::Autonat(event)) => {
                        match event {
                            autonat::Event::StatusChanged { old, new } => {
                                info!("NAT status changed: {old:?} -> {new:?}");
                            }
                            autonat::Event::InboundProbe { .. } => {}
                            _ => {}
                        }
                    }

                    // -- DCUtR: hole punching --
                    SwarmEvent::Behaviour(NetworkBehaviourEvent::Dcutr(event)) => {
                        info!("DCUtR event: {event:?}");
                    }

                    // -- Relay client events --
                    SwarmEvent::Behaviour(NetworkBehaviourEvent::RelayClient(event)) => {
                        match event {
                            relay::client::Event::ReservationReqAccepted { relay_peer_id, renewal, .. } => {
                                info!("Relay reservation accepted by {relay_peer_id} (renewal={renewal})");
                            }
                            _ => {}
                        }
                    }

                    // -- Connection events --
                    SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                        let addr = endpoint.get_remote_address().clone();
                        info!("Connected to {peer_id} at {addr}");
                        connected_peers.entry(peer_id).or_default().push(addr);
                    }
                    SwarmEvent::ConnectionClosed { peer_id, .. } => {
                        if connected_peers.get(&peer_id).is_some() {
                            // Only remove if no more connections to this peer.
                            let still_connected = swarm.is_connected(&peer_id);
                            if !still_connected {
                                connected_peers.remove(&peer_id);
                            }
                        }
                    }

                    _ => {}
                }
            }

            // ---- Commands from Tauri handlers ----
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else {
                    info!("Command channel closed, stopping swarm");
                    break;
                };

                match cmd {
                    Command::GetStatus { reply } => {
                        let local_peer_id = swarm.local_peer_id().to_string();
                        let listening_addresses: Vec<String> = swarm
                            .listeners()
                            .map(|a| a.to_string())
                            .collect();
                        let _ = reply.send(super::P2PStatus {
                            peer_id: local_peer_id,
                            listening_addresses,
                            connected_peer_count: connected_peers.len(),
                        });
                    }

                    Command::GetPeers { reply } => {
                        let trust = trust_list.lock().await;
                        let peers: Vec<PeerInfo> = connected_peers
                            .iter()
                            .map(|(peer_id, addrs)| {
                                let level = trust
                                    .entries.get(&peer_id.to_string())
                                    .copied()
                                    .unwrap_or(TrustLevel::Neutral);
                                PeerInfo {
                                    peer_id: peer_id.to_string(),
                                    addresses: addrs.iter().map(|a| a.to_string()).collect(),
                                    trust_level: level,
                                }
                            })
                            .collect();
                        let _ = reply.send(peers);
                    }

                    Command::ShareFile { file, reply } => {
                        let hash = file.content_hash.clone();
                        let key = kad::RecordKey::new(&hash);
                        match swarm.behaviour_mut().kademlia.start_providing(key) {
                            Ok(_query_id) => {
                                shared_files.lock().await.insert(hash, file);
                                let _ = reply.send(Ok(()));
                            }
                            Err(e) => {
                                let _ = reply.send(Err(format!("DHT announce failed: {e:?}")));
                            }
                        }
                    }

                    Command::UnshareFile { content_hash, reply } => {
                        shared_files.lock().await.remove(&content_hash);
                        let key = kad::RecordKey::new(&content_hash);
                        swarm.behaviour_mut().kademlia.stop_providing(&key);
                        let _ = reply.send(Ok(()));
                    }

                    Command::ListShared { reply } => {
                        let files = shared_files.lock().await;
                        let _ = reply.send(files.values().cloned().collect());
                    }

                    Command::BrowsePeer { peer_id, reply } => {
                        if !swarm.is_connected(&peer_id) {
                            let _ = reply.send(Err(format!("Peer {peer_id} not connected. Use Connect first.")));
                        } else {
                            let req_id = swarm
                                .behaviour_mut()
                                .file_share
                                .send_request(&peer_id, FileRequest::ListFiles);
                            pending_browse.insert(req_id, reply);
                        }
                    }

                    Command::DownloadFile { content_hash, peer_id, output_dir, reply } => {
                        if let Some(target_peer) = peer_id {
                            if !swarm.is_connected(&target_peer) {
                                let _ = reply.send(Err(format!("Peer {target_peer} not connected")));
                            } else {
                                let req = FileRequest::GetFile { hash: content_hash.clone() };
                                let req_id = swarm
                                    .behaviour_mut()
                                    .file_share
                                    .send_request(&target_peer, req);
                                pending_downloads.insert(req_id, PendingDownload::Requesting {
                                    reply,
                                    content_hash,
                                    output_dir,
                                });
                            }
                        } else {
                            // DHT mode: look up providers.
                            let key = kad::RecordKey::new(&content_hash);
                            let query_id = swarm.behaviour_mut().kademlia.get_providers(key);
                            pending_provider_queries.insert(query_id, PendingDownload::FindingProviders {
                                peer_id: None,
                                output_dir,
                                reply,
                            });
                        }
                    }

                    Command::Connect { addr, reply } => {
                        match addr.parse::<Multiaddr>() {
                            Ok(multiaddr) => {
                                match swarm.dial(multiaddr) {
                                    Ok(_) => {
                                        let _ = reply.send(Ok("Dialing".into()));
                                    }
                                    Err(e) => {
                                        let _ = reply.send(Err(format!("Dial failed: {e}")));
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = reply.send(Err(format!("Invalid address: {e}")));
                            }
                        }
                    }

                    Command::SetTrust { peer_id, level, reply } => {
                        let mut trust = trust_list.lock().await;
                        trust.entries.insert(peer_id.to_string(), level);
                        if level == TrustLevel::Blocked {
                            let _ = swarm.disconnect_peer_id(peer_id);
                        }
                        let _ = reply.send(Ok(()));
                    }
                }
            }
        }
    }
}
