//! PC-side REST API (`/api/v1`) that turns the desktop `web_service` into a
//! dataset snapshot server + batched-writeback receiver for the Android thin
//! client.
//!
//! Compiled only under the `desktop` feature (it is part of the server stack).
//! The endpoints are intentionally tiny and mirror what `sync.rs` (phone side)
//! expects:
//!
//! * `GET  /api/v1/status`                     - handshake / TCP-probe target
//! * `POST /api/v1/pair/request`               - device pairing handshake (confirm dialog)
//! * `GET  /api/v1/pair/status`                - poll a pending pairing request
//! * `GET  /api/v1/datasets/{uuid}/manifest`   - snapshot metadata + overall hash
//! * `GET  /api/v1/datasets/{uuid}/changes`    - incremental row-log page for one dataset
//! * `GET  /api/v1/app/changes`                - same page for the per-user app-data scope
//! * `GET  /api/v1/app/state`                  - one identity's full app-data history (backfill)
//! * `GET  /api/v1/datasets/{uuid}/snapshot`   - the whole dataset dir as tar.gz
//! * `POST /api/v1/sync/changes`               - replay queued writeback changes
//! * `GET  /api/v1/chat/messages`              - cross-device chat thread page
//! * `POST /api/v1/chat/message`               - post a chat message (multipart: uuid/text/device_name/file parts)
//! * `GET  /api/v1/chat/attachment/{uuid}`     - stream one stored attachment
//!
//! Auth is owned by the outer trust-zone layer in `web_service.rs`: loopback +
//! Tailscale pass unauthenticated; other networks need a valid per-device
//! Ed25519 signature (see `pairing.rs`) except `/status` and `/pair/*` here.
//! The legacy shared `api_token` is no longer enforced.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use axum::extract::{DefaultBodyLimit, Multipart, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::io::{ReaderStream, SyncIoBridge};
use uuid::Uuid;

use crate::ai;
use crate::datasets;
use crate::datasets::book::{BookChapter, BookSentence, BookSentenceWord};
use crate::datasets::cards::{Card, Tag};
use crate::datasets::dictation::{ListenCue, ListenDictation};
use crate::datasets::read_aloud::{ReadAttempt, ReadText};
use crate::settings::SettingsState;
use crate::sync;
use crate::xp;

/// Shared state for the REST router: the Tauri app handle (to reach managed
/// state). Authentication lives in the outer zone layer of `web_service.rs`.
#[derive(Clone)]
pub struct RestState {
    pub app: AppHandle,
}

/// Build the `/api/v1` router. Mounted by `sync::server::build_router`.
pub fn router(app: AppHandle) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/pair/request", post(pair_request))
        .route("/pair/status", get(pair_status))
        .route("/datasets", get(datasets_list))
        .route("/datasets/{uuid}/manifest", get(manifest))
        .route("/datasets/{uuid}/changes", get(changes))
        .route("/app/changes", get(app_changes))
        .route("/app/state", get(app_state))
        .route("/datasets/{uuid}/snapshot", get(snapshot))
        .route("/file", get(file_endpoint))
        .route("/sync/changes", post(sync_changes))
        .route("/chat/messages", get(chat_messages))
        // File uploads go through this route only; axum's default 2 MB body
        // limit would silently reject larger attachments.
        .route(
            "/chat/message",
            post(chat_message).layer(DefaultBodyLimit::max(256 * 1024 * 1024)),
        )
        .route("/chat/attachment/{uuid}", get(chat_attachment))
        .route("/wiki/dirs", get(wiki_dirs))
        .route("/wiki/dir", get(wiki_dir_list))
        .route("/wiki/file", get(wiki_file))
        .route("/wiki/search", get(wiki_search))
        .with_state(RestState { app })
}

// ---------------------------------------------------------------------------
// GET /status
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct StatusResp {
    version: &'static str,
    app_name: &'static str,
    dataset_count: usize,
    ok: bool,
    /// Wire protocol version so a follower can refuse `/changes`/`/file`/chat
    /// `after_id` against an older hub (§3.1, §7).
    protocol_version: u32,
    /// This machine's sync role (`"hub"` | `"follower"`) — both desktops run the
    /// web service, so the beacon/status must say which role *this* one plays.
    role: String,
    /// Cluster (hub identity group) this workspace belongs to (§3.1).
    cluster_id: String,
    /// Hostname of this machine, so a scanned candidate can be labelled with
    /// *which* PC it is. Empty if the OS did not report one.
    machine: String,
}

