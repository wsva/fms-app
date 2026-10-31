//! PC-side REST API (`/api/v1`) that turns the desktop `web_service` into a
//! dataset snapshot server + batched-writeback receiver for the Android thin
//! client.
//!
//! Compiled only under the `desktop` feature (it is part of the server stack).
//! The endpoints are intentionally tiny and mirror what `sync.rs` (phone side)
//! expects:
//!
//! * `GET  /api/v1/status`                     - handshake / TCP-probe target
//! * `GET  /api/v1/cluster`                    - capability advertisement (features[], authority_kind, epoch)
//! * `POST /api/v1/pair/request`               - device pairing handshake (confirm dialog)
//! * `GET  /api/v1/pair/status`                - poll a pending pairing request
//! * `GET  /api/v1/datasets/{uuid}/manifest`   - snapshot metadata + overall hash
//! * `GET  /api/v1/datasets/{uuid}/changes`    - incremental row-log page for one dataset
//! * `GET  /api/v1/app/changes`                - same page for the per-user app-data scope
//! * `GET  /api/v1/app/state`                  - one identity's full app-data history (backfill)
//! * `GET  /api/v1/datasets/{uuid}/snapshot`   - the whole dataset dir as tar.gz
//! * `POST /api/v1/datasets/{uuid}/ingest`     - declare a follower→hub push (diff plan)
//! * `PUT  /api/v1/datasets/{uuid}/ingest/file` - stream one pushed file's bytes
//! * `PUT  /api/v1/datasets/{uuid}/ingest/db`  - stream the pushed frozen dataset DB
//! * `POST /api/v1/datasets/{uuid}/ingest/commit` - install the staged tree (trash + rename)
//! * `GET  /api/v1/datasets/{uuid}/db`         - a follower's read of a pushed DB
//! * `POST /api/v1/sync/changes`               - replay queued writeback changes
//! * `GET  /api/v1/chat/messages`              - cross-device chat thread page
//! * `POST /api/v1/chat/message`               - post a chat message (multipart: uuid/text/device_name/file parts)
//! * `GET  /api/v1/chat/attachment/{uuid}`     - stream one stored attachment
//! * `GET  /api/v1/wiki/datasets`              - hub wiki datasets (read-only browse)
//! * `GET  /api/v1/wiki/dataset/dir`           - one directory of a hub wiki dataset
//! * `GET  /api/v1/wiki/dataset/file`          - one classified wiki file of a hub dataset
//! * `GET  /api/v1/wiki/dataset/search`        - full-text search inside one hub wiki dataset
//!
//! Auth is owned by the outer trust-zone layer in `web_service.rs`: loopback +
//! Tailscale pass unauthenticated; other networks need a valid per-device
//! Ed25519 signature (see `pairing.rs`) except `/status`, `/cluster`, and
//! `/pair/*` here.
//! The legacy shared `api_token` is no longer enforced.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use axum::extract::{DefaultBodyLimit, Multipart, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
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
use crate::datasets::read_aloud::ReadText;
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
        .route("/cluster", get(cluster))
        .route("/pair/request", post(pair_request))
        .route("/pair/status", get(pair_status))
        .route("/datasets", get(datasets_list))
        .route("/datasets/{uuid}/manifest", get(manifest))
        .route("/datasets/{uuid}/changes", get(changes))
        .route("/app/changes", get(app_changes))
        .route("/app/state", get(app_state))
        .route("/datasets/{uuid}/snapshot", get(snapshot))
        .route("/datasets/{uuid}/db", get(dataset_db))
        .route("/datasets/{uuid}/ingest", post(ingest_declare))
        // Streamed push bodies: the digest rides in the signed query and the
        // client flags the body "not signed" (`x-fms-stream`), so the zone
        // guard never buffers it — but axum's own default body limit still
        // would, hence `disable` on exactly these two routes.
        .route(
            "/datasets/{uuid}/ingest/file",
            put(ingest_file).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/datasets/{uuid}/ingest/db",
            put(ingest_db).layer(DefaultBodyLimit::disable()),
        )
        .route("/datasets/{uuid}/ingest/commit", post(ingest_commit))
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
        .route("/wiki/datasets", get(wiki_datasets_hub))
        .route("/wiki/dataset/dir", get(wiki_dataset_dir_hub))
        .route("/wiki/dataset/file", get(wiki_dataset_file_hub))
        .route("/wiki/dataset/search", get(wiki_dataset_search_hub))
        .with_state(RestState { app })
}

// ---------------------------------------------------------------------------
// GET /status
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct StatusResp {
    version: &'static str,
    app_name: &'static str,
    /// Every dataset of every type, i.e. the same set `dataset_list` returns.
    /// This used to be `datasets::list_datasets`, which is the dictation listing
    /// only, so `/status` reported 19 while `dataset_list` reported 66.
    dataset_count: usize,
    /// The same total, split by `DatasetType`, so a single number is never the
    /// only answer to "what is here".
    dataset_counts: std::collections::BTreeMap<&'static str, usize>,
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
    /// Whether `/mcp` (and the `/agent.md` manual that describes it) is served.
    /// The user can switch it off on the Agent page (Tools → Agent → Surface);
    /// this field is how a peer distinguishes "404 because the surface is off"
    /// from "404 because the address is wrong".
    mcp_enabled: bool,
}

