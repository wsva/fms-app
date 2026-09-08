# FMS Relay Server

A lightweight libp2p relay and bootstrap server for the fms-app P2P file sharing network.

## What it does

Most users are behind NATs and don't have public IPs, making direct P2P connections impossible. This server solves that by providing:

1. **Circuit Relay** — NATed peers connect outbound to this server, which then relays traffic between them when direct connections aren't possible
2. **DCUtR Coordination** — Coordinates simultaneous hole punching so peers can upgrade from relayed to direct connections
3. **AutoNAT Probing** — Helps peers detect whether they're behind a NAT
4. **Kademlia Bootstrap** — Acts as a well-known entry point to the P2P network, helping peers discover each other via the DHT

## Architecture

```
Peer A (NAT) ──outbound──→ Relay VPS ←──outbound── Peer B (NAT)

1. Both peers connect outbound to relay (works through any NAT)
2. Relay registers them in Kademlia DHT
3. AutoNAT detects each peer's NAT status
4. DCUtR coordinates hole punching via relay
5a. Hole punch succeeds → direct P2P connection (fast)
5b. Hole punch fails → traffic flows through relay (slower but works)
```

The relay only handles coordination messages and fallback forwarding — actual file transfers happen directly between peers whenever possible.

## Build

### Prerequisites

- Rust toolchain (1.75+)
- Cross-compile target if building for a different platform

### Build for Linux x86_64 (most VPS)

```bash
# Add the target if not already present
rustup target add x86_64-unknown-linux-gnu

# Build release binary
cargo build --release --target x86_64-unknown-linux-gnu
```

The binary will be at `target/x86_64-unknown-linux-gnu/release/fms-relay`.

### Build for the current platform

```bash
cargo build --release
```

The binary will be at `target/release/fms-relay`.

## Setup

### 1. Upload to VPS

```bash
scp target/x86_64-unknown-linux-gnu/release/fms-relay user@YOUR_VPS:~/
```

### 2. Open the firewall

```bash
# UFW (Ubuntu/Debian)
sudo ufw allow 4001/tcp

# Or iptables
sudo iptables -A INPUT -p tcp --dport 4001 -j ACCEPT

# Or firewalld (CentOS/Fedora)
sudo firewall-cmd --permanent --add-port=4001/tcp
sudo firewall-cmd --reload
```

Also ensure your VPS provider's security group / network rules allow TCP port 4001.

### 3. Run

```bash
# Default port (4001)
./fms-relay

# Custom port
./fms-relay 8080
```

On first run it generates a persistent Ed25519 keypair in `./identity.key`. Subsequent runs reuse the same identity.

The server prints its peer ID and multiaddr:

```
Relay server peer ID: 12D3KooWAbcDef...
Listening on /ip4/0.0.0.0/tcp/4001
Share this multiaddr with clients:
  /ip4/<YOUR_VPS_PUBLIC_IP>/tcp/4001/p2p/12D3KooWAbcDef...
```

### 4. Run as a systemd service

Create `/etc/systemd/system/fms-relay.service`:

```ini
[Unit]
Description=FMS P2P Relay Server
After=network.target

[Service]
Type=simple
User=root
ExecStart=/opt/fms-relay/fms-relay 4001
WorkingDirectory=/opt/fms-relay
Restart=always
RestartSec=5
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

Then enable and start:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now fms-relay
sudo systemctl status fms-relay
```

View logs with:

```bash
journalctl -u fms-relay -f
```

### 5. Connect the fms-app client

In `src-tauri/src/p2p/commands.rs`, update the relay address in `p2p_init`:

```rust
let relay_addr: Option<&str> = Some(
    "/ip4/YOUR_VPS_IP/tcp/4001/p2p/12D3KooWAbcDef..."
);
```

Replace `YOUR_VPS_IP` with your VPS public IP and `12D3KooWAbcDef...` with the actual peer ID printed by the relay server.

## Resource usage

The relay server is very lightweight:

- **Memory**: ~20-50 MB
- **CPU**: negligible (only coordination messages, not file data)
- **Bandwidth**: minimal under normal operation; increases only when relaying fallback traffic between peers that can't establish direct connections
- **Disk**: a few KB for the identity key file

## Configuration

| Argument | Default | Description |
|---|---|---|
| Port (positional) | `4001` | TCP port to listen on |

| Environment Variable | Default | Description |
|---|---|---|
| `RUST_LOG` | `info` | Log level (`trace`, `debug`, `info`, `warn`, `error`) |

## Files

| File | Description |
|---|---|
| `identity.key` | Persistent Ed25519 keypair (auto-generated on first run) |

## Troubleshooting

**Peers can't connect:**
- Check that the firewall allows TCP on the configured port
- Check your VPS provider's network security rules
- Verify the multiaddr matches exactly (IP, port, and peer ID)

**High relay traffic:**
- This means peers can't establish direct connections. Check that DCUtR and AutoNAT are working (check app logs for "NAT status changed" and "DCUtR event" messages)
- Most peers on residential NATs should be able to hole-punch successfully

**Relay won't start:**
- Check if the port is already in use: `ss -tlnp | grep 4001`
- Delete `identity.key` to regenerate if corrupted