async fn status(State(st): State<RestState>) -> Response {
    let settings = st.app.state::<SettingsState>();
    let count = datasets::list_datasets(&settings).len();
    (
        StatusCode::OK,
        Json(StatusResp {
            version: env!("CARGO_PKG_VERSION"),
            app_name: "fms-app",
            dataset_count: count,
            ok: true,
            protocol_version: crate::sync::PROTOCOL_VERSION,
            role: settings.role(),
            cluster_id: settings.cluster_id(),
            machine: crate::sync::machine_name(),
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST /pair/request + GET /pair/status  (pairing handshake, `pairing.rs`)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PairReqBody {
    name: String,
    pubkey_hex: String,
    /// Identity the phone declares (`workspace_identity()`); bound to the
    /// device on approval and enforced on writeback. Empty for legacy phones.
    #[serde(default)]
    user_key: String,
}

/// Pairing handshake. Reached from untrusted networks without a device
/// signature (the zone guard allow-lists `/api/v1/pair/*`). Approval always
/// requires the PC owner to answer the confirm dialog — by design, so that no
/// bearer secret can stand in for a human decision.
async fn pair_request(State(st): State<RestState>, Json(body): Json<PairReqBody>) -> Response {
    let settings = st.app.state::<SettingsState>();
    // The identity is derived from the pubkey server-side; a claimed device_id
    // is never trusted. Reject structurally invalid keys.
    let pubkey = match sync::pairing::hex_decode(&body.pubkey_hex) {
        Ok(b) if b.len() == 32 => b,
        _ => return json_error(StatusCode::BAD_REQUEST, "pubkey_hex must be 32 bytes of hex"),
    };
    let _ = pubkey; // validated only; the hex string is what we store
    let name = body.name.trim().chars().take(64).collect::<String>();
    let name = if name.is_empty() { "device".to_string() } else { name };

    let outcome = match sync::pairing::request_pair(&st.app, &settings, &body.pubkey_hex, &name, &body.user_key) {
        Ok(o) => o,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };
    match outcome {
        sync::pairing::PairRequestOutcome::Approved => {
            Json(serde_json::json!({ "state": "approved" })).into_response()
        }
        sync::pairing::PairRequestOutcome::Denied => {
            Json(serde_json::json!({ "state": "denied" })).into_response()
        }
        sync::pairing::PairRequestOutcome::Pending { request_id, fingerprint } => {
            Json(serde_json::json!({
                "state": "pending",
                "request_id": request_id,
                "fingerprint": fingerprint,
            }))
            .into_response()
        }
    }
}

#[derive(Deserialize)]
struct PairStatusQuery {
    device_id: String,
}

/// Poll a pairing request (dialog path only): pending -> approved/denied.
async fn pair_status(
    State(st): State<RestState>,
    Query(q): Query<PairStatusQuery>,
) -> Response {
    let settings = st.app.state::<SettingsState>();
    match sync::pairing::pending_status(&settings, &q.device_id) {
        Ok(state) => Json(serde_json::json!({ "state": state })).into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

// ---------------------------------------------------------------------------
// GET /datasets  (lightweight list for the phone's sync picker)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct DatasetListItem {
    uuid: String,
    name: String,
    updated: String,
    /// One of `dictation` | `card` | `book` | `read_aloud` — lets the follower
    /// group the list and know which local root to unpack a snapshot into.
    dataset_type: String,
    /// Dictation: media file count. Card: card count. Read-aloud: text count.
    /// Book: 0 (not tracked). Zeroed under `?lite=1` (the incremental sync round
    /// only needs uuid + type + updated, so counting rows per dataset is wasted
    /// work).
    media_count: usize,
    status: String,
}

#[derive(Deserialize)]
struct DatasetsListQuery {
    /// `1` (or `true`) skips the per-dataset row/media counts in the response.
    #[serde(default)]
    lite: Option<String>,
}

async fn datasets_list(
    State(st): State<RestState>,
    Query(q): Query<DatasetsListQuery>,
) -> Response {
    let settings = st.app.state::<SettingsState>();
    let lite = matches!(q.lite.as_deref(), Some("1") | Some("true"));
    let mut items: Vec<DatasetListItem> = Vec::new();

    // Dictation datasets. Raw-import folders (a directory with media/ but no
    // info.json) carry an empty uuid and cannot be resolved by the
    // manifest/snapshot endpoints, so skip them rather than offer dead entries.
    for d in datasets::list_datasets(&settings) {
        if d.info.uuid.is_empty() {
            continue;
        }
        items.push(DatasetListItem {
            uuid: d.info.uuid,
            name: d.info.name,
            updated: d.info.updated,
            dataset_type: "dictation".into(),
            media_count: if lite { 0 } else { d.media_count },
            status: d.status,
        });
    }

    // Card datasets.
    for d in crate::datasets::cards::list_card_datasets(&settings) {
        if d.info.uuid.is_empty() {
            continue;
        }
        items.push(DatasetListItem {
            uuid: d.info.uuid,
            name: d.info.name,
            updated: d.info.updated,
            dataset_type: "card".into(),
            media_count: if lite { 0 } else { d.card_count },
            status: "ready".into(),
        });
    }

    // Books (reading library).
    for b in crate::datasets::book::list_books(&settings) {
        if b.uuid.is_empty() {
            continue;
        }
        items.push(DatasetListItem {
            uuid: b.uuid,
            name: b.title,
            updated: b.updated_at,
            dataset_type: "book".into(),
            media_count: 0,
            status: "ready".into(),
        });
    }

    // Read-aloud datasets (texts + recorded attempts, attempts' WAVs in media/).
    for d in crate::datasets::read_aloud::list_datasets(&settings) {
        if d.uuid.is_empty() {
            continue;
        }
        items.push(DatasetListItem {
            uuid: d.uuid.clone(),
            name: d.name,
            updated: d.updated_at,
            dataset_type: "read_aloud".into(),
            media_count: if lite { 0 } else { crate::datasets::read_aloud::count_texts(&settings, &d.uuid) },
            status: "ready".into(),
        });
    }

    (StatusCode::OK, Json(items)).into_response()
}

// ---------------------------------------------------------------------------
// Manifest + hashing helpers
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct Manifest {
    file_count: usize,
    total_bytes: u64,
    overall_hash: String,
    updated_at: String,
    /// Dataset type (`dictation` | `card` | `book` | `read_aloud`) so the
    /// follower unpacks the snapshot into the matching local root.
    #[serde(default)]
    dataset_type: String,
    /// Per non-DB file content hash (`{ rel_path: sha256 }`), for incremental
    /// `/file` fetches. Excludes the dataset DB, which the row log owns (§3.2),
    /// so a single row edit does not force a whole-DB file re-download.
    #[serde(default)]
    files: std::collections::BTreeMap<String, String>,
}

/// The dataset SQLite file + its WAL/SHM sidecars are excluded from the hashed
/// file set (§3.2): they are content the row log and snapshot own, not media.
fn is_db_file(rel: &str) -> bool {
    rel == "data.sqlite3" || rel == "data.sqlite3-wal" || rel == "data.sqlite3-shm"
}

/// SHA-256 (hex) of a file's contents, streamed in chunks.
fn hash_file_contents(path: &Path) -> Result<String, String> {
    use std::io::Read as _;
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

/// Cached per-file hash (§3.2): reuse the recorded sha256 while the file's
/// `(size, mtime)` tuple is unchanged; recompute and upsert otherwise. Skipping
/// this cache would mean re-hashing multi-GB media on every manifest call.
fn cached_file_hash(
    cache: Option<&rusqlite::Connection>,
    dataset_uuid: &str,
    rel: &str,
    path: &Path,
    size: u64,
    mtime: u64,
) -> String {
    if let Some(conn) = cache {
        let cached: Option<String> = conn
            .query_row(
                "SELECT sha256 FROM file_hash_cache \
                 WHERE dataset_uuid = ?1 AND path = ?2 AND size = ?3 AND mtime = ?4",
                rusqlite::params![dataset_uuid, rel, size as i64, mtime as i64],
                |r| r.get(0),
            )
            .ok();
        if let Some(h) = cached {
            return h;
        }
    }
    let h = hash_file_contents(path).unwrap_or_default();
    if let Some(conn) = cache {
        let _ = conn.execute(
            "INSERT OR REPLACE INTO file_hash_cache (dataset_uuid, path, size, mtime, sha256) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![dataset_uuid, rel, size as i64, mtime as i64, &h],
        );
    }
    h
}

/// Compute the manifest over the **non-DB** file set: file count, total bytes,
/// a per-file `sha256` map, and an `overall_hash` folded from those hashes so
/// it is a fast "nothing changed" short-circuit. `updated_at` comes from
/// info.json when present.
fn compute_manifest(
    dir: &Path,
    dataset_uuid: &str,
    cache: Option<&rusqlite::Connection>,
) -> Manifest {
    let files: Vec<(String, PathBuf)> = collect_rel_files(dir)
        .into_iter()
        .filter(|(rel, _)| !is_db_file(rel))
        .collect();
    let mut overall = Sha256::new();
    let mut total_bytes = 0u64;
    let mut map = std::collections::BTreeMap::new();
    for (rel, path) in &files {
        let meta = std::fs::metadata(path).ok();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let mtime = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        total_bytes += size;
        let h = cached_file_hash(cache, dataset_uuid, rel, path, size, mtime);
        overall.update(rel.as_bytes());
        overall.update([0u8]);
        overall.update(h.as_bytes());
        map.insert(rel.clone(), h);
    }
    let overall_hash = format!("{:x}", overall.finalize());

    let info_path = dir.join("info.json");
    let updated_at = std::fs::read_to_string(&info_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("updated").and_then(|u| u.as_str()).map(String::from))
        .unwrap_or_default();

    Manifest {
        file_count: files.len(),
        total_bytes,
        overall_hash,
        updated_at,
        dataset_type: String::new(),
        files: map,
    }
}

/// Recursively collect `(rel_path_forward_slash, absolute_path)` pairs, sorted
/// by rel_path so the hash is stable across machines/runs.
fn collect_rel_files(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(root, &p, out);
        } else {
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, p));
        }
    }
}

/// `PRAGMA wal_checkpoint(TRUNCATE)` so the archived `data.sqlite3` is
/// self-contained (no sidecar `-wal`/`-shm` needed on the receiving end).
fn checkpoint_db(dir: &Path) {
    let db = dir.join("data.sqlite3");
    if let Ok(conn) = rusqlite::Connection::open(&db) {
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }
}

/// Produce a point-in-time copy of a dataset's `data.sqlite3` via `VACUUM INTO`.
/// Unlike `wal_checkpoint(TRUNCATE)` — which does **not** block concurrent
/// writers and so could archive a torn image — `VACUUM INTO` reads a consistent
/// snapshot transaction, safe under live edits (§3.2 snapshot correctness).
/// Returns the temp file path (caller deletes it) or `None` when there is no DB
/// to copy (raw-import folders carry none).
fn snapshot_db_copy(dir: &Path) -> Option<PathBuf> {
    let db = dir.join("data.sqlite3");
    if !db.exists() {
        return None;
    }
    let dest = std::env::temp_dir().join(format!("fms-snap-{}-{}.sqlite3", Uuid::new_v4().simple(), std::process::id()));
    let _ = std::fs::remove_file(&dest);
    let conn = rusqlite::Connection::open(&db).ok()?;
    // `VACUUM INTO` takes a text expression; a bound parameter is allowed in a
    // prepared statement and avoids quoting/escaping the temp path.
    conn.prepare_cached("VACUUM INTO ?1")
        .ok()?
        .execute(rusqlite::params![dest.to_string_lossy().as_ref()])
        .ok()?;
    Some(dest)
}

async fn manifest(State(st): State<RestState>, axum::extract::Path(uuid): axum::extract::Path<String>) -> Response {
    let settings = st.app.state::<SettingsState>();
    let (dir, ty) = match datasets::find_dataset_dir_typed(&settings, &uuid) {
        Ok(d) => d,
        Err(e) => return json_error(StatusCode::NOT_FOUND, &e),
    };
    checkpoint_db(&dir);
    let cache = crate::datasets::dictation::open_app_db(&settings).ok();
    let mut m = compute_manifest(&dir, &uuid, cache.as_ref());
    m.dataset_type = ty.as_str().to_string();
    (StatusCode::OK, Json(m)).into_response()
}

// ---------------------------------------------------------------------------
// GET /snapshot  (whole dataset dir streamed as one tar.gz)
// ---------------------------------------------------------------------------

async fn snapshot(State(st): State<RestState>, axum::extract::Path(uuid): axum::extract::Path<String>) -> Response {
    let settings = st.app.state::<SettingsState>();
    let (dir, _ty) = match datasets::find_dataset_dir_typed(&settings, &uuid) {
        Ok(d) => d,
        Err(e) => return json_error(StatusCode::NOT_FOUND, &e),
    };

    // Read the hub cursor *before* freezing the DB image: a follower records this
    // seq as its cursor, so any change that races in afterwards (seq > this) is
    // re-delivered by the next `/changes` pull rather than silently lost. The
    // snapshot already containing a slightly newer row is then a harmless,
    // idempotent re-apply.
    let hub_seq: i64 = crate::datasets::dictation::open_app_db(&settings)
        .ok()
        .and_then(|conn| crate::sync::change_log::hub_seq_for(&conn, &uuid).ok())
        .unwrap_or(0);

    // Freeze a consistent DB image, then hash + archive the non-DB set exactly
    // as the manifest endpoint would (the manifest excludes the DB itself).
    let snap_db = snapshot_db_copy(&dir);
    let cache = crate::datasets::dictation::open_app_db(&settings).ok();
    let m = compute_manifest(&dir, &uuid, cache.as_ref());

    // Archive the frozen copy under the name `data.sqlite3` and never the live
    // sidecars (the vacuum image is self-contained, WAL off on the receiver).
    let files: Vec<(String, PathBuf)> = collect_rel_files(&dir)
        .into_iter()
        .filter(|(rel, _)| rel != "data.sqlite3-wal" && rel != "data.sqlite3-shm")
        .map(|(rel, p)| {
            if rel == "data.sqlite3" {
                if let Some(sp) = &snap_db {
                    return (rel, sp.clone());
                }
            }
            (rel, p)
        })
        .collect();

    // Bridge a blocking tar.gz writer into an async body via an in-memory duplex.
    let (client_half, server_half) = tokio::io::duplex(64 * 1024);
    tauri::async_runtime::spawn_blocking(move || {
        let mut writer = SyncIoBridge::new(server_half);
        let gz = GzEncoder::new(&mut writer, Compression::fast()); // level 1: media already compressed
        let mut tb = tar::Builder::new(gz);
        for (rel, path) in &files {
            if let Ok(mut f) = std::fs::File::open(path) {
                let _ = tb.append_file(rel, &mut f);
            }
        }
        // Return the GzEncoder from the tar builder, then finish() it so the gzip
        // trailer is written and the `&mut writer` borrow is released.
        if let Ok(gz) = tb.into_inner() {
            let _ = gz.finish();
        }
        let _ = writer.flush();
        // writer dropped here -> closes the duplex -> EOF for the reader.
        if let Some(sp) = &snap_db {
            let _ = std::fs::remove_file(sp);
        }
    });

    let stream = ReaderStream::new(client_half);
    let body = axum::body::Body::from_stream(stream);
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-snapshot-hash",
        m.overall_hash.parse().unwrap(),
    );
    headers.insert(
        "x-snapshot-seq",
        hub_seq.to_string().parse().unwrap(),
    );
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        "application/gzip".parse().unwrap(),
    );
    (StatusCode::OK, headers, body).into_response()
}

