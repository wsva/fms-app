//! Follower-side hub identity verification.
//!
//! The problem this closes: a follower used to re-find its hub purely by the
//! `pc_url` it remembered, so the moment the hub's DHCP address changed the
//! link broke — and worse, a *different* machine that happened to be handed the
//! old address (or that learned the `cluster_id` from a beacon) could pose as
//! the hub, because `cluster_id` is just an unsigned UUID.
//!
//! [`verify_hub`] is the gate the reconnect path runs before it silently commits
//! a re-found address: it checks the host is in the cluster we already bound to,
//! then — when we hold the hub's public key from pairing — poses a fresh nonce
//! and confirms the machine behind the IP can still sign it. Only possession of
//! the hub's private key answers correctly, so a recycled address or a stolen
//! cluster id is rejected. A hub that predates the challenge route degrades to
//! cluster-only trust (still better than trusting a bare URL).

#![allow(unused_imports)]
use std::time::Duration;

use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::UnparsedPublicKey;
use serde_json::Value;

use crate::settings::SettingsState;
use super::*;

/// The result of checking a candidate address against the remembered hub.
pub(crate) enum HubVerify {
    /// The host proved possession of the remembered hub private key.
    Proven,
    /// The host is in our cluster but served no verifiable signature (no stored
    /// key to check, or a hub predating `/hub/challenge`). Trust the cluster we
    /// already bound to — the honest fallback, not a silent switch.
    UnprovenByCluster,
    /// Not our hub: a cluster mismatch, or a signature that did not verify. The
    /// `String` says why, for the caller to surface.
    Rejected(String),
    /// Nothing answered — the caller moves on to the next candidate.
    Unreachable,
}

/// Read the host's unsigned `/api/v1/status`. `None` on any failure.
async fn host_status(client: &reqwest::Client, base: &str) -> Option<Value> {
    let resp = client.get(format!("{base}/api/v1/status")).send().await.ok()?;
    resp.json::<Value>().await.ok()
}

/// A 32-byte random nonce, hex-encoded, unique to this verification attempt.
fn fresh_nonce() -> String {
    let mut bytes = [0u8; 32];
    let rng = SystemRandom::new();
    // A failed RNG is unrecoverable and rare; treat it as an all-zero nonce,
    // which is still a valid, if predictable, challenge value.
    let _ = rng.fill(&mut bytes);
    hex_encode(&bytes)
}

/// Verify that `base` is the hub identified by `expected_cluster` and
/// `stored_pubkey_hex` (the pubkey captured at pairing; empty on a follower that
/// paired before this feature). `expected_cluster` is this device's adopted
/// cluster id (empty = nothing bound yet, so only the key gate applies).
pub(crate) async fn verify_hub(
    base: &str,
    expected_cluster: &str,
    stored_pubkey_hex: &str,
) -> HubVerify {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return HubVerify::Unreachable;
    }
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(4))
        .timeout(Duration::from_secs(8))
        .build()
    {
        Ok(c) => c,
        Err(_) => return HubVerify::Unreachable,
    };

    // 1. Reachable + same cluster. A host that reports a *different* cluster is
    //    emphatically not our hub. An empty host cluster (a pre-field peer) is
    //    not a mismatch — we let the key gate decide below.
    let status = match host_status(&client, base).await {
        Some(v) => v,
        None => return HubVerify::Unreachable,
    };
    if status.get("ok").and_then(|o| o.as_bool()) != Some(true) {
        return HubVerify::Unreachable;
    }
    let host_cluster = status.get("cluster_id").and_then(|c| c.as_str()).unwrap_or("");
    if !expected_cluster.is_empty() && !host_cluster.is_empty() && host_cluster != expected_cluster {
        return HubVerify::Rejected(format!(
            "host is in cluster '{host_cluster}', not the remembered '{expected_cluster}'"
        ));
    }

    // 2. No remembered key → nothing to prove against; accept on cluster match.
    if stored_pubkey_hex.is_empty() {
        return HubVerify::UnprovenByCluster;
    }

    // 3. Pose a fresh nonce and ask the host to sign it with its hub key.
    let nonce = fresh_nonce();
    let resp = match client
        .post(format!("{base}/api/v1/hub/challenge"))
        .json(&serde_json::json!({ "nonce_hex": nonce }))
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return HubVerify::Unreachable,
    };
    // A 404 means the host has no challenge route: a hub that predates the
    // identity. Fall back to the cluster trust we already have.
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return HubVerify::UnprovenByCluster;
    }
    let v: Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return HubVerify::Rejected("malformed challenge reply".to_string()),
    };
    let sig_hex = v.get("sig_hex").and_then(|s| s.as_str()).unwrap_or("");

    let pubkey = match hex_decode(stored_pubkey_hex) {
        Ok(p) if p.len() == 32 => p,
        _ => return HubVerify::Rejected("stored hub pubkey is malformed".to_string()),
    };
    let sig = match hex_decode(sig_hex) {
        Ok(s) => s,
        Err(_) => return HubVerify::Rejected("challenge signature is malformed".to_string()),
    };
    let msg = crate::sync::hub_challenge_message(&nonce);
    // Belt and braces: the pubkey the host echoed must be the one we remember.
    if let Some(echoed) = v.get("pubkey_hex").and_then(|p| p.as_str()) {
        if echoed != stored_pubkey_hex.trim() {
            return HubVerify::Rejected("host presented a different pubkey than the one we paired".to_string());
        }
    }
    match UnparsedPublicKey::new(&ring::signature::ED25519, &pubkey).verify(msg.as_bytes(), &sig) {
        Ok(()) => HubVerify::Proven,
        Err(_) => HubVerify::Rejected(
            "hub signature did not verify — a different machine answers on this address".to_string(),
        ),
    }
}

