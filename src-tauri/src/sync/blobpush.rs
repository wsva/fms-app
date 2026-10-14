//! Owner-facing **changed-file transport** — the app-side client for the
//! content-addressed blob store the website hosts (sync-v2.md §7, the plane §14
//! deferred as "matching the host route", and `learning/fms` stood up first in
//! commit 0b5a5ad). This is the *producer* half: re-push one dataset's live tree
//! and move only the bytes that actually changed.
//!
//! Two phases over one owner-only route, told apart by content-type, so an
//! unchanged tree uploads zero bytes:
//! * **Phase 1 — diff** (`application/json`, `{ files: [{path, sha256, size}] }`):
//!   the server replies `{ uuid, total, need: [{path, sha256, size}] }` — the
//!   hashes it does not already hold. Read-only, no writes.
//! * **Phase 2 — ingest** (`multipart/form-data`, a `tree` JSON field naming the
//!   *whole* tree plus one `blob` part per needed hash, its filename being the
//!   sha): the server re-verifies every uploaded digest (a claimed hash is never
//!   trusted — mismatch is refused), stores the new blobs (dedup if present),
//!   confirms the whole tree's bytes are on disk, then swaps the live tree +
//!   global refcounts inside one advisory-locked transaction. It returns
//!   `{ uuid, applied, stored, deduped }`.
//!
//! Blob identity is the plain per-file `sha256` of raw bytes — the *same* recipe
//! the hub's own manifest mints ([`crate::sync::rest`]'s `hash_file_contents`) and
//! the website's `sinkBlob` recomputes, so a claim is verifiable end-to-end. DB
//! files never belong in the tree (`assertValidRelPath` → `isDbFile` rejects them):
//! their content churns through the row-log / snapshot plane, not blobs.
//!
//! The route **overwrites the authoritative live tree**, so it is owner-only —
//! account Bearer against the account website (`crate::auth::base_url()`), the
//! same credential `sync/publish.rs` and `sync/mirror.rs` use, and deliberately
//! *not* the LAN `pc_url`. A missing token is a hard, actionable error (a write).
//! A foreign uuid comes back 409 (`owned-by-other`), a missing/deleted row 404.
//!
//! This is the *incremental* path; the whole-archive upload (`sync/upload.rs`)
//! stays for bootstrap + explicit publish. Pure addition: nothing here is reached
//! by the round, writeback, pull, upload, or availability-consumption paths.
//! Compiled on **all platforms** (reqwest + sha2 + serde_json only, like `upload.rs`).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::State;

use crate::datasets;
use crate::settings::SettingsState;

/// A content file of the live tree: its `/`-separated rel path, verified hash,
/// byte size, and where it lives on disk.
struct Ref {
    path: String,
    sha: String,
    size: u64,
    abs: PathBuf,
}

/// The dataset SQLite file + its WAL/SHM sidecars + derived per-copy indexes are
/// excluded from the blob tree (the row-log and snapshot own them; the website's
/// `assertValidRelPath` rejects a DB rel-path outright). Mirrors `upload.rs` /
/// `rest.rs` `is_db_file` so the three planes agree on what "content" means.
fn is_db_file(rel: &str) -> bool {
    rel == "data.sqlite3" || rel == "data.sqlite3-wal" || rel == "data.sqlite3-shm"
        || datasets::is_derived_index(rel)
}

/// Recurse `dir` collecting `(rel_path_forward_slash, abs_path)` for every non-DB
/// file (info.json included). Rel paths are `/`-separated and dataset-rooted, so
/// they survive the website's traversal check (`assertValidRelPath`).
fn collect_tree(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    // Sorted by rel path so the `tree` JSON is byte-stable across machines/runs.
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, out);
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if !rel.is_empty() && !is_db_file(&rel) {
                out.push((rel, path));
            }
        }
    }
}

/// Stream-sha256 (hex) + byte size of one file. The same digest the website
/// recomputes on ingest, so a claimed hash is checked, never trusted.
fn hash_and_size(path: &Path) -> Result<(String, u64), String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        size += n as u64;
        h.update(&buf[..n]);
    }
    Ok((format!("{:x}", h.finalize()), size))
}

/// Enumerate + hash the whole live tree. Pure filesystem work, run to completion
/// *before* any await (no DB handle, no borrowed state crosses a suspension).
fn build_refs(dir: &Path) -> Result<Vec<Ref>, String> {
    let mut refs = Vec::new();
    for (rel, abs) in collect_tree(dir) {
        let (sha, size) = hash_and_size(&abs)?;
        refs.push(Ref { path: rel, sha, size, abs });
    }
    Ok(refs)
}

/// The tree as `[{path, sha256, size}]` — the phase-1 `files` array and the
/// phase-2 `tree` field share it.
fn refs_json(refs: &[Ref]) -> Value {
    Value::Array(
        refs.iter()
            .map(|r| json!({ "path": r.path, "sha256": r.sha, "size": r.size }))
            .collect(),
    )
}