// ---------------------------------------------------------------------------
// GET /datasets/{uuid}/changes  (incremental row-log pull, §3.2 / §3.3)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ChangesQuery {
    /// Follower's per-dataset `sync_log` cursor. Absent ⇒ treated as -1
    /// (unknown) so a not-yet-migrated copy is told to resync (§3.2).
    #[serde(default)]
    after: Option<i64>,
}

/// Stream `sync_log` rows for one dataset after a seq cursor. The response
/// carries `pruned_up_to` (gap detection → `resync_required`) and the hub's
/// current `seq` (drift check, §3.2).
async fn changes(
    State(st): State<RestState>,
    axum::extract::Path(uuid): axum::extract::Path<String>,
    Query(q): Query<ChangesQuery>,
) -> Response {
    let settings = st.app.state::<SettingsState>();
    // Reject unknown datasets with 404 before touching the log.
    if let Err(e) = datasets::find_dataset_dir_typed(&settings, &uuid) {
        return json_error(StatusCode::NOT_FOUND, &e);
    }
    let conn = match crate::datasets::dictation::open_app_db(&settings) {
        Ok(c) => c,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };
    let after = q.after.unwrap_or(-1);
    // Opportunistic hub-side retention (§3.5): at most once a day this compacts
    // the journal and raises `pruned_up_to`, which is what makes a follower with
    // a stale cursor below the new mark get `resync_required` from `read_changes`
    // below. Only the hub owns a journal worth pruning.
    if settings.role() == "hub" {
        crate::sync::change_log::maybe_prune(&conn);
    }
    match crate::sync::change_log::read_changes(&conn, &uuid, after) {
        Ok(page) => (StatusCode::OK, Json(page)).into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

// ---------------------------------------------------------------------------
// GET /app/changes  (per-user app-data journal, §3.2 / §3.3)
// ---------------------------------------------------------------------------

/// The journal page for [`sync::change_log::APP_SCOPE`] — `dictation` progress and
/// `xp` awards, which live in the app DB and therefore belong to no dataset.
/// Followers pull it once per round regardless of which datasets they subscribe
/// to, so per-user state no longer depends on a dataset copy being present (and
/// awards with no dataset at all, e.g. book reading XP, finally have a route that
/// serves them). Same page shape as `/datasets/{uuid}/changes`; there is no
/// dataset to resolve, because the scope is a fixed key.
async fn app_changes(
    State(st): State<RestState>,
    Query(q): Query<ChangesQuery>,
) -> Response {
    let settings = st.app.state::<SettingsState>();
    let conn = match crate::datasets::dictation::open_app_db(&settings) {
        Ok(c) => c,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };
    let after = q.after.unwrap_or(-1);
    if settings.role() == "hub" {
        crate::sync::change_log::maybe_prune(&conn);
    }
    match crate::sync::change_log::read_changes(&conn, sync::change_log::APP_SCOPE, after) {
        Ok(page) => (StatusCode::OK, Json(page)).into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

// ---------------------------------------------------------------------------
// GET /app/state  (per-user history backfill, §3.2 / §3.6)
// ---------------------------------------------------------------------------

/// Row cap per table per `/app/state` page. A ledger grows with every award, so one
/// unbounded read could hand a follower a body far larger than the rest of a sync
/// round; the response carries a next cursor for each table and the follower pages
/// through them, so the cap bounds one request, not the history.
const HISTORY_ROW_LIMIT: i64 = 20_000;

#[derive(Deserialize)]
struct AppStateQuery {
    /// Only a *fallback* — see [`resolve_read_identity`]. A device bound to a real
    /// account can never use it to read somebody else's history.
    user_key: Option<String>,
    /// Rowid cursors for the two tables (`0` = from the start). Independent
    /// because the two lists have different lengths and different page ends.
    after_dictation: Option<i64>,
    after_xp: Option<i64>,
}

#[derive(Serialize)]
struct AppStateResp {
    /// Whose history this is, as resolved from the device binding.
    user_key: String,
    /// Current progress per (media, subtitle) pair — state, not deltas, since the
    /// journal that would carry the history is pruned.
    dictation: Vec<ListenDictation>,
    /// XP deltas oldest-first, replayable through the ordinary award path.
    xp: Vec<xp::XpHistoryRow>,
    /// Next rowid cursors, `Some` only when that page was full. `None` for both
    /// means the follower has the whole history.
    next_dictation_after: Option<i64>,
    next_xp_after: Option<i64>,
    /// The hub's stored total, so a follower can compare it after recomputing and
    /// notice a divergence instead of reporting a clean backfill.
    lifetime_xp: i64,
    generated_at: String,
}

/// Resolve whose per-user history a read may return.
///
/// Stricter than [`resolve_write_identity`] by design: a write carries a declared
/// `user_key` that can be *compared* against the binding and rejected on mismatch,
/// whereas a GET has no such body — so a client-chosen key must never override a
/// real binding. It is only consulted when the binding is itself the transient
/// `"local"` sentinel (device paired while logged out), mirroring the adoption rule
/// the write path uses. With no device binding (trusted zone) the PC's own
/// workspace identity is served.
fn resolve_read_identity(
    auth: &sync::pairing::AuthContext,
    requested: &str,
    settings: &SettingsState,
) -> String {
    let Some(bound) = auth.bound_user_id.as_deref() else {
        return crate::auth::workspace_identity(settings);
    };
    let bound_real = !bound.is_empty() && bound != "local";
    let requested_real = !requested.is_empty() && requested != "local";
    if bound_real {
        bound.to_string()
    } else if requested_real {
        requested.to_string()
    } else {
        bound.to_string()
    }
}

/// The state-based counterpart to `/app/changes`: everything the hub still knows
/// about one identity's per-user history, regardless of journal retention. The row
/// log is forward-only and pruned, and a fresh subscription starts at the hub's
/// current seq, so deltas alone can never deliver history older than that point —
/// this endpoint is how a new device gets its past.
async fn app_state(
    State(st): State<RestState>,
    Extension(auth): Extension<sync::pairing::AuthContext>,
    Query(q): Query<AppStateQuery>,
) -> Response {
    let settings = st.app.state::<SettingsState>();
    let user_key = resolve_read_identity(&auth, q.user_key.as_deref().unwrap_or(""), &settings);
    let (dictation, next_dictation_after) = match datasets::dictation::dictation_rows_for_user(
        &settings,
        &user_key,
        q.after_dictation.unwrap_or(0),
        HISTORY_ROW_LIMIT,
    ) {
        Ok(page) => page,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };
    let (xp_rows, next_xp_after) = match xp::ledger_rows_for_user(
        &settings,
        &user_key,
        q.after_xp.unwrap_or(0),
        HISTORY_ROW_LIMIT,
    ) {
        Ok(page) => page,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };
    // Best-effort: this number is a cross-check for the follower's log, so a read
    // failure must not fail the backfill.
    let hub_total = xp::user_total(&settings, &user_key).unwrap_or(0);
    let resp = AppStateResp {
        user_key,
        lifetime_xp: hub_total,
        dictation,
        xp: xp_rows,
        next_dictation_after,
        next_xp_after,
        generated_at: chrono::Utc::now().to_rfc3339(),
    };
    (StatusCode::OK, Json(resp)).into_response()
}

// ---------------------------------------------------------------------------
// GET /file  (one non-DB file's bytes, resumable, §3.3)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct FileQuery {
    dataset: String,
    path: String,
}

/// Serve a single non-DB file out of a dataset directory. Canonicalizes the
/// path and refuses anything resolving outside the dataset dir (traversal),
/// refuses the dataset DB sidecars, and honours `Range` so a multi-GB media
/// fetch can resume on flaky Wi-Fi (§3.3).
async fn file_endpoint(
    State(st): State<RestState>,
    headers: HeaderMap,
    Query(q): Query<FileQuery>,
) -> Response {
    let settings = st.app.state::<SettingsState>();
    let (dir, _ty) = match datasets::find_dataset_dir_typed(&settings, &q.dataset) {
        Ok(d) => d,
        Err(e) => return json_error(StatusCode::NOT_FOUND, &e),
    };
    let rel = q.path.trim_start_matches('/');
    if rel.is_empty() || is_db_file(rel) {
        return json_error(
            StatusCode::FORBIDDEN,
            "only non-database files are served via /file",
        );
    }
    let target = dir.join(rel);
    // Path-containment guard (mirrors `dataset_file` in web_service.rs).
    let base_c = match dir.canonicalize() {
        Ok(p) => p,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let target_c = match target.canonicalize() {
        Ok(p) => p,
        Err(_) => return json_error(StatusCode::NOT_FOUND, "file not found"),
    };
    if !target_c.starts_with(&base_c) {
        return json_error(StatusCode::FORBIDDEN, "access denied");
    }
    if !target_c.is_file() {
        return json_error(StatusCode::NOT_FOUND, "not a file");
    }
    crate::sync::server::stream_file(&target_c, &headers).await
}

// ---------------------------------------------------------------------------
// POST /sync/changes  (batched writeback replay)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SyncChangesReq {
    /// Identity the phone is writing under (`workspace_identity()`). Checked
    /// against the device's bound identity before any change is applied.
    #[serde(default)]
    user_key: String,
    changes: Vec<Change>,
}

#[derive(Deserialize)]
struct Change {
    id: String,
    kind: String,
    #[serde(default)]
    dataset_uuid: Option<String>,
    payload: serde_json::Value,
    /// Device-local ISO timestamp of the user action (§3.3 / §3.6). Drives
    /// later-`edit_time`-wins conflicts and, once the apply path is built
    /// (Phase 4), the applied row's `updated_at`. Carried here so the hub has
    /// it available for the log entry and comparison.
    #[serde(default)]
    edit_time: String,
    #[serde(default)]
    #[allow(dead_code)]
    queued_at: String,
}

#[derive(Serialize)]
struct ChangeResult {
    id: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// §3.4 conflict: a losing push is still acknowledged (`ok = true`, so the
    /// follower stops resending it) but flagged `rejected`, carrying the hub's
    /// winning entry. The follower drops its stale queued change; the winner
    /// re-converges it on the next `/changes` pull (no seq is rewritten here).
    #[serde(skip_serializing_if = "Option::is_none")]
    rejected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    winner_seq: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    winner_payload: Option<serde_json::Value>,
}

impl ChangeResult {
    fn applied(id: String) -> Self {
        ChangeResult { id, ok: true, error: None, rejected: None, winner_seq: None, winner_payload: None }
    }
    fn failed(id: String, error: String) -> Self {
        ChangeResult { id, ok: false, error: Some(error), rejected: None, winner_seq: None, winner_payload: None }
    }
    fn losing(id: String, winner_seq: i64, winner_payload: serde_json::Value) -> Self {
        ChangeResult {
            id,
            ok: true,
            error: None,
            rejected: Some(true),
            winner_seq: Some(winner_seq),
            winner_payload: Some(winner_payload),
        }
    }
}

/// Parse a client `edit_time` into a comparable UTC instant, tolerating both
/// RFC3339 and SQLite `datetime('now')` (`YYYY-MM-DD HH:MM:SS`, assumed UTC)
/// formats, and clamping anything dated in the future to the hub's now (§3.4: a
/// follower with a fast clock must not win every later conflict forever). An
/// empty or unparseable stamp is treated as "arrived now".
fn normalize_edit_time(raw: &str, hub_now: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return hub_now;
    }
    let parsed = chrono::DateTime::parse_from_rfc3339(trimmed)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S").map(|n| n.and_utc())
        })
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S").map(|n| n.and_utc())
        });
    match parsed {
        Ok(dt) if dt > hub_now => hub_now,
        Ok(dt) => dt,
        Err(_) => hub_now,
    }
}

