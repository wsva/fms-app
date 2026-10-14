//! Owner-facing registry writes — **publish** + **withdraw / re-list** a published
//! version (sync-v2.md §7, §8.4, §11.5). The producer counterpart to the takedown
//! *consumption* in [`super::client::availability`]: an owner promotes an uploaded
//! immutable version to the dataset's live pin, or takes it back down, and adopted
//! copies downstream learn the `withdrawn` state on their next availability check.
//!
//! Two owner-only account-Bearer routes on the website, both resolved by
//! `resolveDatasetWriter` (`learning/fms/src/app/api/v1/publish/**`):
//! * `POST /api/v1/publish` — moves `dataset.current_version` to the pinned (or
//!   newest-uploaded) version and optionally sets `visibility` / `tag_uuids`. It
//!   carries **no bytes**: the upload route (`sync/upload.rs`) already wrote the
//!   immutable `v<N>.tar.gz`; publish only promotes it. A publish of a uuid this
//!   account does not own is the website's 409, answered by `dataset_remint_uuid`.
//! * `POST /api/v1/publish/availability` — flips one version's `available` flag
//!   (metadata-only, INV-8: the bytes stay). `available:false` is the §11.5 takedown
//!   (a fresh read of that exact version then returns 410); `true` restores it. It
//!   deliberately does **not** move the pin (INV-17) and never deletes anything.
//!
//! Target is the **account website** (`crate::auth::base_url()`) under the OAuth
//! Bearer — the registry cloud plane, the same credential `sync/mirror.rs` uses for
//! handover, and deliberately *not* the LAN `pc_url`: an owner running a desktop hub
//! still publishes to the cloud. A missing token is a hard, actionable error (this is
//! a write), unlike the read paths which degrade to anonymous.
//!
//! Compiled on **all platforms** (reqwest + rusqlite only, like `upload.rs`) — a
//! mobile client may publish what it uploaded. Pure addition: nothing here is reached
//! by the round, writeback, pull, upload, or availability-consumption paths.

use std::time::Duration;

use serde_json::{json, Value};
use tauri::State;

use crate::settings::SettingsState;

/// Website route that promotes a version to `current_version` (§8.4).
const PUBLISH_PATH: &str = "/api/v1/publish";

/// Website route that flips one published version's `available` flag (§11.5).
const AVAILABILITY_PATH: &str = "/api/v1/publish/availability";

/// Build the `POST /api/v1/publish` body. `uuid` is always present; `version`,
/// `visibility`, and `tag_uuids` are included only when meaningfully supplied — the
/// website treats an absent `version` as "the newest uploaded" and an absent
/// `visibility`/`tag_uuids` as "no change", so we must not send nulls for them.
fn publish_body(
    uuid: &str,
    version: Option<i64>,
    visibility: Option<&str>,
    tag_uuids: Option<&[String]>,
) -> Value {
    let mut body = json!({ "uuid": uuid });
    if let Some(v) = version.filter(|v| *v >= 1) {
        body["version"] = json!(v);
    }
    if let Some(vis) = visibility.map(str::trim).filter(|s| !s.is_empty()) {
        body["visibility"] = json!(vis);
    }
    if let Some(tags) = tag_uuids.filter(|t| !t.is_empty()) {
        body["tag_uuids"] = json!(tags);
    }
    body
}

/// Build the `POST /api/v1/publish/availability` body. `available` is ALWAYS emitted,
/// for `true` and `false` alike: the website refuses a request that omits it, so an
/// accidental takedown of the pinned version is structurally impossible (the flag is
/// never defaulted).
fn availability_body(uuid: &str, version: i64, available: bool) -> Value {
    json!({ "uuid": uuid, "version": version, "available": available })
}

/// Guard the inputs both commands share: a non-empty uuid and a positive version
/// (when one is being addressed). Mirrors the website's own 400s so the failure is
/// local and clear rather than a round-trip.
fn check_uuid(uuid: &str) -> Result<String, String> {
    let uuid = uuid.trim().to_string();
    if uuid.is_empty() {
        return Err("uuid is required (the dataset to act on)".to_string());
    }
    Ok(uuid)
}

fn check_version(version: i64) -> Result<(), String> {
    if version < 1 {
        return Err(format!("version must be a positive integer (got {version})"));
    }
    Ok(())
}

/// Map a failed publish / availability response to an actionable message, house style
/// of `sync/mirror.rs::handover_http_error` (owner/auth-shaped 4xx, not batch-shaped).
async fn publish_http_error(resp: reqwest::Response, api_path: &str) -> String {
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let msg = body["error"].as_str().unwrap_or("");
    let suffix = |s: &str| if s.is_empty() { String::new() } else { format!(" — {s}") };
    match status {
        400 => format!(
            "the website rejected the request body for '{api_path}' — a version must be a \
             positive integer, visibility one of private/shared/public, tag_uuids a list of \
             strings (400){}",
            suffix(msg)
        ),
        401 => "sign in again — the website rejected or did not receive your access token (401)"
            .to_string(),
        403 => format!(
            "the website rejected this registry write; the credential is read-only or lacks \
             access (403){}",
            suffix(msg)
        ),
        404 => format!(
            "no dataset / uploaded version to act on at '{api_path}' — upload it (or publish a \
             version that exists) first (404){}",
            suffix(msg)
        ),
        409 => format!(
            "you do not own this dataset uuid on the website (409){} — mint a new copy with \
             dataset_remint_uuid and publish that instead",
            suffix(msg)
        ),
        c if c >= 500 => format!(
            "the website is failing right now (HTTP {c}) — retryable{}",
            suffix(msg)
        ),
        c => format!("registry write '{api_path}' failed (HTTP {c}): {msg}"),
    }
}