async fn status(State(st): State<RestState>) -> Response {
    let settings = st.app.state::<SettingsState>();
    // Count by scanning every type instead of asking the dictation listing: the
    // two disagreed because `list_datasets` only walks the dictation roots.
    let mut dataset_counts = std::collections::BTreeMap::new();
    for ty in datasets::DatasetType::ALL {
        dataset_counts.insert(ty.as_str(), datasets::scan_datasets(&settings, ty).len());
    }
    let count: usize = dataset_counts.values().sum();
    (
        StatusCode::OK,
        Json(StatusResp {
            version: env!("CARGO_PKG_VERSION"),
            app_name: "FmS",
            dataset_count: count,
            dataset_counts,
            ok: true,
            protocol_version: crate::sync::PROTOCOL_VERSION,
            role: settings.role(),
            cluster_id: settings.cluster_id(),
            machine: crate::sync::machine_name(),
            mcp_enabled: settings
                .settings
                .lock()
                .map(|s| s.mcp_enabled)
                .unwrap_or(true),
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /cluster — capability advertisement (sync-v2.md §7)
// ---------------------------------------------------------------------------

/// "Who am I talking to, and what can it do for me." A pre-pairing probe like
/// `/status`: it carries the authority kind, cluster id, epoch, protocol
/// version, and the `features[]` list the client negotiates against before it
/// writes (INV-12). A host that does not serve this route advertises nothing and
/// the client default-denies — which is exactly today's website behavior, so
/// adding the route changes nothing for a follower until it starts consulting it.
async fn cluster(State(st): State<RestState>) -> Response {
    let settings = st.app.state::<SettingsState>();
    (
        StatusCode::OK,
        Json(sync::ClusterInfo {
            // A desktop web service is always the LAN/tailnet authority kind,
            // matching `client::upstream_of`'s provenance vocabulary.
            authority_kind: "desktop_hub".to_string(),
            cluster_id: settings.cluster_id(),
            // Placeholder until handover (phase 7b) defines the bump rule.
            epoch: 0,
            protocol_version: sync::PROTOCOL_VERSION,
            features: sync::HUB_FEATURES.iter().map(|s| s.to_string()).collect(),
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
    /// One of `dictation` | `card` | `book` | `read_aloud` | `wiki` — lets the follower
    /// group the list and know which local root to unpack a snapshot into.
    dataset_type: String,
    /// Dictation: media file count. Card: card count. Read-aloud: text count.
    /// Wiki: markdown file count. Book: 0 (not tracked). Zeroed under `?lite=1` (the incremental sync round
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
            updated: d.info.updated_at,
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
            updated: d.info.updated_at,
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
            name: b.name,
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

    // Wiki datasets (syncable markdown trees; content is the .md files).
    for d in crate::datasets::wiki::list_datasets(&settings) {
        items.push(DatasetListItem {
            uuid: d.uuid,
            name: d.name,
            updated: d.updated,
            dataset_type: "wiki".into(),
            media_count: if lite { 0 } else { d.file_count },
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
    /// Dataset type (`dictation` | `card` | `book` | `read_aloud` | `wiki`) so the
    /// follower unpacks the snapshot into the matching local root.
    #[serde(default)]
    dataset_type: String,
    /// Per non-DB file content hash (`{ rel_path: sha256 }`), for incremental
    /// `/file` fetches. Excludes the dataset DB, which the row log owns (§3.2),
    /// so a single row edit does not force a whole-DB file re-download.
    #[serde(default)]
    files: std::collections::BTreeMap<String, String>,
    /// How many times this dataset's whole DB has been replaced by an ingest
    /// commit. `overall_hash` cannot report a DB-only change (the DB is excluded
    /// from it), so a follower compares this number to decide whether the cues it
    /// holds are stale (`docs/design/dataset-push.md`). A pre-ingest hub is 0.
    #[serde(default)]
    content_gen: i64,
}

/// The dataset SQLite file + its WAL/SHM sidecars are excluded from the hashed
/// file set (§3.2): they are content the row log and snapshot own, not media.
/// The derived per-copy indexes join them via [`crate::datasets::is_derived_index`]
/// — that predicate is the single place naming them, because a hub must refuse
/// to hash, ship or serve one, whichever module owns it.
fn is_db_file(rel: &str) -> bool {
    rel == "data.sqlite3" || rel == "data.sqlite3-wal" || rel == "data.sqlite3-shm"
        || crate::datasets::is_derived_index(rel)
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
/// The recipe lives in `blobpush::cached_hash` because the push driver needs the
/// byte-identical shortcut for a follower's own tree.
fn cached_file_hash(
    cache: Option<&rusqlite::Connection>,
    dataset_uuid: &str,
    rel: &str,
    path: &Path,
    size: u64,
    mtime: u64,
) -> String {
    crate::sync::blobpush::cached_hash(cache, dataset_uuid, rel, path, size, mtime)
}

/// Compute the manifest over the **non-DB** file set: file count, total bytes,
/// a per-file `sha256` map, and an `overall_hash` folded from those hashes so
/// it is a fast "nothing changed" short-circuit. `updated_at` and `dataset_type`
/// come from the shared `info.json` descriptor when one parses (a raw-import
/// folder or a pre-migration file leaves both empty).
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

    let (updated_at, dataset_type) = match datasets::read_info(dir).ok() {
        Some(info) => (info.updated_at, info.dataset_type.as_str().to_string()),
        None => (String::new(), String::new()),
    };

    Manifest {
        file_count: files.len(),
        total_bytes,
        overall_hash,
        updated_at,
        dataset_type,
        files: map,
        content_gen: 0,
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

/// Publish hygiene (§17.6 item 1): drop the legacy per-dataset practice tables
/// from a frozen snapshot copy so a borrower's `migrate_legacy` /
/// `migrate_legacy_attempts` seeds nothing — the publisher's pre-v2 schedule and
/// scores never ride the content channel to a subscriber. The receiver's
/// `open_card_db` / `open_read_aloud_db` recreate both empty on first open
/// (`CREATE TABLE IF NOT EXISTS`), so this cannot break the installing side. The
/// manifest never hashes `data.sqlite3` (`compute_manifest` filters `is_db_file`),
/// so the served `x-snapshot-hash` is unaffected. Any future archive that ships a
/// dataset DB off-device must call this too.
fn sanitize_snapshot_db(path: &Path) {
    if let Ok(conn) = rusqlite::Connection::open(path) {
        // Idempotent; a dataset carries at most one of these (card_review for card
        // datasets, read_attempt for read-aloud). Each table's indexes drop with it.
        let _ = conn.execute_batch(
            "DROP TABLE IF EXISTS card_review;\
             DROP TABLE IF EXISTS read_attempt;",
        );
    }
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
    m.content_gen = cache
        .as_ref()
        .map(|c| sync::content_gen(c, &uuid))
        .unwrap_or(0);
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

    // A read-aloud clip may be only a *span* of a recording until something needs
    // the audio, and this is the moment: the recordings stay where the user keeps
    // them and `align/` — the index that could resolve one — is a derived index
    // that never ships, so a follower could not cut the clip either. Cutting here
    // (before the manifest is computed, so the hash and the bytes agree) costs
    // one decode pass per recording, once; it is deliberately *not* in
    // `/manifest`, which is the cheap "is there anything new?" poll. Blocking
    // work, so off the async runtime — and never on a read-only base.
    if !datasets::is_under_download_root(&settings, &dir) {
        let d = dir.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            let is_read_aloud =
                datasets::read_info_opt(&d).is_some_and(|i| i.dataset_type == datasets::DatasetType::Read);
            if !is_read_aloud {
                return;
            }
            match datasets::read_aloud::standard_audio::materialize_all(&d) {
                Ok(0) => {}
                Ok(n) => log::info!(
                    "[sync] {}: cut {n} lazy standard-audio clip(s) for the snapshot",
                    d.display()
                ),
                Err(e) => log::warn!(
                    "[sync] {}: standard-audio materialise failed, snapshotting as-is: {e}",
                    d.display()
                ),
            }
        })
        .await;
    }

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
    if let Some(sp) = &snap_db {
        sanitize_snapshot_db(sp);
    }
    let cache = crate::datasets::dictation::open_app_db(&settings).ok();
    let m = compute_manifest(&dir, &uuid, cache.as_ref());

    // Archive the frozen copy under the name `data.sqlite3` and never the live
    // sidecars (the vacuum image is self-contained, WAL off on the receiver).
    // Derived indexes never ship at all — the receiver rebuilds its own.
    let files: Vec<(String, PathBuf)> = collect_rel_files(&dir)
        .into_iter()
        .filter(|(rel, _)| {
            rel != "data.sqlite3-wal"
                && rel != "data.sqlite3-shm"
                && !crate::datasets::is_derived_index(rel)
        })
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
    if settings.is_hub() {
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
    if settings.is_hub() {
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
    /// Rowid cursors for the per-user practice-event tables (sync-v2.md §17.6 /
    /// INV-21), same meaning as above. Only a v6 hub serves them; an older client
    /// omits them and gets none. The server always answers (cheap, additive) — the
    /// *client* decides whether to request/parse them from the advertised protocol.
    after_reviews: Option<i64>,
    after_attempts: Option<i64>,
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
    /// This identity's card-review events (`card_review_event`), oldest-first,
    /// in the `review::payload` shape `apply_remote` accepts (protocol v6+).
    reviews: Vec<serde_json::Value>,
    /// This identity's read-aloud attempt events (`read_attempt_event`),
    /// oldest-first, in the `attempt_payload` shape `apply_remote_save` accepts
    /// (protocol v6+). `audio_path` is device-local — the row syncs, the WAV does not.
    attempts: Vec<serde_json::Value>,
    /// Next rowid cursors, `Some` only when that page was full. `None` for both
    /// means the follower has the whole history.
    next_dictation_after: Option<i64>,
    next_xp_after: Option<i64>,
    next_reviews_after: Option<i64>,
    next_attempts_after: Option<i64>,
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
    // Per-user practice events (protocol v6): the two append-only planes, paged the
    // same way. Always served — an older client simply ignores the extra fields.
    let (reviews, next_reviews_after) = match datasets::cards::review::rows_for_user(
        &settings,
        &user_key,
        q.after_reviews.unwrap_or(0),
        HISTORY_ROW_LIMIT,
    ) {
        Ok(page) => page,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };
    let (attempts, next_attempts_after) = match datasets::read_aloud::rows_for_user(
        &settings,
        &user_key,
        q.after_attempts.unwrap_or(0),
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
        reviews,
        attempts,
        next_dictation_after,
        next_xp_after,
        next_reviews_after,
        next_attempts_after,
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
// Follower → hub dataset push (`docs/design/dataset-push.md`)
//
// Four small routes rather than one archive upload: the file plane already knows
// every file's sha256, so a push whose media is already on the hub uploads
// nothing but the changed bytes. The dataset DB travels on its own route because
// it is deliberately outside that plane (`is_db_file`) — and in this one
// direction it *is* the authored content (hand-cut cue times).
//
// The result is staged as a whole tree and installed by rename, so a failed push
// can never leave the hub holding half a dataset.
// ---------------------------------------------------------------------------

/// The staging parent, created beside the dataset directories: inside no dataset
/// (so it is never hashed, served or listed) and holding no `info.json` (so
/// discovery never mistakes it for one).
const INGEST_STAGE_DIR: &str = ".fms-incoming";
/// An abandoned run directory older than this is pruned on the next declare.
const INGEST_RUN_TTL_SECS: u64 = 24 * 60 * 60;

#[derive(Serialize, Deserialize, Clone)]
struct IngestRef {
    path: String,
    sha256: String,
    #[serde(default)]
    size: u64,
}

#[derive(Deserialize)]
struct IngestDeclareReq {
    run: String,
    /// The dataset's type. Required only when the hub does not have the dataset
    /// yet, i.e. when a follower contributes one it created itself.
    #[serde(default)]
    dataset_type: String,
    #[serde(default)]
    files: Vec<IngestRef>,
    /// Paths the pushing tree no longer has; they are simply absent from the
    /// staged tree, so the install drops them with the old directory.
    #[serde(default)]
    deletes: Vec<String>,
    /// The frozen `data.sqlite3` the client wants the hub to adopt.
    #[serde(default)]
    db: Option<IngestRef>,
}

/// What a staging run recorded at declare; `commit` re-checks every claim in it.
#[derive(Serialize, Deserialize)]
struct IngestRun {
    uuid: String,
    run: String,
    dataset_type: String,
    /// The hub revision the client observed, echoed back at commit so a racing
    /// second push is refused rather than silently winning.
    content_gen: i64,
    created_at: i64,
    files: Vec<IngestRef>,
    deletes: Vec<String>,
    db: Option<IngestRef>,
}

/// Runs currently committing, so two pushes to one dataset cannot interleave
/// their renames. `std::sync` because the guard is held only across the blocking
/// install, never across an await.
fn ingest_commits() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static RUNNING: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    RUNNING.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

struct CommitGuard(String);

impl Drop for CommitGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = ingest_commits().lock() {
            set.remove(&self.0);
        }
    }
}

fn begin_commit(uuid: &str) -> Result<CommitGuard, String> {
    let mut set = ingest_commits()
        .lock()
        .map_err(|_| "ingest lock is poisoned; restart the hub".to_string())?;
    if !set.insert(uuid.to_string()) {
        return Err("another push to this dataset is being installed; retry in a moment".into());
    }
    Ok(CommitGuard(uuid.to_string()))
}

/// A run id becomes a path component, so keep it boring.
fn valid_run(run: &str) -> bool {
    !run.is_empty() && run.len() <= 40 && run.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Normalize and vet a pushed relative path. Refuses anything that could escape
/// the dataset directory, and the files that never ride this plane: the DB (its
/// own route), its sidecars and every derived index.
fn ingest_rel_path(raw: &str) -> Result<String, String> {
    if raw.trim().is_empty() {
        return Err("empty file path".into());
    }
    if raw.contains('\\') {
        return Err(format!("'{raw}': use forward slashes"));
    }
    let rel = raw.trim_start_matches('/');
    for seg in rel.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(format!("'{raw}': not a relative path"));
        }
        if seg.contains(':') {
            return Err(format!("'{raw}': ':' is not valid in a portable file name"));
        }
    }
    if is_db_file(rel) {
        return Err(format!(
            "'{raw}': the dataset DB and derived indexes never ride a push"
        ));
    }
    Ok(rel.to_string())
}

/// `rel/a/b` under `root`, component-wise so Windows gets its own separators and
/// no single string ever contains a traversal.
fn rel_to_path(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for seg in rel.split('/') {
        p.push(seg);
    }
    p
}

/// Resolve the directory a push targets: the hub's resolved writable copy when
/// it has one, else the default type root (`<datasets>/<type>/<uuid>`), which
/// commit creates — that is how a follower contributes a dataset it authored.
fn ingest_target(
    settings: &SettingsState,
    uuid: &str,
    type_hint: &str,
) -> Result<(PathBuf, datasets::DatasetType, bool), String> {
    if let Ok((dir, ty)) = datasets::find_dataset_dir_typed(settings, uuid) {
        return Ok((dir, ty, true));
    }
    if type_hint.is_empty() {
        return Err(format!(
            "dataset {uuid} is not on this hub; pass dataset_type to contribute it"
        ));
    }
    let ty: datasets::DatasetType = type_hint
        .parse()
        .map_err(|_| format!("unknown dataset type '{type_hint}'"))?;
    datasets::assert_uuid_free(settings, uuid)?;
    Ok((
        datasets::datasets_dir(settings).join(ty.as_str()).join(uuid),
        ty,
        false,
    ))
}

fn ingest_stage(parent: &Path, uuid: &str, run: &str) -> PathBuf {
    parent
        .join(INGEST_STAGE_DIR)
        .join(format!("{uuid}__{run}"))
}

/// Drop abandoned runs so a cancelled push does not hold disk forever.
fn prune_stale_runs(parent: &Path) {
    let Ok(entries) = std::fs::read_dir(parent.join(INGEST_STAGE_DIR)) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for e in entries.flatten() {
        let stale = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .map(|d| d.as_secs() > INGEST_RUN_TTL_SECS)
            .unwrap_or(false);
        if stale {
            log::info!("[ingest] pruning abandoned run {}", e.path().display());
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Hard-link (copy as fallback) every hub file the push keeps into the staged
/// tree. Untouched media therefore costs a directory entry rather than a byte
/// copy, which is what lets the whole install be two renames on one volume.
fn link_kept_files(
    hub_dir: &Path,
    tree: &Path,
    replaced: &std::collections::HashSet<String>,
) -> Result<usize, String> {
    let mut n = 0usize;
    for (rel, abs) in collect_rel_files(hub_dir) {
        if replaced.contains(&rel) || is_db_file(&rel) {
            continue;
        }
        let dst = rel_to_path(tree, &rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir '{rel}': {e}"))?;
        }
        if std::fs::hard_link(&abs, &dst).is_err() {
            std::fs::copy(&abs, &dst).map_err(|e| format!("copy '{rel}': {e}"))?;
        }
        n += 1;
    }
    Ok(n)
}

/// The declare half: build the staged merge base and report what the client
/// still owes. Blocking (walks the dataset, hard-links every kept file), so it
/// runs on the blocking pool.
#[allow(clippy::too_many_arguments)]
fn ingest_stage_plan(
    dir: &Path,
    uuid: &str,
    ty: &datasets::DatasetType,
    exists: bool,
    run: &str,
    files: Vec<IngestRef>,
    deletes: Vec<String>,
    db: Option<IngestRef>,
    cache: Option<&rusqlite::Connection>,
) -> Result<serde_json::Value, String> {
    let parent = dir
        .parent()
        .ok_or_else(|| format!("dataset path '{}' has no parent", dir.display()))?;
    prune_stale_runs(parent);

    let stage = ingest_stage(parent, uuid, run);
    let tree = stage.join("tree");
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&tree).map_err(|e| format!("create staging: {e}"))?;

    let hub = if exists {
        compute_manifest(dir, uuid, cache)
    } else {
        // A dataset the hub does not have yet: nothing to diff against, nothing
        // to keep.
        compute_manifest(&tree, uuid, cache)
    };
    let hub_files = if exists {
        hub.files.clone()
    } else {
        std::collections::BTreeMap::new()
    };
    let gen = cache.map(|c| sync::content_gen(c, uuid)).unwrap_or(0);

    let mut need: Vec<String> = Vec::new();
    let (mut creates, mut updates) = (0usize, 0usize);
    let mut upload_bytes = 0u64;
    for f in &files {
        match hub_files.get(&f.path) {
            None => {
                creates += 1;
                need.push(f.path.clone());
                upload_bytes += f.size;
            }
            Some(sha) if sha == &f.sha256 => {} // already there: upload nothing
            Some(_) => {
                updates += 1;
                need.push(f.path.clone());
                upload_bytes += f.size;
            }
        }
    }

    // Carry forward every hub file this push neither rewrites nor drops — by
    // hard link, so untouched media costs a directory entry, not a copy. The
    // exclusion set is `need ∪ deletes`, *not* the whole declared tree: a file
    // whose digest already matches is not uploaded, so skipping it here would
    // make it vanish from the installed tree.
    let mut replaced: std::collections::HashSet<String> = need.iter().cloned().collect();
    replaced.extend(deletes.iter().cloned());
    let linked = if exists {
        link_kept_files(dir, &tree, &replaced)?
    } else {
        0
    };

    if let Some(d) = &db {
        upload_bytes += d.size;
    }
    let deletes_present = deletes.iter().filter(|d| hub_files.contains_key(*d)).count();

    // A push that carries no DB must not silently drop the hub's: hard-link it
    // forward like any other untouched file.
    if db.is_none() && exists {
        let src = dir.join("data.sqlite3");
        if src.is_file() {
            let dst = tree.join("data.sqlite3");
            if std::fs::hard_link(&src, &dst).is_err() {
                let _ = std::fs::copy(&src, &dst);
            }
        }
    }

    let record = IngestRun {
        uuid: uuid.to_string(),
        run: run.to_string(),
        dataset_type: ty.as_str().to_string(),
        content_gen: gen,
        created_at: chrono::Utc::now().timestamp(),
        files: files.clone(),
        deletes: deletes.clone(),
        db: db.clone(),
    };
    let body = serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?;
    std::fs::write(stage.join("run.json"), body).map_err(|e| format!("write run: {e}"))?;

    Ok(serde_json::json!({
        "run": run,
        "need": need,
        "need_db": db.is_some(),
        "creates": creates,
        "updates": updates,
        "deletes": deletes_present,
        "unchanged": hub_files.len().saturating_sub(updates + deletes_present),
        "linked": linked,
        "upload_bytes": upload_bytes,
        "content_gen": gen,
        "hub_file_count": hub_files.len(),
        "staging": format!("{INGEST_STAGE_DIR}/{uuid}__{run}"),
    }))
}

async fn ingest_declare(
    State(st): State<RestState>,
    Extension(auth): Extension<sync::pairing::AuthContext>,
    axum::extract::Path(uuid): axum::extract::Path<String>,
    Json(req): Json<IngestDeclareReq>,
) -> Response {
    let settings = st.app.state::<SettingsState>();
    if !settings.is_hub() {
        return json_error(
            StatusCode::CONFLICT,
            "this machine is a follower; push to the hub, not to a follower",
        );
    }
    if !valid_run(&req.run) {
        return json_error(StatusCode::BAD_REQUEST, "run must be 1-40 alphanumerics");
    }
    let (dir, ty, exists) = match ingest_target(&settings, &uuid, &req.dataset_type) {
        Ok(v) => v,
        Err(e) => return json_error(StatusCode::NOT_FOUND, &e),
    };
    if datasets::is_under_download_root(&settings, &dir) {
        return json_error(
            StatusCode::CONFLICT,
            "the hub's copy is a read-only download base — fork it into a writable \
             location (Datasets page) before pushing over it",
        );
    }

    // Vet every name before touching the filesystem.
    let mut files = Vec::with_capacity(req.files.len());
    for f in &req.files {
        match ingest_rel_path(&f.path) {
            Ok(path) => files.push(IngestRef {
                path,
                sha256: f.sha256.trim().to_lowercase(),
                size: f.size,
            }),
            Err(e) => return json_error(StatusCode::BAD_REQUEST, &e),
        }
    }
    let mut deletes = Vec::with_capacity(req.deletes.len());
    for d in &req.deletes {
        match ingest_rel_path(d) {
            Ok(path) if path == datasets::INFO_FILE => {
                return json_error(StatusCode::BAD_REQUEST, "info.json is never deleted")
            }
            Ok(path) => deletes.push(path),
            Err(e) => return json_error(StatusCode::BAD_REQUEST, &e),
        }
    }
    let db = match req.db {
        Some(d) => {
            if d.sha256.trim().len() != 64 {
                return json_error(StatusCode::BAD_REQUEST, "db.sha256 must be 64 hex chars");
            }
            Some(IngestRef {
                path: "data.sqlite3".to_string(),
                sha256: d.sha256.trim().to_lowercase(),
                size: d.size,
            })
        }
        None => None,
    };

    log::info!(
        "[ingest] declare uuid={uuid} run={} type={} files={} deletes={} db={} device={:?} exists={exists}",
        req.run,
        ty.as_str(),
        files.len(),
        deletes.len(),
        db.is_some(),
        auth.device_id,
    );
    let cache = crate::datasets::dictation::open_app_db(&settings).ok();
    let run = req.run.clone();
    let plan = tauri::async_runtime::spawn_blocking(move || {
        ingest_stage_plan(&dir, &uuid, &ty, exists, &run, files, deletes, db, cache.as_ref())
    })
    .await;
    match plan {
        Ok(Ok(v)) => (StatusCode::OK, Json(v)).into_response(),
        Ok(Err(e)) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &format!("staging failed: {e}")),
    }
}

#[derive(Deserialize)]
struct IngestPutQuery {
    run: String,
    /// The type hint that locates the staging parent for a dataset the hub does
    /// not have yet (empty is fine once it does).
    #[serde(default)]
    dataset_type: String,
    /// Forward-slash relative path; absent on the `/db` route.
    #[serde(default)]
    path: String,
    /// The digest the bytes must hash to — in the *signed* query, because the
    /// body itself is not part of the signature.
    sha: String,
}

/// Stream a request body into `dest`, hashing as it goes, and refuse a digest
/// mismatch. Never buffers: media can be multi-GB.
async fn ingest_write_body(
    body: axum::body::Body,
    dest: &Path,
    want_sha: &str,
) -> Result<u64, String> {
    use futures_util::StreamExt as _;
    use tokio::io::AsyncWriteExt as _;
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }
    let mut file = tokio::fs::File::create(dest).await.map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut total = 0u64;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("read upload body: {e}"))?;
        h.update(&chunk);
        total += chunk.len() as u64;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
    }
    file.flush().await.map_err(|e| e.to_string())?;
    let got = format!("{:x}", h.finalize());
    if got != want_sha {
        drop(file);
        let _ = tokio::fs::remove_file(dest).await;
        return Err(format!("sha mismatch: expected {want_sha}, received {got}"));
    }
    Ok(total)
}

/// Open the staging run a PUT belongs to, checking it is still the one declared
/// for this dataset and run id.
fn open_stage(settings: &SettingsState, uuid: &str, q: &IngestPutQuery) -> Result<PathBuf, String> {
    if !valid_run(&q.run) {
        return Err("run must be 1-40 alphanumerics".into());
    }
    let (dir, _, _) = ingest_target(settings, uuid, &q.dataset_type)?;
    let parent = dir
        .parent()
        .ok_or_else(|| format!("dataset path '{}' has no parent", dir.display()))?;
    let stage = ingest_stage(parent, uuid, &q.run);
    if !stage.join("run.json").is_file() {
        return Err(format!(
            "no staging run '{}' for this dataset — call ingest again",
            q.run
        ));
    }
    Ok(stage)
}

async fn ingest_file(
    State(st): State<RestState>,
    Extension(_auth): Extension<sync::pairing::AuthContext>,
    axum::extract::Path(uuid): axum::extract::Path<String>,
    Query(q): Query<IngestPutQuery>,
    body: axum::body::Body,
) -> Response {
    let settings = st.app.state::<SettingsState>();
    let rel = match ingest_rel_path(&q.path) {
        Ok(r) => r,
        Err(e) => return json_error(StatusCode::BAD_REQUEST, &e),
    };
    let stage = match open_stage(&settings, &uuid, &q) {
        Ok(s) => s,
        Err(e) => return json_error(StatusCode::CONFLICT, &e),
    };
    let dest = rel_to_path(&stage.join("tree"), &rel);
    match ingest_write_body(body, &dest, q.sha.trim()).await {
        Ok(bytes) => {
            log::info!("[ingest] {uuid} <- {rel} ({bytes} bytes)");
            (StatusCode::OK, Json(serde_json::json!({ "path": rel, "bytes": bytes })))
                .into_response()
        }
        Err(e) => {
            let code = if e.starts_with("sha mismatch") {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            json_error(code, &format!("{rel}: {e}"))
        }
    }
}

async fn ingest_db(
    State(st): State<RestState>,
    Extension(_auth): Extension<sync::pairing::AuthContext>,
    axum::extract::Path(uuid): axum::extract::Path<String>,
    Query(q): Query<IngestPutQuery>,
    body: axum::body::Body,
) -> Response {
    let settings = st.app.state::<SettingsState>();
    let stage = match open_stage(&settings, &uuid, &q) {
        Ok(s) => s,
        Err(e) => return json_error(StatusCode::CONFLICT, &e),
    };
    // Beside the tree, not inside it: commit moves it in once it has checked the
    // staged `info.json`, so an abandoned run leaves no `data.sqlite3` in the
    // tree that the manifest would ignore but a reader could still find.
    let dest = stage.join("db.sqlite3");
    match ingest_write_body(body, &dest, q.sha.trim()).await {
        Ok(bytes) => {
            log::info!("[ingest] {uuid} <- data.sqlite3 ({bytes} bytes)");
            (StatusCode::OK, Json(serde_json::json!({ "bytes": bytes }))).into_response()
        }
        Err(e) => {
            let code = if e.starts_with("sha mismatch") {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            json_error(code, &format!("data.sqlite3: {e}"))
        }
    }
}

#[derive(Deserialize)]
struct IngestCommitReq {
    run: String,
    #[serde(default)]
    dataset_type: String,
    /// The revision the client saw at declare; a mismatch means another push
    /// landed in between, so this one must re-plan rather than overwrite it.
    #[serde(default)]
    content_gen: i64,
}

/// The commit half. Blocking: it hashes every staged file, renames the old
/// directory aside, and installs the staged tree — all on the blocking pool so a
/// multi-GB verification never stalls the async runtime.
fn ingest_install(
    settings: &SettingsState,
    dir: &Path,
    uuid: &str,
    ty: &datasets::DatasetType,
    exists: bool,
    run: &str,
    expect_gen: i64,
    conn: &rusqlite::Connection,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let conflict = |m: String| (StatusCode::CONFLICT, m);
    let _guard = begin_commit(uuid).map_err(conflict)?;

    let parent = dir
        .parent()
        .ok_or_else(|| (StatusCode::INTERNAL_SERVER_ERROR, "dataset path has no parent".to_string()))?;
    let stage = ingest_stage(parent, uuid, run);
    let raw = std::fs::read(stage.join("run.json")).map_err(|_| {
        conflict(format!(
            "no staging run '{run}' for this dataset — it was pruned or never declared; re-run the push"
        ))
    })?;
    let record: IngestRun = serde_json::from_slice(&raw)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("staging record: {e}")))?;
    if record.uuid != uuid || record.run != run {
        return Err(conflict("staging run belongs to another dataset".into()));
    }

    // Two guards, one window: the revision recorded at declare, and the one the
    // client echoed. Either may be stale if a second push committed meanwhile.
    let gen_now = sync::content_gen(conn, uuid);
    if record.content_gen != gen_now || expect_gen != gen_now {
        return Err(conflict(format!(
            "the hub's copy changed during this push (revision {gen_now}, this push expected \
             {} / {expect_gen}); re-run the push",
            record.content_gen
        )));
    }
    // A dataset that did not exist at declare must still not exist now.
    if !exists && datasets::find_dataset_dir_typed(settings, uuid).is_ok() {
        return Err(conflict(
            "the dataset appeared on the hub while this push was running; re-run it".into(),
        ));
    }

    let tree = stage.join("tree");
    let info = datasets::read_info(&tree).map_err(|e| {
        conflict(format!(
            "the pushed copy has no readable info.json ({e}); the dataset dir on the \
             follower needs one before it can be pushed"
        ))
    })?;
    if info.uuid != uuid {
        return Err(conflict(format!(
            "info.json declares uuid '{}' but the push targets '{uuid}'",
            info.uuid
        )));
    }
    if info.dataset_type != *ty {
        return Err(conflict(format!(
            "info.json declares type '{}' but the push targets '{}'",
            info.dataset_type.as_str(),
            ty.as_str()
        )));
    }

    // Re-verify every claim against the staged bytes: the PUTs checked their own
    // digests, this checks the tree that is about to become the dataset.
    for f in &record.files {
        let p = rel_to_path(&tree, &f.path);
        if !p.is_file() {
            return Err(conflict(format!("'{}' was never uploaded", f.path)));
        }
        let sha = hash_file_contents(&p)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("'{}': {e}", f.path)))?;
        if sha != f.sha256 {
            return Err(conflict(format!(
                "'{}' does not match the digest this push declared",
                f.path
            )));
        }
    }
    for d in &record.deletes {
        if rel_to_path(&tree, d).exists() {
            return Err(conflict(format!(
                "'{d}' was declared deleted but is still in the pushed tree"
            )));
        }
    }
    if let Some(db) = &record.db {
        let staged = stage.join("db.sqlite3");
        let sha = hash_file_contents(&staged).map_err(|e| {
            conflict(format!("the dataset DB was never uploaded ({e})"))
        })?;
        if sha != db.sha256 {
            return Err(conflict(
                "the uploaded dataset DB does not match the digest this push declared".into(),
            ));
        }
    }

    // Backup exactly what the push destroys: the files it replaces, the ones it
    // drops, and the old DB. Everything else survives by hard-link into the new
    // tree, so trashing it would duplicate bytes to no purpose.
    let mut trashed: Vec<String> = Vec::new();
    let mut victims: Vec<String> = record.files.iter().map(|f| f.path.clone()).collect();
    victims.extend(record.deletes.iter().cloned());
    victims.sort();
    victims.dedup();
    for rel in &victims {
        let p = rel_to_path(dir, rel);
        if p.is_file() {
            match datasets::media::move_to_trash(&p) {
                Ok(t) => trashed.push(t.display().to_string()),
                Err(e) => log::warn!("[ingest] {uuid}: could not trash '{rel}': {e}"),
            }
        }
    }
    if record.db.is_some() {
        let old_db = dir.join("data.sqlite3");
        if old_db.is_file() {
            match datasets::media::move_to_trash(&old_db) {
                Ok(t) => trashed.push(t.display().to_string()),
                Err(e) => log::warn!("[ingest] {uuid}: could not trash the old DB: {e}"),
            }
        }
    }
    if let Some(db) = &record.db {
        std::fs::rename(stage.join("db.sqlite3"), tree.join("data.sqlite3"))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("install DB: {e}")))?;
        let _ = db;
    }

    // Two renames install the tree. The old directory is moved aside first, so a
    // failure here can be rolled back instead of leaving the hub with nothing.
    let prev = stage.join("prev");
    let moved_aside = if dir.exists() {
        std::fs::rename(dir, &prev)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("move aside: {e}")))?;
        true
    } else {
        std::fs::create_dir_all(parent)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("mkdir: {e}")))?;
        false
    };
    if let Err(e) = std::fs::rename(&tree, dir) {
        if moved_aside {
            let _ = std::fs::rename(&prev, dir);
        }
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("install pushed tree: {e}"),
        ));
    }
    let _ = std::fs::remove_dir_all(&prev);

    let new_gen = sync::set_content_gen(conn, uuid, gen_now + 1)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let m = compute_manifest(dir, uuid, Some(conn));
    let _ = std::fs::remove_dir_all(&stage);
    log::info!(
        "[ingest] {uuid}: installed {} file(s) at revision {new_gen} ({} trashed)",
        m.file_count,
        trashed.len()
    );
    Ok(serde_json::json!({
        "content_gen": new_gen,
        "overall_hash": m.overall_hash,
        "file_count": m.file_count,
        "total_bytes": m.total_bytes,
        "trashed_to": trashed,
        "created": !exists,
        "dataset_type": ty.as_str(),
    }))
}