/// Parse a `sync_log` `edit_time` (always written as RFC3339 UTC by the hub) for
/// comparison. Returns `None` on a malformed/legacy value so the caller can fall
/// back to treating the existing entry as the oldest possible (incoming wins).
fn parse_log_time(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .ok()
}

/// Resolve the identity that writeback changes must be attributed to, and
/// reject a device that tries to write as a different user than it was paired
/// for.
///
/// * **Paired device** (`auth.bound_user_id` present): the bound identity is
///   authoritative. If both the bound identity and the phone's declared
///   `user_key` are real accounts (non-empty, not the `"local"` sentinel) and
///   they differ, the batch is rejected — the device was re-provisioned to
///   another account and must re-pair. A device bound while logged out
///   (`"local"`) adopts the phone's now-logged-in identity so sync never
///   dead-ends after a later login.
/// * **Trusted zone** (loopback / Tailscale agent path, no device binding): the
///   PC's own current workspace identity is used, preserving legacy behavior.
fn resolve_write_identity(
    auth: &sync::pairing::AuthContext,
    phone_key: &str,
    settings: &SettingsState,
) -> Result<String, String> {
    let Some(bound) = auth.bound_user_id.as_deref() else {
        // Trusted zone: attribute to the PC's current workspace user.
        return Ok(crate::auth::workspace_identity(settings));
    };
    let bound_real = !bound.is_empty() && bound != "local";
    let phone_real = !phone_key.is_empty() && phone_key != "local";
    if bound_real && phone_real && bound != phone_key {
        return Err(format!(
            "device is paired as '{}' but sent changes for '{}'; re-pair the device to switch users",
            bound, phone_key
        ));
    }
    if bound_real {
        Ok(bound.to_string())
    } else if phone_real {
        Ok(phone_key.to_string())
    } else {
        Ok(bound.to_string())
    }
}

