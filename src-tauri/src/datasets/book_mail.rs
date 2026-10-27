//! The book artifacts' bridge to the agent mailbox.
//!
//! This is §5.1 of `docs/design/agent/mailbox.md`: the *source* side of the
//! ledger. The mailbox owns the request; this module owns the artifact row the
//! request is fulfilled by, and the two are joined by one guarded statement.
//!
//! Three properties live here rather than in the ledger, because they are
//! properties of the artifact table:
//!
//! - **The target is derived, never chosen.** An artifact always lands at
//!   `target_uuid(subject_key)`, so a delivery cannot overwrite another
//!   subject's result and a re-delivery is a no-op rather than a second copy.
//! - **The guard is inside the insert.** `INSERT … SELECT … WHERE EXISTS` reads
//!   the claim from the app database *attached to the artifact's own
//!   connection*, so a delivery whose claim moved writes nothing at all
//!   (§5.1). A `WHERE` after `ON CONFLICT` would filter only the update branch
//!   and let a first delivery through unguarded — the zombie case.
//! - **Provenance is stamped at write time.** `mail_uuid`, `delivered_by` and
//!   `source_hash` are columns on `book_artifact`, so "who wrote this, when,
//!   with which model" survives the mail row being archived.
//!
//! `book_artifact_job` is no longer written or read: its rows were requests
//! without a lifecycle, and the ledger replaces them. The table is left in the
//! schema so an older dataset still opens; pending rows in it are dropped
//! because the panel re-requests from the ledger and a stale row there has no
//! claim to reconcile against.

use std::path::Path;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::{json, Value};

use super::book::{find_book_dir, find_book_dir_write, open_book_db};
use super::book_artifacts::{
    hash_text, save_artifact_row, source_text, ArtifactJob, BookArtifact, GENERATED_KINDS,
};
use super::row_stamp_at;
use crate::mail::{self, ClaimRequest, Mail, NewRequest, Refusal};
use crate::settings::SettingsState;

// ============================================================
// Identity
// ============================================================

/// The room: one scope of one book. Everything asked for about the same
/// paragraph shares it, so the dock groups an explanation and a digest of the
/// same text under one channel.
pub(crate) fn channel_id(book_uuid: &str, scope_uuid: &str) -> String {
    format!("book:{book_uuid}:{scope_uuid}")
}

/// The thing produced. `target_uuid` is derived from this, which is why the
/// kind has to be part of it and the scope alone is not enough.
pub(crate) fn subject_key(book_uuid: &str, scope_uuid: &str, kind: &str) -> String {
    format!("book:{book_uuid}:{scope_uuid}:{kind}")
}

/// The uuid an artifact for this subject must carry. Callers never pass one:
/// the identity is a function of the subject, so two writers cannot produce two
/// copies and a re-delivery is an update rather than an insert.
pub(crate) fn target_for(book_uuid: &str, scope_uuid: &str, kind: &str) -> String {
    mail::target_uuid(&subject_key(book_uuid, scope_uuid, kind))
}

fn subject_label(kind: &str, scope_uuid: &str) -> String {
    let short: String = scope_uuid.chars().take(8).collect();
    format!("{kind} · {short}")
}

/// Epoch milliseconds as the row stamp the rest of the artifact table uses.
fn stamp(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(row_stamp_at)
        .unwrap_or_default()
}

/// One shape for "the ledger said no", so a refusal reaches the panel and the
/// agent with both halves: the code to branch on and the sentence to act on.
fn refusal_text(r: Refusal) -> String {
    match r.successor {
        Some(successor) => format!(
            "{}: {} (it was replaced by request {successor})",
            r.code, r.message
        ),
        None => format!("{}: {}", r.code, r.message),
    }
}

// ============================================================
// The payload: what the agent needs, resolved at read time
// ============================================================