async fn ingest_commit(
    State(st): State<RestState>,
    Extension(auth): Extension<sync::pairing::AuthContext>,
    axum::extract::Path(uuid): axum::extract::Path<String>,
    Json(req): Json<IngestCommitReq>,
) -> Response {
    let settings = st.app.state::<SettingsState>();
    if !settings.is_hub() {
        return json_error(
            StatusCode::CONFLICT,
            "this machine is a follower; push to the hub, not to a follower",
        );
    }
    let (dir, ty, exists) = match ingest_target(&settings, &uuid, &req.dataset_type) {
        Ok(v) => v,
        Err(e) => return json_error(StatusCode::NOT_FOUND, &e),
    };
    if datasets::is_under_download_root(&settings, &dir) {
        return json_error(
            StatusCode::CONFLICT,
            "the hub's copy is a read-only download base — fork it into a writable \
             location before pushing over it",
        );
    }
    log::info!(
        "[ingest] commit uuid={uuid} run={} device={:?}",
        req.run, auth.device_id
    );
    let conn = match crate::datasets::dictation::open_app_db(&settings) {
        Ok(c) => c,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };
    let run = req.run.clone();
    let gen = req.content_gen;
    // The blocking closure owns the app handle rather than a borrow of the router
    // state: `State` is scoped to this handler frame, the handle is not.
    let app = st.app.clone();
    let out = tauri::async_runtime::spawn_blocking(move || {
        let settings = app.state::<SettingsState>();
        ingest_install(&settings, &dir, &uuid, &ty, exists, &run, gen, &conn)
    })
    .await;
    match out {
        Ok(Ok(v)) => (StatusCode::OK, Json(v)).into_response(),
        Ok(Err((code, msg))) => json_error(code, &msg),
        Err(e) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("commit failed: {e}"),
        ),
    }
}

