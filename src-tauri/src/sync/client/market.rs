//! Online dataset market — the website catalog as a browse/subscribe surface.
//!
//! Compiled on **all** platforms (reqwest + serde_json only, like the rest of
//! `client/`), and deliberately **role-agnostic**: market consumption never
//! grants or needs journal authority, so a `role = "hub"` workspace browses and
//! downloads from the cloud exactly like a follower (the authority-only paths —
//! inbox drain, handover, journal append — keep their `role == "hub"` gates
//! elsewhere). The effective origin is `pc_url` when set, else the signed-in
//! account's website ([`super::auth::hub_base_or_account`]), which is what makes
//! this the pull path for hubs that never configure an upstream.
//!
//! Model (docs/design/market.md):
//! * `GET /api/v1/market` is the rich catalog — description, tags, visibility,
//!   `is_owner`/`is_subscribed`, the pinned `published_version`. When the host
//!   does not advertise the `market` capability yet, the list **degrades** to
//!   `GET /api/v1/datasets?lite=1` rows (public/self only, no subscribe or
//!   visibility controls) so the surface works against today's website.
//! * Download is *not* reimplemented here: the frontend calls the existing
//!   `dataset_sync_snapshot`, which adopts a registry copy into the read-only
//!   download root under the `owned-locally` guard.
//! * Subscribe/unsubscribe and the owner's visibility toggle ride the same
//!   Bearer path as uploads (`hub_request` → website branch).

#![allow(unused_imports)]
use std::io::{BufReader, Write as _};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};

use crate::datasets;
use crate::settings::{self, SettingsState};
use super::*;

/// The capability the website must advertise on `/api/v1/cluster` before the
/// rich market routes are attempted (INV-20 default-deny, same pattern as
/// `registry_reads`).
pub(crate) const MARKET_FEATURE: &str = "market";

/// Browse filters for [`market_list`]: multi-type chips, one tag, free text.
/// All optional — an empty query lists everything the caller may read.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MarketQuery {
    #[serde(default)]
    pub types: Vec<String>,
    pub tag: Option<String>,
    pub q: Option<String>,
}

/// `GET /api/v1/market?type=a&type=b&tag=&q=` — each type is a repeated key,
/// mirroring the web market action's filter shape. Pure, so the wire contract
/// is unit-tested without a host.
pub(crate) fn market_query_string(query: &MarketQuery) -> String {
    let mut pairs: Vec<(&str, String)> = Vec::new();
    for t in &query.types {
        if !t.is_empty() {
            pairs.push(("type", t.clone()));
        }
    }
    if let Some(tag) = query.tag.as_ref().filter(|s| !s.is_empty()) {
        pairs.push(("tag", tag.to_string()));
    }
    if let Some(q) = query.q.as_ref().filter(|s| !s.is_empty()) {
        pairs.push(("q", q.to_string()));
    }
    build_query(&pairs)
}

/// Map one rich `/api/v1/market` row onto the render-ready shape. The website
/// already computed `is_owner`/`is_subscribed` on its principal; a missing flag
/// (older host mid-rollout) falls back to an email match client-side.
pub(crate) fn map_market_row(row: &Value, email: Option<&str>) -> Value {
    let owner = row["owner"]
        .as_str()
        .or_else(|| row["user_id"].as_str())
        .unwrap_or("")
        .to_string();
    let is_owner = row["is_owner"].as_bool().unwrap_or_else(|| {
        !owner.is_empty() && email.map(|e| e.eq_ignore_ascii_case(&owner)).unwrap_or(false)
    });
    json!({
        "uuid": row["uuid"],
        "name": row["name"],
        "description": row["description"],
        "dataset_type": row["dataset_type"],
        "visibility": row["visibility"],
        "language": row["language"],
        "is_owner": is_owner,
        "is_subscribed": row["is_subscribed"].as_bool().unwrap_or(false),
        "owner": owner,
        "tags": row["tags"],
        "media_count": row["media_count"],
        "card_count": row["card_count"],
        "total_bytes": row["total_bytes"],
        "file_count": row["file_count"],
        "updated": row["updated"],
        "published_version": row["published_version"],
        "lite": false,
    })
}

