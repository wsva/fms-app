//! Sync conformance suite (sync-v2.md §13).
//!
//! A small data-driven harness: every `vectors/*.json` file describes one
//! invariant the hub journal must uphold, and this runner replays it against the
//! real journal engine through the `#[doc(hidden)]` [`fms_app_lib::conformance`]
//! seam. Phase 1 runs the vectors against the desktop hub only; phase 2's parity
//! claim is that the *same* vectors pass unchanged against the website routes.
//!
//! See `README.md` in this directory for how to add a vector.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use fms_app_lib::conformance;

/// Fixed dataset uuid the non-app vectors journal under. Any stable key works —
/// the journal is scope-addressed, and the scope is just a string.
const DATASET_SCOPE: &str = "ds-conformance";

fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/sync_conformance/vectors")
}

/// Resolve a vector's `scope` field to the journal key it addresses.
fn scope_of(v: &Value) -> String {
    match v.get("scope").and_then(|s| s.as_str()).unwrap_or("dataset") {
        "app" => conformance::APP_SCOPE.to_string(),
        _ => DATASET_SCOPE.to_string(),
    }
}

fn seed_db(v: &Value) -> (rusqlite::Connection, String) {
    let conn = conformance::new_hub_db();
    let scope = scope_of(v);
    if let Some(seed) = v.get("seed").and_then(|s| s.as_array()) {
        for s in seed {
            let object_id = s.get("object_id").and_then(|x| x.as_str()).unwrap_or("");
            let kind = s.get("kind").and_then(|x| x.as_str()).unwrap_or("");
            let edit_time = s
                .get("edit_time")
                .and_then(|x| x.as_str())
                .unwrap_or("2026-01-01T00:00:00Z");
            let user_key = s.get("user_key").and_then(|x| x.as_str()).unwrap_or("");
            let payload = s
                .get("payload")
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default()))
                .to_string();
            conformance::append(&conn, &scope, object_id, kind, edit_time, user_key, &payload);
        }
    }
    (conn, scope)
}

fn after_of(v: &Value) -> i64 {
    v.get("after").and_then(|a| a.as_i64()).unwrap_or(-1)
}

/// Sorted key set of a JSON object, for order-insensitive shape assertions.
fn key_set(obj: &Value) -> Vec<String> {
    let mut ks: Vec<String> = obj
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    ks.sort();
    ks
}

fn expected_keys(expect: &Value, field: &str) -> Vec<String> {
    let mut ks: Vec<String> = expect
        .get(field)
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    ks.sort();
    ks
}

fn check_page_shape(v: &Value, errs: &mut Vec<String>) {
    let (conn, scope) = seed_db(v);
    let page = conformance::read_page_json(&conn, &scope, after_of(v));
    let expect = &v["expect"];

    let got = key_set(&page);
    let want = expected_keys(expect, "keys");
    if got != want {
        errs.push(format!("page keys {got:?} != expected {want:?}"));
    }
    if let Some(resync) = expect.get("resync_required").and_then(|b| b.as_bool()) {
        let got_resync = page.get("resync_required").and_then(|b| b.as_bool());
        if got_resync != Some(resync) {
            errs.push(format!("resync_required {got_resync:?} != expected {resync}"));
        }
    }
    let entries = page.get("entries").and_then(|e| e.as_array()).cloned().unwrap_or_default();
    if let Some(n) = expect.get("entry_count").and_then(|x| x.as_i64()) {
        if entries.len() as i64 != n {
            errs.push(format!("entry_count {} != expected {n}", entries.len()));
        }
    }
    if let Some(first) = entries.first() {
        let want_entry = expected_keys(expect, "entry_keys");
        if !want_entry.is_empty() {
            let got_entry = key_set(first);
            if got_entry != want_entry {
                errs.push(format!("entry keys {got_entry:?} != expected {want_entry:?}"));
            }
        }
    }
}