/// A request's parameters, as written into `payload_json`. The source text is
/// **not** copied in: it is resolved when the panel or an agent reads the
/// request, so a later edit is visible instead of frozen into the mail.
#[derive(Debug, Clone, Default)]
struct JobSpec {
    scope_type: String,
    scope_uuid: String,
    kind: String,
    instructions: String,
    model: Option<String>,
    /// The source text's hash at request time, so "stale" stays decidable even
    /// when the request predates any claim.
    content_hash: String,
}

fn spec_of(mail: &Mail) -> JobSpec {
    let value: Value = serde_json::from_str(&mail.payload_json).unwrap_or(Value::Null);
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    JobSpec {
        scope_type: text("scope_type"),
        scope_uuid: text("scope_uuid"),
        kind: text("kind"),
        instructions: text("instructions"),
        model: value
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        content_hash: text("content_hash"),
    }
}

/// A mail row as the Understand panel's job list shows it — the shape that
/// predates the ledger, so an attached agent sees the same fields it always did.
fn job_of(book: &Connection, m: &Mail) -> ArtifactJob {
    let spec = spec_of(m);
    let text = source_text(book, &spec.scope_type, &spec.scope_uuid).unwrap_or_default();
    let stale = !spec.content_hash.is_empty()
        && !text.is_empty()
        && hash_text(&text) != spec.content_hash;
    ArtifactJob {
        uuid: m.uuid.clone(),
        book_uuid: m.ref_id.clone().unwrap_or_default(),
        scope_type: spec.scope_type,
        scope_uuid: spec.scope_uuid,
        kind: spec.kind,
        instructions: spec.instructions,
        model: spec.model,
        status: m.status.clone(),
        created_at: stamp(m.created_at),
        updated_at: stamp(m.updated_at),
        source_text: text,
        stale,
    }
}

// ============================================================
// Requests
// ============================================================

/// Ask for an artifact to be generated by an attached agent (T1).
///
/// Idempotent per `(scope, kind)`: an open request for the same thing is
/// returned rather than duplicated, and a *different* request on the same
/// subject supersedes an untaken one (§2.2). A row a live agent is holding is
/// refused with `claim_required`, which is the panel's cue to offer *Ask again*
/// rather than silently queueing a second ask.
pub(crate) fn request_artifact(
    settings: &SettingsState,
    book_uuid: &str,
    scope_type: &str,
    scope_uuid: &str,
    kind: &str,
    instructions: Option<String>,
    model: Option<String>,
) -> Result<ArtifactJob, String> {
    if !GENERATED_KINDS.contains(&kind) {
        return Err(format!(
            "kind '{kind}' cannot be requested from an agent. Valid kinds: {}",
            GENERATED_KINDS.join(", ")
        ));
    }
    let dir = find_book_dir_write(settings, book_uuid)?;
    let book = open_book_db(&dir)?;
    let text = source_text(&book, scope_type, scope_uuid)?;
    if text.trim().is_empty() {
        return Err("this scope has no text to generate from".into());
    }

    let payload = json!({
        "book_uuid": book_uuid,
        "scope_type": scope_type,
        "scope_uuid": scope_uuid,
        "kind": kind,
        "instructions": instructions.clone().unwrap_or_default(),
        "model": model,
        "content_hash": hash_text(&text),
    })
    .to_string();
    let channel = channel_id(book_uuid, scope_uuid);
    let subject = subject_key(book_uuid, scope_uuid, kind);
    let label = subject_label(kind, scope_uuid);
    let body = format!(
        "Generate a {kind} for this {scope_type} of the open book. The text is in the \
         request payload's `data` envelope once you claim it; deliver it with \
         book_save_artifact, citing job_uuid and claim_id."
    );

    let mut conn = mail::open(settings)?;
    let request = NewRequest::new(&channel, &subject, &label)
        .body(&body)
        .payload(&payload)
        .resource(Some("llm"))
        .refer(Some("book"), Some(book_uuid))
        .origin(mail::ORIGIN_USER);
    let receipt = mail::request(&mut conn, mail::now_ms(), &request).map_err(refusal_text)?;
    Ok(job_of(&book, &receipt.mail))
}