/// Map one `GET /api/v1/datasets?lite=1` row onto the same shape in degraded
/// mode: uuid/name/type/counts/`updated` are the whole contract (dev guide §3),
/// so description/tags stay null and `is_owner` is decided by the email match
/// the lite list enables through its `user_id` field.
pub(crate) fn map_lite_row(row: &Value, email: Option<&str>) -> Value {
    let owner = row["user_id"].as_str().unwrap_or("").to_string();
    let is_owner = !owner.is_empty()
        && email.map(|e| e.eq_ignore_ascii_case(&owner)).unwrap_or(false);
    json!({
        "uuid": row["uuid"],
        "name": row["name"],
        "description": Value::Null,
        "dataset_type": row["dataset_type"],
        "visibility": row["visibility"],
        "language": Value::Null,
        "is_owner": is_owner,
        "is_subscribed": false,
        "owner": owner,
        "tags": Value::Null,
        "media_count": row["media_count"],
        "card_count": row["card_count"],
        "total_bytes": Value::Null,
        "file_count": Value::Null,
        "updated": row["updated"],
        "published_version": row["current_version"],
        "lite": true,
    })
}

/// The coarse market sync-state bucket for one catalog row, decided by the
/// local `dataset_sync_state` row (None = never pulled here). Mirrors
/// [`crate::sync::client::status`]'s vocabulary so the badges agree across
/// surfaces; a row whose bytes were evicted (empty hash sentinel, INV-9) is
/// "not_downloaded" again while its provenance row survives.
/// Pure, so the mapping is unit-tested without settings or a host.
pub(crate) fn sync_state_bucket(row: Option<&SyncStateEntry>) -> &'static str {
    match row {
        None => "not_downloaded",
        Some(r) if r.overall_hash.is_empty() => "not_downloaded",
        Some(r) if r.cursor.is_none() => "needs_resync",
        Some(_) => "downloaded",
    }
}

/// The visibility vocabulary the website route accepts (mirror of its
/// `VISIBILITIES`). Pure gate so a bad value fails locally with an actionable
/// message instead of a round-trip 400.
pub(crate) fn is_known_visibility(v: &str) -> bool {
    matches!(v, "private" | "shared" | "public")
}

