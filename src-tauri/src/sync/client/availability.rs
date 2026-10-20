//! Registry takedown consumption (sync-v2.md §11.5 / INV-22) — the app-side half.
//!
//! The website lets a publisher take a *published version* down without deleting
//! its bytes (`POST /api/v1/publish/availability`), after which a read of that
//! exact version returns `410 Gone`. An app that adopted that version must learn
//! about it and mark its local copy `withdrawn` — keeping the directory and its
//! progress fully usable (INV-22) while only fresh re-reads are refused. The
//! `dataset_sync_state.availability` column, its prune protection
//! (`prune_never_deletes_withdrawn`), and the Status surface already existed; this
//! module is the producer that finally writes them.
//!
//! Contract facts (verified against the website `registry.ts::resolveVersionRow`):
//! * The signal is a `410` on an **explicit** version read. A pin read (no
//!   version) silently walks to the next-lower *available* version — that is a
//!   version change, not a takedown — so the check always asks for the exact
//!   `published_version` the copy was adopted at.
//! * Target is the copy's recorded `upstream_origin` (a website registry host),
//!   not the currently-connected `pc_base` — provenance-faithful (INV-19). A LAN
//!   mirror has no registry route, so it is never selected here.
//! * Auth is the account Bearer (`crate::auth::get_access_token`), optional: a
//!   public dataset's manifest is readable anonymously, so a missing token is not
//!   fatal and the takedown check still resolves either way.
//!
//! Compiled on all platforms (a pure reqwest + app-DB read/write client path, like
//! the rest of `sync/client`). Pure addition: nothing here is reached by the round,
//! writeback, or adoption paths.

use std::time::Duration;

use rusqlite::Connection;
use serde_json::{json, Value};
use tauri::State;

use crate::datasets;
use crate::settings::SettingsState;
use super::{ensure_sync_tables, set_availability};

/// Map a registry manifest read's HTTP status to the availability to record.
/// 2xx → `listed`. `410` (this exact published version was taken down, §11.5) →
/// `withdrawn`. Anything else → `None`: a 401/404/5xx or a transport error is not
/// a takedown signal, so the last known value is kept rather than clobbered by a
/// transient failure (§19: report the unknown, do not guess it).
fn availability_for_status(status: u16) -> Option<&'static str> {
    if (200..300).contains(&status) {
        Some("listed")
    } else if status == 410 {
        Some("withdrawn")
    } else {
        None
    }
}

/// Registry-adopted copies worth checking: those that pinned a version and know
/// their upstream origin. A LAN mirror (`desktop_hub`) has no registry manifest
/// route to ask, so it is never selected here. Returns `(uuid, version, origin)`.
fn registry_adoptions(conn: &Connection) -> Result<Vec<(String, i64, String)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT dataset_uuid, published_version, upstream_origin FROM dataset_sync_state \
             WHERE upstream_kind = 'registry' AND published_version IS NOT NULL \
               AND upstream_origin <> '' ORDER BY dataset_uuid",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Check whether each registry-adopted dataset is still available where it was
