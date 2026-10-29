//! Follower → hub **dataset push** (sync-v2.md §7, the write half of the file
//! plane; contract in `docs/design/dataset-push.md`).
//!
//! The round and `dataset_sync_snapshot` move content *toward* a follower. This
//! module is the other direction, for the case the sync model did not have: a
//! follower pulls a dataset from the LAN hub, authors it locally (imports media,
//! cuts or retimes cues, adds cards), and needs that work to become the hub's
//! copy. Regenerating it on the hub from the VTTs would silently discard the cue
//! work, so the whole directory travels — including `data.sqlite3`, which is
//! deliberately outside the content plane everywhere else (`rest::is_db_file`).
//!
//! The transport is the existing file plane, reversed, so an unchanged media
//! file costs one sha256 and zero bytes:
//!
//! 1. hash the local tree (through the same `(size, mtime)` cache the hub
//!    manifest uses) and diff it against the hub's `/manifest`;
//! 2. `POST /ingest` declares the whole local tree — the hub builds a staged
//!    merge base and answers which digests it is missing;
//! 3. `PUT /ingest/file` per missing path and `PUT /ingest/db` for the frozen DB;
//! 4. `POST /ingest/commit` installs it (old copy to the trash first) and returns
//!    the new revision;
//! 5. record the hub's `overall_hash` as this copy's baseline, so the next round
//!    sees no drift, and the new `content_gen`, so this device does not pull back
//!    the DB it just sent.
//!
//! Guards, all fail-closed and actionable:
//! * LAN hub only — the website has its own versioned upload (`hub_dataset_upload`);
//! * never a read-only base (a `download/` adoption has nothing authored to send,
//!   and the hub refuses to overwrite one anyway);
//! * never a silent overwrite of newer hub work (`hub_changed`), unless the caller
//!   explicitly says so.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::datasets;
use crate::settings::SettingsState;
use super::*;

/// Bytes per read when streaming an upload body.
const PUSH_CHUNK: usize = 256 * 1024;
/// Minimum gap between two `dataset-sync-progress` events.
const PUSH_PROGRESS_MS: u128 = 400;

// ---------------------------------------------------------------------------
// Preview — what a push would do, without touching anything
// ---------------------------------------------------------------------------

/// One push's plan, as the confirm dialog and an agent both read it.
#[derive(Serialize, Clone)]
pub struct PushPreview {
    pub dataset_uuid: String,
    pub name: String,
    pub dataset_type: String,
    pub local_dir: String,
    pub hub_base: String,
    /// Files the hub does not have (their digests differ or they are new).
    pub creates: usize,
    pub updates: usize,
    /// Hub files this push would remove (gone from the local tree).
    pub deletes: usize,
    pub unchanged: usize,
    pub upload_bytes: u64,
    /// Size of the frozen DB that will ride along, 0 when the dataset has none.
    pub db_bytes: u64,
    /// The hub's copy moved since this device last synced it — a push would
    /// overwrite work it has not seen.
    pub hub_changed: bool,
    /// No `dataset_sync_state` row: this copy was never pulled from this hub.
    pub no_baseline: bool,
    pub hub_content_gen: i64,
    pub local_content_gen: i64,
}

/// Everything the driver needs, computed once. `hub_files`/`local` carry the
/// per-file digests so the upload step never re-hashes.
struct PushPlan {
    dir: PathBuf,
    ty: datasets::DatasetType,
    hub_base: String,
    hub_hash: String,
    hub_content_gen: i64,
    /// The hub's rel paths at plan time, so a late change (the descriptor stamp)
    /// can still be classified as a create or an update.
    hub_paths: std::collections::BTreeSet<String>,
    local: BTreeMap<String, String>,
    /// Local paths the hub is missing, with their size on disk.
    send: Vec<(String, u64)>,
    deletes: Vec<String>,
    unchanged: usize,
    creates: usize,
    updates: usize,
    db_changed: bool,
    preview: PushPreview,
}

/// A push needs a LAN hub: device pairing, trust zones, and a dataset directory
/// the hub actually hosts. The website answers none of those.
fn push_base(settings: &SettingsState) -> Result<String, String> {
    let base = pc_base(settings);
    if base.is_empty() {
        return Err(
            "no hub configured — set the PC address on Devices & Hub, then run a sync round \
             first so this device knows the hub's copy"
                .into(),
        );
    }
    if crate::sync::hub_kind(&base) == crate::sync::HubKind::Website {
        return Err(
            "the configured hub is the website: a live directory is not pushed there — use \
             'Publish to hub' (hub_dataset_upload) for the online registry"
                .into(),
        );
    }
    Ok(base)
}

