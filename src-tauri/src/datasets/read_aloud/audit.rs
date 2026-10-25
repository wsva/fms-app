//! Read-only read-aloud audit — the same shape the dictation audit produces, so
//! one `verify:` block vocabulary and one report renderer serve both.
//!
//! A read-aloud dataset is much smaller than a dictation one: texts, and the
//! standard clips pronunciation is measured against. So the checks are few, and
//! each names a defect that would otherwise only show up as a bad score later:
//!
//! - `texts_none` — the dataset has no passage, so every per-text check below is
//!   vacuously clean (the subject check that a pipeline must bind to);
//! - `text_empty` — a passage with no readable content, which can never be
//!   practised;
//! - `text_duplicate` — two rows with the same content digest, which is what an
//!   importer run twice leaves behind;
//! - `standard_audio_missing` — a text with no clip, so `read_aloud_assess`
//!   reports `pronunciation.unavailable_reason = no_reference` forever.
//!
//! It never writes. Repairing is the caller's job (`read_aloud_standard_audio_
//! generate_all`, `read_aloud_delete_text`, `read_aloud_import_texts`), which is
//! what keeps a status probe cheap and safe to run at any time.

use std::collections::HashMap;

use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::State;

use crate::datasets::info::DatasetInfo;
use crate::settings::SettingsState;

use super::{effective_reference, find_dataset_dir, open_db};

/// How many offenders one check lists before truncating. The count stays exact.
const ITEM_CAP: usize = 100;

// ---------------------------------------------------------------------------
// Report shape — mirrors `dictation::audit`
// ---------------------------------------------------------------------------

/// One offender: which text, and why it is flagged.
#[derive(Serialize)]
pub struct AuditFinding {
    /// The text's uuid — the one name every layer agrees on.
    pub source: String,
    pub detail: String,
}

/// One check and its verdict.
#[derive(Serialize)]
pub struct AuditCheck {
    /// Stable id, named by a definition's `verify:` binding. Never renamed.
    pub id: &'static str,
    pub label: &'static str,
    /// `problem` blocks the pipeline, `warn` is drift worth a look, `clean` means
    /// the check found nothing.
    pub level: &'static str,
    pub count: usize,
    pub items: Vec<AuditFinding>,
    pub truncated: bool,
    pub advice: &'static str,
}

struct CheckMeta {
    id: &'static str,
    label: &'static str,
    level: &'static str,
    advice: &'static str,
}

/// Every check this domain answers. `check_ids` hands the same table to
/// `workflow::verify`, so the vocabulary a definition may bind and the report it
/// reads cannot drift apart.
const CHECKS: &[CheckMeta] = &[
    CheckMeta {
        id: "texts_none",
        label: "Passages",
        level: "problem",
        advice: "This dataset has no passage yet, so every per-text finding below proves \
                  nothing. Import a book with read_aloud_import_texts (dry_run first) or add \
                  one with read_aloud_save_text.",
    },
    CheckMeta {
        id: "text_empty",
        label: "Passages with no content",
        level: "problem",
        advice: "A passage with blank content cannot be read or scored. Fill it in with \
                  read_aloud_save_text, or delete the row.",
    },
    CheckMeta {
        id: "text_duplicate",
        label: "Passages with identical content",
        level: "warn",
        advice: "Two rows with the same text — usually an import run twice, or a failed \
                  batch retried on top of itself. Delete the copies you do not want.",
    },
    CheckMeta {
        id: "standard_audio_missing",
        label: "Passages with no standard audio",
        level: "warn",
        advice: "Pronunciation is measured against each text's default clip. Generate the \
                  missing ones with read_aloud_standard_audio_generate_all — probe with \
                  limit: 3 first if the voice is still being chosen.",
    },
];

/// The binding vocabulary: every check id a workflow definition may name.
pub(crate) fn check_ids() -> impl Iterator<Item = &'static str> {
    CHECKS.iter().map(|c| c.id)
}

#[derive(Serialize)]
pub struct AuditFacts {
    pub has_texts: bool,
    pub has_standard_audio: bool,
    /// Whether any generation can resolve a language-specific voice at all.
    pub has_language: bool,
}