/// `GET /datasets/{uuid}/db` — a point-in-time copy of the dataset DB plus the
/// revision it belongs to, so a follower can adopt a pushed DB without
/// re-downloading the whole dataset (which is all `overall_hash` could tell it).
async fn dataset_db(
    State(st): State<RestState>,
    axum::extract::Path(uuid): axum::extract::Path<String>,
) -> Response {
    let settings = st.app.state::<SettingsState>();
    let (dir, _ty) = match datasets::find_dataset_dir_typed(&settings, &uuid) {
        Ok(d) => d,
        Err(e) => return json_error(StatusCode::NOT_FOUND, &e),
    };
    let gen = crate::datasets::dictation::open_app_db(&settings)
        .map(|c| sync::content_gen(&c, &uuid))
        .unwrap_or(0);
    // `VACUUM INTO` reads one consistent snapshot, so a concurrent write cannot
    // hand the follower a torn image the way a raw file copy would.
    let Some(tmp) = snapshot_db_copy(&dir) else {
        return json_error(StatusCode::NOT_FOUND, "dataset has no data.sqlite3");
    };
    let body = match std::fs::read(&tmp) {
        Ok(b) => b,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
    };
    let _ = std::fs::remove_file(&tmp);
    let sha = crate::sync::blobpush::sha256_of_bytes(&body);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/vnd.sqlite3".to_string()),
            (header::CONTENT_LENGTH, body.len().to_string()),
            (header::HeaderName::from_static("x-content-sha256"), sha),
            (header::HeaderName::from_static("x-content-gen"), gen.to_string()),
        ],
        body,
    )
        .into_response()
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
pub(crate) struct Change {
    pub(crate) id: String,
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) dataset_uuid: Option<String>,
    pub(crate) payload: serde_json::Value,
    /// Device-local ISO timestamp of the user action (§3.3 / §3.6). Drives
    /// later-`edit_time`-wins conflicts and, once the apply path is built
    /// (Phase 4), the applied row's `updated_at`. Carried here so the hub has
    /// it available for the log entry and comparison.
    #[serde(default)]
    pub(crate) edit_time: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub(crate) queued_at: String,
    /// The authority epoch this device recorded for the change's scope when it
    /// queued the edit (INV-13). The hub refuses the write if it is below the
    /// scope's current epoch — the scope was handed over since this device last
    /// pulled, so a naive apply would resurrect a superseded authority's edit.
    /// Defaults to `0` (a pre-handover client, or the never-handed-over case).
    #[serde(default)]
    pub(crate) epoch: i64,
}

