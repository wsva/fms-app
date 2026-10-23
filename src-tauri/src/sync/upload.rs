//! Dataset export + upload to the website hub (dev guide T2).
//!
//! The pull side of sync lives in [`super::client`]; this module is the only
//! path that *sends* a dataset: pack `<datasets>/<type>/<uuid>` as tar.gz (with
//! the SQLite file frozen via the backup API, never its WAL sidecars) and
//! `POST /api/dataset/upload` with OAuth Bearer auth — the website route is
//! account-scoped, not device-scoped (guide §3.6). Compiled on **all**
//! platforms (reqwest / tar / flate2 / rusqlite only, like `client.rs`).
//!
//! Upload state lands in the app DB (`dataset_upload_state`), never inside the
//! dataset directory — a shipped copy must not carry this device's upload
//! history (guide §4.2).

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use flate2::write::GzEncoder;
use flate2::Compression;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, State};

use crate::datasets;
use crate::settings::SettingsState;

/// Hub route that accepts an archive (guide §3.6 — multipart `file`,
/// optional `visibility`, repeated `tag_uuids`).
const UPLOAD_PATH: &str = "/api/dataset/upload";

/// A dataset dir's SQLite file + sidecars. Excluded from the walk as *files*
/// (the frozen copy replaces `data.sqlite3`); the snapshot/manifest contract
/// hashes non-DB content only, and a live WAL would ride along as garbage.
fn is_db_file(rel: &str) -> bool {
    rel == "data.sqlite3" || rel == "data.sqlite3-wal" || rel == "data.sqlite3-shm"
        || datasets::is_derived_index(rel)
}

// ---------------------------------------------------------------------------
// Progress plumbing
// ---------------------------------------------------------------------------

/// Counting `Write` used as the sink behind the gzip encoder: every byte the
/// tar+gz pipeline flushes is added up and reported (throttled) on the
/// `dataset-upload-progress` channel, so the UI can show packing progress
/// before the upload even starts.
struct ProgressWriter<'a> {
    inner: std::fs::File,
    written: u64,
    last_emit: std::time::Instant,
    app: &'a AppHandle,
    uuid: &'a str,
}

impl<'a> ProgressWriter<'a> {
    fn new(inner: std::fs::File, app: &'a AppHandle, uuid: &'a str) -> Self {
        Self {
            inner,
            written: 0,
            last_emit: std::time::Instant::now(),
            app,
            uuid,
        }
    }
}

impl<'a> Write for ProgressWriter<'a> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.written += n as u64;
        if self.last_emit.elapsed().as_millis() >= 300 {
            let _ = self.app.emit(
                "dataset-upload-progress",
                json!({ "uuid": self.uuid, "phase": "pack", "sent": self.written }),
            );
            self.last_emit = std::time::Instant::now();
        }
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Chunked `Stream` over the packed archive for the multipart body: reqwest
/// polls it while sending, so yields translate directly into upload progress
/// events (guide T2 progress requirement).
struct UploadStream {
    bytes: Vec<u8>,
    pos: usize,
    last_emit: std::time::Instant,
    app: AppHandle,
    uuid: String,
}

impl UploadStream {
    const CHUNK: usize = 64 * 1024;
}

impl futures_util::Stream for UploadStream {
    // reqwest's `Part::stream` body bound: Bytes items under a type-erased
    // boxed error (see reqwest multipart Body impls).
    type Item = Result<bytes::Bytes, Box<dyn std::error::Error + Send + Sync>>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let me = self.get_mut();
        if me.pos >= me.bytes.len() {
            return Poll::Ready(None);
        }
        let end = (me.pos + Self::CHUNK).min(me.bytes.len());
        let chunk = bytes::Bytes::copy_from_slice(&me.bytes[me.pos..end]);
        me.pos = end;
        if me.last_emit.elapsed().as_millis() >= 300 || me.pos == me.bytes.len() {
            let _ = me.app.emit(
                "dataset-upload-progress",
                json!({
                    "uuid": me.uuid, "phase": "upload",
                    "sent": me.pos, "total": me.bytes.len(),
                }),
            );
            me.last_emit = std::time::Instant::now();
        }
        Poll::Ready(Some(Ok(chunk)))
    }
}

// ---------------------------------------------------------------------------
// Packing
// ---------------------------------------------------------------------------