async fn sync_changes(
    State(st): State<RestState>,
    Extension(auth): Extension<sync::pairing::AuthContext>,
    Json(req): Json<SyncChangesReq>,
) -> Response {
    let settings = st.app.state::<SettingsState>();

    // Enforce the device->user binding before touching any data.
    let write_identity = match resolve_write_identity(&auth, &req.user_key, &settings) {
        Ok(id) => id,
        Err(e) => {
            log::warn!("[rest] writeback rejected (device {:?}): {}", auth.device_id, e);
            return json_error(StatusCode::FORBIDDEN, &e);
        }
    };

    // Idempotency ledger + hub sync tables live in the app-level DB (survive
    // dataset swaps). `ensure_hub_tables` creates `applied_changes` alongside
    // `sync_log`, so the hub's own writes and follower replays share one journal.
    let ledger = match datasets::dictation::open_app_db(&settings) {
        Ok(conn) => {
            if let Err(e) = crate::sync::change_log::ensure_hub_tables(&conn) {
                return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e);
            }
            conn
        }
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    let mut results = Vec::with_capacity(req.changes.len());
    let hub_now = chrono::Utc::now();
    for ch in req.changes {
        // Skip anything already applied (idempotent by change id).
        if crate::sync::change_log::already_applied(&ledger, &ch.id) {
            results.push(ChangeResult::applied(ch.id));
            continue;
        }

        // Later-`edit_time`-wins conflict resolution (§3.4) for mutable *state*
        // rows. The hub's newest `sync_log` entry for the same object mirrors the
        // live row (`updated_at`) and any tombstone (its `op`), so a push that
        // loses on time is rejected without a new seq being written — the follower
        // drops it and re-converges from the winner on its next `/changes` pull.
        if crate::sync::change_log::ChangeClass::of(&ch.kind).coalescible() {
            let dataset_uuid = ch.dataset_uuid.clone().unwrap_or_default();
            // Compare keys the way both sides write them: the journal scope
            // (per-user app data lives under `@app`, not the dataset) plus the
            // per-user object id resolved against *this* batch's enforced
            // identity — otherwise one user's push can lose a later-wins contest
            // against another user's row.
            let scope = crate::sync::change_log::scope_for(&ch.kind, &dataset_uuid).to_string();
            let object_id = crate::sync::change_log::object_id_for(&ch.kind, &ch.payload, &write_identity);
            if !object_id.is_empty() {
                let incoming = normalize_edit_time(&ch.edit_time, hub_now);
                let latest = crate::sync::change_log::latest_for_object(&ledger, &scope, &object_id)
                    .ok()
                    .flatten();
                if let Some((winner_seq, winner_edit, _op, winner_payload)) = latest {
                    // A missing/unparseable stored time is treated as oldest so
                    // the fresh push wins rather than being wrongly rejected.
                    let existing = parse_log_time(&winner_edit).unwrap_or(hub_now);
                    if existing >= incoming {
                        log::info!(
                            "[rest] writeback {} ({}) lost to hub seq {} (edit_time {} >= incoming {})",
                            ch.id, ch.kind, winner_seq, winner_edit, incoming.to_rfc3339()
                        );
                        results.push(ChangeResult::losing(ch.id, winner_seq, winner_payload));
                        continue;
                    }
                } else if crate::sync::change_log::op_for(&ch.kind) == "upsert"
                    && !crate::sync::change_log::is_app_scope_kind(&ch.kind)
                    && crate::sync::change_log::is_tombstoned_at_least(
                        settings.inner(),
                        &dataset_uuid,
                        &object_id,
                        &incoming.to_rfc3339(),
                    )
                {
                    // No log entry, but a recorded tombstone at/after this edit:
                    // the row was deleted before incremental sync existed, so an
                    // offline edit resurrecting it is dropped against the delete
                    // (§3.4) rather than re-creating a dead row.
                    log::info!(
                        "[rest] writeback {} ({}) dropped: tombstone >= incoming edit_time",
                        ch.id, ch.kind
                    );
                    results.push(ChangeResult::losing(ch.id, 0, serde_json::Value::Null));
                    continue;
                }
            }
        }

        match replay(&settings, &write_identity, &ch).await {
            Ok(()) => {
                crate::sync::change_log::mark_applied(&ledger, &ch.id);
                results.push(ChangeResult::applied(ch.id));
            }
            Err(e) => {
                log::warn!("[rest] writeback {} ({}) failed: {}", ch.id, ch.kind, e);
                results.push(ChangeResult::failed(ch.id, e));
            }
        }
    }

    (StatusCode::OK, Json(results)).into_response()
}