/// Signed GET of one dataset's manifest from the connected LAN hub.
async fn hub_manifest(settings: &SettingsState, base: &str, uuid: &str) -> Result<Value, String> {
    let api_path = format!("/api/v1/datasets/{uuid}/manifest");
    hub_get_json_at(settings, base, &api_path, "").await
}

/// Read a dataset's local sync row's `overall_hash` (the baseline this copy was
/// last reconciled against) and the generation it last installed.
fn local_state(conn: &rusqlite::Connection, uuid: &str) -> (Option<String>, i64) {
    let baseline = conn
        .query_row(
            "SELECT overall_hash FROM dataset_sync_state WHERE dataset_uuid = ?1",
            [uuid],
            |r| r.get::<_, String>(0),
        )
        .ok();
    (baseline, crate::sync::content_gen(conn, uuid))
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Build the plan. Pure reads: `/manifest`, the local tree, and the app DB. The
/// tree is hashed through the shared `(size, mtime)` cache, so a second push of
/// an unchanged dataset costs one directory walk.
async fn plan_push(
    settings: &SettingsState,
    uuid: &str,
) -> Result<PushPlan, String> {
    let base = push_base(settings)?;
    let (dir, ty) = datasets::find_dataset_dir_typed(settings, uuid)?;
    if datasets::is_under_download_root(settings, &dir) {
        return Err(format!(
            "'{}' is the read-only downloaded base of '{}' — there is nothing authored on this \
             device to send. Build the dataset first (subtitles, waveforms or cues): the first \
             write forks it into a writable copy, and that copy is what a push sends. The hub's \
             own directory is never replaced with an untouched copy of itself.",
            dir.display(),
            uuid
        ));
    }
    if uuid == datasets::FAVORITES_DATASET_UUID {
        return Err("the built-in Favorites dataset is app-owned and never pushed".into());
    }
    let info = datasets::read_info(&dir).ok();
    let name = info
        .as_ref()
        .map(|i| i.name.clone())
        .unwrap_or_else(|| uuid.to_string());

    // Hub side: the current per-file digests + revision.
    let manifest = hub_manifest(settings, &base, uuid).await?;
    let hub_files = super::filediff::parse_files(&manifest["files"]);
    let hub_hash = manifest["overall_hash"].as_str().unwrap_or("").to_string();
    let hub_content_gen = manifest["content_gen"].as_i64().unwrap_or(0);
    let hub_file_count = manifest["file_count"].as_u64().unwrap_or(0) as usize;

    // Local side, in the hub's own vocabulary.
    let conn = datasets::dictation::open_app_db(settings)?;
    crate::sync::blobpush::ensure_hash_cache(&conn)?;
    let hash_dir = dir.clone();
    let hash_uuid = uuid.to_string();
    let local = tauri::async_runtime::spawn_blocking(move || {
        crate::sync::blobpush::content_tree_hashes_cached(&hash_dir, &hash_uuid, &conn)
    })
    .await
    .map_err(|e| format!("hashing the local tree failed: {e}"))??;

    // The planner is direction-agnostic: asking it to "fetch" the local tree from
    // the hub's returns exactly the paths the *hub* is missing, and the hub-only
    // paths are what the hub must delete. Same tested function, roles swapped.
    let diff = super::filediff::plan(&hub_files, &local);

    let mut creates = 0usize;
    let mut updates = 0usize;
    let mut send: Vec<(String, u64)> = Vec::new();
    let mut upload_bytes = 0u64;
    for rel in &diff.fetch {
        let size = file_size(&dir.join(rel_path(rel)));
        if hub_files.contains_key(rel) {
            updates += 1;
        } else {
            creates += 1;
        }
        upload_bytes += size;
        send.push((rel.clone(), size));
    }
    let unchanged = local
        .iter()
        .filter(|(p, sha)| hub_files.get(*p) == Some(*sha))
        .count();

    let (baseline, local_content_gen) = {
        let conn = datasets::dictation::open_app_db(settings)?;
        local_state(&conn, uuid)
    };
    // A hub copy that moved since the last sync, or one this device never synced
    // at all, is work the push would overwrite without ever having seen it.
    let no_baseline = baseline.is_none() && hub_file_count > 0;
    let hub_changed = match &baseline {
        Some(b) => b != &hub_hash,
        None => no_baseline,
    };

    let db_path = dir.join("data.sqlite3");
    let db_changed = db_path.is_file();
    let db_bytes = if db_changed { file_size(&db_path) } else { 0 };

    let preview = PushPreview {
        dataset_uuid: uuid.to_string(),
        name,
        dataset_type: ty.as_str().to_string(),
        local_dir: dir.display().to_string(),
        hub_base: base.clone(),
        creates,
        updates,
        deletes: diff.delete.len(),
        unchanged,
        upload_bytes: upload_bytes + db_bytes,
        db_bytes,
        hub_changed,
        no_baseline,
        hub_content_gen,
        local_content_gen,
    };
    Ok(PushPlan {
        dir,
        ty,
        hub_base: base,
        hub_hash,
        hub_content_gen,
        hub_paths: hub_files.keys().cloned().collect(),
        local,
        send,
        deletes: diff.delete,
        unchanged,
        creates,
        updates,
        db_changed,
        preview,
    })
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// Stream one file to a `PUT /ingest/*` route. The digest rides in the signed
/// query and `x-fms-stream` tells the hub's zone guard not to buffer the body,
/// so a multi-GB file never sits in memory on either end.
async fn put_file(
    settings: &SettingsState,
    base: &str,
    api_path: &str,
    qs: &str,
    path: &Path,
) -> Result<Value, String> {
    let client = crate::auth::hub_client_builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(3600))
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("{base}{api_path}?{qs}");
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("open '{}': {e}", path.display()))?;
    let stream = futures_util::stream::unfold(file, |mut f| async move {
        let mut buf = vec![0u8; PUSH_CHUNK];
        match tokio::io::AsyncReadExt::read(&mut f, &mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<_, std::io::Error>(buf), f))
            }
            Err(e) => Some((Err(e), f)),
        }
    });
    let req = hub_request(client.put(url), settings, "PUT", api_path, qs, b"").await?;
    let resp = req
        .header("x-fms-stream", "1")
        .header("content-type", "application/octet-stream")
        .body(reqwest::Body::wrap_stream(stream))
        .send()
        .await
        .map_err(|e| format!("upload '{}' failed: {e}", path.display()))?;
    if !resp.status().is_success() {
        return Err(hub_http_error(resp, api_path).await);
    }
    resp.json::<Value>()
        .await
        .map_err(|e| format!("unexpected ingest response: {e}"))
}