/// The book's outstanding requests, oldest first — the panel's list and the
/// agent's `book_list_artifact_jobs`. Terminal rows are excluded, so a
/// delivered or cancelled request stops offering itself (§12 step 3).
pub(crate) fn list_jobs(
    settings: &SettingsState,
    book_uuid: &str,
    status: Option<String>,
) -> Result<Vec<ArtifactJob>, String> {
    let dir = find_book_dir(settings, book_uuid)?;
    let book = open_book_db(&dir)?;
    let conn = mail::open(settings)?;
    // "pending" is the word the pre-ledger table used; accepting it keeps an
    // agent that filtered on it working.
    let filter = status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| if s == "pending" { "queued".to_string() } else { s.to_string() });
    let rows = mail::open_for_ref(&conn, "book", book_uuid, mail::now_ms())
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .filter(|m| filter.as_deref().map(|f| m.status == f).unwrap_or(true))
        .map(|m| job_of(&book, &m))
        .collect())
}

/// The user changes their mind (T15). A delivered artifact is untouched: the
/// request is cancelled, the result stays.
pub(crate) fn cancel_job(settings: &SettingsState, book_uuid: &str, uuid: &str) -> Result<(), String> {
    let conn = mail::open(settings)?;
    let row = mail::read_one(&conn, uuid)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no request {uuid}"))?;
    if row.ref_kind.as_deref() != Some("book") || row.ref_id.as_deref() != Some(book_uuid) {
        return Err(format!("request {uuid} does not belong to book {book_uuid}"));
    }
    mail::user_cancel(&conn, uuid, mail::REASON_REFUSED, mail::now_ms())
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ============================================================
// The fulfilling write
// ============================================================

/// What a delivery produced.
#[derive(Debug, Clone, Serialize)]
pub struct Delivery {
    pub artifact_uuid: String,
    pub mail_uuid: Option<String>,
    /// `delivered` — the row closed on this claim.
    /// `written_unconfirmed` — the artifact landed but the row was displaced
    /// inside the same statement, so it did not close (§5.1's residual).
    /// `already_delivered` — a retried or zombie delivery; nothing written.
    /// `unsolicited` — no request existed, so the artifact is recorded without
    /// a link to mail.
    pub outcome: &'static str,
}

/// Write an artifact and close the request it answers, in that order and with
/// the row's claim protecting the write.
///
/// The three cases §5.1 names, in one function because they differ only in
/// which claim the write is guarded by:
///
/// 1. **`job_uuid` + `claim_id`** — the agent holds the row: T5, guarded write, T7.
/// 2. **`job_uuid` alone** — an unannotated delivery. If the row is free it is
///    claimed exactly as an agent would (capacity, attempt charge), and the
///    artifact's vintage is *unknown* rather than fresh. If the row is held by
///    a live lease, the call is refused with `claim_required` — a fallback that
///    correlated against a live claim would hand the zombie its bypass.
/// 3. **no `job_uuid`** — no request at all: a legitimate unsolicited artifact,
///    recorded and never linked.
pub(crate) fn deliver(
    settings: &SettingsState,
    book_uuid: &str,
    mut artifact: BookArtifact,
    job_uuid: Option<&str>,
    claim_id: Option<&str>,
    who: &str,
) -> Result<Delivery, String> {
    let dir = find_book_dir_write(settings, book_uuid)?;
    let book = open_book_db(&dir)?;
    // The guarded statement waits on the app DB's writer rather than failing:
    // the same 30 s courtesy the app's own openers extend.
    let _ = book.busy_timeout(Duration::from_secs(10));
    let mut app = mail::open(settings)?;
    let now = mail::now_ms();

    let subject = subject_key(book_uuid, &artifact.scope_uuid, &artifact.kind);
    let target = mail::target_uuid(&subject);
    // The caller cannot choose where the artifact lands (§5.1).
    artifact.uuid = target.clone();
    artifact.book_uuid = book_uuid.to_string();
    if artifact.created_at.trim().is_empty() {
        artifact.created_at = super::now_stamp();
    }
    artifact.updated_at = super::now_stamp();

    let row: Option<Mail> = match job_uuid {
        Some(uuid) => Some(
            mail::read_one(&app, uuid)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("unknown: no request {uuid}"))?,
        ),
        None => mail::open_on_target(&app, &target).map_err(|e| e.to_string())?,
    };

    let Some(row) = row else {
        // Case 3: nothing asked for this. Still stamped, still visible.
        artifact.mail_uuid = None;
        artifact.delivered_by = Some(who.to_string());
        artifact.source_hash = None;
        save_artifact_row(&book, &artifact)?;
        journal(settings, book_uuid, &artifact);
        return Ok(Delivery {
            artifact_uuid: target,
            mail_uuid: None,
            outcome: "unsolicited",
        });
    };

    match row.status.as_str() {
        mail::DELIVERED => {
            return Ok(Delivery {
                artifact_uuid: row.result_ref.clone().unwrap_or(target),
                mail_uuid: Some(row.uuid),
                outcome: "already_delivered",
            });
        }
        mail::CANCELLED | mail::FAILED => {
            return Err(format!(
                "orphan_refused: request {} is {} ({}). A cancel means cancel — ask again \
                 if the artifact is still wanted.",
                row.uuid,
                row.status,
                row.reason.as_deref().unwrap_or("no reason recorded")
            ));
        }
        _ => {}
    }

    // Which claim guards the write, and how fresh is the text behind it.
    let mut adopted = false;
    let claim = match claim_id {
        Some(id) => id.to_string(),
        None => {
            let live = matches!(row.status.as_str(), mail::CLAIMED | mail::DELIVERING)
                && row.expires_at.map(|e| e > now).unwrap_or(false);
            if live {
                return Err(format!(
                    "claim_required: request {} is being worked on by {}. Claim it with \
                     mail_claim {{mail_uuid, claim_id}} and deliver citing both, or wait for it.",
                    row.uuid,
                    row.claimed_by.as_deref().unwrap_or("another agent")
                ));
            }
            // Adoption is a real claim: same capacity count, same attempt charge.
            let request = ClaimRequest::new(who).item(&row.uuid);
            match mail::claim(&mut app, now, &request, |_| mail::Reconciled::Absent) {
                Ok(held) => {
                    adopted = true;
                    held.mail.claim_id.unwrap_or_default()
                }
                Err(r) if r.code == "reconciled" => {
                    let closed = mail::read_one(&app, &row.uuid)
                        .map_err(|e| e.to_string())?
                        .and_then(|m| m.result_ref);
                    return Ok(Delivery {
                        artifact_uuid: closed.unwrap_or(target),
                        mail_uuid: Some(row.uuid),
                        outcome: "already_delivered",
                    });
                }
                Err(r) => return Err(refusal_text(r)),
            }
        }
    };
    if claim.is_empty() {
        return Err("late_refused: the request has no claim to deliver against".into());
    }

    let spec = spec_of(&row);
    artifact.mail_uuid = Some(row.uuid.clone());
    artifact.delivered_by = Some(who.to_string());
    // An adopted delivery has no claim-time hash: the agent may have generated
    // from text it fetched long before it called, so `NULL` (unknown vintage)
    // is the only honest stamp. It reads as possibly stale, not as fresh.
    artifact.source_hash = if adopted {
        None
    } else {
        row.source_hash
            .clone()
            .filter(|h| !h.is_empty())
            .or_else(|| Some(spec.content_hash.clone()).filter(|h| !h.is_empty()))
    };

    // T5: freeze the row into the two-minute write lease.
    let began = mail::begin_delivery(&app, &row.uuid, &claim, now).map_err(|e| e.to_string())?;
    if began == 0 {
        return Err(format!(
            "late_refused: request {} no longer names this claim (it moved to {}. Not written.",
            row.uuid,
            mail::read_one(&app, &row.uuid)
                .ok()
                .flatten()
                .and_then(|m| m.claimed_by)
                .unwrap_or_else(|| "another agent".to_string())
        ));
    }

    let app_path = mail::db_path(settings)?;
    let written = match guarded_write(&book, &app_path, &artifact, &row.uuid, &claim) {
        Ok(n) => n,
        Err(e) => {
            // The write itself failed: roll back to `claimed` so the holder can
            // retry in place rather than waiting out the lease (T8).
            let _ = mail::abort_delivery(&app, &row.uuid, &claim, now);
            return Err(e);
        }
    };
    if written == 0 {
        let _ = mail::abort_delivery(&app, &row.uuid, &claim, now);
        return Err(format!(
            "late_refused: the claim on request {} moved before the write landed, so nothing \
             was written. Claim again for the current state.",
            row.uuid
        ));
    }

    let closed = mail::finish_delivery(&app, &row.uuid, &claim, &target, now)
        .map_err(|e| e.to_string())?;
    journal(settings, book_uuid, &artifact);
    Ok(Delivery {
        artifact_uuid: target,
        mail_uuid: Some(row.uuid),
        outcome: if closed == 1 { "delivered" } else { "written_unconfirmed" },
    })
}