async fn replay(
    settings: &tauri::State<'_, SettingsState>,
    write_identity: &str,
    ch: &Change,
) -> Result<(), String> {
    // Per-user app data journals under the `@app` scope, so the scope field says
    // nothing about which dataset the row belongs to: `dictation` needs no
    // dataset at all (its app-DB row is keyed by media + subtitle), and `xp`
    // carries its own `dataset_uuid` inside the payload.
    let dataset_uuid = if sync::change_log::is_app_scope_kind(&ch.kind) {
        String::new()
    } else {
        ch.dataset_uuid.clone().unwrap_or_default()
    };
    match ch.kind.as_str() {
        "cue_save" => {
            let cue: ListenCue =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            datasets::dictation::listen_save_cue(settings.clone(), dataset_uuid, cue).await
        }
        "cue_delete" => {
            let cue_uuid: String = extract_cue_uuid(&ch.payload)?;
            datasets::dictation::listen_delete_cue(settings.clone(), dataset_uuid, cue_uuid).await
        }
        "dictation" => {
            let d: ListenDictation =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            // Attribute to the enforced identity, not the PC's current workspace.
            datasets::dictation::listen_save_dictation_as(
                settings.inner(),
                &dataset_uuid,
                &d,
                write_identity,
            )
            .await
        }
        "xp" => {
            let amount = ch.payload.get("amount").and_then(|v| v.as_i64()).unwrap_or(0);
            let source = ch
                .payload
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("sync")
                .to_string();
            let reference_id = ch
                .payload
                .get("reference_id")
                .and_then(|v| v.as_str())
                .unwrap_or(ch.id.as_str())
                .to_string();
            // The enforced identity is authoritative; a client-supplied
            // `user_id` in the payload is never trusted. The dataset attribution
            // does come from the payload, since the change arrived under the
            // `@app` scope.
            let ds_payload = ch
                .payload
                .get("dataset_uuid")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let ds = if dataset_uuid.is_empty() {
                if ds_payload.is_empty() {
                    None
                } else {
                    Some(ds_payload.as_str())
                }
            } else {
                Some(dataset_uuid.as_str())
            };
            xp::xp_award_internal(
                settings.inner(),
                write_identity,
                amount,
                &source,
                &reference_id,
                ds,
            )
            .map(|_| ())
        }
        "card_save" => {
            let card: Card =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            datasets::cards::card_save(settings.clone(), dataset_uuid, card).await.map(|_| ())
        }
        "card_delete" => {
            let card_uuid = extract_str(&ch.payload, "card_uuid")?;
            datasets::cards::card_delete(settings.clone(), dataset_uuid, card_uuid).await
        }
        "card_review" => {
            let card_uuid = extract_str(&ch.payload, "card_uuid")?;
            let quality = ch.payload.get("quality").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            datasets::cards::card_test_submit(settings.clone(), dataset_uuid, card_uuid, quality).await.map(|_| ())
        }
        "card_tag_save" => {
            let tag: Tag =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            datasets::cards::card_tag_save(settings.clone(), dataset_uuid, tag).await.map(|_| ())
        }
        "card_tag_delete" => {
            let tag_uuid = extract_str(&ch.payload, "tag_uuid")?;
            datasets::cards::card_tag_delete(settings.clone(), dataset_uuid, tag_uuid).await
        }
        "card_set_tags" => {
            let card_uuid = extract_str(&ch.payload, "card_uuid")?;
            let tag_uuids: Vec<String> = ch.payload.get("tag_uuids").and_then(|v| v.as_array()).map(|arr| {
                arr.iter().filter_map(|v| v.as_str().map(String::from)).collect()
            }).unwrap_or_default();
            datasets::cards::card_set_tags(settings.clone(), dataset_uuid, card_uuid, tag_uuids).await
        }
        "book_chapter_save" => {
            let chapter: BookChapter =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            datasets::book::book_save_chapter(settings.clone(), dataset_uuid, chapter).await
        }
        "book_chapter_delete" => {
            let uuid = extract_str(&ch.payload, "uuid")?;
            datasets::book::book_delete_chapter(settings.clone(), dataset_uuid, uuid).await
        }
        "book_sentence_save" => {
            let sentence: BookSentence =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            datasets::book::book_save_sentence(settings.clone(), dataset_uuid, sentence).await
        }
        "book_sentences_save" => {
            let sentences: Vec<BookSentence> =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            datasets::book::book_save_sentences(settings.clone(), dataset_uuid, sentences).await
        }
        "book_sentence_delete" => {
            let uuid = extract_str(&ch.payload, "uuid")?;
            datasets::book::book_delete_sentence(settings.clone(), dataset_uuid, uuid).await
        }
        "book_word_save" => {
            let word: BookSentenceWord =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            datasets::book::book_save_word(settings.clone(), dataset_uuid, word).await
        }
        "book_word_delete" => {
            let uuid = extract_str(&ch.payload, "uuid")?;
            datasets::book::book_delete_word(settings.clone(), dataset_uuid, uuid).await
        }
        "read_text_save" => {
            let text: ReadText =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            datasets::read_aloud::save_text_row(settings.inner(), &dataset_uuid, &text)
        }
        "read_text_delete" => {
            let uuid = extract_str(&ch.payload, "uuid")?;
            datasets::read_aloud::read_aloud_delete_text(settings.clone(), dataset_uuid, uuid).await
        }
        "read_attempt_save" => {
            let mut attempt: ReadAttempt =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            // Per-user history: attribute to the enforced identity, never to the
            // `user_id` the device declared (same rule as the `dictation` kind).
            attempt.user_id = write_identity.to_string();
            datasets::read_aloud::save_attempt_row(settings.inner(), &dataset_uuid, &attempt)
        }
        "read_attempt_delete" => {
            let uuid = extract_str(&ch.payload, "uuid")?;
            datasets::read_aloud::read_aloud_delete_attempt(settings.clone(), dataset_uuid, uuid)
                .await
        }
        other => Err(format!("unknown change kind: {}", other)),
    }
}