/// Send one authenticated market mutation (`POST`/`DELETE` subscribe, `PATCH`
/// visibility) and return the host's JSON. A 401 clears the stored token like
/// every other website path (no silent-refresh route exists); other non-2xx
/// codes map through [`hub_http_error`].
async fn send_market_json(
    settings: &SettingsState,
    base: &str,
    method: &str,
    api_path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let client = crate::auth::hub_client_builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let body_bytes = match &body {
        Some(v) => serde_json::to_vec(v).map_err(|e| e.to_string())?,
        None => Vec::new(),
    };
    let mut req = match method {
        "POST" => client.post(format!("{base}{api_path}")),
        "DELETE" => client.delete(format!("{base}{api_path}")),
        "PATCH" => client
            .patch(format!("{base}{api_path}"))
            .header("Content-Type", "application/json"),
        other => return Err(format!("unsupported market method {other}")),
    };
    if !body_bytes.is_empty() {
        req = req.body(body_bytes.clone());
    }
    let req = hub_request(req, settings, method, api_path, "", &body_bytes).await?;
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Cannot reach hub at {base}: {e}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        // Renews in place; the next market read carries the fresh token. The
        // session itself survives (only a refused refresh ends it).
        crate::auth::token_rejected(settings).await;
    }
    if !status.is_success() {
        return Err(hub_http_error(resp, api_path).await);
    }
    // Subscribe echoes `{ ok, subscribed }`; a 2xx with a non-JSON body still
    // means "done", so synthesize the echo rather than failing on it.
    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(_) => Ok(json!({ "ok": true })),
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Browse the online market. Against a host that advertises `market` this is
/// the rich `GET /api/v1/market`; otherwise it degrades to the lite catalog.
/// Every row is enriched with its local sync-state bucket, and the payload
/// carries the sections the Pull tab's market branch renders: `datasets`
/// (catalog), `downloads` (this device's registry adoptions, even when the
/// publisher vanished from the catalog), `published` (what this device uploaded
/// to the account, with the stored publish pin).
#[tauri::command]
pub async fn market_list(
    settings: State<'_, SettingsState>,
    query: Option<MarketQuery>,
) -> Result<Value, String> {
    let query = query.unwrap_or_default();
    let base = hub_base_or_account(&settings)?;
    if crate::sync::hub_kind(&base) != crate::sync::HubKind::Website {
        return Err(format!(
            "the market lives on the website origin, but the configured hub is {base} (a LAN PC); \
             use the hub catalog for LAN pulls"
        ));
    }
    let email = crate::auth::get_stored_email(&settings);
    let cluster = fetch_cluster_at(&settings, &base).await;
    let market_ok = advertises(&cluster, MARKET_FEATURE);

    // `is_owner` is computed by the host on its own principal; the client-side
    // email match is only a fallback, so request the rich path when either the
    // host advertises `market` or a Bearer token is attached regardless.
    let mut degraded_reason: Option<String> = None;
    let mut rows: Vec<Value> = Vec::new();
    if market_ok {
        let qs = market_query_string(&query);
        match hub_get_json_at(&settings, &base, "/api/v1/market", &qs).await {
            Ok(v) => {
                rows = v
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|r| map_market_row(r, email.as_deref()))
                    .collect();
            }
            Err(e) => {
                // A 404 means the advertisement outran the deploy — degrade
                // honestly. Anything else (offline, 401, 5xx) is a real failure
                // the surface must show rather than mask with a thinner list.
                if !e.contains("(404)") {
                    return Err(e);
                }
                degraded_reason = Some(e);
            }
        }
    } else {
        degraded_reason = Some(if email.is_none() {
            "not signed in — the market needs your account".to_string()
        } else {
            format!("host at {base} does not advertise the 'market' capability yet")
        });
    }
    let lite = rows.is_empty() && degraded_reason.is_some();
    if lite {
        let qs = if query.types.len() == 1 {
            format!("lite=1&type={}", query.types[0])
        } else {
            "lite=1".to_string()
        };
        let v = hub_get_json_at(&settings, &base, "/api/v1/datasets", &qs)
            .await
            .map_err(|e| degraded_reason.clone().unwrap_or(e))?;
        rows = v
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|r| map_lite_row(r, email.as_deref()))
            .collect();
    }

    // One app-DB read folds in every local fact the sections need.
    let conn = datasets::dictation::open_app_db(&settings)?;
    ensure_sync_tables(&conn)?;
    let sync_rows: Vec<SyncStateEntry> = local_sync_entries(&conn)?;
    let uploads: Vec<Value> = crate::sync::upload::upload_state_rows(&conn)?;
    let upload_by_uuid: std::collections::HashMap<String, &Value> = uploads
        .iter()
        .filter_map(|u| u["dataset_uuid"].as_str().map(|k| (k.to_string(), u)))
        .collect();

    // Catalog rows: attach the local bucket + withdrawal flag.
    for row in &mut rows {
        let uuid = row["uuid"].as_str().unwrap_or("").to_string();
        let sr = sync_rows.iter().find(|s| s.dataset_uuid == uuid);
        row["sync_state"] = json!(sync_state_bucket(sr));
        row["withdrawn"] = json!(sr.map(|s| s.availability == "withdrawn").unwrap_or(false));
        if let Some(u) = upload_by_uuid.get(&uuid) {
            row["uploaded_version"] = u["version"].clone();
            // The locally-known publish pin; the market row's own
            // `published_version` (host-authoritative) wins when present.
            if row["published_version"].is_null() {
                row["published_version"] = u["current_version"].clone();
            }
        }
    }

    // Registry adoptions on this device, whether or not the catalog still
    // lists them (a withdrawn copy stays practicable, INV-22).
    let mut downloads: Vec<Value> = Vec::new();
    for s in sync_rows.iter().filter(|s| s.upstream_kind == "registry") {
        let name = rows
            .iter()
            .find(|r| r["uuid"].as_str() == Some(s.dataset_uuid.as_str()))
            .and_then(|r| r["name"].as_str())
            .map(|n| n.to_string())
            .or_else(|| {
                datasets::find_dataset_dir_typed(&settings, &s.dataset_uuid)
                    .ok()
                    .and_then(|(dir, _)| datasets::read_info_opt(&dir).map(|i| i.name))
            })
            .unwrap_or_else(|| s.dataset_uuid.clone());
        downloads.push(json!({
            "uuid": s.dataset_uuid,
            "name": name,
            "on_disk": !s.overall_hash.is_empty(),
            "sync_state": sync_state_bucket(Some(s)),
            "availability": s.availability,
            "withdrawn": s.availability == "withdrawn",
            "published_version": s.published_version,
            "upstream_origin": s.upstream_origin,
            "bytes": s.bytes,
            "file_count": s.file_count,
            "synced_at": s.synced_at,
        }));
    }

    // What this device uploaded under its own account, joined with the catalog
    // row (name/visibility) when the host still lists it.
    let published: Vec<Value> = uploads
        .iter()
        .map(|u| {
            let uuid = u["dataset_uuid"].as_str().unwrap_or("").to_string();
            let cat = rows.iter().find(|r| r["uuid"].as_str() == Some(uuid.as_str()));
            json!({
                "uuid": uuid,
                "version": u["version"],
                "current_version": u["current_version"],
                "published_at": u["published_at"],
                "uploaded_at": u["uploaded_at"],
                "archive_sha256": u["archive_sha256"],
                "name": cat.map(|c| c["name"].clone()).unwrap_or(Value::Null),
                "dataset_type": cat.map(|c| c["dataset_type"].clone()).unwrap_or(Value::Null),
                "visibility": cat.map(|c| c["visibility"].clone()).unwrap_or(Value::Null),
                "is_subscribed": cat.map(|c| c["is_subscribed"].clone()).unwrap_or(Value::Null),
            })
        })
        .collect();

    Ok(json!({
        "datasets": rows,
        "downloads": downloads,
        "published": published,
        "origin": base,
        "lite": lite,
        "market_available": market_ok && !lite,
        "can_subscribe": market_ok && !lite,
        "signed_in": email.is_some(),
        "degraded_reason": degraded_reason.unwrap_or_default(),
    }))
}

