use serde::Serialize;
use tauri::{AppHandle, State};

use crate::datasets::cards::find_card_dataset_dir;
use crate::datasets::{is_under_download_root, touch_info};
use crate::settings::SettingsState;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Result of one manual card-dataset sync, kept as the stable surface shape for the
/// UI action and the MCP tools. Since phase 7b a card dataset syncs through the
/// dataset-scope row journal (see [`crate::sync::client::sync_one_dataset_journal`]),
/// so these counts are folded from that round: `pulled` rows applied from the hub,
/// `pushed` local rows acknowledged, `conflicts` pending local edits the hub's
/// newer entry displaced (INV-14).
#[derive(Clone, Serialize, Debug)]
pub struct SyncResult {
    pub pulled: i32,
    pub pushed: i32,
    pub conflicts: i32,
    pub server_time: String,
}

fn skipped() -> SyncResult {
    SyncResult {
        pulled: 0,
        pushed: 0,
        conflicts: 0,
        server_time: String::new(),
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// A sync round is a **write**: pulled rows land in this dataset's DB, and the local
/// rows it then pushes were produced by earlier rounds writing here. A dataset
/// under the download root is a read-only base — the online registry's copy, not
/// ours to mutate — so the round is refused rather than forked: an overlay of a
/// shared base would be a second, unpublished author of content we do not own
/// (sync-v2.md §16.3).
///
/// Prospective today: nothing populates the download root, and the auto-sync arm
/// only reaches datasets inside the writable type root. The guard is what keeps it
/// that way when an adoption flow starts putting bases there.
fn ensure_sync_target_writable(
    settings: &SettingsState,
    dataset_dir: &std::path::Path,
    dataset_uuid: &str,
) -> Result<(), String> {
    if is_under_download_root(settings, dataset_dir) {
        return Err(format!(
            "Card dataset {dataset_uuid} is a read-only shared copy ({}); \
             its content belongs to the publisher. Copy it into your own workspace \
             to edit or sync it — practice reviews are recorded separately and need \
             no sync round here.",
            dataset_dir.display()
        ));
    }
    Ok(())
}

/// One card dataset through the dataset-scope journal (sync-v2.md §8.3, phase 7b).
///
/// Card rows are ordinary journal kinds (`card_save`/`card_tag_*`), so a dataset's
/// sync is no longer a bespoke `/api/card/sync/*` round trip with its own conflict
/// engine — it is the *same* cloud-hub journal client the round uses, scoped to one
/// dataset. This is the single path the UI action and the MCP `card_sync`/`card_sync_all`
/// tools ride; the former website card endpoints are gone.
///
/// A dataset that is not journal-synced against the connected host (its recorded
/// upstream is a desktop hub, or the host does not advertise `dataset_journal`)
/// returns a zero [`SyncResult`] — the old skip — while a provenance disagreement
/// (INV-19) is an actionable error from [`crate::sync::client::card_sync_target`].
async fn sync_one_card_dataset(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<SyncResult, String> {
    let path = find_card_dataset_dir(&settings, &dataset_uuid)?;
    ensure_sync_target_writable(settings.inner(), &path, &dataset_uuid)?;
    let synced =
        crate::sync::client::sync_one_dataset_journal(app, settings, &dataset_uuid).await?;
    match synced {
        None => Ok(skipped()),
        Some(j) => {
            touch_info(&path)?;
            Ok(SyncResult {
                pulled: j.pulled as i32,
                pushed: j.pushed as i32,
                conflicts: j.conflicts as i32,
                server_time: String::new(),
            })
        }
    }
}

/// Full bidirectional sync for one card dataset, now riding the dataset-scope
/// journal: pushes this dataset's pending rows and applies its pulled row journal.
/// Only for datasets upstreamed by the cloud hub, and never for a read-only shared
/// copy under the download root.
#[tauri::command]
pub async fn card_sync_full(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<SyncResult, String> {
    log::info!("[CardSync] journal sync for dataset '{dataset_uuid}'");
    sync_one_card_dataset(app, settings, dataset_uuid).await
}

/// Sync all owned card datasets through the journal.
#[tauri::command]
pub async fn card_sync_all(
    app: AppHandle,
    settings: State<'_, SettingsState>,
) -> Result<Vec<(String, Result<SyncResult, String>)>, String> {
    let datasets = crate::datasets::cards::list_card_datasets(&settings);
    // Keep the friendly top-level auth error: the journal path degrades to an
    // anonymous request (→ a per-dataset 401) without a token, but the whole fan-out
    // is pointless when signed out.
    crate::auth::access_token(&settings)
        .await
        .map_err(|e| format!("Not logged in: {}", e))?;
    let user_id = crate::auth::get_stored_user_id(&settings).unwrap_or_default();
    let mut results = Vec::new();

    for ds in &datasets {
        // Only sync datasets owned by the current user.
        if ds.info.sharing.owner_id != user_id {
            continue;
        }
        let uuid = ds.info.uuid.clone();
        let result = sync_one_card_dataset(app.clone(), settings.clone(), uuid.clone()).await;
        results.push((uuid, result));
    }

    Ok(results)
}