#[derive(Serialize)]
pub(crate) struct ChangeResult {
    pub(crate) id: String,
    pub(crate) ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    /// §3.4 conflict: a losing push is still acknowledged (`ok = true`, so the
    /// follower stops resending it) but flagged `rejected`, carrying the hub's
    /// winning entry. The follower drops its stale queued change; the winner
    /// re-converges it on the next `/changes` pull (no seq is rewritten here).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rejected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) winner_seq: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) winner_payload: Option<serde_json::Value>,
    /// The journal kind of the row that won the LWW contest. Present when a
    /// newer live entry beat this push (INV-4 traceability); absent when the
    /// rejection is a tombstone-only drop, since the winning delete's kind was
    /// not looked up (there is no live row to disambiguate against). Needed by
    /// the inbox producer's rollback path: `object_id_for` maps both
    /// `card_save` and `card_set_tags` to `card_uuid`, so without `winner_kind`
    /// the producer would have to shape-detect the winner payload to decide
    /// whether to re-apply the full card or the whole tag set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) winner_kind: Option<String>,
}

impl ChangeResult {
    pub(crate) fn applied(id: String) -> Self {
        ChangeResult { id, ok: true, error: None, rejected: None, winner_seq: None, winner_payload: None, winner_kind: None }
    }
    pub(crate) fn failed(id: String, error: String) -> Self {
        ChangeResult { id, ok: false, error: Some(error), rejected: None, winner_seq: None, winner_payload: None, winner_kind: None }
    }
    pub(crate) fn losing(
        id: String,
        winner_seq: i64,
        winner_payload: serde_json::Value,
        winner_kind: Option<String>,
    ) -> Self {
        ChangeResult {
            id,
            ok: true,
            error: None,
            rejected: Some(true),
            winner_seq: Some(winner_seq),
            winner_payload: Some(winner_payload),
            winner_kind,
        }
    }
}

