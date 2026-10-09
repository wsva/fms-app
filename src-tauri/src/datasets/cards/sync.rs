use std::cmp::Ordering;
use std::fs;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::datasets::cards::{find_card_dataset_dir, open_card_db};
use crate::datasets::{canonical_stamp, stamps_cmp, touch_info};
use crate::settings::SettingsState;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct SyncPullResponse {
    pub dataset_uuid: String,
    pub cards: Vec<SyncCard>,
    pub tags: Vec<SyncTag>,
    pub server_time: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct SyncCard {
    pub uuid: String,
    pub question: String,
    pub suggestion: String,
    pub answer: String,
    pub note: String,
    pub familiarity: i32,
    pub question_hash: Option<String>,
    pub source_card_uuid: Option<String>,
    pub source_dataset_uuid: Option<String>,
    pub deleted_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct SyncTag {
    pub uuid: String,
    pub name: String,
    pub color: Option<String>,
    pub deleted_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct SyncPushRequest {
    pub cards: Vec<SyncCard>,
    pub tags: Vec<SyncTag>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct SyncPushResponse {
    pub accepted: i32,
    pub conflicts: Vec<SyncConflict>,
    pub server_time: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct SyncConflict {
    pub uuid: String,
    pub local_updated_at: String,
    pub remote_updated_at: String,
    pub winner: String,
}

#[derive(Clone, Serialize, Debug)]
pub struct SyncResult {
    pub pulled: i32,
    pub pushed: i32,
    pub conflicts: i32,
    pub server_time: String,
}

// ---------------------------------------------------------------------------
// Sync Logic
// ---------------------------------------------------------------------------

// The origin is the compile-time `crate::auth::BASE_URL` — the same origin login is
// verified against — and never a per-dataset field. These requests carry the OAuth
// bearer token, so a `sync_url` inside an imported `info.json` could have sent that
// token to a server of the importer's choosing.

/// Pull changes from the online server for a dataset.
async fn pull_changes(
    dataset_uuid: &str,
    since: &str,
    access_token: &str,
) -> Result<SyncPullResponse, String> {
    let url = format!(
        "{}/api/card/sync/pull?dataset_uuid={}&since={}",
        crate::auth::BASE_URL.trim_end_matches('/'),
        dataset_uuid,
        since,
    );

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await
        .map_err(|e| format!("Pull request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Pull failed with status: {}", response.status()));
    }

    response
        .json::<SyncPullResponse>()
        .await
        .map_err(|e| format!("Failed to parse pull response: {}", e))
}

/// Push local changes to the online server.
async fn push_changes(
    dataset_uuid: &str,
    changes: &SyncPushRequest,
    access_token: &str,
) -> Result<SyncPushResponse, String> {
    let url = format!(
        "{}/api/card/sync/push?dataset_uuid={}",
        crate::auth::BASE_URL.trim_end_matches('/'),
        dataset_uuid,
    );

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .json(changes)
        .send()
        .await
        .map_err(|e| format!("Push request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Push failed with status: {}", response.status()));
    }

    response
        .json::<SyncPushResponse>()
        .await
        .map_err(|e| format!("Failed to parse push response: {}", e))
}

/// Order a local and a remote `updated_at` as instants rather than as text.
///
/// The two sides come from different producers: Rust writes
/// `2026-10-09T07:12:34.567890+00:00`, while an HTTP peer may send
/// `2026-10-09T07:12:34.567Z` or a stamp carrying its own offset. Text order
/// across those shapes is not instant order — `'Z'` out-ranks `'+'` and every
/// digit — and the answer decides whether a local edit survives, so both sides
/// are parsed whenever they can be. A stamp that no form can read falls back to
/// text, which keeps the change flowing instead of dropping it.
fn stamp_order(local: &str, remote: &str) -> Ordering {
    stamps_cmp(local, remote).unwrap_or_else(|| local.cmp(remote))
}

/// Apply pulled cards to the local database.
fn apply_pulled_cards(conn: &Connection, cards: &[SyncCard]) -> Result<i32, String> {
    let mut count = 0i32;

    for card in cards {
        // Check if card exists locally
        let existing: Option<String> = conn
            .query_row(
                "SELECT updated_at FROM card WHERE uuid = ?1",
                rusqlite::params![card.uuid],
                |row| row.get(0),
            )
            .ok();

        match existing {
            None => {
                // New card from remote - insert if not deleted
                if card.deleted_at.is_none() {
                    conn.execute(
                        "INSERT INTO card (uuid, question, suggestion, answer, note, familiarity, \
                         question_hash, source_card_uuid, source_dataset_uuid, deleted_at, \
                         created_at, updated_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, ?11)",
                        rusqlite::params![
                            card.uuid,
                            card.question,
                            card.suggestion,
                            card.answer,
                            card.note,
                            card.familiarity,
                            card.question_hash,
                            card.source_card_uuid,
                            card.source_dataset_uuid,
                            // Stored in one shape: the incremental cursor below
                            // (`updated_at > ?1`) is a text comparison SQLite can
                            // only win if every row shares a width and a suffix.
                            canonical_stamp(&card.created_at),
                            canonical_stamp(&card.updated_at),
                        ],
                    )
                    .map_err(|e| e.to_string())?;
                    count += 1;
                }
            }
            Some(local_updated_at) => {
                // Card exists - compare the two stamps as instants.
                let order = stamp_order(&local_updated_at, &card.updated_at);
                if let Some(ref remote_deleted) = card.deleted_at {
                    // Remote deleted the card. An equal stamp honours the delete:
                    // the tombstone is not newer than the local row, so the local
                    // edit cannot have happened after it.
                    if order != Ordering::Greater {
                        conn.execute(
                            "UPDATE card SET deleted_at = ?1, updated_at = ?1 WHERE uuid = ?2",
                            rusqlite::params![canonical_stamp(remote_deleted), card.uuid],
                        )
                        .map_err(|e| e.to_string())?;
                        count += 1;
                    }
                    // else: local is newer, keep local
                } else if order == Ordering::Less {
                    // Remote is newer - update local
                    conn.execute(
                        "UPDATE card SET question = ?2, suggestion = ?3, answer = ?4, note = ?5, \
                         familiarity = ?6, question_hash = ?7, source_card_uuid = ?8, \
                         source_dataset_uuid = ?9, updated_at = ?10 \
                         WHERE uuid = ?1",
                        rusqlite::params![
                            card.uuid,
                            card.question,
                            card.suggestion,
                            card.answer,
                            card.note,
                            card.familiarity,
                            card.question_hash,
                            card.source_card_uuid,
                            card.source_dataset_uuid,
                            // Same rule as the insert above: whatever shape the
                            // peer's stamp arrives in, the row keeps ours, or the
                            // next `updated_at > ?1` cursor can read past it.
                            canonical_stamp(&card.updated_at),
                        ],
                    )
                    .map_err(|e| e.to_string())?;
                    count += 1;
                }
                // else: local is newer or equal, keep local
            }
        }
    }

    Ok(count)
}

/// Get local changes to push (cards updated since last sync).
fn get_local_changes(conn: &Connection, since: &str) -> Result<Vec<SyncCard>, String> {
    // The cursor arrives from the server's clock, so it may carry a shape this
    // side never writes. SQLite compares `updated_at` as text, and a suffix-only
    // difference can exclude a row that is genuinely newer than the cursor.
    let since = canonical_stamp(since);
    let mut stmt = conn
        .prepare(
            "SELECT uuid, question, suggestion, answer, note, familiarity, question_hash, \
             source_card_uuid, source_dataset_uuid, deleted_at, created_at, updated_at \
             FROM card WHERE updated_at > ?1 ORDER BY updated_at ASC",
        )
        .map_err(|e| e.to_string())?;

    let cards = stmt
        .query_map(rusqlite::params![since], |row| {
            Ok(SyncCard {
                uuid: row.get(0)?,
                question: row.get(1)?,
                suggestion: row.get(2)?,
                answer: row.get(3)?,
                note: row.get(4)?,
                familiarity: row.get(5)?,
                question_hash: row.get(6)?,
                source_card_uuid: row.get(7)?,
                source_dataset_uuid: row.get(8)?,
                deleted_at: row.get(9)?,
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(cards)
}

/// Update the sync state after a successful sync.
fn update_sync_state(
    conn: &Connection,
    dataset_uuid: &str,
    last_synced_at: &str,
    clock_offset_ms: i32,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO sync_state (dataset_uuid, last_synced_at, clock_offset_ms, pending_push) \
         VALUES (?1, ?2, ?3, 0) \
         ON CONFLICT(dataset_uuid) DO UPDATE SET \
            last_synced_at = ?2, clock_offset_ms = ?3, pending_push = 0",
        rusqlite::params![dataset_uuid, canonical_stamp(last_synced_at), clock_offset_ms],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Get the access token from stored auth.
fn get_access_token(settings: &SettingsState) -> Result<String, String> {
    // Read the auth tokens file directly
    let auth_path = if let Some(ws_dir) = settings.workspace_dir.lock().unwrap().as_ref() {
        ws_dir.join("auth.json")
    } else {
        crate::app_paths::data_root().join("auth.json")
    };

    if !auth_path.exists() {
        return Err("Not logged in. Please log in first.".into());
    }

    let content = fs::read_to_string(&auth_path).map_err(|e| e.to_string())?;
    let content = content.trim_start_matches('\u{FEFF}');
    let tokens: serde_json::Value = serde_json::from_str(content).map_err(|e| e.to_string())?;

    tokens
        .get("access_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "No access token found".to_string())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Full bidirectional sync for a card dataset.
#[tauri::command]
pub async fn card_sync_full(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<SyncResult, String> {
    let path = find_card_dataset_dir(&settings, &dataset_uuid)?;
    let conn = open_card_db(&path)?;

    // Get access token
    let access_token = get_access_token(&settings)?;

    // Get current sync state
    let sync_state: Option<(String, i32)> = conn
        .query_row(
            "SELECT last_synced_at, clock_offset_ms FROM sync_state WHERE dataset_uuid = ?1",
            rusqlite::params![dataset_uuid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();

    let (last_synced_at, _clock_offset_ms): (String, i32) = sync_state.unwrap_or_else(|| {
        // Default to epoch if no sync state
        ("1970-01-01T00:00:00Z".to_string(), 0i32)
    });

    // Phase 1: Pull remote changes
    log::info!(
        "[CardSync] Pulling changes for dataset '{}' since {}",
        dataset_uuid,
        &last_synced_at
    );

    let pull_result = pull_changes(&dataset_uuid, &last_synced_at, &access_token).await?;
    let server_time = pull_result.server_time.clone();

    // Calculate clock offset
    let clock_offset_ms = 0i32; // Simplified: would need proper datetime diff

    // Apply pulled cards
    let pulled = apply_pulled_cards(&conn, &pull_result.cards)?;
    log::info!("[CardSync] Pulled {} card(s)", pulled);

    // Phase 2: Push local changes
    let local_changes = get_local_changes(&conn, &last_synced_at)?;

    let pushed = if !local_changes.is_empty() {
        log::info!(
            "[CardSync] Pushing {} local change(s) for dataset '{}'",
            local_changes.len(),
            dataset_uuid
        );

        let push_request = SyncPushRequest {
            cards: local_changes,
            tags: Vec::new(), // TODO: sync tags
        };

        let push_result = push_changes(&dataset_uuid, &push_request, &access_token).await?;
        log::info!(
            "[CardSync] Pushed {} card(s), {} conflict(s)",
            push_result.accepted,
            push_result.conflicts.len()
        );

        push_result.accepted
    } else {
        0
    };

    // Phase 3: Update sync state
    update_sync_state(&conn, &dataset_uuid, &server_time, clock_offset_ms)?;

    // Update dataset timestamp
    touch_info(&path)?;

    Ok(SyncResult {
        pulled,
        pushed,
        conflicts: 0,
        server_time,
    })
}

/// Sync all owned card datasets.
#[tauri::command]
pub async fn card_sync_all(
    settings: State<'_, SettingsState>,
) -> Result<Vec<(String, Result<SyncResult, String>)>, String> {
    let datasets = crate::datasets::cards::list_card_datasets(&settings);
    let mut results = Vec::new();

    // Get access token once
    let _access_token = match get_access_token(&settings) {
        Ok(t) => t,
        Err(e) => {
            return Err(format!("Not logged in: {}", e));
        }
    };

    for ds in &datasets {
        // Only sync datasets owned by current user
        let user_id = crate::auth::get_stored_user_id(&settings).unwrap_or_default();
        if ds.info.sharing.owner_id != user_id {
            continue;
        }

        let result = card_sync_full_inner(&settings, &ds.info.uuid).await;
        results.push((ds.info.uuid.clone(), result));
    }

    Ok(results)
}

/// Internal sync function that can be called from card_sync_all.
async fn card_sync_full_inner(
    settings: &SettingsState,
    dataset_uuid: &str,
) -> Result<SyncResult, String> {
    let path = find_card_dataset_dir(settings, dataset_uuid)?;
    let conn = open_card_db(&path)?;

    // Get access token
    let access_token = get_access_token(settings)?;

    // Get current sync state
    let sync_state: Option<(String, i32)> = conn
        .query_row(
            "SELECT last_synced_at, clock_offset_ms FROM sync_state WHERE dataset_uuid = ?1",
            rusqlite::params![dataset_uuid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();

    let (last_synced_at, clock_offset_ms) = sync_state.unwrap_or_else(|| {
        ("1970-01-01T00:00:00Z".to_string(), 0i32)
    });

    // Pull
    let pull_result = pull_changes(dataset_uuid, &last_synced_at, &access_token).await?;
    let server_time = pull_result.server_time.clone();
    let pulled = apply_pulled_cards(&conn, &pull_result.cards)?;

    // Push
    let local_changes = get_local_changes(&conn, &last_synced_at)?;
    let pushed = if !local_changes.is_empty() {
        let push_request = SyncPushRequest {
            cards: local_changes,
            tags: Vec::new(),
        };
        let push_result = push_changes(dataset_uuid, &push_request, &access_token).await?;
        push_result.accepted
    } else {
        0
    };

    // Update sync state
    update_sync_state(&conn, dataset_uuid, &server_time, clock_offset_ms)?;

    // Update dataset timestamp
    touch_info(&path)?;

    Ok(SyncResult {
        pulled,
        pushed,
        conflicts: 0,
        server_time,
    })
}