/// Freeze the dataset's SQLite DB into `dest` via the SQLite backup API: one
/// consistent snapshot, journal stripped (the archive always ships `data.sqlite3`
/// in this frozen form — guide §4.2).
fn freeze_db(dir: &Path, dest: &Path) -> Result<(), String> {
    let src = Connection::open(dir.join("data.sqlite3")).map_err(|e| e.to_string())?;
    let _ = src.pragma_update(None, "journal_mode", "DELETE");
    let mut dst_conn = Connection::open(dest).map_err(|e| e.to_string())?;
    {
        let backup = rusqlite::backup::Backup::new(&src, &mut dst_conn)
            .map_err(|e| e.to_string())?;
        backup
            .run_to_completion(256, Duration::from_millis(10), None)
            .map_err(|e| e.to_string())?;
    }
    drop(dst_conn);
    drop(src);
    Ok(())
}

/// Recurse a dataset dir collecting `(rel_path, abs_path)` for every non-DB
/// file. Rel paths are `/`-separated and archive-rooted (no top-level dir),
/// matching the frozen manifest/snapshot contract (guide §4.1).
fn collect_files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, &rel, out)?;
        } else if !is_db_file(&rel) && rel != "info.json" {
            out.push((rel, path));
        }
    }
    Ok(())
}

/// Pack the dataset dir into a tar.gz at `archive_path`: refreshed `info.json`
/// + all non-DB files + the freshly frozen DB as `data.sqlite3`. Returns the
/// archive's byte size; emits packing progress through `app`.
fn pack_archive(
    app: &AppHandle,
    dir: &Path,
    archive_path: &Path,
) -> Result<u64, String> {
    // Refresh the descriptor stamp *before* packing: it becomes the hub's
    // `updated` / `manifest.updated_at`. `sharing.*` is left untouched (the
    // website ignores it — visibility is set on the upload form).
    datasets::touch_info(dir)?;

    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect_files(dir, "", &mut files)?;

    let file = std::fs::File::create(archive_path).map_err(|e| e.to_string())?;
    let uuid_str = dir.file_name().unwrap_or_default().to_string_lossy().to_string();
    let pw = ProgressWriter::new(file, app, &uuid_str);
    let gz = GzEncoder::new(pw, Compression::default());
    let mut ar = tar::Builder::new(gz);
    let mut data = Vec::new();

    let info_rel = datasets::INFO_FILE;
    ar.append_file(
        info_rel,
        &mut std::fs::File::open(dir.join(info_rel)).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    for (rel, path) in &files {
        data.clear();
        let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
        f.read_to_end(&mut data).map_err(|e| e.to_string())?;
        let mut header = tar::Header::new_ustar();
        header.set_path(Path::new(rel)).map_err(|e| e.to_string())?;
        header.set_size(data.len() as u64);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o644);
        header.set_mtime(
            f.metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0),
        );
        header.set_cksum();
        ar.append(&header, &data[..]).map_err(|e| e.to_string())?;
    }

    // The frozen DB under the canonical name, whatever the source journal mode.
    let tmp_db = archive_path.with_extension("sqlite3.freeze");
    let db_present = dir.join("data.sqlite3").exists();
    if db_present {
        freeze_db(dir, &tmp_db)?;
        ar.append_file("data.sqlite3", &mut std::fs::File::open(&tmp_db).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }

    let gz = ar.into_inner().map_err(|e| e.to_string())?;
    let pw = gz.finish().map_err(|e| e.to_string())?;
    let written = pw.written;
    drop(pw);
    let _ = std::fs::remove_file(&tmp_db);
    Ok(written)
}

// ---------------------------------------------------------------------------
// Send + state
// ---------------------------------------------------------------------------

