export interface P2PStatus {
  peer_id: string;
  listening_addresses: string[];
  connected_peer_count: number;
}

export interface PeerInfo {
  peer_id: string;
  addresses: string[];
  trust_level: "trusted" | "neutral" | "blocked";
}

export interface SharedFile {
  content_hash: string;
  name: string;
  created_at: string;
  file_type: "model" | "dataset";
  size: number;
}

export interface TransferProgress {
  file_hash: string;
  peer_id: string;
  bytes_received: number;
  total_bytes: number;
}