/// The one statement that makes first-write-wins true.
///
/// The app database is attached **for this statement only** — ordinary dataset
/// work keeps a single-database connection, and the guard's read is over in the
/// same order of magnitude as the insert it protects.
fn guarded_write(
    book: &Connection,
    app_path: &Path,
    a: &BookArtifact,
    mail_uuid: &str,
    claim_id: &str,
) -> Result<usize, String> {
    // `?11` is the mail uuid and `?15` is the claim; `SELECT` rather than
    // `VALUES` is what lets the guard sit between the rows and the conflict
    // clause, so a displaced writer inserts nothing at all instead of
    // overwriting the winner's row.
    let sql = format!(
        "INSERT INTO book_artifact \
             (uuid, book_uuid, scope_type, scope_uuid, kind, content, source, model, \
              parent_uuid, content_hash, mail_uuid, delivered_by, source_hash, \
              created_at, updated_at) \
         SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14 \
          WHERE {} \
         ON CONFLICT(uuid) DO UPDATE SET \
             content = excluded.content, content_hash = excluded.content_hash, \
             source = excluded.source, model = excluded.model, \
             mail_uuid = excluded.mail_uuid, delivered_by = excluded.delivered_by, \
             source_hash = excluded.source_hash, updated_at = excluded.updated_at",
        mail::guard::claim_guard(11, 15)
    );
    let params: Vec<&dyn rusqlite::ToSql> = vec![
        &a.uuid,
        &a.book_uuid,
        &a.scope_type,
        &a.scope_uuid,
        &a.kind,
        &a.content,
        &a.source,
        &a.model,
        &a.parent_uuid,
        &a.content_hash,
        &mail_uuid,
        &a.delivered_by,
        &a.source_hash,
        &a.updated_at,
        &claim_id,
    ];
    mail::guard::write(book, app_path, &sql, &params)
}

/// Close whatever request this write satisfied, when the app wrote it rather
/// than an agent: the built-in generator, or an artifact pulled from another
/// device. A live claim is left alone — its holder's guarded write closes it.
pub(crate) fn note_local_write(
    settings: &SettingsState,
    book_uuid: &str,
    artifact: &BookArtifact,
    by: &str,
) {
    let subject = subject_key(book_uuid, &artifact.scope_uuid, &artifact.kind);
    let target = if artifact.uuid.trim().is_empty() {
        mail::target_uuid(&subject)
    } else {
        artifact.uuid.clone()
    };
    let Ok(conn) = mail::open(settings) else {
        return;
    };
    if let Ok(Some(row)) = mail::open_on_target(&conn, &target) {
        let _ = mail::close_by_app(&conn, &row.uuid, by, &target, mail::now_ms());
    }
}

fn journal(settings: &SettingsState, book_uuid: &str, artifact: &BookArtifact) {
    let _ = crate::sync::change_log::commit_change(
        settings,
        "book_artifact_save",
        book_uuid,
        &serde_json::to_value(artifact).unwrap_or_default(),
    );
}