/// Ensure the app-DB table recording what this device has pushed. Kept out of
/// the dataset dir on purpose (guide §4.2).
fn ensure_upload_table(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS dataset_upload_state (
            dataset_uuid  TEXT PRIMARY KEY,
            version       INTEGER NOT NULL,
            archive_sha256 TEXT NOT NULL,
            archive_size  TEXT NOT NULL,
            uploaded_at   TEXT NOT NULL,
            current_version INTEGER,
            published_at  TEXT
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    // The cloud publish pin, echoed by the upload route's catalog row and by every
    // publish (`dataset.current_version` is null until the first publish). No REST
    // route serves the version list to the app, so the pin exists only here — the
    // owner surface probes its live availability against the registry instead of
    // trusting this column (a publish made from the website UI after our last
    // echo is invisible here; the probe keeps the shown state honest).
    crate::sync::client::ensure_column(conn, "dataset_upload_state", "current_version", "INTEGER")?;
    crate::sync::client::ensure_column(conn, "dataset_upload_state", "published_at", "TEXT")?;
    Ok(())
}

/// The dataset uuids this device has ever uploaded to the website, sorted.
/// The round's step-7 auto-cadence drains exactly these through the
/// changed-file diff (the blob route refuses a tree for an unknown uuid, so a
/// dataset that was never uploaded has nothing to reconcile).
pub(crate) fn upload_state_uuids(conn: &Connection) -> Result<Vec<String>, String> {
    ensure_upload_table(conn)?;
    let mut stmt = conn
        .prepare("SELECT dataset_uuid FROM dataset_upload_state ORDER BY dataset_uuid")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// The cloud publish pin + last uploaded version this device recorded for one
/// dataset. `(0, 0)` when it never uploaded from here.
pub(crate) fn upload_publish_pin(conn: &Connection, uuid: &str) -> (i64, i64) {
    if ensure_upload_table(conn).is_err() {
        return (0, 0);
    }
    conn.query_row(
        "SELECT COALESCE(current_version, 0), COALESCE(version, 0) FROM dataset_upload_state WHERE dataset_uuid = ?1",
        params![uuid],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap_or((0, 0))
}

/// Stamp the publish echo (the website's `current_version`) into the upload row.
/// A no-op when this device never uploaded that dataset — you can only publish
/// what you uploaded from somewhere, and an unknown uuid must not gain a
/// half-populated state row.
pub(crate) fn record_publish_echo(conn: &Connection, uuid: &str, current_version: i64) {
    if ensure_upload_table(conn).is_err() {
        return;
    }
    let _ = conn.execute(
        "UPDATE dataset_upload_state SET current_version = ?2, published_at = datetime('now') WHERE dataset_uuid = ?1",
        params![uuid, current_version],
    );
}

/// Map a failed upload response per the T7 error table, tagging a 409 with
/// `owned-by-other` so the UI can offer the save-as-new-copy recovery (never
/// silently overwrite another account's uuid).
async fn upload_http_error(resp: reqwest::Response, new_uuid: &str) -> String {
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let msg = body["error"].as_str().unwrap_or("");
    match status {
        401 => "sign in again — the hub rejected or did not receive your access token (401)"
            .to_string(),
        403 => format!(
            "this account cannot upload here, or the API key is read-only (403){}",
            if msg.is_empty() { String::new() } else { format!(" — {msg}") }
        ),
        409 => format!("owned-by-other: {msg} (409) [uuid={new_uuid}]"),
        413 => format!(
            "archive too large for the hub's limits (413){}",
            if msg.is_empty() { String::new() } else { format!(" — {msg}") }
        ),
        c if c >= 500 => format!("hub is failing right now (HTTP {c}) — retryable: {msg}"),
        c => format!("upload failed (HTTP {c}): {msg}"),
    }
}

/// `POST /api/dataset/upload` with Bearer auth + progress-streamed file part.
/// Returns the hub's `{dataset, version, archive_sha256, archive_size}` body.
async fn send_archive(
    app: &AppHandle,
    settings: &SettingsState,
    uuid: &str,
    archive_path: &Path,
    visibility: Option<&str>,
    tag_uuids: &[String],
) -> Result<Value, String> {
    // Target is the **account website** (`crate::auth::base_url()`), the same
    // origin `sync_mirror_push`, `sync_publish_dataset_version`, and
    // `dataset_push_changed_files` already use. Publishing to the cloud is an
    // owner-side operation, orthogonal to the workspace's LAN `role` and its
    // `pc_url` (which describes an upstream to sync FROM, not a destination
    // to publish TO). This lets a Hub with no upstream still upload its own
    // datasets. A missing OAuth token is a hard, actionable error (a write).
    let base = crate::auth::base_url();
    let token = crate::auth::access_token(settings).await?;

    let archive_bytes = std::fs::read(archive_path).map_err(|e| e.to_string())?;
    let sha = format!("{:x}", Sha256::digest(&archive_bytes));
    let size = archive_bytes.len() as u64;

    // `dataset=` carries the uuid explicitly (guide §3.6): the form field is
    // the authoritative identity, not a guess from the archive layout.
    let qs = format!("dataset={}", super::client::qenc(uuid));
    let url = format!("{base}{UPLOAD_PATH}?{qs}");

    // A generous overall timeout: this is a whole-archive PUT over WAN.
    let client = crate::auth::hub_client_builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(3600))
        .build()
        .map_err(|e| e.to_string())?;
    let part = UploadStream {
        bytes: archive_bytes,
        pos: 0,
        last_emit: std::time::Instant::now(),
        app: app.clone(),
        uuid: uuid.to_string(),
    };
    let mut form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::stream(reqwest::Body::wrap_stream(part))
            .file_name(format!("{uuid}.tar.gz"))
            .mime_str("application/gzip")
            .map_err(|e| e.to_string())?,
    );
    if let Some(v) = visibility {
        form = form.text("visibility", v.to_string());
    }
    for t in tag_uuids {
        form = form.text("tag_uuids", t.clone());
    }
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("cannot reach hub at {base}: {e}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        crate::auth::token_rejected(settings).await;
    }
    if !status.is_success() {
        return Err(upload_http_error(resp, uuid).await);
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("unexpected upload response: {e}"))?;

    // Record success (version + archive identity) in the app DB.
    let version = body["version"].as_i64().unwrap_or(0);
    // The upload route answers with the owner's catalog row, whose `current_version`
    // is the cloud publish pin (null until the first publish) — stamp it so the
    // owner surface knows which version to probe. An absent field (older host)
    // must not erase a pin we learned earlier, hence the COALESCE-guarded write.
    let pin = body["dataset"]["current_version"].as_i64();
    let conn = datasets::dictation::open_app_db(settings)?;
    ensure_upload_table(&conn)?;
    conn.execute(
        "INSERT INTO dataset_upload_state (dataset_uuid, version, archive_sha256, archive_size, uploaded_at, current_version, published_at) VALUES (?1, ?2, ?3, ?4, datetime('now'), ?5, CASE WHEN ?5 IS NULL THEN NULL ELSE datetime('now') END) ON CONFLICT(dataset_uuid) DO UPDATE SET version = excluded.version, archive_sha256 = excluded.archive_sha256, archive_size = excluded.archive_size, uploaded_at = excluded.uploaded_at, current_version = COALESCE(excluded.current_version, dataset_upload_state.current_version), published_at = COALESCE(excluded.published_at, dataset_upload_state.published_at)",
        params![uuid, version, sha, size.to_string(), pin],
    )
    .map_err(|e| e.to_string())?;

    let _ = app.emit(
        "dataset-upload-progress",
        json!({ "uuid": uuid, "phase": "done", "sent": size, "total": size }),
    );
    Ok(body)
}

/// Shared core for both commands: resolve dir → refresh + pack → send.
async fn pack_and_send(
    app: &AppHandle,
    settings: &SettingsState,
    dir: &Path,
    uuid: &str,
    visibility: Option<&str>,
    tag_uuids: &[String],
) -> Result<Value, String> {
    let tmp_root = crate::app_paths::data_subdir("upload_tmp");
    std::fs::create_dir_all(&tmp_root).map_err(|e| e.to_string())?;
    let archive = tmp_root.join(format!("{uuid}.tar.gz"));
    let _ = std::fs::remove_file(&archive);

    log::info!("[upload] {uuid}: packing archive from {}", dir.display());
    let size = pack_archive(app, dir, &archive)?;
    let result = send_archive(app, settings, uuid, &archive, visibility, tag_uuids).await;
    let _ = std::fs::remove_file(&archive);
    let body = match result {
        Ok(body) => body,
        Err(e) => {
            // Without this the failure only lives in the returned string — the
            // app log would show a pack line and then nothing.
            log::error!("[upload] {uuid}: failed after packing {size} bytes: {e}");
            return Err(e);
        }
    };
    log::info!("[upload] {uuid}: packed {size} bytes, hub answered {body}");
    Ok(body)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Export a local dataset and upload it to the website hub.
/// `visibility` (`private|shared|public`) is honoured by the hub only on the
/// uuid's **first** upload — later uploads keep the recorded visibility.
/// Never retried on 409/403; `owned-by-other` in the error means "save as a
/// new copy" (`dataset_remint_uuid`).
#[tauri::command]
pub async fn dataset_export_upload(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    visibility: Option<String>,
    tag_uuids: Option<Vec<String>>,
) -> Result<Value, String> {
    log::info!(
        "[upload] dataset_export_upload: uuid={dataset_uuid} visibility={visibility:?} tag_uuids={tag_uuids:?}"
    );
    let (dir, _ty) = datasets::find_dataset_dir_typed(&settings, &dataset_uuid)?;
    if !dir.join(datasets::INFO_FILE).exists() {
        log::error!(
            "[upload] {dataset_uuid}: directory {} has no {} — not uploadable",
            dir.display(),
            datasets::INFO_FILE
        );
        return Err(format!(
            "'{}' has no {} — not an uploadable dataset directory",
            dir.display(),
            datasets::INFO_FILE
        ));
    }
    pack_and_send(
        &app,
        &settings,
        &dir,
        &dataset_uuid,
        visibility.as_deref(),
        &tag_uuids.unwrap_or_default(),
    )
    .await
}

/// Rows this device has uploaded: lets the UI show "this copy is uploaded (vN)"
/// and detect a locally-changed copy (archive hash differs from the record).
#[tauri::command]
pub async fn dataset_upload_state(
    settings: tauri::State<'_, SettingsState>,
) -> Result<Vec<Value>, String> {
    let conn = datasets::dictation::open_app_db(&settings)?;
    upload_state_rows(&conn)
}

/// The `dataset_upload_state` table as JSON rows, over a caller-supplied
/// connection (the market surface joins it against the catalog in one app-DB
/// read). Ensures the table exists first — a fresh device reads back empty.
pub(crate) fn upload_state_rows(conn: &Connection) -> Result<Vec<Value>, String> {
    ensure_upload_table(conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT dataset_uuid, version, archive_sha256, archive_size, uploaded_at, current_version, published_at \
             FROM dataset_upload_state ORDER BY dataset_uuid",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "dataset_uuid": r.get::<_, String>(0)?,
                "version": r.get::<_, i64>(1)?,
                "archive_sha256": r.get::<_, String>(2)?,
                "archive_size": r.get::<_, String>(3)?,
                "uploaded_at": r.get::<_, String>(4)?,
                "current_version": r.get::<_, Option<i64>>(5)?,
                "published_at": r.get::<_, Option<String>>(6)?,
            }))
        })
        .map_err(|e| e.to_string())?;
    let out: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
    Ok(out)
}

