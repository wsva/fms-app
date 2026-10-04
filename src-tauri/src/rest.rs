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
//! * `GET  /api/v1/datasets/{uuid}/snapshot`   - the whole dataset dir as tar.gz
//! * `POST /api/v1/sync/changes`               - replay queued writeback changes
//!
//! Auth is owned by the outer trust-zone layer in `web_service.rs`: loopback +
//! Tailscale pass unauthenticated; other networks need a valid per-device
//! Ed25519 signature (see `pairing.rs`) except `/status` and `/pair/*` here.
//! The legacy shared `api_token` is no longer enforced.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};
use tokio_util::io::{ReaderStream, SyncIoBridge};

use crate::book::{self, BookChapter, BookSentence, BookSentenceWord};
use crate::cards::{self, Card, Tag};
use crate::dataset;
use crate::dictation::{self, ListenCue, ListenDictation};
use crate::pairing;
use crate::settings::SettingsState;
use crate::xp;

/// Shared state for the REST router: the Tauri app handle (to reach managed
/// state). Authentication lives in the outer zone layer of `web_service.rs`.
#[derive(Clone)]
pub struct RestState {
    pub app: AppHandle,
}

/// Build the `/api/v1` router. Mounted by `web_service::build_router`.
pub fn router(app: AppHandle) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/pair/request", post(pair_request))
        .route("/pair/status", get(pair_status))
        .route("/datasets", get(datasets_list))
        .route("/datasets/{uuid}/manifest", get(manifest))
        .route("/datasets/{uuid}/snapshot", get(snapshot))
        .route("/sync/changes", post(sync_changes))
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
}

