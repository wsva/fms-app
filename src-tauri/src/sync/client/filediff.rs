//! Pure diff planner for the incremental changed-file *download* (sync-v2.md §7
//! consumption half). The round reads the hub manifest's per-file hashes
//! (`GET /api/v1/datasets/{uuid}/manifest` → `files: { rel_path: sha256 }`), lists
//! the local live tree with the *same* recipe (via
//! [`crate::sync::blobpush::content_tree_hashes`]), and asks this module which
//! files to fetch and which to delete. Keeping the comparison here — no I/O, no
//! awaiting — makes the whole decision unit-testable; the caller does the fetch,
//! verify, and atomic write.
//!
//! DB files are already absent from both maps (the manifest hash and
//! `content_tree_hashes` each exclude them — the row journal owns the DB), so a
//! plan never names one. This is the live path only: a read-only registry
//! adoption keeps the whole-archive snapshot and never reaches these functions.

use std::collections::BTreeMap;

use serde_json::Value;

/// What one dataset's file sync must do to converge its local tree on the hub's:
/// re-fetch `fetch` (new or content-changed), remove `delete` (gone upstream).
/// Both are sorted so the operation order is deterministic across runs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct FilePlan {
    pub fetch: Vec<String>,
    pub delete: Vec<String>,
}

impl FilePlan {
    pub fn is_empty(&self) -> bool {
        self.fetch.is_empty() && self.delete.is_empty()
    }
}

/// Diff a local tree against the hub's: a hub path whose recorded sha differs
/// from the local one (or is missing locally) is fetched; a local path the hub no
/// longer lists is deleted. Unchanged paths appear in neither list.
pub(crate) fn plan(local: &BTreeMap<String, String>, hub: &BTreeMap<String, String>) -> FilePlan {
    let mut fetch = Vec::new();
    for (path, sha) in hub {
        match local.get(path) {
            Some(have) if have == sha => {}
            _ => fetch.push(path.clone()),
        }
    }
    let mut delete: Vec<String> = local
        .keys()
        .filter(|p| !hub.contains_key(*p))
        .cloned()
        .collect();
    fetch.sort();
    delete.sort();
    FilePlan { fetch, delete }
}

/// Read a manifest `files` object (`{ path: sha }`) into a map, ignoring any
/// non-string value. A registry manifest (`files: {}`) or a pre-per-file hub
/// yields an empty map, which the caller treats as "cannot diff — fall back to a
/// whole-archive snapshot."
pub(crate) fn parse_files(v: &Value) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if let Some(obj) = v.as_object() {
        for (k, val) in obj {
            if let Some(s) = val.as_str() {
                map.insert(k.clone(), s.to_string());
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn fetches_new_and_modified_and_skips_equal() {
        let local = map(&[("a.mp3", "111"), ("same.vtt", "aaa"), ("gone.txt", "zzz")]);
        let hub = map(&[("a.mp3", "222"), ("same.vtt", "aaa"), ("new.txt", "nnn")]);
        let p = plan(&local, &hub);
        assert_eq!(p.fetch, vec!["a.mp3", "new.txt"]); // modified + added
        assert_eq!(p.delete, vec!["gone.txt"]); // removed upstream
    }

    #[test]
    fn identical_trees_produce_an_empty_plan() {
        let t = map(&[("a", "1"), ("b/c", "2")]);
        let p = plan(&t, &t);
        assert!(p.is_empty());
    }

    #[test]
    fn empty_hub_deletes_everything_locally_but_fetches_nothing() {
        // Only reached defensively; the caller snapshots when the hub map is empty.
        let local = map(&[("a", "1")]);
        let p = plan(&local, &BTreeMap::new());
        assert_eq!(p.delete, vec!["a"]);
        assert!(p.fetch.is_empty());
    }

    #[test]
    fn parse_files_coerces_and_drops_non_strings() {
        let v = serde_json::json!({ "a.mp3": "111", "n": 5, "obj": {}, "z.vtt": "zzz" });
        let m = parse_files(&v);
        assert_eq!(m, map(&[("a.mp3", "111"), ("z.vtt", "zzz")]));
    }

    #[test]
    fn parse_files_empty_object_is_empty_map() {
        let v = serde_json::json!({});
        assert!(parse_files(&v).is_empty());
    }
}