/// Enumerate every **local** dataset across all types (dictation, card, book,
/// read_aloud, wiki) so the Website Hub UI can offer an Upload control for copies
/// that exist on disk but are not in the hub catalog yet. The per-type frontend
/// listings (`dataset_list`, `card_dataset_list`, …) each scan one tree; upload is
/// type-agnostic, so this walks `DatasetType::ALL` the same way
/// [`crate::datasets::find_dataset_dir_typed`] resolves a uuid. Read-only: no
/// `info.json`, an unparseable descriptor, or a type/dir mismatch is skipped.
#[tauri::command]
pub async fn sync_local_datasets(
    settings: State<'_, SettingsState>,
) -> Result<Vec<Value>, String> {
    let mut out: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for ty in datasets::DatasetType::ALL {
        for root in datasets::dataset_roots(&settings, ty) {
            let entries = match std::fs::read_dir(&root) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() || !path.join(datasets::INFO_FILE).exists() {
                    continue;
                }
                let info = match datasets::read_info_opt(&path) {
                    Some(i) => i,
                    None => continue,
                };
                if info.dataset_type != ty {
                    continue;
                }
                if !seen.insert(info.uuid.clone()) {
                    // A uuid is a dataset's global primary key — the catalog, snapshots
                    // and `dataset_sync_state` all resolve by it, so the first scanned
                    // copy wins and the other vanishes from every uuid-keyed surface.
                    // Keep that dedupe, but never let it be silent.
                    log::warn!(
                        "[sync] duplicate dataset uuid {}: {} is shadowed by an earlier scan — re-mint one of them",
                        info.uuid,
                        path.display()
                    );
                    continue;
                }
                out.push(json!({
                    "uuid": info.uuid,
                    "name": info.name,
                    "dataset_type": ty.as_str(),
                }));
            }
        }
    }
    out.sort_by(|a, b| a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or("")));
    Ok(out)
}