/// Unique lowercase hashes the server says it still needs. A hash reached at two
/// paths uploads once (the store dedups on the hash anyway), and the server
/// lowercases each claimed name, so we normalize before matching our own refs.
fn need_shas(need: &Value) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    if let Some(arr) = need.as_array() {
        for n in arr {
            if let Some(s) = n["sha256"].as_str() {
                let s = s.to_ascii_lowercase();
                if seen.insert(s.clone()) {
                    out.push(s);
                }
            }
        }
    }
    out
}

/// Map a failed `/objects` response per the owner-write error table, tagging a
/// 409 with `owned-by-other` (the recover path is a remint, never a silent
/// overwrite) — house style of `sync/publish.rs::publish_http_error`.
async fn objects_http_error(resp: reqwest::Response, uuid: &str) -> String {
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let msg = body["error"].as_str().unwrap_or("");
    let suffix = |s: &str| if s.is_empty() { String::new() } else { format!(" — {s}") };
    match status {
        400 => format!(
            "the website rejected the blob tree for '{uuid}' — a file needs \
             {{path, sha256, size}}, a hash is 64 hex chars, no DB file, no \
             path traversal (400){}",
            suffix(msg)
        ),
        401 => "sign in again — the website rejected or did not receive your access token (401)"
            .to_string(),
        403 => format!(
            "the website rejected this blob write; the credential is read-only or lacks access \
             (403){}",
            suffix(msg)
        ),
        404 => format!(
            "no dataset '{uuid}' on the website to receive files — upload it (or publish a \
             version) first (404){}",
            suffix(msg)
        ),
        409 => format!(
            "owned-by-other: you do not own this dataset uuid on the website, or a needed blob \
             went missing (409){} [uuid={uuid}] — mint a new copy with dataset_remint_uuid and \
             push that instead",
            suffix(msg)
        ),
        c if c >= 500 => format!(
            "the website is failing right now (HTTP {c}) — retryable{}",
            suffix(msg)
        ),
        c => format!("changed-file push for '{uuid}' failed (HTTP {c}): {msg}"),
    }
}

/// Build the write client + resolved base + a hard-required token, shared by both
/// phases. A missing token is an actionable error for a write (not a degrade to
/// anonymous like the reads), matching `sync/publish.rs`.
async fn website_write(
    settings: &SettingsState,
) -> Result<(reqwest::Client, String, String), String> {
    let base = crate::auth::base_url();
    let token = crate::auth::get_access_token(settings)?;
    // A generous overall timeout: this is a multi-MB blob PUT over WAN.
    let client = crate::auth::hub_client_builder()
        .timeout(Duration::from_secs(3600))
        .build()
        .map_err(|e| e.to_string())?;
    Ok((client, base, token))
}

/// Handle 401 (drop the rejected token) then map any non-success to an error.
async fn check_ok(resp: reqwest::Response, settings: &SettingsState, uuid: &str) -> Result<Value, String> {
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        crate::auth::token_rejected(settings);
    }
    if !status.is_success() {
        return Err(objects_http_error(resp, uuid).await);
    }
    resp.json::<Value>()
        .await
        .map_err(|e| format!("unexpected changed-file response for '{uuid}': {e}"))
}