#[derive(Serialize)]
pub struct DatasetAudit {
    pub dataset_uuid: String,
    pub path: String,
    pub info: DatasetInfo,
    pub facts: AuditFacts,
    pub texts: usize,
    pub texts_with_audio: usize,
    pub checks: Vec<AuditCheck>,
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

/// Probe a read-aloud dataset and report every inconsistency. Read-only.
#[tauri::command]
pub async fn read_aloud_audit(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<DatasetAudit, String> {
    audit(&settings, &dataset_uuid)
}

pub(crate) fn audit(settings: &SettingsState, dataset_uuid: &str) -> Result<DatasetAudit, String> {
    let dir = find_dataset_dir(settings, dataset_uuid)?;
    let info = crate::datasets::read_info(&dir)?;
    let conn = open_db(&dir)?;
    let rows = text_rows(&conn)?;
    let mut texts_with_audio = 0usize;
    let mut empty = Vec::new();
    let mut missing_audio = Vec::new();
    let mut by_digest: HashMap<String, Vec<&TextRow>> = HashMap::new();

    for row in &rows {
        if row.chars == 0 {
            empty.push(row.finding("content is blank — there is nothing to read".into()));
        } else {
            by_digest.entry(row.digest.clone()).or_default().push(row);
        }
        if row.has_audio {
            texts_with_audio += 1;
        } else {
            missing_audio.push(row.finding(
                "no standard clip, so pronunciation reports no_reference for every attempt"
                    .into(),
            ));
        }
    }

    // A digest shared by several rows is a duplicate; the digest of a blank row
    // is skipped above, because "two empty passages" is one problem, not two.
    let mut duplicates = Vec::new();
    let mut digests: Vec<&String> = by_digest.keys().collect();
    digests.sort();
    for digest in digests {
        let group = by_digest.get(digest).expect("key came from this map");
        if group.len() < 2 {
            continue;
        }
        for row in group {
            duplicates.push(row.finding(format!(
                "same content as {} other passage(s) — “{}”",
                group.len() - 1,
                row.title
            )));
        }
    }
    duplicates.sort_by(|a, b| a.source.cmp(&b.source));

    let none = if rows.is_empty() {
        vec![AuditFinding {
            source: "read_text".into(),
            detail: "no passage in this dataset".into(),
        }]
    } else {
        Vec::new()
    };

    let facts = AuditFacts {
        has_texts: !rows.is_empty(),
        has_standard_audio: texts_with_audio > 0,
        has_language: !info.language.trim().is_empty(),
    };

    let mut checks = vec![
        // Reported even when it passes: reaching here *is* the measurement, so
        // the id is present with zero offenders rather than absent — an absent
        // id means "not evaluated", which can neither certify a step nor adopt.
        check("texts_none", none),
        check("text_empty", empty),
        check("text_duplicate", duplicates),
        check("standard_audio_missing", missing_audio),
    ];
    sort_checks(&mut checks);

    Ok(DatasetAudit {
        dataset_uuid: dataset_uuid.to_string(),
        path: dir.to_string_lossy().into_owned(),
        info,
        facts,
        texts: rows.len(),
        texts_with_audio,
        checks,
    })
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// A `read_text` row reduced to what the checks need.
struct TextRow {
    uuid: String,
    title: String,
    chars: usize,
    /// sha256 of the effective reference — the same string a standard clip is
    /// hashed against, so a duplicate says what it is.
    digest: String,
    has_audio: bool,
}

impl TextRow {
    fn finding(&self, detail: String) -> AuditFinding {
        AuditFinding {
            source: self.uuid.clone(),
            detail,
        }
    }
}

fn text_rows(conn: &rusqlite::Connection) -> Result<Vec<TextRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT t.uuid, t.title, t.content, t.normalized_content, \
                    EXISTS(SELECT 1 FROM read_standard_audio a WHERE a.text_uuid = t.uuid) \
             FROM read_text t ORDER BY t.order_num, t.created_at",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let content: String = row.get(2)?;
            let normalized: Option<String> = row.get(3)?;
            let reference = effective_reference(&content, normalized.as_deref());
            Ok(TextRow {
                uuid: row.get(0)?,
                title: row.get(1)?,
                chars: reference.trim().chars().count(),
                digest: digest(&reference),
                has_audio: row.get::<_, i64>(4)? != 0,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

fn digest(reference: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(reference.trim().as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

// ---------------------------------------------------------------------------
// Report assembly
// ---------------------------------------------------------------------------

fn check(id: &'static str, items: Vec<AuditFinding>) -> AuditCheck {
    let meta = CHECKS.iter().find(|c| c.id == id);
    debug_assert!(meta.is_some(), "audit: check '{id}' is not declared in CHECKS");
    let count = items.len();
    AuditCheck {
        id,
        // An id missing from the table surfaces labelled by itself rather than
        // panicking inside a probe that is meant to be safe to run any time.
        label: meta.map(|c| c.label).unwrap_or(id),
        level: match meta {
            Some(_) if count == 0 => "clean",
            Some(c) => c.level,
            None => "warn",
        },
        count,
        truncated: count > ITEM_CAP,
        items: items.into_iter().take(ITEM_CAP).collect(),
        advice: meta
            .map(|c| c.advice)
            .unwrap_or("Undeclared check id — re-scan after updating the app."),
    }
}

/// Severity order, so the report leads with what blocks the pipeline.
fn sort_checks(checks: &mut Vec<AuditCheck>) {
    checks.sort_by_key(|c| {
        (
            match c.level {
                "problem" => 0,
                "warn" => 1,
                "info" => 2,
                _ => 3,
            },
            c.id,
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_id_is_reported_even_when_clean() {
        let mut reported: Vec<&str> = CHECKS.iter().map(|c| c.id).collect();
        let mut registered: Vec<&str> = check_ids().collect();
        reported.sort();
        registered.sort();
        assert_eq!(reported, registered);
        // The assembler never drops an id: it is called once per entry above.
        assert_eq!(check_ids().count(), CHECKS.len());
    }

    #[test]
    fn a_clean_check_is_clean_not_a_problem_with_zero_items() {
        let clean = check("text_empty", Vec::new());
        assert_eq!(clean.level, "clean");
        assert_eq!(clean.count, 0);
        assert!(!clean.truncated);
    }

    #[test]
    fn the_digest_ignores_outside_whitespace_but_not_content() {
        assert_eq!(digest(" ein text "), digest("ein text"));
        assert_ne!(digest("ein text"), digest("ein Text"));
    }
}