/// 409 recovery: mint a fresh uuid for a dataset the hub claims belongs to
/// another account, then upload under the new identity. Rewrites `info.json`,
/// renames the directory under its type root, and keeps local references intact
/// (`source_dataset_uuid` in every local card dataset, `dataset_sync_state`).
/// No silent overwrite of the existing hub row — by design (guide §3.6).
#[tauri::command]
pub async fn dataset_remint_uuid(
    app: AppHandle,
    settings: tauri::State<'_, SettingsState>,
    dataset_uuid: String,
    visibility: Option<String>,
    tag_uuids: Option<Vec<String>>,
) -> Result<Value, String> {
    let (dir, ty) = datasets::find_dataset_dir_typed(&settings, &dataset_uuid)?;
    let new_uuid = uuid::Uuid::new_v4().to_string();

    // 1. Rewrite the descriptor (dir stays the routing authority for type).
    let mut info = datasets::read_info(&dir)?;
    info.uuid = new_uuid.clone();
    info.updated_at = datasets::now_stamp();
    datasets::write_info(&dir, &info)?;

    // 2. Rename under the same type root; refuse if the name is somehow taken.
    let new_dir = dir.parent().unwrap().join(&new_uuid);
    if new_dir.exists() {
        return Err(format!("target directory for {} already exists", new_uuid));
    }
    std::fs::rename(&dir, &new_dir).map_err(|e| e.to_string())?;

    // 3. Local references follow the copy (fork lineage + our own sync row).
    let card_root = settings.datasets_dir().join(ty.as_str());
    if let Ok(root) = std::fs::read_dir(&card_root) {
        for entry in root.filter_map(|e| e.ok()) {
            let path = entry.path();
            if ty == datasets::DatasetType::Card && path.join("data.sqlite3").exists() {
                if let Ok(conn) = Connection::open(&path.join("data.sqlite3")) {
                    let _ = conn.execute(
                        "UPDATE card SET source_dataset_uuid = ?1 WHERE source_dataset_uuid = ?2",
                        params![new_uuid, dataset_uuid],
                    );
                }
            }
        }
    }
    if let Ok(conn) = datasets::dictation::open_app_db(&settings) {
        let _ = conn.execute(
            "UPDATE OR IGNORE dataset_sync_state SET dataset_uuid = ?1 WHERE dataset_uuid = ?2",
            params![new_uuid, dataset_uuid],
        );
        let _ = conn.execute(
            "DELETE FROM dataset_sync_state WHERE dataset_uuid = ?1",
            params![dataset_uuid],
        );
    }

    // 4. Upload the renamed dataset under its new identity.
    let body = pack_and_send(
        &app,
        &settings,
        &new_dir,
        &new_uuid,
        visibility.as_deref(),
        &tag_uuids.unwrap_or_default(),
    )
    .await?;
    log::info!("[upload] reminted {dataset_uuid} -> {new_uuid} and uploaded");
    Ok(json!({ "old_uuid": dataset_uuid, "new_uuid": new_uuid, "upload": body }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_state_uuids_creates_table_and_sorts() {
        let conn = Connection::open_in_memory().unwrap();
        // Fresh device: the table is created on demand and reads back empty.
        assert!(upload_state_uuids(&conn).unwrap().is_empty());
        conn.execute(
            "INSERT INTO dataset_upload_state (dataset_uuid, version, archive_sha256, archive_size, uploaded_at) VALUES (?1, 1, 'sha', '10', 'now')",
            params!["ds-b"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO dataset_upload_state (dataset_uuid, version, archive_sha256, archive_size, uploaded_at) VALUES (?1, 1, 'sha', '10', 'now')",
            params!["ds-a"],
        )
        .unwrap();
        assert_eq!(
            upload_state_uuids(&conn).unwrap(),
            vec!["ds-a".to_string(), "ds-b".to_string()]
        );
    }

    #[test]
    fn publish_pin_survives_old_schema_and_echoes_update_it() {
        let conn = Connection::open_in_memory().unwrap();
        // Simulate a pre-upgrade install: the table WITHOUT the two new columns.
        conn.execute(
            "CREATE TABLE dataset_upload_state (dataset_uuid TEXT PRIMARY KEY, version INTEGER NOT NULL, archive_sha256 TEXT NOT NULL, archive_size TEXT NOT NULL, uploaded_at TEXT NOT NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO dataset_upload_state VALUES ('ds', 3, 'sha', '10', 'now')",
            [],
        )
        .unwrap();
        // ensure_upload_table grows the old schema idempotently.
        ensure_upload_table(&conn).unwrap();
        ensure_upload_table(&conn).unwrap();
        // No pin recorded yet, but the last upload is known.
        assert_eq!(upload_publish_pin(&conn, "ds"), (0, 3));
        record_publish_echo(&conn, "ds", 3);
        assert_eq!(upload_publish_pin(&conn, "ds"), (3, 3));
        // An unknown uuid never gains a half-populated row.
        record_publish_echo(&conn, "ghost", 9);
        assert_eq!(upload_publish_pin(&conn, "ghost"), (0, 0));
    }
}