fn check_resync(v: &Value, errs: &mut Vec<String>) {
    let (conn, scope) = seed_db(v);
    if let Some(mark) = v.get("prune_mark").and_then(|m| m.as_i64()) {
        conformance::set_prune_mark(&conn, &scope, mark);
    }
    let page = conformance::read_page_json(&conn, &scope, after_of(v));
    let expect = &v["expect"];

    if let Some(resync) = expect.get("resync_required").and_then(|b| b.as_bool()) {
        let got = page.get("resync_required").and_then(|b| b.as_bool());
        if got != Some(resync) {
            errs.push(format!("resync_required {got:?} != expected {resync}"));
        }
    }
    let entries = page.get("entries").and_then(|e| e.as_array()).cloned().unwrap_or_default();
    if let Some(n) = expect.get("entry_count").and_then(|x| x.as_i64()) {
        if entries.len() as i64 != n {
            errs.push(format!("entry_count {} != expected {n}", entries.len()));
        }
    }
}

fn check_prune_forces_resync(v: &Value, errs: &mut Vec<String>) {
    let (conn, scope) = seed_db(v);
    let expect = &v["expect"];

    // Before the retention pass the seeded delete is visible and carries its
    // tombstone op — this is the row a stale follower must never silently miss.
    let before = conformance::read_page_json(&conn, &scope, 0);
    if let Some(want_op) = expect.get("seed_op").and_then(|s| s.as_str()) {
        let got_op = before
            .get("entries")
            .and_then(|e| e.as_array())
            .and_then(|a| a.first())
            .and_then(|e| e.get("op"))
            .and_then(|o| o.as_str());
        if got_op != Some(want_op) {
            errs.push(format!("seed op {got_op:?} != expected {want_op:?}"));
        }
    }

    let keep_days = v
        .get("prune")
        .and_then(|p| p.get("keep_days"))
        .and_then(|k| k.as_i64())
        .unwrap_or(0);
    conformance::prune(&conn, keep_days);

    let after_page = conformance::read_page_json(&conn, &scope, after_of(v));
    if let Some(resync) = expect.get("resync_required").and_then(|b| b.as_bool()) {
        let got = after_page.get("resync_required").and_then(|b| b.as_bool());
        if got != Some(resync) {
            errs.push(format!("post-prune resync_required {got:?} != expected {resync}"));
        }
    }
}

fn check_features(v: &Value, errs: &mut Vec<String>) {
    let expect = &v["expect"];
    let mut got = conformance::advertised_features();
    got.sort();
    let mut want = expected_keys(expect, "features");
    want.sort();
    if got != want {
        errs.push(format!("advertised features {got:?} != expected {want:?}"));
    }
}

/// Assert the `GET /api/v1/cluster` payload shape (INV-12/INV-20): the exact key
/// set the client deserializes into `sync::ClusterInfo`, and the `features[]`
/// list it negotiates against. Drives [`conformance::sample_cluster_json`], the
/// same struct the hub serializes, so no socket is opened.
fn check_cluster_contract(v: &Value, errs: &mut Vec<String>) {
    let cluster = conformance::sample_cluster_json();
    let expect = &v["expect"];

    let got = key_set(&cluster);
    let want = expected_keys(expect, "keys");
    if got != want {
        errs.push(format!("cluster keys {got:?} != expected {want:?}"));
    }
    let mut got_features: Vec<String> = cluster
        .get("features")
        .and_then(|f| f.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    got_features.sort();
    let mut want_features = expected_keys(expect, "features");
    want_features.sort();
    if got_features != want_features {
        errs.push(format!("cluster features {got_features:?} != expected {want_features:?}"));
    }
}

fn run_vector(v: &Value) -> Vec<String> {
    let mut errs = Vec::new();
    match v.get("check").and_then(|c| c.as_str()).unwrap_or("") {
        "page_shape" => check_page_shape(v, &mut errs),
        "resync" => check_resync(v, &mut errs),
        "prune_forces_resync" => check_prune_forces_resync(v, &mut errs),
        "features" => check_features(v, &mut errs),
        "cluster_contract" => check_cluster_contract(v, &mut errs),
        other => errs.push(format!("unknown check {other:?}")),
    }
    errs
}

#[test]
fn sync_conformance_vectors() {
    let dir = vectors_dir();
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no vectors found in {}", dir.display());

    let mut failures: Vec<String> = Vec::new();
    for path in &paths {
        let name = vector_name(path);
        let raw = fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let v: Value = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{}: invalid JSON: {e}", path.display()));
        let errs = run_vector(&v);
        if errs.is_empty() {
            println!("[conformance] PASS {name}");
        } else {
            println!("[conformance] FAIL {name}");
            for e in &errs {
                failures.push(format!("{name}: {e}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} conformance vector(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn vector_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("vector")
        .to_string()
}
