use futures::StreamExt;
use libp2p::{identify, kad, noise, relay, swarm::SwarmEvent, tcp, yamux};
use log::info;

/// Combined network behaviour for the relay server.
#[derive(libp2p::swarm::NetworkBehaviour)]
struct RelayBehaviour {
    relay: relay::Behaviour,
    identify: identify::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|p| p.parse().ok())
        .unwrap_or(4001);

    // Persistent identity so clients can whitelist this peer.
    let key_path = "identity.key";
    let keypair = if std::path::Path::new(key_path).exists() {
        let bytes = std::fs::read(key_path)?;
        libp2p::identity::Keypair::from_protobuf_encoding(&bytes)?
    } else {
        let kp = libp2p::identity::Keypair::generate_ed25519();
        std::fs::write(key_path, kp.to_protobuf_encoding()?)?;
        kp
    };
    let peer_id = keypair.public().to_peer_id();

    info!("Relay server peer ID: {peer_id}");

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|key| {
            Ok(RelayBehaviour {
                relay: relay::Behaviour::new(peer_id, relay::Config {
                    max_reservations: 256,
                    ..Default::default()
                }),
                identify: identify::Behaviour::new(
                    identify::Config::new(
                        "/fms-app/1.0.0".into(),
                        key.public(),
                    )
                    .with_agent_version("fms-relay/0.1.0".into()),
                ),
                kademlia: kad::Behaviour::new(
                    peer_id,
                    kad::store::MemoryStore::new(peer_id),
                ),
            })
        })?
        .build();

    let listen_addr: libp2p::Multiaddr = format!("/ip4/0.0.0.0/tcp/{port}").parse()?;
    swarm.listen_on(listen_addr)?;

    info!("Listening on /ip4/0.0.0.0/tcp/{port}");
    info!("Share this multiaddr with clients:");
    info!("  /ip4/<YOUR_VPS_PUBLIC_IP>/tcp/{port}/p2p/{peer_id}");

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::Behaviour(RelayBehaviourEvent::Relay(event)) => {
                info!("Relay event: {event:?}");
            }
            SwarmEvent::Behaviour(RelayBehaviourEvent::Identify(
                identify::Event::Received { peer_id, info, .. },
            )) => {
                for addr in info.listen_addrs {
                    swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                }
            }
            SwarmEvent::NewListenAddr { address, .. } => {
                info!("Listening on {address}");
            }
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                info!("Peer connected: {peer_id}");
            }
            SwarmEvent::ConnectionClosed { peer_id, .. } => {
                info!("Peer disconnected: {peer_id}");
            }
            _ => {}
        }
    }
}