async fn status(State(st): State<RestState>) -> Response {
    let settings = st.app.state::<SettingsState>();
    let count = dataset::list_datasets(&settings).len();
    (
        StatusCode::OK,
        Json(StatusResp {
            version: env!("CARGO_PKG_VERSION"),
            app_name: "fms-app",
            dataset_count: count,
            ok: true,
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
    let pubkey = match pairing::hex_decode(&body.pubkey_hex) {
        Ok(b) if b.len() == 32 => b,
        _ => return json_error(StatusCode::BAD_REQUEST, "pubkey_hex must be 32 bytes of hex"),
    };
    let _ = pubkey; // validated only; the hex string is what we store
    let name = body.name.trim().chars().take(64).collect::<String>();
    let name = if name.is_empty() { "device".to_string() } else { name };

    let outcome = match pairing::request_pair(&st.app, &settings, &body.pubkey_hex, &name, &body.user_key) {
        Ok(o) => o,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };
    match outcome {
        pairing::PairRequestOutcome::Approved => {
            Json(serde_json::json!({ "state": "approved" })).into_response()
        }
        pairing::PairRequestOutcome::Denied => {
            Json(serde_json::json!({ "state": "denied" })).into_response()
        }
        pairing::PairRequestOutcome::Pending { request_id, fingerprint } => {
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
    match pairing::pending_status(&settings, &q.device_id) {
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
    /// One of `dictation` | `card` | `book` — lets the phone group the list and
    /// know which local root to unpack a snapshot into.
    dataset_type: String,
    /// Dictation: media file count. Card: card count. Book: 0 (not tracked).
    media_count: usize,
    status: String,
}

async fn datasets_list(State(st): State<RestState>) -> Response {
    let settings = st.app.state::<SettingsState>();
    let mut items: Vec<DatasetListItem> = Vec::new();

    // Dictation datasets. Raw-import folders (a directory with media/ but no
    // info.json) carry an empty uuid and cannot be resolved by the
    // manifest/snapshot endpoints, so skip them rather than offer dead entries.
    for d in dataset::list_datasets(&settings) {
        if d.info.uuid.is_empty() {
            continue;
        }
        items.push(DatasetListItem {
            uuid: d.info.uuid,
            name: d.info.name,
            updated: d.info.updated,
            dataset_type: "dictation".into(),
            media_count: d.media_count,
            status: d.status,
        });
    }

    // Card datasets.
    for d in crate::cards::list_card_datasets(&settings) {
        if d.info.uuid.is_empty() {
            continue;
        }
        items.push(DatasetListItem {
            uuid: d.info.uuid,
            name: d.info.name,
            updated: d.info.updated,
            dataset_type: "card".into(),
            media_count: d.card_count,
            status: "ready".into(),
        });
    }

    // Books (reading library).
    for b in crate::book::list_books(&settings) {
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
    /// Dataset type (`dictation` | `card` | `book`) so the phone unpacks the
    /// snapshot into the matching local root.
    #[serde(default)]
    dataset_type: String,
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

/// Compute the manifest: file count, total bytes, and a SHA-256 over the
/// sorted `(rel_path, size, mtime_ms)` tuples. `updated_at` comes from info.json
/// when present.
fn compute_manifest(dir: &Path) -> Manifest {
    let files = collect_rel_files(dir);
    let mut hasher = Sha256::new();
    let mut total_bytes = 0u64;
    for (rel, path) in &files {
        let meta = std::fs::metadata(path).ok();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let mtime_ms = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        total_bytes += size;
        hasher.update(rel.as_bytes());
        hasher.update([0u8]);
        hasher.update(size.to_le_bytes());
        hasher.update(mtime_ms.to_le_bytes());
    }
    let overall_hash = format!("{:x}", hasher.finalize());

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

async fn manifest(State(st): State<RestState>, axum::extract::Path(uuid): axum::extract::Path<String>) -> Response {
    let settings = st.app.state::<SettingsState>();
    let (dir, ty) = match dataset::find_dataset_dir_typed(&settings, &uuid) {
        Ok(d) => d,
        Err(e) => return json_error(StatusCode::NOT_FOUND, &e),
    };
    checkpoint_db(&dir);
    let mut m = compute_manifest(&dir);
    m.dataset_type = ty.as_str().to_string();
    (StatusCode::OK, Json(m)).into_response()
}

// ---------------------------------------------------------------------------
// GET /snapshot  (whole dataset dir streamed as one tar.gz)
// ---------------------------------------------------------------------------

async fn snapshot(State(st): State<RestState>, axum::extract::Path(uuid): axum::extract::Path<String>) -> Response {
    let settings = st.app.state::<SettingsState>();
    let (dir, _ty) = match dataset::find_dataset_dir_typed(&settings, &uuid) {
        Ok(d) => d,
        Err(e) => return json_error(StatusCode::NOT_FOUND, &e),
    };

    // Checkpoint + hash the files before archiving so the header carries the
    // same value the manifest endpoint would report.
    checkpoint_db(&dir);
    let m = compute_manifest(&dir);

    // Enumerate files now (sync) so the archive contents match the hashed set.
    let files = collect_rel_files(&dir);

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
    });

    let stream = ReaderStream::new(client_half);
    let body = axum::body::Body::from_stream(stream);
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-snapshot-hash",
        m.overall_hash.parse().unwrap(),
    );
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        "application/gzip".parse().unwrap(),
    );
    (StatusCode::OK, headers, body).into_response()
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
}

/// Ensure the idempotency ledger exists in the app DB.
fn ensure_applied_table(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_applied (
            id         TEXT PRIMARY KEY,
            applied_at TEXT NOT NULL
        );",
    )
    .map_err(|e| e.to_string())
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
    auth: &pairing::AuthContext,
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
    Extension(auth): Extension<pairing::AuthContext>,
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

    // Idempotency ledger lives in the app-level DB (survives dataset swaps).
    let ledger = match dictation::open_app_db(&settings) {
        Ok(conn) => {
            if let Err(e) = ensure_applied_table(&conn) {
                return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e);
            }
            conn
        }
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    let mut results = Vec::with_capacity(req.changes.len());
    for ch in req.changes {
        // Skip anything already applied (idempotent by change id).
        let already: bool = ledger
            .query_row(
                "SELECT COUNT(*) FROM sync_applied WHERE id = ?1",
                rusqlite::params![&ch.id],
                |row| row.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        if already {
            results.push(ChangeResult { id: ch.id, ok: true, error: None });
            continue;
        }

        match replay(&settings, &write_identity, &ch).await {
            Ok(()) => {
                let _ = ledger.execute(
                    "INSERT OR IGNORE INTO sync_applied (id, applied_at) VALUES (?1, datetime('now'))",
                    rusqlite::params![&ch.id],
                );
                results.push(ChangeResult { id: ch.id, ok: true, error: None });
            }
            Err(e) => {
                log::warn!("[rest] writeback {} ({}) failed: {}", ch.id, ch.kind, e);
                results.push(ChangeResult {
                    id: ch.id,
                    ok: false,
                    error: Some(e),
                });
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
    let dataset_uuid = ch.dataset_uuid.clone().unwrap_or_default();
    match ch.kind.as_str() {
        "cue_save" => {
            let cue: ListenCue =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            dictation::listen_save_cue(settings.clone(), dataset_uuid, cue).await
        }
        "cue_delete" => {
            let cue_uuid: String = extract_cue_uuid(&ch.payload)?;
            dictation::listen_delete_cue(settings.clone(), dataset_uuid, cue_uuid).await
        }
        "dictation" => {
            let d: ListenDictation =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            // Attribute to the enforced identity, not the PC's current workspace.
            dictation::listen_save_dictation_as(
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
            // `user_id` in the payload is never trusted.
            let ds = if dataset_uuid.is_empty() {
                None
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
            cards::card_save(settings.clone(), dataset_uuid, card).await.map(|_| ())
        }
        "card_delete" => {
            let card_uuid = extract_str(&ch.payload, "card_uuid")?;
            cards::card_delete(settings.clone(), dataset_uuid, card_uuid).await
        }
        "card_review" => {
            let card_uuid = extract_str(&ch.payload, "card_uuid")?;
            let quality = ch.payload.get("quality").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            cards::card_test_submit(settings.clone(), dataset_uuid, card_uuid, quality).await.map(|_| ())
        }
        "card_tag_save" => {
            let tag: Tag =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            cards::card_tag_save(settings.clone(), dataset_uuid, tag).await.map(|_| ())
        }
        "card_tag_delete" => {
            let tag_uuid = extract_str(&ch.payload, "tag_uuid")?;
            cards::card_tag_delete(settings.clone(), dataset_uuid, tag_uuid).await
        }
        "card_set_tags" => {
            let card_uuid = extract_str(&ch.payload, "card_uuid")?;
            let tag_uuids: Vec<String> = ch.payload.get("tag_uuids").and_then(|v| v.as_array()).map(|arr| {
                arr.iter().filter_map(|v| v.as_str().map(String::from)).collect()
            }).unwrap_or_default();
            cards::card_set_tags(settings.clone(), dataset_uuid, card_uuid, tag_uuids).await
        }
        "book_chapter_save" => {
            let chapter: BookChapter =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            book::book_save_chapter(settings.clone(), dataset_uuid, chapter).await
        }
        "book_chapter_delete" => {
            let uuid = extract_str(&ch.payload, "uuid")?;
            book::book_delete_chapter(settings.clone(), dataset_uuid, uuid).await
        }
        "book_sentence_save" => {
            let sentence: BookSentence =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            book::book_save_sentence(settings.clone(), dataset_uuid, sentence).await
        }
        "book_sentences_save" => {
            let sentences: Vec<BookSentence> =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            book::book_save_sentences(settings.clone(), dataset_uuid, sentences).await
        }
        "book_sentence_delete" => {
            let uuid = extract_str(&ch.payload, "uuid")?;
            book::book_delete_sentence(settings.clone(), dataset_uuid, uuid).await
        }
        "book_word_save" => {
            let word: BookSentenceWord =
                serde_json::from_value(ch.payload.clone()).map_err(|e| e.to_string())?;
            book::book_save_word(settings.clone(), dataset_uuid, word).await
        }
        "book_word_delete" => {
            let uuid = extract_str(&ch.payload, "uuid")?;
            book::book_delete_word(settings.clone(), dataset_uuid, uuid).await
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
// Small response helper
// ---------------------------------------------------------------------------

fn json_error(status: StatusCode, msg: &str) -> Response {
    (status, Json(serde_json::json!({ "error": msg }))).into_response()
}