/// Read the hub's advertised `hub_pubkey` + `hub_fingerprint` from its unsigned
/// `/status` (used to persist them at pairing time, trust-on-first-use). Empty
/// strings on any failure or a hub that predates the fields.
pub(crate) async fn read_hub_identity(base: &str) -> (String, String) {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return (String::new(), String::new());
    }
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(4))
        .timeout(Duration::from_secs(8))
        .build()
    {
        Ok(c) => c,
        Err(_) => return (String::new(), String::new()),
    };
    match host_status(&client, base).await {
        Some(v) => (
            v.get("hub_pubkey").and_then(|p| p.as_str()).unwrap_or("").to_string(),
            v.get("hub_fingerprint").and_then(|p| p.as_str()).unwrap_or("").to_string(),
        ),
        None => (String::new(), String::new()),
    }
}

/// Persist the hub's identity on this device (workspace-scoped, alongside the
/// cluster adoption it accompanies). Only widens empty fields: a follower that
/// already bound a hub key keeps it, so a mid-life hub re-key never silently
/// swaps the credential the reconnect path verifies against.
pub(crate) fn bind_hub_identity(
    settings: &SettingsState,
    pubkey_hex: &str,
    fingerprint: &str,
) -> Result<(), String> {
    if pubkey_hex.is_empty() {
        return Ok(());
    }
    let ws_dir = settings.workspace_dir.lock().unwrap().clone();
    let mut s = settings.settings.lock().unwrap().clone();
    if s.hub_pubkey.is_empty() {
        s.hub_pubkey = pubkey_hex.to_string();
        s.hub_fingerprint = fingerprint.to_string();
        SettingsState::save(&s, ws_dir.as_ref())?;
        *settings.settings.lock().unwrap() = s;
        log::info!("[sync] bound hub identity fingerprint='{fingerprint}' (trust-on-first-use)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    /// The whole anti-spoof rests on both sides using the identical signed bytes
    /// and the identical key. This is that contract without a live server: mint
    /// a hub key, sign a follower-fresh nonce the way `rest::hub_challenge` does,
    /// and verify it the way [`verify_hub`] does.
    fn keypair() -> Ed25519KeyPair {
        let der = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        Ed25519KeyPair::from_pkcs8_maybe_unchecked(der.as_ref()).unwrap()
    }

    #[test]
    fn challenge_round_trips_for_the_matching_key() {
        let hub = keypair();
        let pubkey_hex = hex_encode(hub.public_key().as_ref());
        let nonce = fresh_nonce();
        let msg = crate::sync::hub_challenge_message(&nonce);
        let sig = hub.sign(msg.as_bytes());

        // A follower holding the same pubkey accepts the honest answer.
        let pk = hex_decode(&pubkey_hex).unwrap();
        assert!(UnparsedPublicKey::new(&ring::signature::ED25519, &pk)
            .verify(msg.as_bytes(), sig.as_ref())
            .is_ok());
    }

    #[test]
    fn a_different_key_or_nonce_never_verifies() {
        let hub = keypair();
        let other = keypair();
        let nonce = fresh_nonce();
        let msg = crate::sync::hub_challenge_message(&nonce);
        let sig = hub.sign(msg.as_bytes());

        // Wrong public key (a machine that spoofed the cluster but not the key).
        let other_pk = hex_decode(hex_encode(other.public_key().as_ref()).as_str()).unwrap();
        assert!(UnparsedPublicKey::new(&ring::signature::ED25519, &other_pk)
            .verify(msg.as_bytes(), sig.as_ref())
            .is_err());

        // A replayed challenge: the same signature against a *different* nonce
        // (what `verify_hub` builds its message from) must fail, which is exactly
        // why the nonce is follower-fresh per attempt.
        let replay_msg = crate::sync::hub_challenge_message(&fresh_nonce());
        let hub_pk = hex_decode(hex_encode(hub.public_key().as_ref()).as_str()).unwrap();
        assert!(UnparsedPublicKey::new(&ring::signature::ED25519, &hub_pk)
            .verify(replay_msg.as_bytes(), sig.as_ref())
            .is_err());
    }

    #[test]
    fn nonce_is_fresh_and_well_formed() {
        let a = fresh_nonce();
        let b = fresh_nonce();
        assert_eq!(a.len(), 64, "32 bytes hex-encode to 64 chars");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "two challenges must not reuse a nonce");
    }
}