/// Signed JSON POST returning parsed JSON (declare / commit).
async fn post_json(
    settings: &SettingsState,
    base: &str,
    api_path: &str,
    body: &Value,
) -> Result<Value, String> {
    let client = crate::auth::hub_client_builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let req = hub_request(client.post(format!("{base}{api_path}")), settings, "POST", api_path, "", &bytes).await?;
    let resp = req
        .header("content-type", "application/json")
        .body(bytes)
        .send()
        .await
        .map_err(|e| format!("cannot reach the hub at {base}: {e}"))?;
    if !resp.status().is_success() {
        return Err(hub_http_error(resp, api_path).await);
    }
    resp.json::<Value>()
        .await
        .map_err(|e| format!("unexpected response from '{api_path}': {e}"))
}

/// A copy forked out of a downloaded base carries `info.layer.role =
/// "overlay"` (plus the base's uuid and version). That block describes *this
/// device's* parentage, not the dataset — and this push is the moment the
/// parentage ends: the hub's copy becomes authoritative, and nothing on the hub
/// may claim to be an overlay of a base it is not. Flatten the descriptor first
/// (the same write `layer_flatten` performs), so the bytes hashed, uploaded and
/// recorded as the new baseline all carry the standalone form; normalising on
/// the hub side instead would leave the two copies permanently one `info.json`
/// apart on every later diff.
fn make_standalone(dir: &Path) -> Result<(), String> {
    let mut info = datasets::read_info(dir)?;
    if info.layer.as_ref().map(|l| l.role.as_str()) != Some(datasets::LAYER_ROLE_OVERLAY) {
        return Ok(());
    }
    info.layer = None;
    info.updated_at = datasets::now_stamp();
    datasets::write_info(dir, &info)?;
    log::info!(
        "[push] {}: flattened a local overlay into a standalone dataset before pushing",
        dir.display()
    );
    Ok(())
}

