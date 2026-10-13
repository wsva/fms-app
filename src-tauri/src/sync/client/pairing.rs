#![allow(unused_imports)]
use std::io::{BufReader, Write as _};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use futures_util::StreamExt;
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tar::Archive;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use ring::signature::{Ed25519KeyPair, KeyPair};

use crate::datasets;
use crate::settings::{self, SettingsState};
use super::*;


// ---------------------------------------------------------------------------
// Pairing (phone side)
// ---------------------------------------------------------------------------

/// Best-effort human-readable device name for the PC's confirm dialog.
pub(crate) fn device_name() -> String {
    // Android: the product model, e.g. "Pixel 8". Static after first call.
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        let probe = |key: &str| {
            Command::new("getprop")
                .arg(key)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        };
        match (|| Some(format!("{} ({})", probe("ro.product.model")?, probe("ro.product.brand")?)))()
            .or_else(|| probe("ro.product.model"))
        {
            Some(m) => m,
            // Desktop (PC-to-PC pairing): name the machine, so the dialog says
            // *which* of the owner's computers is asking.
            None => std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .ok()
                .map(|h| h.trim().to_string())
                .filter(|h| !h.is_empty())
                .map(|h| format!("{} ({})", h, std::env::consts::OS))
                .unwrap_or_else(|| format!("{} device", std::env::consts::OS)),
        }
    }).clone()
}

/// The hub's advertised `cluster_id`, read from its unsigned `/status`. Empty on
/// any failure: a hub that predates the field, or one that is momentarily
/// unreachable, must not turn a completed pairing into an error.
pub(crate) async fn hub_cluster_id(client: &reqwest::Client, base: &str) -> String {
    match client.get(format!("{base}/api/v1/status")).send().await {
        Ok(resp) => resp
            .json::<Value>()
            .await
            .unwrap_or(Value::Null)
            .get("cluster_id")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string(),
        Err(_) => String::new(),
    }
}

/// Trust-on-first-use cluster binding on the **pairing** path (§3.1, §7): a
/// follower adopts the id of the hub that has just approved it, and refuses a hub
/// belonging to a different cluster instead of silently switching.
///
/// This hook is what makes the binding real. Adoption previously lived only
/// inside [`pc_check_status`], which no screen has called since the connect UI
/// moved out of Settings, so followers ran with an empty `x-fms-cluster`: the hub's
/// mis-pairing check never fired and nothing recorded which hub they belonged to.
/// Pairing has already succeeded by the time this runs, so a mismatch is returned
/// as a warning next to the approval rather than an error that would misreport the
/// pairing itself as failed.
pub(crate) async fn bind_cluster_after_pairing(
    settings: &SettingsState,
    client: &reqwest::Client,
    base: &str,
) -> Option<String> {
    let hub_cluster = hub_cluster_id(client, base).await;
    if hub_cluster.is_empty() {
        return None;
    }
    match adopt_or_verify_cluster(settings, &hub_cluster) {
        Ok(()) => None,
        Err(e) => {
            log::warn!("[sync] pair approved but cluster binding refused: {e}");
            Some(e)
        }
    }
}