/// Read a required string field out of a change payload.
fn extract_str(payload: &serde_json::Value, key: &str) -> Result<String, String> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("payload missing '{key}'"))
}

fn extract_cue_uuid(payload: &serde_json::Value) -> Result<String, String> {
    if let Some(s) = payload.as_str() {
        return Ok(s.to_string());
    }
    payload
        .get("cue_uuid")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| payload.get("uuid").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .ok_or_else(|| "cue_delete payload missing cue uuid".to_string())
}

// ---------------------------------------------------------------------------
// GET /wiki/*  (read-only wiki browsing for the paired Android client)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct WikiPathQuery {
    path: String,
}

#[derive(Deserialize)]
struct WikiSearchQuery {
    keyword: String,
}

/// Path-containment guard: a paired device must only reach files/dirs that live
/// inside one of the allowed wiki roots (the default wiki directory plus every
/// linked dir recorded in `meta.json`). `require_md` additionally restricts the
/// target to a markdown file, so the file endpoint can't be turned into an
/// arbitrary-file reader. Returns `Err(response)` ready to be sent back.
fn check_wiki_path(settings: &SettingsState, raw: &str, require_md: bool) -> Result<(), Response> {
    let target = match Path::new(raw).canonicalize() {
        Ok(t) => t,
        Err(_) => return Err(json_error(StatusCode::NOT_FOUND, &format!("path not found: {raw}"))),
    };
    let roots = crate::wiki::local_impl::wiki_root_paths(settings);
    if !roots.iter().any(|r| target.starts_with(r)) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "path is outside the allowed wiki roots",
        ));
    }
    if require_md {
        let is_md = target
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("md"))
            .unwrap_or(false);
        if !is_md {
            return Err(json_error(
                StatusCode::BAD_REQUEST,
                "only markdown (.md) files can be read",
            ));
        }
    }
    Ok(())
}