/// Freeze the dataset DB into a temp file via the SQLite backup API — one
/// consistent image, journal stripped — and return its path, digest and size.
/// Sized like `upload::pack_archive`'s copy, and never taken from a live
/// `data.sqlite3` (a torn image would be worse than no push at all).
fn freeze_dataset_db(dir: &Path) -> Result<Option<(PathBuf, String, u64)>, String> {
    let db = dir.join("data.sqlite3");
    if !db.is_file() {
        return Ok(None);
    }
    let dest = std::env::temp_dir().join(format!(
        "fms-push-{}-{}.sqlite3",
        Uuid::new_v4().simple(),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&dest);
    crate::sync::upload::freeze_db(dir, &dest)?;
    let size = file_size(&dest);
    let sha = crate::sync::blobpush::hash_of_file(&dest)?;
    Ok(Some((dest, sha, size)))
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// What a push would send, without sending it: the counts and byte totals the
/// confirm dialog shows, plus `hub_changed` when the hub's copy moved since this
/// device last synced it. Read-only — safe to call before a confirmation.
#[tauri::command]
pub async fn dataset_push_preview(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<PushPreview, String> {
    let plan = plan_push(&settings, uuid.trim()).await?;
    Ok(plan.preview)
}

/// Push a local dataset's whole directory to the LAN hub (docs/design/dataset-push.md).
///
/// `allow_hub_changes` is the explicit "overwrite the hub's newer copy" consent the
/// UI collects after showing a preview: without it, a hub copy that changed since
/// this device last synced aborts the push instead of being replaced.
#[tauri::command]
pub async fn dataset_push_to_hub(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
    allow_hub_changes: Option<bool>,
) -> Result<Value, String> {
    push_one(&app, &settings, uuid.trim(), allow_hub_changes.unwrap_or(false)).await
}

/// Push every locally-authored dataset that is not a read-only adoption. A
/// per-dataset failure does not abort the batch: each entry reports its own
/// `status`, so one unreadable dataset cannot block the rest.
#[tauri::command]
pub async fn dataset_push_all(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    allow_hub_changes: Option<bool>,
) -> Result<Value, String> {
    push_base(&settings)?;
    let candidates = crate::sync::upload::sync_local_datasets(settings.clone()).await?;
    let allow = allow_hub_changes.unwrap_or(false);
    let mut pushed = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut results: Vec<Value> = Vec::new();
    for ds in &candidates {
        let uuid = ds["uuid"].as_str().unwrap_or("").to_string();
        if uuid.is_empty() {
            continue;
        }
        let dir = match datasets::find_dataset_dir_typed(&settings, &uuid) {
            Ok((d, _)) => d,
            Err(_) => continue,
        };
        if datasets::is_under_download_root(&settings, &dir) {
            skipped += 1;
            results.push(json!({ "uuid": uuid, "status": "skipped", "reason": "read_only_base" }));
            continue;
        }
        match push_one(&app, &settings, &uuid, allow).await {
            Ok(v) => {
                let changed = v["pushed"].as_bool().unwrap_or(false);
                if changed {
                    pushed += 1;
                } else {
                    skipped += 1;
                }
                results.push(json!({
                    "uuid": uuid,
                    "name": ds["name"],
                    "status": if changed { "pushed" } else { "unchanged" },
                    "result": v,
                }));
            }
            Err(e) => {
                failed += 1;
                log::warn!("[push] {uuid}: {e}");
                results.push(json!({ "uuid": uuid, "name": ds["name"], "status": "failed", "error": e }));
            }
        }
    }
    Ok(json!({
        "pushed": pushed,
        "unchanged": skipped,
        "failed": failed,
        "datasets": results,
    }))
}

/// One push, end to end. Public within the crate so `dataset_push_all` and the
/// MCP twin share exactly this path.
pub(crate) async fn push_one(
    app: &AppHandle,
    settings: &State<'_, SettingsState>,
    uuid: &str,
    allow_hub_changes: bool,
) -> Result<Value, String> {
    let mut plan = plan_push(settings, uuid).await?;
    if plan.preview.hub_changed && !allow_hub_changes {
        return Err(format!(
            "hub_changed: the hub's copy of '{}' moved since this device last synced it (hub revision {}, this copy last saw {}). Re-run with allow_hub_changes to replace it, or pull the hub's version first.",
            plan.preview.name, plan.hub_content_gen, plan.preview.local_content_gen
        ));
    }

    // Nothing to send: skip before the descriptor stamp, so a no-op push leaves
    // both sides byte-identical instead of minting a new `updated_at` that every
    // other follower would then have to fetch.
    if plan.send.is_empty() && plan.deletes.is_empty() && !plan.db_changed {
        log::info!("[push] {uuid}: already identical to the hub; nothing sent");
        return Ok(json!({
            "pushed": false,
            "reason": "unchanged",
            "uuid": uuid,
            "name": plan.preview.name,
            "dataset_type": plan.ty.as_str(),
            "files_unchanged": plan.unchanged,
            "content_gen": plan.preview.hub_content_gen,
            "overall_hash": plan.hub_hash,
        }));
    }

    // The descriptor stamp becomes the hub's `updated` after the commit, so it
    // is refreshed *before* hashing: the bytes hashed here are the bytes
    // uploaded, and a second push of the same tree finds no drift. An overlay is
    // flattened first — see `make_standalone` for why this side owns that.
    make_standalone(&plan.dir)?;
    datasets::touch_info(&plan.dir)?;
    {
        let rel = datasets::INFO_FILE.to_string();
        let abs = plan.dir.join(&rel);
        let sha = {
            let conn = datasets::dictation::open_app_db(settings)?;
            crate::sync::blobpush::cached_hash(
                Some(&conn),
                uuid,
                &rel,
                &abs,
                file_size(&abs),
                mtime_ms(&abs),
            )
        };
        if plan.local.get(&rel) != Some(&sha) {
            plan.local.insert(rel.clone(), sha);
            if plan.send.iter().all(|(p, _)| p != &rel) {
                if plan.hub_paths.contains(&rel) {
                    plan.updates += 1;
                } else {
                    plan.creates += 1;
                }
                plan.send.push((rel, file_size(&abs)));
            }
        }
    }

    let run = Uuid::new_v4().simple().to_string();
    let base = plan.hub_base.clone();
    let uuid_owned = uuid.to_string();
    // Recomputed after the stamp so the progress total matches what is sent.
    let total_bytes: u64 = plan.send.iter().map(|(_, s)| *s).sum::<u64>()
        + if plan.db_changed {
            file_size(&plan.dir.join("data.sqlite3"))
        } else {
            0
        };

    let _ = app.emit(
        "dataset-sync-progress",
        json!({ "uuid": uuid_owned, "kind": "push", "phase": "push", "sent": 0, "total": total_bytes }),
    );

    // 1. Declare: the whole local tree + the paths the hub should drop. The hub
    //    answers with the digests it is actually missing, which is authoritative
    //    (it may hold files this device lost).
    let files: Vec<Value> = plan
        .local
        .iter()
        .map(|(path, sha)| json!({ "path": path, "sha256": sha, "size": file_size(&plan.dir.join(rel_path(path))) }))
        .collect();
    let declare_path = format!("/api/v1/datasets/{uuid}/ingest");
    let mut body = json!({
        "run": run,
        "dataset_type": plan.ty.as_str(),
        "files": files,
        "deletes": plan.deletes,
    });
    if plan.db_changed {
        body["db"] = json!({ "path": "data.sqlite3", "sha256": "", "size": 0 });
    }
    let frozen = tauri::async_runtime::spawn_blocking({
        let dir = plan.dir.clone();
        move || freeze_dataset_db(&dir)
    })
    .await
    .map_err(|e| format!("freezing the dataset DB failed: {e}"))??;
    if let Some((_, sha, size)) = &frozen {
        body["db"] = json!({ "path": "data.sqlite3", "sha256": sha, "size": size });
    }
    let declared = post_json(settings, &base, &declare_path, &body).await?;
    let need: Vec<String> = declared["need"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    let need_db = declared["need_db"].as_bool().unwrap_or(false);
    let hub_gen = declared["content_gen"].as_i64().unwrap_or(0);

    // 2. Upload only what the hub asked for.
    let mut sent = 0u64;
    let mut started = Instant::now();
    let mut files_sent = 0usize;
    for rel in &need {
        let abs = plan.dir.join(rel_path(rel));
        let sha = plan.local.get(rel).cloned().unwrap_or_default();
        let size = file_size(&abs);
        let api_path = format!("/api/v1/datasets/{uuid}/ingest/file");
        let qs = build_query(&[
            ("run", run.clone()),
            ("dataset_type", plan.ty.as_str().to_string()),
            ("path", rel.clone()),
            ("sha", sha),
        ]);
        put_file(settings, &base, &api_path, &qs, &abs).await?;
        files_sent += 1;
        sent += size;
        if started.elapsed().as_millis() >= PUSH_PROGRESS_MS {
            let _ = app.emit(
                "dataset-sync-progress",
                json!({ "uuid": uuid_owned, "kind": "push", "phase": "push", "sent": sent, "total": total_bytes }),
            );
            started = Instant::now();
        }
    }
    if need_db {
        if let Some((tmp, sha, size)) = &frozen {
            let api_path = format!("/api/v1/datasets/{uuid}/ingest/db");
            let qs = build_query(&[
                ("run", run.clone()),
                ("dataset_type", plan.ty.as_str().to_string()),
                ("sha", sha.clone()),
            ]);
            let res = put_file(settings, &base, &api_path, &qs, tmp).await;
            let _ = std::fs::remove_file(tmp);
            res?;
            sent += size;
            let _ = app.emit(
                "dataset-sync-progress",
                json!({ "uuid": uuid_owned, "kind": "push", "phase": "push", "sent": sent, "total": total_bytes }),
            );
        }
    } else if let Some((tmp, _, _)) = &frozen {
        let _ = std::fs::remove_file(tmp);
    }

    // 3. Commit: the hub re-verifies, trashes what it replaces, and installs.
    let commit_path = format!("/api/v1/datasets/{uuid}/ingest/commit");
    let committed = post_json(
        settings,
        &base,
        &commit_path,
        &json!({ "run": run, "dataset_type": plan.ty.as_str(), "content_gen": hub_gen }),
    )
    .await?;

    let new_gen = committed["content_gen"].as_i64().unwrap_or(hub_gen + 1);
    let new_hash = committed["overall_hash"]
        .as_str()
        .unwrap_or(&plan.hub_hash)
        .to_string();

    // 4. Record the new baseline: the next round must see no drift, and must not
    //    pull back the DB this device just sent.
    {
        let conn = datasets::dictation::open_app_db(settings)?;
        let _ = crate::sync::set_content_gen(&conn, uuid, new_gen);
        let changed = conn
            .execute(
                "UPDATE dataset_sync_state SET overall_hash = ?2, synced_at = datetime('now'), \
                    pushed_at = datetime('now'), pushed_revision = ?3 \
                 WHERE dataset_uuid = ?1",
                rusqlite::params![uuid, &new_hash, new_gen],
            )
            .unwrap_or(0);
        if changed == 0 {
            log::info!(
                "[push] {uuid}: no dataset_sync_state row (locally authored copy); \
                 recorded revision {new_gen} only"
            );
        }
    }
    let _ = app.emit(
        "dataset-sync-progress",
        json!({ "uuid": uuid_owned, "kind": "push", "phase": "done" }),
    );

    log::info!(
        "[push] {uuid}: {} file(s) uploaded, {} deleted, revision {new_gen} on {base}",
        files_sent,
        plan.deletes.len()
    );
    Ok(json!({
        "pushed": true,
        "uuid": uuid,
        "name": plan.preview.name,
        "dataset_type": plan.ty.as_str(),
        "files_sent": files_sent,
        "files_unchanged": plan.unchanged,
        "deletes": plan.deletes.len(),
        "db_sent": need_db,
        "bytes_sent": sent,
        "content_gen": new_gen,
        "overall_hash": new_hash,
        "trashed_to": committed["trashed_to"],
        "created_on_hub": committed["created"],
    }))
}

/// A `/`-separated rel path as native path segments (never a single string, so a
/// traversal cannot be reassembled).
fn rel_path(rel: &str) -> PathBuf {
    let mut p = PathBuf::new();
    for seg in rel.split('/') {
        p.push(seg);
    }
    p
}

fn mtime_ms(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