/// POST a JSON body to the account website under the OAuth Bearer and return the
/// parsed response, clearing a rejected token on 401. Shared by both commands so the
/// signing/error path is written once.
async fn post_website_json(
    settings: &SettingsState,
    api_path: &str,
    body: Value,
) -> Result<Value, String> {
    let base = crate::auth::base_url();
    // A missing token is a hard, actionable error for a write (unlike the reads).
    let token = crate::auth::get_access_token(settings)?;
    let client = crate::auth::hub_client_builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("{base}{api_path}");
    let resp = client
        .post(&url)
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("cannot reach the website at {url}: {e}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        crate::auth::token_rejected(settings);
    }
    if !status.is_success() {
        return Err(publish_http_error(resp, api_path).await);
    }
    resp.json::<Value>()
        .await
        .map_err(|e| format!("unexpected response from '{api_path}': {e}"))
}

/// Promote a dataset version to its live pin — the explicit act that crosses the
/// publish boundary (sync-v2.md §8.4, INV-17). `version` selects which already-
/// uploaded version to pin; omitted, the website pins the newest uploaded one.
/// `visibility` (`private|shared|public`) and `tag_uuids` are optional metadata
/// applied in the same transaction. Carries no bytes — run `dataset_export_upload`
/// first. Owner-only, account-Bearer, against the account website. Returns the
/// website's `{ dataset, current_version }`.
#[tauri::command]
pub async fn sync_publish_dataset_version(
    settings: State<'_, SettingsState>,
    uuid: String,
    version: Option<i64>,
    visibility: Option<String>,
    tag_uuids: Option<Vec<String>>,
) -> Result<Value, String> {
    let uuid = check_uuid(&uuid)?;
    if let Some(v) = version {
        check_version(v)?;
    }
    let body = publish_body(&uuid, version, visibility.as_deref(), tag_uuids.as_deref());
    let rj = post_website_json(&settings, PUBLISH_PATH, body).await?;
    log::info!(
        "[sync] published dataset '{uuid}' to version {}",
        rj["current_version"].as_i64().unwrap_or(0)
    );
    Ok(rj)
}

/// Take a published version down, or restore it (sync-v2.md §11.5, INV-8/INV-17/INV-22).
/// `available = false` marks the version unavailable: a fresh read of that exact
/// version then returns `410 Gone`, WITHOUT deleting its bytes. `available = true`
/// restores it. This is metadata-only — it never moves `current_version` (INV-17) and
/// subscribers keep their installed copies (INV-22); the downstream effect is that a
/// copy's next `sync_check_availability` records it `withdrawn`. Owner-only, account-
/// Bearer, against the account website. Returns the website's `{ uuid, version,
/// available }`.
#[tauri::command]
pub async fn sync_set_dataset_availability(
    settings: State<'_, SettingsState>,
    uuid: String,
    version: i64,
    available: bool,
) -> Result<Value, String> {
    let uuid = check_uuid(&uuid)?;
    check_version(version)?;
    let body = availability_body(&uuid, version, available);
    let rj = post_website_json(&settings, AVAILABILITY_PATH, body).await?;
    log::info!(
        "[sync] version {version} of dataset '{uuid}' marked {}",
        if available { "available" } else { "withdrawn" }
    );
    Ok(rj)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_body_omits_absent_fields_and_keeps_uuid() {
        // Newest-upload, no metadata: only the uuid goes on the wire.
        let v = publish_body("ds", None, None, None);
        assert_eq!(v, json!({ "uuid": "ds" }));
    }

    #[test]
    fn publish_body_includes_only_supplied_values() {
        let tags = vec!["t1".to_string(), "t2".to_string()];
        let v = publish_body("ds", Some(7), Some("shared"), Some(&tags));
        assert_eq!(v["uuid"], json!("ds"));
        assert_eq!(v["version"], json!(7));
        assert_eq!(v["visibility"], json!("shared"));
        assert_eq!(v["tag_uuids"], json!(["t1", "t2"]));

        // A version of 0 is treated as absent (the website would 400 on it).
        let v = publish_body("ds", Some(0), None, None);
        assert!(v.get("version").is_none());

        // Blank visibility and an empty tag list are dropped, not sent.
        let v = publish_body("ds", None, Some("   "), Some(&[]));
        assert!(v.get("visibility").is_none());
        assert!(v.get("tag_uuids").is_none());
    }

    #[test]
    fn availability_body_always_emits_available_flag() {
        // Both directions carry the explicit boolean — never defaulted/omitted.
        assert_eq!(
            availability_body("ds", 3, false),
            json!({ "uuid": "ds", "version": 3, "available": false })
        );
        assert_eq!(
            availability_body("ds", 3, true),
            json!({ "uuid": "ds", "version": 3, "available": true })
        );
    }

    #[test]
    fn check_uuid_trims_and_rejects_empty() {
        assert_eq!(check_uuid("  ds  ").unwrap(), "ds");
        assert!(check_uuid("   ").is_err());
    }

    #[test]
    fn check_version_requires_positive() {
        assert!(check_version(1).is_ok());
        assert!(check_version(0).is_err());
        assert!(check_version(-2).is_err());
    }
}