/// `GET /wiki/dirs` — top-level wiki roots (default dir + linked dirs).
async fn wiki_dirs(State(st): State<RestState>) -> Response {
    let settings = st.app.state::<SettingsState>();
    match crate::wiki::local_impl::wiki_list_dirs_local(&settings) {
        Ok(entries) => (StatusCode::OK, Json(entries)).into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

/// `GET /wiki/dir?path=` — contents of one wiki directory.
async fn wiki_dir_list(State(st): State<RestState>, Query(q): Query<WikiPathQuery>) -> Response {
    let settings = st.app.state::<SettingsState>();
    if let Err(resp) = check_wiki_path(&settings, &q.path, false) {
        return resp;
    }
    match crate::wiki::local_impl::wiki_list_dir_local(&q.path) {
        Ok(entries) => (StatusCode::OK, Json(entries)).into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

/// `GET /wiki/file?path=` — markdown content of one file, wrapped as `{ content }`.
async fn wiki_file(State(st): State<RestState>, Query(q): Query<WikiPathQuery>) -> Response {
    let settings = st.app.state::<SettingsState>();
    if let Err(resp) = check_wiki_path(&settings, &q.path, true) {
        return resp;
    }
    match crate::wiki::local_impl::wiki_read_file_local(&q.path) {
        Ok(content) => (
            StatusCode::OK,
            Json(serde_json::json!({ "content": content })),
        )
            .into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

/// `GET /wiki/search?keyword=` — full-text search across the PC's wiki index.
async fn wiki_search(State(st): State<RestState>, Query(q): Query<WikiSearchQuery>) -> Response {
    let settings = st.app.state::<SettingsState>();
    match crate::wiki::local_impl::wiki_search_local(&settings, &q.keyword) {
        Ok(results) => (StatusCode::OK, Json(results)).into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

// ---------------------------------------------------------------------------
// /chat/*  (cross-device chat thread, served from the PC-side store)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ChatListQuery {
    /// `created_at` cursor; empty returns the newest page.
    #[serde(default)]
    after: String,
    /// Monotonic rowid cursor (§4.3). When present it wins over `after`: the
    /// integer id never ties, so incremental polls cannot drop same-millisecond
    /// sends. A hub older than protocol v2 simply never sets it.
    #[serde(default)]
    after_id: Option<i64>,
    #[serde(default)]
    limit: Option<i64>,
}

/// `GET /chat/messages?after=&after_id=&limit=` — one page of the shared thread
/// plus the caller's own device id, so a phone knows which bubbles are "mine".
async fn chat_messages(
    State(st): State<RestState>,
    Extension(auth): Extension<sync::pairing::AuthContext>,
    Query(q): Query<ChatListQuery>,
) -> Response {
    let settings = st.app.state::<SettingsState>();
    let limit = q.limit.unwrap_or(200);
    let listed = match q.after_id {
        Some(id) => ai::chat::list_messages_after_id(&settings, id, limit),
        None => ai::chat::list_messages(&settings, &q.after, limit),
    };
    match listed {
        Ok(messages) => {
            let self_device = auth.device_id.unwrap_or_else(|| "pc".to_string());
            (
                StatusCode::OK,
                Json(ai::chat::ChatListResponse { messages, self_device }),
            )
                .into_response()
        }
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

/// `POST /chat/message` — multipart: `uuid` / `text` / `device_name` fields
/// plus any number of `file` parts. Idempotent on `uuid`, so a phone retrying
/// a send whose response was lost cannot duplicate the message.
async fn chat_message(
    State(st): State<RestState>,
    Extension(auth): Extension<sync::pairing::AuthContext>,
    mut multipart: Multipart,
) -> Response {
    let settings = st.app.state::<SettingsState>();
    let mut uuid = String::new();
    let mut text = String::new();
    let mut claimed_name = String::new();
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    // Uploads are spooled to a temp dir, then copied into the chat store; the
    // temp dir is removed whichever way the request ends.
    let tmp_dir = std::env::temp_dir().join(format!("fms-chat-upload-{}", Uuid::new_v4()));

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&tmp_dir);
                return json_error(StatusCode::BAD_REQUEST, &format!("malformed multipart body: {e}"));
            }
        };
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            let orig = field.file_name().unwrap_or("file").to_string();
            let bytes = match field.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&tmp_dir);
                    return json_error(StatusCode::BAD_REQUEST, &format!("cannot read upload: {e}"));
                }
            };
            if std::fs::create_dir_all(&tmp_dir).is_err() {
                let _ = std::fs::remove_dir_all(&tmp_dir);
                return json_error(StatusCode::INTERNAL_SERVER_ERROR, "cannot prepare upload dir");
            }
            let path = tmp_dir.join(format!("{}.bin", files.len()));
            if let Err(e) = std::fs::write(&path, &bytes) {
                let _ = std::fs::remove_dir_all(&tmp_dir);
                return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
            }
            files.push((path, orig));
        } else {
            let value = match field.text().await {
                Ok(v) => v,
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&tmp_dir);
                    return json_error(StatusCode::BAD_REQUEST, &format!("cannot read field: {e}"));
                }
            };
            match name.as_str() {
                "uuid" => uuid = value,
                "text" => text = value,
                "device_name" => claimed_name = value,
                _ => {}
            }
        }
    }

    let response = chat_message_inner(&settings, &auth, uuid, text, claimed_name, &files)
        .map(|msg| {
            let _ = st.app.emit("chat-message", &msg);
            (StatusCode::OK, Json(msg)).into_response()
        })
        .unwrap_or_else(|e| json_error(StatusCode::BAD_REQUEST, &e));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    response
}

fn chat_message_inner(
    settings: &SettingsState,
    auth: &sync::pairing::AuthContext,
    uuid: String,
    text: String,
    claimed_name: String,
    files: &[(PathBuf, String)],
) -> Result<ai::chat::ChatMessage, String> {
    // Stored verbatim as the primary key and echoed back, so shape-check it.
    if !ai::chat::valid_message_uuid(&uuid) {
        return Err("missing or invalid 'uuid' field".to_string());
    }
    if text.trim().is_empty() && files.is_empty() {
        return Err("message is empty — send text or at least one file".to_string());
    }
    // A paired device is attributed by its registry entry; the name it claims
    // is only a fallback (legacy clients / trusted-zone callers).
    let sender_device = auth.device_id.clone().unwrap_or_else(|| "pc".to_string());
    let sender_name = match auth.device_id.as_deref() {
        Some(id) => sync::pairing::list_devices(settings)
            .ok()
            .and_then(|ds| ds.into_iter().find(|d| d.device_id == id).map(|d| d.name))
            .or_else(|| {
                if claimed_name.is_empty() { None } else { Some(claimed_name.clone()) }
            })
            .unwrap_or_else(|| "Device".to_string()),
        None => ai::chat::pc_sender_name(),
    };
    ai::chat::insert_message(settings, &uuid, &sender_device, &sender_name, &text, files)
}

/// `GET /chat/attachment/{uuid}` — stream one stored attachment back to the
/// caller. Resolved through the DB row only, so the path can't be steered at
/// arbitrary files on the PC.
async fn chat_attachment(
    State(st): State<RestState>,
    axum::extract::Path(uuid): axum::extract::Path<String>,
) -> Response {
    let settings = st.app.state::<SettingsState>();
    if !ai::chat::valid_message_uuid(&uuid) {
        return json_error(StatusCode::BAD_REQUEST, "invalid attachment id");
    }
    let (path, att) = match ai::chat::attachment_file(&settings, &uuid) {
        Ok(v) => v,
        Err(e) => return json_error(StatusCode::NOT_FOUND, &e),
    };
    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let body = axum::body::Body::from_stream(ReaderStream::new(file));
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&att.mime).unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    let disposition = format!("inline; filename=\"{}\"", ai::chat::sanitize_filename(&att.filename));
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition).unwrap_or_else(|_| HeaderValue::from_static("inline")),
    );
    (StatusCode::OK, headers, body).into_response()
}

// ---------------------------------------------------------------------------
// Small response helper
// ---------------------------------------------------------------------------

fn json_error(status: StatusCode, msg: &str) -> Response {
    (status, Json(serde_json::json!({ "error": msg }))).into_response()
}