/// adopted from (sync-v2.md §11.5 / INV-22). For every copy that pinned a version,
/// re-reads `GET {upstream_origin}/api/v1/registry/{uuid}/manifest?version={N}` —
/// the exact version it holds, not the pin. A `200` marks it `listed`; a `410 Gone`
/// (the publisher took that version down) marks it `withdrawn`, and the copy is
/// **kept**: its files and progress stay usable, only fresh re-reads are refused.
/// Any other status, or an unreachable origin, leaves the last value untouched and
/// is reported rather than guessed. Website-only by nature (registry reads are a
/// website-host feature); on a LAN-hub-only device this finds no rows and reports
/// zero checks (surfaced in the result, not an error).
#[tauri::command]
pub async fn sync_check_availability(settings: State<'_, SettingsState>) -> Result<Value, String> {
    // Read the candidate rows with the connection scoped to this synchronous
    // block — a `Connection` is not `Send`, so it is dropped before any `.await`.
    let rows = {
        let conn = datasets::dictation::open_app_db(&settings)?;
        ensure_sync_tables(&conn)?;
        registry_adoptions(&conn)?
    };

    let client = crate::auth::hub_client_builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    // Optional: a public dataset's manifest is readable anonymously, so a missing
    // token is not fatal here — the takedown check still resolves either way.
    // Awaited so a token due for renewal is renewed before it is attached.
    let token = crate::auth::access_token(&settings).await.ok();

    let mut results: Vec<Value> = Vec::new();
    for (uuid, version, origin) in &rows {
        let url = format!("{origin}/api/v1/registry/{uuid}/manifest?version={version}");
        let mut req = client.get(&url);
        if let Some(t) = &token {
            req = req.bearer_auth(t);
        }
        let status = match req.send().await {
            Ok(resp) => resp.status().as_u16(),
            Err(e) => {
                results.push(json!({
                    "uuid": uuid,
                    "version": version,
                    "availability": "unknown",
                    "error": format!("cannot reach the registry at {origin}: {e}"),
                }));
                continue;
            }
        };
        match availability_for_status(status) {
            Some(av) => {
                // Re-open per write, outside the `.await` above (not `Send`-safe).
                if let Ok(conn) = datasets::dictation::open_app_db(&settings) {
                    let _ = set_availability(&conn, uuid, av);
                }
                results.push(json!({
                    "uuid": uuid,
                    "version": version,
                    "http_status": status,
                    "availability": av,
                }));
            }
            None => results.push(json!({
                "uuid": uuid,
                "version": version,
                "http_status": status,
                "availability": "unknown",
                "note": "not a takedown signal; last availability kept",
            })),
        }
    }

    let withdrawn = results
        .iter()
        .filter(|r| r["availability"] == json!("withdrawn"))
        .count();
    log::info!(
        "[sync] availability check: {} registry copy(ies), {withdrawn} now withdrawn",
        results.len()
    );

    Ok(json!({
        "checked": results.len(),
        "withdrawn": withdrawn,
        "results": results,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_for_status_maps_only_2xx_and_410() {
        assert_eq!(availability_for_status(200), Some("listed"));
        assert_eq!(availability_for_status(204), Some("listed"));
        assert_eq!(availability_for_status(410), Some("withdrawn"));
        // A transient/auth/not-found status is neither a takedown nor a restore.
        assert_eq!(availability_for_status(401), None);
        assert_eq!(availability_for_status(404), None);
        assert_eq!(availability_for_status(500), None);
    }

    #[test]
    fn registry_adoptions_selects_only_pinned_registry_rows() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_sync_tables(&conn).unwrap();
        // A LAN mirror: no origin, no version — never selected.
        conn.execute(
            "INSERT INTO dataset_sync_state (dataset_uuid, overall_hash, synced_at, upstream_kind) \
             VALUES ('lan', 'h', 'now', 'desktop_hub')",
            [],
        )
        .unwrap();
        // A registry copy with an origin and a pinned version — selected.
        conn.execute(
            "INSERT INTO dataset_sync_state (dataset_uuid, overall_hash, synced_at, upstream_kind, upstream_origin, published_version) \
             VALUES ('reg', 'h', 'now', 'registry', 'https://x', 7)",
            [],
        )
        .unwrap();
        // A registry copy that never resolved a pin (NULL version) — not selected.
        conn.execute(
            "INSERT INTO dataset_sync_state (dataset_uuid, overall_hash, synced_at, upstream_kind, upstream_origin) \
             VALUES ('reg-nov', 'h', 'now', 'registry', 'https://x')",
            [],
        )
        .unwrap();

        let rows = registry_adoptions(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "reg");
        assert_eq!(rows[0].1, 7);
        assert_eq!(rows[0].2, "https://x");
    }
}