/// Parse a client `edit_time` into a comparable UTC instant, tolerating both
/// RFC 3339 (with any offset) and SQLite `datetime('now')` (`YYYY-MM-DD HH:MM:SS`,
/// assumed UTC) formats, and clamping anything dated in the future to the hub's
/// now (§3.4: a follower with a fast clock must not win every later conflict
/// forever). An empty or unparseable stamp is treated as "arrived now".
///
/// Reading the stamp is the shared codec's job (`datasets::parse_stamp`), so a
/// peer that ever starts sending its own local offset is understood here as well
/// as in the card conflict resolution.
fn normalize_edit_time(raw: &str, hub_now: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
    match datasets::parse_stamp(raw) {
        Some(dt) if dt > hub_now => hub_now,
        Some(dt) => dt,
        None => hub_now,
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
    let mut ledger = match datasets::dictation::open_app_db(&settings) {
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
        results.push(apply_one_change(&mut ledger, &settings, &write_identity, hub_now, &ch).await);
    }

    (StatusCode::OK, Json(results)).into_response()
}

/// The single per-change apply kernel shared by the HTTP writeback handler and
/// the hub-side inbox drainer (sync-v2 §8.3 / INV-18: a drained proposal must
/// ride the SAME command path a follower's writeback does, so LWW, tombstones,
/// dedup, epoch guard and the append choke point behave identically).
///
/// The extracted body is byte-identical to the loop that used to sit inside
/// `sync_changes` — `continue` becomes `return`, and `ch.id` is cloned at each
/// construction site because the shared fn takes the change by reference (the
/// drainer needs to keep the entry around for the ack).
pub(crate) async fn apply_one_change(
    ledger: &mut rusqlite::Connection,
    settings: &tauri::State<'_, SettingsState>,
    write_identity: &str,
    hub_now: chrono::DateTime<chrono::Utc>,
    ch: &Change,
) -> ChangeResult {
    // INV-13 split-brain guard, before anything is touched: a writer whose
    // recorded epoch for this scope is below the hub's current epoch lost the
    // handover and must not apply. Left `ok = false` so the change stays
    // queued — the follower's next `/changes` pull records the new epoch, and
    // the retry then succeeds. This is a no-op while every scope sits at its
    // minted epoch 0 (no handover has run), so a bump is the only way to hit it.
    {
        let ch_scope = crate::sync::change_log::scope_for(
            &ch.kind,
            ch.dataset_uuid.as_deref().unwrap_or(""),
        );
        let hub_epoch = crate::sync::change_log::scope_epoch(ledger, ch_scope);
        if ch.epoch < hub_epoch {
            log::warn!(
                "[rest] writeback {} ({}) refused: stale epoch {} < authority {} for scope {}",
                ch.id, ch.kind, ch.epoch, hub_epoch, ch_scope
            );
            return ChangeResult::failed(
                ch.id.clone(),
                format!(
                    "stale_epoch: presented {} < authority {} for scope '{}'; re-pull this scope to learn the new epoch, then retry (INV-13/INV-20)",
                    ch.epoch, hub_epoch, ch_scope
                ),
            );
        }
    }

    // Skip anything already applied (idempotent by change id).
    if crate::sync::change_log::already_applied(ledger, &ch.id) {
        return ChangeResult::applied(ch.id.clone());
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
        let object_id = crate::sync::change_log::object_id_for(&ch.kind, &ch.payload, write_identity);
        if !object_id.is_empty() {
            let incoming = normalize_edit_time(&ch.edit_time, hub_now);
            let latest = crate::sync::change_log::latest_for_object(ledger, &scope, &object_id)
                .ok()
                .flatten();
            if let Some((winner_seq, winner_edit, _op, winner_kind, winner_payload)) = latest {
                // A missing/unparseable stored time is treated as oldest so
                // the fresh push wins rather than being wrongly rejected.
                let existing = parse_log_time(&winner_edit).unwrap_or(hub_now);
                if existing >= incoming {
                    log::info!(
                        "[rest] writeback {} ({}) lost to hub seq {} kind={} (edit_time {} >= incoming {})",
                        ch.id, ch.kind, winner_seq, winner_kind, winner_edit, incoming.to_rfc3339()
                    );
                    return ChangeResult::losing(
                        ch.id.clone(),
                        winner_seq,
                        winner_payload,
                        Some(winner_kind),
                    );
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
                // (§3.4) rather than re-creating a dead row. `winner_kind` is
                // `None` here — the tombstone's originating kind was not
                // looked up, and a producer rolling back a delete has nothing
                // to disambiguate against (the row is gone).
                log::info!(
                    "[rest] writeback {} ({}) dropped: tombstone >= incoming edit_time",
                    ch.id, ch.kind
                );
                return ChangeResult::losing(ch.id.clone(), 0, serde_json::Value::Null, None);
            }
        }
    }

    match replay(settings, write_identity, ch).await {
        Ok(()) => {
            crate::sync::change_log::mark_applied(ledger, &ch.id);
            ChangeResult::applied(ch.id.clone())
        }
        Err(e) => {
            log::warn!("[rest] writeback {} ({}) failed: {}", ch.id, ch.kind, e);
            ChangeResult::failed(ch.id.clone(), e)
        }
    }
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
        // The hub stores the author's event verbatim and logs it onward under the
        // originating identity (`apply_remote` does the append, since this replay
        // path runs without an `ApplyGuard`). Re-running `card_test_submit` here
        // would fold the rating a second time on top of the hub's own history.
        "card_review" => {
            datasets::cards::review::apply_remote(settings.inner(), &dataset_uuid, &ch.payload, write_identity)
                .map(|_| ())
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
        // A card attachment is dataset content: the row replays here, while its
        // bytes ride the file manifest (so a row may arrive before its file).
        "card_media_save" => {
            datasets::cards::media::apply_remote_save(
                settings.inner(),
                &dataset_uuid,
                &ch.payload,
            )
            .map(|_| ())
        }
        "card_media_delete" => {
            datasets::cards::media::apply_remote_delete(
                settings.inner(),
                &dataset_uuid,
                &ch.payload,
            )
            .map(|_| ())
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
        "book_artifact_save" => {
            let artifact: datasets::book_artifacts::BookArtifact =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            datasets::book_artifacts::save_artifact_inner(settings, &dataset_uuid, artifact, "sync")
        }
        "book_artifact_delete" => {
            let uuid = extract_str(&ch.payload, "uuid")?;
            datasets::book_artifacts::book_delete_artifact(settings.clone(), dataset_uuid, uuid).await
        }
        "book_term_save" => {
            let term: datasets::book_artifacts::BookTerm =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            datasets::book_artifacts::book_save_term(settings.clone(), dataset_uuid, term).await.map(|_| ())
        }
        "book_term_delete" => {
            let uuid = extract_str(&ch.payload, "uuid")?;
            datasets::book_artifacts::book_delete_term(settings.clone(), dataset_uuid, uuid).await
        }
        "book_quiz_result_save" => {
            datasets::book_artifacts::apply_quiz_result_row(settings.inner(), &dataset_uuid, &ch.payload)
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
        // A standard clip is dataset content: the row replays here, while its wav
        // rides the file manifest (so a row may arrive before its audio).
        "read_standard_audio_save" => {
            datasets::read_aloud::standard_audio::apply_remote_save(
                settings.inner(),
                &dataset_uuid,
                &ch.payload,
            )
            .map(|_| ())
        }
        "read_standard_audio_delete" => {
            datasets::read_aloud::standard_audio::apply_remote_delete(
                settings.inner(),
                &dataset_uuid,
                &ch.payload,
            )
            .map(|_| ())
        }
        "read_attempt_save" => {
            // Per-user practice history: store the event the author sent in the
            // app DB (INSERT OR IGNORE), never a write into the shared dataset.
            datasets::read_aloud::apply_remote_save(settings.inner(), &dataset_uuid, &ch.payload, write_identity)
                .map(|_| ())
        }
        "read_attempt_delete" => {
            datasets::read_aloud::apply_remote_delete(settings.inner(), &dataset_uuid, &ch.payload, write_identity)
                .map(|_| ())
        }
        "wiki_file_save" => {
            let rel = extract_str(&ch.payload, "rel_path")?;
            let content = ch.payload.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
            datasets::wiki::write_file(settings.inner(), &dataset_uuid, &rel, &content)
        }
        "wiki_file_delete" => {
            let rel = extract_str(&ch.payload, "rel_path")?;
            datasets::wiki::delete_file(settings.inner(), &dataset_uuid, &rel)
        }
        // Per-user app data: a todo replays into the app DB attributed to the
        // enforced identity, never a payload-supplied user_key.
        "todo" => crate::todo::apply_remote(settings.inner(), write_identity, &ch.payload),
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
// GET /wiki/dataset/*  (read-only wiki-dataset browsing for followers)
//
// Addressed by dataset uuid + relative path: the uuid must resolve through
// `find_dataset_dir` to a `wiki` dataset on this machine, so a paired device
// can only reach wiki datasets it is allowed to browse, never arbitrary files.
// No mutating route is exposed here — editing a hub wiki dataset requires
// downloading it (snapshot + writeback).
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct WikiDatasetDirQuery {
    uuid: String,
    #[serde(default)]
    rel: String,
}

#[derive(Deserialize)]
struct WikiDatasetSearchQuery {
    uuid: String,
    keyword: String,
}

/// `GET /wiki/datasets` — the hub's wiki datasets.
async fn wiki_datasets_hub(State(st): State<RestState>) -> Response {
    let settings = st.app.state::<SettingsState>();
    let datasets = datasets::wiki::list_datasets(&settings);
    (
        StatusCode::OK,
        Json(serde_json::json!({ "datasets": datasets })),
    )
        .into_response()
}

/// `GET /wiki/dataset/dir?uuid=&rel=` — one directory of a hub wiki dataset.
async fn wiki_dataset_dir_hub(
    State(st): State<RestState>,
    Query(q): Query<WikiDatasetDirQuery>,
) -> Response {
    let settings = st.app.state::<SettingsState>();
    match datasets::wiki::list_dir(&settings, &q.uuid, &q.rel) {
        Ok(entries) => (StatusCode::OK, Json(entries)).into_response(),
        Err(e) => json_error(StatusCode::NOT_FOUND, &e),
    }
}

/// `GET /wiki/dataset/file?uuid=&rel=` — classified file preview wrapped as
/// `{ kind, content, size, truncated }`. Non-UTF-8 files answer `binary`
/// instead of erroring, and images/media answer with their kind only (a
/// follower cannot stream across the hub link yet). The hub-side absolute
/// path is never leaked.
async fn wiki_dataset_file_hub(
    State(st): State<RestState>,
    Query(q): Query<WikiDatasetDirQuery>,
) -> Response {
    let settings = st.app.state::<SettingsState>();
    match datasets::wiki::read_file(&settings, &q.uuid, &q.rel) {
        Ok(f) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "kind": f.kind,
                "content": f.content,
                "size": f.size,
                "truncated": f.truncated,
            })),
        )
            .into_response(),
        Err(e) => json_error(StatusCode::NOT_FOUND, &e),
    }
}

/// `GET /wiki/dataset/search?uuid=&keyword=` — FTS inside one hub wiki dataset
/// (the hub indexes that dataset on demand; the index never ships).
async fn wiki_dataset_search_hub(
    State(st): State<RestState>,
    Query(q): Query<WikiDatasetSearchQuery>,
) -> Response {
    let settings = st.app.state::<SettingsState>();
    match datasets::wiki::search_dataset(&settings, &q.uuid, &q.keyword) {
        Ok(results) => (StatusCode::OK, Json(results)).into_response(),
        Err(e) => json_error(StatusCode::NOT_FOUND, &e),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway dataset dir with a `data.sqlite3` carrying both content tables
    /// (`card`, `read_text`) and both legacy practice tables (`card_review`,
    /// `read_attempt`), each with one row. The caller removes the dir.
    fn seeded_dataset_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("fms-snap-sanitize-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = rusqlite::Connection::open(dir.join("data.sqlite3")).unwrap();
        conn.execute_batch(
            "CREATE TABLE card (uuid TEXT PRIMARY KEY, front TEXT);
             INSERT INTO card (uuid, front) VALUES ('c1', 'hello');
             CREATE TABLE card_review (uuid TEXT PRIMARY KEY, card_uuid TEXT, familiarity INTEGER);
             INSERT INTO card_review (uuid, card_uuid, familiarity) VALUES ('r1', 'c1', 4);
             CREATE TABLE read_text (uuid TEXT PRIMARY KEY, content TEXT);
             INSERT INTO read_text (uuid, content) VALUES ('t1', 'world');
             CREATE TABLE read_attempt (uuid TEXT PRIMARY KEY, text_uuid TEXT, score REAL);
             INSERT INTO read_attempt (uuid, text_uuid, score) VALUES ('a1', 't1', 88.0);",
        )
        .unwrap();
        drop(conn);
        dir
    }

    fn table_exists(conn: &rusqlite::Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
            rusqlite::params![name],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            > 0
    }

    fn row_count(conn: &rusqlite::Connection, table: &str) -> i64 {
        conn.query_row(
            &format!("SELECT count(*) FROM {table}"),
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    }

    #[test]
    fn sanitize_snapshot_db_drops_practice_tables_keeps_content() {
        let dir = seeded_dataset_dir("drops");
        // Mirror snapshot(): freeze a point-in-time copy, then sanitize it.
        let copy = snapshot_db_copy(&dir).expect("a dataset DB exists to copy");
        sanitize_snapshot_db(&copy);

        let conn = rusqlite::Connection::open(&copy).unwrap();
        assert!(!table_exists(&conn, "card_review"), "publisher schedule must be stripped");
        assert!(!table_exists(&conn, "read_attempt"), "publisher scores must be stripped");
        assert!(table_exists(&conn, "card"), "content table must survive");
        assert!(table_exists(&conn, "read_text"), "content table must survive");
        assert_eq!(row_count(&conn, "card"), 1);
        assert_eq!(row_count(&conn, "read_text"), 1);
        drop(conn);

        let _ = std::fs::remove_file(&copy);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_snapshot_db_is_idempotent() {
        let dir = seeded_dataset_dir("idempotent");
        let copy = snapshot_db_copy(&dir).expect("a dataset DB exists to copy");
        sanitize_snapshot_db(&copy);
        // A second pass must be a harmless no-op (DROP TABLE IF EXISTS).
        sanitize_snapshot_db(&copy);

        let conn = rusqlite::Connection::open(&copy).unwrap();
        assert!(!table_exists(&conn, "card_review"));
        assert!(!table_exists(&conn, "read_attempt"));
        assert_eq!(row_count(&conn, "card"), 1);
        assert_eq!(row_count(&conn, "read_text"), 1);
        drop(conn);

        let _ = std::fs::remove_file(&copy);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