/// Subscribe the signed-in account to a `shared` market dataset. Errors are
/// actionable verbatim (404 → the publisher does not offer it to you).
#[tauri::command]
pub async fn market_subscribe(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<Value, String> {
    market_subscription(&settings, &uuid, true).await
}

/// Undo a [`market_subscription`] subscription. Never deletes the local copy —
/// evict/forget stay explicit, separate actions.
#[tauri::command]
pub async fn market_unsubscribe(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<Value, String> {
    market_subscription(&settings, &uuid, false).await
}

async fn market_subscription(settings: &SettingsState, uuid: &str, on: bool) -> Result<Value, String> {
    let base = hub_base_or_account(settings)?;
    if crate::sync::hub_kind(&base) != crate::sync::HubKind::Website {
        return Err("subscriptions are a website fact; the configured hub is a LAN PC".to_string());
    }
    let verb = if on { "subscribed" } else { "unsubscribed" };
    let v = send_market_json(
        settings,
        &base,
        if on { "POST" } else { "DELETE" },
        &format!("/api/v1/market/{uuid}/subscribe"),
        None,
    )
    .await
    .map_err(|e| format!("{e} — nothing changed for '{uuid}'"))?;
    Ok(json!({ "uuid": uuid, "subscribed": on, "echo": v, "result": verb }))
}

/// Owner-only visibility toggle (`PATCH /api/v1/market/{uuid}`). The host's
/// echo of `current_version` refreshes the local publish pin through the same
/// seam every other publish echo uses, so the My Published badge stays honest.
#[tauri::command]
pub async fn market_set_visibility(
    settings: State<'_, SettingsState>,
    uuid: String,
    visibility: String,
) -> Result<Value, String> {
    if !is_known_visibility(&visibility) {
        return Err(format!(
            "visibility must be one of private | shared | public (got '{visibility}')"
        ));
    }
    let base = hub_base_or_account(&settings)?;
    if crate::sync::hub_kind(&base) != crate::sync::HubKind::Website {
        return Err("visibility is a website fact; the configured hub is a LAN PC".to_string());
    }
    let v = send_market_json(
        &settings,
        &base,
        "PATCH",
        &format!("/api/v1/market/{uuid}"),
        Some(json!({ "visibility": visibility })),
    )
    .await?;
    if let Some(pin) = v["dataset"]["current_version"].as_i64().or_else(|| v["current_version"].as_i64()) {
        let conn = datasets::dictation::open_app_db(&settings)?;
        crate::sync::upload::record_publish_echo(&conn, &uuid, pin);
    }
    Ok(json!({ "uuid": uuid, "visibility": visibility, "echo": v }))
}

/// Who the market surface is speaking as: the stored account email, no network
/// call. Drives the sign-in empty state and the client-side ownership fallback.
#[tauri::command]
pub async fn market_whoami(settings: State<'_, SettingsState>) -> Result<Value, String> {
    let email = crate::auth::get_stored_email(&settings);
    Ok(json!({
        "email": email.clone(),
        "signed_in": email.is_some(),
        "origin": crate::auth::base_url(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(uuid: &str, hash: &str, cursor: Option<i64>, kind: &str) -> SyncStateEntry {
        SyncStateEntry {
            dataset_uuid: uuid.to_string(),
            overall_hash: hash.to_string(),
            synced_at: "now".into(),
            bytes: 0,
            file_count: 0,
            cursor,
            upstream_kind: kind.to_string(),
            upstream_origin: String::new(),
            availability: "listed".to_string(),
            published_version: None,
            resync_reason: None,
            epoch: 0,
        }
    }

    #[test]
    fn market_query_string_encodes_filters() {
        let q = MarketQuery {
            types: vec!["dictation".into(), "card".into()],
            tag: Some("t-1".into()),
            q: Some("hello world".into()),
        };
        assert_eq!(
            market_query_string(&q),
            "type=dictation&type=card&tag=t-1&q=hello%20world"
        );
        // Empty values are dropped, not sent as bare keys.
        let blank = MarketQuery {
            types: vec![String::new()],
            tag: Some(String::new()),
            q: None,
        };
        assert_eq!(market_query_string(&blank), "");
    }

    #[test]
    fn sync_state_bucket_matches_the_status_vocabulary() {
        assert_eq!(sync_state_bucket(None), "not_downloaded");
        // Evicted base: provenance row survives with the empty-hash sentinel.
        assert_eq!(
            sync_state_bucket(Some(&entry("u", "", Some(3), "registry"))),
            "not_downloaded"
        );
        assert_eq!(
            sync_state_bucket(Some(&entry("u", "h", None, "registry"))),
            "needs_resync"
        );
        assert_eq!(
            sync_state_bucket(Some(&entry("u", "h", Some(1), "registry"))),
            "downloaded"
        );
    }

    #[test]
    fn lite_row_marks_ownership_by_email_match() {
        let row = json!({
            "uuid": "d1", "name": "Audiobook", "dataset_type": "book",
            "user_id": "me@example.com", "media_count": 4, "updated": "2026-10-01T00:00:00Z"
        });
        let mine = map_lite_row(&row, Some("me@example.com"));
        assert_eq!(mine["is_owner"], json!(true));
        assert_eq!(mine["lite"], json!(true));
        assert_eq!(mine["visibility"], Value::Null);

        let theirs = map_lite_row(&row, Some("other@example.com"));
        assert_eq!(theirs["is_owner"], json!(false));
        // Anonymous probe: nothing can be "owned".
        assert_eq!(map_lite_row(&row, None)["is_owner"], json!(false));
    }

    #[test]
    fn market_row_honours_host_flags_with_email_fallback() {
        let row = json!({
            "uuid": "d1", "name": "Cards", "dataset_type": "card", "visibility": "shared",
            "owner": "a@b.c", "is_owner": false, "is_subscribed": true,
            "tags": [{ "uuid": "t", "name": "German" }], "published_version": 3
        });
        let mapped = map_market_row(&row, Some("a@b.c"));
        // The host's own verdict wins over the client-side match.
        assert_eq!(mapped["is_owner"], json!(false));
        assert_eq!(mapped["is_subscribed"], json!(true));
        assert_eq!(mapped["published_version"], json!(3));
        assert_eq!(mapped["lite"], json!(false));

        // A host mid-rollout that omits is_owner: the email match decides.
        let bare = json!({ "uuid": "d1", "name": "Cards", "user_id": "a@b.c" });
        assert_eq!(map_market_row(&bare, Some("a@b.c"))["is_owner"], json!(true));
        assert_eq!(map_market_row(&bare, Some("z@b.c"))["is_owner"], json!(false));
    }

    #[test]
    fn visibility_vocabulary_matches_the_website_route() {
        assert!(is_known_visibility("private"));
        assert!(is_known_visibility("shared"));
        assert!(is_known_visibility("public"));
        assert!(!is_known_visibility("Public"));
        assert!(!is_known_visibility(""));
    }
}
