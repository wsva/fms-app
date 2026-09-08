"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Share2,
  Play,
  Square,
  Copy,
  Users,
  HardDrive,
  Search,
  Download,
  Shield,
  ShieldOff,
  Ban,
  Loader2,
} from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { type P2PStatus, type PeerInfo, type SharedFile, type TransferProgress } from "@/lib/p2p/types";

export default function P2PPage() {
  const [status, setStatus] = useState<P2PStatus | null>(null);
  const [peers, setPeers] = useState<PeerInfo[]>([]);
  const [sharedFiles, setSharedFiles] = useState<SharedFile[]>([]);
  const [transfers, setTransfers] = useState<TransferProgress[]>([]);
  const [running, setRunning] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  // Relay address
  const [relayAddr, setRelayAddr] = useState("");

  // Connect
  const [connectAddr, setConnectAddr] = useState("");
  // Browse
  const [browsePeerId, setBrowsePeerId] = useState("");
  const [browseResults, setBrowseResults] = useState<SharedFile[]>([]);
  const [browsing, setBrowsing] = useState(false);
  // Download
  const [downloadHash, setDownloadHash] = useState("");
  const [downloadPeer, setDownloadPeer] = useState("");
  const [downloadDir, setDownloadDir] = useState("");

  // ---- Fetch status ----

  const fetchStatus = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const s = await invoke<P2PStatus>("p2p_get_status");
      setStatus(s);
    } catch {
      // Not running yet
    }
  }, []);

  const fetchPeers = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const p = await invoke<PeerInfo[]>("p2p_get_peers");
      setPeers(p);
    } catch {
      // Ignore
    }
  }, []);

  const fetchShared = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const f = await invoke<SharedFile[]>("p2p_list_shared");
      setSharedFiles(f);
    } catch {
      // Ignore
    }
  }, []);

  // ---- Init / Start ----

  const handleInit = async () => {
    if (!isTauri()) return;
    setLoading(true);
    setError("");
    try {
      // Save relay address to settings first.
      const settings = await invoke<any>("settings_get");
      settings.p2p_relay_addr = relayAddr;
      await invoke("settings_set", { settings });
      await invoke("p2p_init");
      setRunning(true);
      await fetchStatus();
    } catch (e) {
      setError(String(e));
    }
    setLoading(false);
  };

  // ---- Connect to peer ----

  const handleConnect = async () => {
    if (!isTauri() || !connectAddr) return;
    setError("");
    try {
      const msg = await invoke<string>("p2p_connect", { addr: connectAddr });
      setError(""); // Clear any previous error
      console.log("Connect result:", msg);
      setConnectAddr("");
      // Refresh after a short delay to let the connection establish
      setTimeout(fetchPeers, 1000);
    } catch (e) {
      setError(String(e));
    }
  };

  // ---- Browse peer ----

  const handleBrowse = async () => {
    if (!isTauri() || !browsePeerId) return;
    setBrowsing(true);
    setError("");
    try {
      const files = await invoke<SharedFile[]>("p2p_browse_peer", {
        peer_id: browsePeerId,
      });
      setBrowseResults(files);
    } catch (e) {
      setError(String(e));
    }
    setBrowsing(false);
  };

  // ---- Download ----

  const handleDownload = async () => {
    if (!isTauri() || !downloadHash || !downloadDir) return;
    setError("");
    try {
      const path = await invoke<string>("p2p_download_file", {
        content_hash: downloadHash,
        peer_id: downloadPeer || null,
        output_dir: downloadDir,
      });
      console.log("Downloaded to:", path);
      setDownloadHash("");
      setDownloadPeer("");
    } catch (e) {
      setError(String(e));
    }
  };

  // ---- Trust ----

  const handleTrust = async (peerId: string, level: string) => {
    if (!isTauri()) return;
    try {
      await invoke("p2p_trust_peer", { peer_id: peerId, level });
      await fetchPeers();
    } catch (e) {
      setError(String(e));
    }
  };

  // ---- Unshare ----

  const handleUnshare = async (hash: string) => {
    if (!isTauri()) return;
    try {
      await invoke("p2p_unshare", { content_hash: hash });
      await fetchShared();
    } catch (e) {
      setError(String(e));
    }
  };

  // ---- Copy to clipboard ----

  const copyToClipboard = (text: string) => {
    navigator.clipboard.writeText(text);
  };

  // ---- Effects ----

  useEffect(() => {
    fetchStatus();
    // Load relay address from settings.
    if (isTauri()) {
      invoke<any>("settings_get")
        .then((s) => setRelayAddr(s.p2p_relay_addr || ""))
        .catch(() => {});
    }
  }, [fetchStatus]);

  // Poll peers and shared files when running
  useEffect(() => {
    if (!running) return;
    const interval = setInterval(() => {
      fetchPeers();
      fetchShared();
      fetchStatus();
    }, 3000);
    return () => clearInterval(interval);
  }, [running, fetchPeers, fetchShared, fetchStatus]);

  // Listen for transfer progress events
  useEffect(() => {
    if (!isTauri()) return;
    const unlisten = listen<TransferProgress>("p2p-transfer-progress", (event) => {
      setTransfers((prev) => {
        const idx = prev.findIndex((t) => t.file_hash === event.payload.file_hash);
        if (idx >= 0) {
          const updated = [...prev];
          updated[idx] = event.payload;
          return updated;
        }
        return [...prev, event.payload];
      });
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // ---- Render ----

  return (
    <div className="flex-1 overflow-auto p-6 space-y-6">
      <div className="flex items-center gap-3 mb-4">
        <Share2 className="text-logo-primary" size={24} />
        <h1 className="text-xl font-semibold">P2P File Sharing</h1>
      </div>

      {error && (
        <div className="bg-red-500/10 border border-red-500/30 text-red-400 px-4 py-2 rounded-lg text-sm">
          {error}
          <button onClick={() => setError("")} className="ml-2 underline">
            dismiss
          </button>
        </div>
      )}

      {/* ---- Node Status ---- */}
      <section className="bg-bg-card border border-border-default rounded-xl p-4 space-y-3">
        <h2 className="text-sm font-semibold text-text-secondary flex items-center gap-2">
          <HardDrive size={16} /> Node Status
        </h2>
        {!running ? (
          <>
            <div className="space-y-1">
              <label className="text-xs text-text-tertiary">Relay server address (optional)</label>
              <input
                type="text"
                value={relayAddr}
                onChange={(e) => setRelayAddr(e.target.value)}
                placeholder="/ip4/1.2.3.4/tcp/4001/p2p/12D3KooW..."
                className="w-full bg-mid-gray/10 border border-mid-gray/40 rounded-lg px-3 py-2 text-sm focus:outline-none focus:ring-1 focus:ring-logo-primary"
              />
            </div>
            <button
              onClick={handleInit}
              disabled={loading}
              className="flex items-center gap-2 px-4 py-2 bg-logo-primary text-white rounded-lg hover:opacity-90 disabled:opacity-50 text-sm"
            >
              {loading && <Loader2 size={14} className="animate-spin" />}
              {loading ? "Starting..." : "Start P2P Node"}
            </button>
          </>
        ) : (
          <div className="space-y-2 text-sm">
            <div className="flex items-center gap-2">
              <span className="text-text-tertiary w-28 shrink-0">Peer ID:</span>
              <code className="bg-mid-gray/20 px-2 py-0.5 rounded text-xs flex-1 truncate">
                {status?.peer_id ?? "—"}
              </code>
              <button
                onClick={() => copyToClipboard(status?.peer_id ?? "")}
                className="p-1 hover:bg-mid-gray/30 rounded"
                title="Copy peer ID"
              >
                <Copy size={14} />
              </button>
            </div>
            <div className="flex items-center gap-2">
              <span className="text-text-tertiary w-28 shrink-0">Listening:</span>
              <span className="text-xs">
                {status?.listening_addresses.join(", ") ?? "—"}
              </span>
            </div>
            <div className="flex items-center gap-2">
              <span className="text-text-tertiary w-28 shrink-0">Peers:</span>
              <span>{status?.connected_peer_count ?? 0}</span>
            </div>
            <button
              onClick={() => setRunning(false)}
              className="flex items-center gap-1 px-3 py-1.5 bg-red-500/20 text-red-400 rounded-lg hover:bg-red-500/30 text-xs"
            >
              <Square size={12} /> Stop
            </button>
          </div>
        )}
      </section>

      {/* ---- Connect ---- */}
      {running && (
        <section className="bg-bg-card border border-border-default rounded-xl p-4 space-y-3">
          <h2 className="text-sm font-semibold text-text-secondary">Connect to Peer</h2>
          <div className="flex gap-2">
            <input
              type="text"
              value={connectAddr}
              onChange={(e) => setConnectAddr(e.target.value)}
              placeholder="/ip4/192.168.1.100/tcp/12345/p2p/PeerID"
              className="flex-1 bg-mid-gray/10 border border-mid-gray/40 rounded-lg px-3 py-2 text-sm focus:outline-none focus:ring-1 focus:ring-logo-primary"
            />
            <button
              onClick={handleConnect}
              className="px-4 py-2 bg-logo-primary text-white rounded-lg hover:opacity-90 text-sm"
            >
              Connect
            </button>
          </div>
        </section>
      )}

      {/* ---- Peers ---- */}
      {running && (
        <section className="bg-bg-card border border-border-default rounded-xl p-4 space-y-3">
          <h2 className="text-sm font-semibold text-text-secondary flex items-center gap-2">
            <Users size={16} /> Connected Peers ({peers.length})
          </h2>
          {peers.length === 0 ? (
            <p className="text-xs text-text-tertiary">No peers connected yet. Wait for mDNS discovery or connect manually.</p>
          ) : (
            <div className="space-y-2">
              {peers.map((peer) => (
                <div
                  key={peer.peer_id}
                  className="flex items-center gap-2 bg-mid-gray/10 rounded-lg p-2 text-xs"
                >
                  <span
                    className={`w-2 h-2 rounded-full shrink-0 ${
                      peer.trust_level === "trusted"
                        ? "bg-green-400"
                        : peer.trust_level === "blocked"
                          ? "bg-red-400"
                          : "bg-gray-400"
                    }`}
                  />
                  <code className="flex-1 truncate">{peer.peer_id}</code>
                  <div className="flex gap-1 shrink-0">
                    <button
                      onClick={() => handleTrust(peer.peer_id, "trusted")}
                      className="p-1 hover:bg-green-500/20 rounded text-green-400"
                      title="Trust"
                    >
                      <Shield size={12} />
                    </button>
                    <button
                      onClick={() => handleTrust(peer.peer_id, "neutral")}
                      className="p-1 hover:bg-gray-500/20 rounded text-gray-400"
                      title="Neutral"
                    >
                      <ShieldOff size={12} />
                    </button>
                    <button
                      onClick={() => handleTrust(peer.peer_id, "blocked")}
                      className="p-1 hover:bg-red-500/20 rounded text-red-400"
                      title="Block"
                    >
                      <Ban size={12} />
                    </button>
                    <button
                      onClick={() => {
                        setBrowsePeerId(peer.peer_id);
                        setBrowseResults([]);
                      }}
                      className="p-1 hover:bg-logo-primary/20 rounded text-logo-primary"
                      title="Browse files"
                    >
                      <Search size={12} />
                    </button>
                  </div>
                </div>
              ))}
            </div>
          )}
        </section>
      )}

      {/* ---- My Shared Files ---- */}
      {running && (
        <section className="bg-bg-card border border-border-default rounded-xl p-4 space-y-3">
          <h2 className="text-sm font-semibold text-text-secondary flex items-center gap-2">
            <Share2 size={16} /> My Shared Files ({sharedFiles.length})
          </h2>
          {sharedFiles.length === 0 ? (
            <p className="text-xs text-text-tertiary">
              No files shared. Use Share buttons on Models or Datasets pages.
            </p>
          ) : (
            <div className="space-y-2">
              {sharedFiles.map((file) => (
                <div
                  key={file.content_hash}
                  className="flex items-center gap-3 bg-mid-gray/10 rounded-lg p-2 text-xs"
                >
                  <span
                    className={`px-1.5 py-0.5 rounded text-[10px] font-medium ${
                      file.file_type === "model"
                        ? "bg-blue-500/20 text-blue-400"
                        : "bg-purple-500/20 text-purple-400"
                    }`}
                  >
                    {file.file_type}
                  </span>
                  <span className="font-medium flex-1 truncate">{file.name}</span>
                  <span className="text-text-tertiary">
                    {(file.size / 1024 / 1024).toFixed(1)} MB
                  </span>
                  <code className="text-text-tertiary truncate max-w-24">
                    {file.content_hash.slice(0, 12)}...
                  </code>
                  <button
                    onClick={() => handleUnshare(file.content_hash)}
                    className="px-2 py-0.5 bg-red-500/20 text-red-400 rounded hover:bg-red-500/30"
                  >
                    Unshare
                  </button>
                </div>
              ))}
            </div>
          )}
        </section>
      )}

      {/* ---- Browse Peer ---- */}
      {running && (
        <section className="bg-bg-card border border-border-default rounded-xl p-4 space-y-3">
          <h2 className="text-sm font-semibold text-text-secondary">Browse Peer Files</h2>
          <div className="flex gap-2">
            <input
              type="text"
              value={browsePeerId}
              onChange={(e) => setBrowsePeerId(e.target.value)}
              placeholder="Peer ID to browse"
              className="flex-1 bg-mid-gray/10 border border-mid-gray/40 rounded-lg px-3 py-2 text-sm focus:outline-none focus:ring-1 focus:ring-logo-primary"
            />
            <button
              onClick={handleBrowse}
              disabled={browsing}
              className="flex items-center gap-1 px-4 py-2 bg-logo-primary text-white rounded-lg hover:opacity-90 disabled:opacity-50 text-sm"
            >
              {browsing && <Loader2 size={14} className="animate-spin" />}
              Browse
            </button>
          </div>
          {browseResults.length > 0 && (
            <div className="space-y-2">
              {browseResults.map((file) => (
                <div
                  key={file.content_hash}
                  className="flex items-center gap-3 bg-mid-gray/10 rounded-lg p-2 text-xs"
                >
                  <span
                    className={`px-1.5 py-0.5 rounded text-[10px] font-medium ${
                      file.file_type === "model"
                        ? "bg-blue-500/20 text-blue-400"
                        : "bg-purple-500/20 text-purple-400"
                    }`}
                  >
                    {file.file_type}
                  </span>
                  <span className="font-medium flex-1 truncate">{file.name}</span>
                  <span className="text-text-tertiary">
                    {(file.size / 1024 / 1024).toFixed(1)} MB
                  </span>
                  <span className="text-text-tertiary text-[10px]">
                    {new Date(file.created_at).toLocaleDateString()}
                  </span>
                  <button
                    onClick={() => {
                      setDownloadHash(file.content_hash);
                      setDownloadPeer(browsePeerId);
                    }}
                    className="flex items-center gap-1 px-2 py-0.5 bg-logo-primary/20 text-logo-primary rounded hover:bg-logo-primary/30"
                  >
                    <Download size={10} /> Download
                  </button>
                </div>
              ))}
            </div>
          )}
        </section>
      )}

      {/* ---- Download ---- */}
      {running && (
        <section className="bg-bg-card border border-border-default rounded-xl p-4 space-y-3">
          <h2 className="text-sm font-semibold text-text-secondary flex items-center gap-2">
            <Download size={16} /> Download File
          </h2>
          <div className="grid grid-cols-1 gap-2">
            <input
              type="text"
              value={downloadHash}
              onChange={(e) => setDownloadHash(e.target.value)}
              placeholder="Content hash (SHA-256)"
              className="bg-mid-gray/10 border border-mid-gray/40 rounded-lg px-3 py-2 text-sm focus:outline-none focus:ring-1 focus:ring-logo-primary"
            />
            <input
              type="text"
              value={downloadPeer}
              onChange={(e) => setDownloadPeer(e.target.value)}
              placeholder="Peer ID (optional — leave empty for DHT)"
              className="bg-mid-gray/10 border border-mid-gray/40 rounded-lg px-3 py-2 text-sm focus:outline-none focus:ring-1 focus:ring-logo-primary"
            />
            <input
              type="text"
              value={downloadDir}
              onChange={(e) => setDownloadDir(e.target.value)}
              placeholder="Output directory path"
              className="bg-mid-gray/10 border border-mid-gray/40 rounded-lg px-3 py-2 text-sm focus:outline-none focus:ring-1 focus:ring-logo-primary"
            />
          </div>
          <button
            onClick={handleDownload}
            disabled={!downloadHash || !downloadDir}
            className="flex items-center gap-2 px-4 py-2 bg-logo-primary text-white rounded-lg hover:opacity-90 disabled:opacity-50 text-sm"
          >
            <Download size={14} /> Download
          </button>
        </section>
      )}

      {/* ---- Transfer Progress ---- */}
      {running && transfers.length > 0 && (
        <section className="bg-bg-card border border-border-default rounded-xl p-4 space-y-3">
          <h2 className="text-sm font-semibold text-text-secondary">Active Transfers</h2>
          <div className="space-y-2">
            {transfers.map((t) => {
              const pct =
                t.total_bytes > 0
                  ? Math.round((t.bytes_received / t.total_bytes) * 100)
                  : 0;
              return (
                <div key={t.file_hash} className="space-y-1">
                  <div className="flex justify-between text-xs">
                    <code className="truncate max-w-48">
                      {t.file_hash.slice(0, 16)}...
                    </code>
                    <span>{pct}%</span>
                  </div>
                  <div className="w-full bg-mid-gray/20 rounded-full h-1.5">
                    <div
                      className="bg-logo-primary h-1.5 rounded-full transition-all"
                      style={{ width: `${pct}%` }}
                    />
                  </div>
                </div>
              );
            })}
          </div>
        </section>
      )}
    </div>
  );
}