/// Re-push one dataset's live tree to the website's content-addressed blob store
/// and move only the bytes that changed (sync-v2.md §7). Owner-only, account
/// Bearer, against the account website; the dataset must already exist there
/// (bootstrap with `dataset_export_upload` first). Enumerates every non-DB file,
/// hashes it, asks the store which hashes it lacks (phase 1), uploads just those
/// blobs, and lets the website atomically swap the dataset's live tree (phase 2).
/// An unchanged tree uploads zero bytes but still reconciles the tree (a local
/// rename/deletion syncs even with no new content). `max_files` is a client-side
/// guard against the hub's per-tree cap. Returns `{ uuid, total, need, applied,
/// stored, deduped }`.
#[tauri::command]
pub async fn dataset_push_changed_files(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<Value, String> {
    let uuid = dataset_uuid.trim().to_string();
    if uuid.is_empty() {
        return Err("dataset_uuid is required (the local dataset to push)".to_string());
    }
    let (dir, _ty) = datasets::find_dataset_dir_typed(&settings, &uuid)?;

    // Enumerate + hash the whole tree before touching the network (no borrowed
    // state or open handle crosses an await).
    let refs = build_refs(&dir)?;
    if refs.is_empty() {
        return Err(format!(
            "'{uuid}' has no content files to push (empty or database-only tree)"
        ));
    }
    let tree = refs_json(&refs);

    let (client, base, token) = website_write(&settings).await?;
    let url = format!("{base}/api/v1/datasets/{}/objects", super::client::qenc(&uuid));

    // Phase 1 — read-only diff: which hashes does the store not hold?
    let resp = client
        .post(&url)
        .bearer_auth(&token)
        .json(&json!({ "files": tree }))
        .send()
        .await
        .map_err(|e| format!("cannot reach the website at {url}: {e}"))?;
    let diff = check_ok(resp, &settings, &uuid).await?;
    let need = need_shas(&diff["need"]);
    let need_set: std::collections::HashSet<&str> = need.iter().map(|s| s.as_str()).collect();

    // Phase 2 — ingest: upload just the needed blobs, then the website swaps the
    // live tree atomically. The `tree` names the WHOLE tree (renames/deletions
    // reconcile it), so it is sent even when `need` is empty (zero-byte re-push).
    let mut form = reqwest::multipart::Form::new().text("tree", tree.to_string());
    let mut uploaded = 0usize;
    for r in &refs {
        if need_set.contains(r.sha.as_str()) {
            let bytes = std::fs::read(&r.abs)
                .map_err(|e| format!("cannot read '{}' for upload: {e}", r.path))?;
            let part = reqwest::multipart::Part::bytes(bytes)
                .file_name(r.sha.clone())
                .mime_str("application/octet-stream")
                .map_err(|e| e.to_string())?;
            form = form.part("blob", part);
            uploaded += 1;
        }
    }
    let resp = client
        .post(&url)
        .bearer_auth(&token)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("changed-file upload for '{uuid}' failed: {e}"))?;
    let body = check_ok(resp, &settings, &uuid).await?;

    log::info!(
        "[blob] {uuid}: tree of {} files, needed {}, uploaded {} blobs -> applied {} stored {} deduped {}",
        refs.len(),
        need.len(),
        uploaded,
        body["applied"].as_i64().unwrap_or(0),
        body["stored"].as_i64().unwrap_or(0),
        body["deduped"].as_i64().unwrap_or(0),
    );
    Ok(json!({
        "uuid": uuid,
        "total": refs.len(),
        "need": need.len(),
        "uploaded": uploaded,
        "applied": body["applied"],
        "stored": body["stored"],
        "deduped": body["deduped"],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A process-unique scratch dir under the system temp dir, removed on drop.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let p = std::env::temp_dir().join(format!("blobpush_{tag}_{nanos}_{}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn write(&self, rel: &str, content: &[u8]) {
            let p = self.0.join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(p, content).unwrap();
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn is_db_file_excludes_db_and_sidecars() {
        assert!(is_db_file("data.sqlite3"));
        assert!(is_db_file("data.sqlite3-wal"));
        assert!(is_db_file("data.sqlite3-shm"));
        assert!(!is_db_file("media/a.mp3"));
        assert!(!is_db_file("info.json"));
        assert!(!is_db_file("subtitle/a.vtt"));
    }

    #[test]
    fn collect_tree_is_sorted_forward_slash_and_skips_db() {
        let s = Scratch::new("collect");
        s.write("media/z.mp3", b"z");
        s.write("media/a.mp3", b"a");
        s.write("subtitle/deep/b.vtt", b"b");
        s.write("info.json", b"{}");
        s.write("data.sqlite3", b"db");
        s.write("data.sqlite3-wal", b"wal");
        let got: Vec<String> = collect_tree(&s.0).into_iter().map(|(rel, _)| rel).collect();
        assert_eq!(got, vec!["info.json", "media/a.mp3", "media/z.mp3", "subtitle/deep/b.vtt"]);
    }

    #[test]
    fn hash_and_size_matches_known_digest() {
        let s = Scratch::new("hash");
        s.write("f.bin", b"abc");
        let (sha, size) = hash_and_size(&s.0.join("f.bin")).unwrap();
        // The canonical SHA-256 of the three bytes "abc".
        assert_eq!(sha, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(size, 3);
    }

    #[test]
    fn refs_json_shape_matches_wire_contract() {
        let refs = vec![Ref {
            path: "media/a.mp3".into(),
            sha: "aa".into(),
            size: 10,
            abs: PathBuf::from("/x"),
        }];
        assert_eq!(
            refs_json(&refs),
            json!([{ "path": "media/a.mp3", "sha256": "aa", "size": 10 }])
        );
    }

    #[test]
    fn need_shas_dedups_and_lowercases() {
        let need = json!([
            { "path": "a", "sha256": "DEADBEEF" },
            { "path": "b", "sha256": "deadbeef" }, // same hash at another path
            { "path": "c", "sha256": "cafe" },
        ]);
        assert_eq!(need_shas(&need), vec!["deadbeef", "cafe"]);
        // A missing/non-array `need` yields no uploads rather than a panic.
        assert!(need_shas(&Value::Null).is_empty());
    }
}
