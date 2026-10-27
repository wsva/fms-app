//! The agent mailbox — the shared ledger an app-side request and one or more
//! agents meet in. Design: `docs/design/agent/mailbox.md` (§4 is the schema,
//! §4.1 the lifecycle, §4.2 the derived folders, §5 the agent contract).
//!
//! One table in the app-level SQLite, beside `agent_todo` and the XP ledger.
//! The split is by dependency, and it is what keeps the machine testable:
//!
//! - [`machine`] — the transition table and nothing else. No SQL, no clock,
//!   no Tauri; it answers "may this party perform this move on this row?".
//! - [`store`] — the schema, the row, the derived `folder` / `owed_by` fold,
//!   and the two writes every other module goes through: `request` (T1 with
//!   its supersede preludes) and `promote` (T11/T12 on any read).
//! - [`claim`] — Phase 0 read, Phase 1 one short `BEGIN IMMEDIATE`, and the
//!   classification that joins them (§4.1). The dataset read it needs is
//!   passed in as a closure, so this module has no idea a dataset exists.
//! - [`db`] — the only edge: `SettingsState` → `Connection`, plus the one
//!   cross-database read the reconcile uses.
//! - [`guard`] — the same-statement guard a fulfilling write splices in, and
//!   the one-statement `ATTACH` that makes it possible (§5.1). Tauri-free on
//!   purpose, so its two-process test can run the real thing.
//! - [`commands`] — the dock's verbs. The three transitions that are the
//!   *user's* (Take over, cancel, reply) live here rather than in the MCP
//!   domain, and the dock publishes them as UI actions.
//!
//! Everything above `db` takes a `rusqlite::Connection` and an explicit `now`,
//! which is what lets the ledger be exercised against `:memory:` in tests
//! without a workspace, a clock or an app handle.
//!
//! The facade below is consumed by `mcp::mail`, which is desktop-only. A mobile
//! build compiles this tree but names none of it, so the re-exports would read
//! as unused.
#![allow(unused_imports)]

pub(crate) mod claim;
pub(crate) mod commands;
pub(crate) mod db;
pub(crate) mod guard;
pub(crate) mod machine;
pub(crate) mod store;

pub(crate) use claim::{claim, fail, release, ClaimRequest, Claimed, Reconciled};
pub(crate) use db::{open, path as db_path, reconcile_read};
pub(crate) use machine::{
    check, get, promotion, take_id, Actor, AttemptDelta, Facts, Guard, Illegal, MailState,
    Transition, CLAIM_PATH, TRANSITIONS,
};
pub(crate) use store::{
    abort_delivery, answer, answer_question, archive, begin_delivery, close_by_app, finish_delivery,
    head, list, mark_handed, mark_read, notice, open_for_ref, open_on_subject, open_on_target, promote,
    read_one, request, sanitise_label, subject_gone, target_uuid, user_cancel, user_takeover, Folder,
    Mail, MailHead, MailQuery, MailSummary, NewNotice, NewRequest, OwedBy, Refusal, RequestReceipt,
};

// ============================================================
// Vocabulary
// ============================================================

/// `agent_mail.status` values that get a name here. The seven live in
/// [`MailState`]; these aliases exist because the SQL in `store` and the
/// queries in `claim` bind strings, and a literal `'waiting_user'` in one
/// of them is a state the enum does not know about.
pub(crate) const QUEUED: &str = "queued";
pub(crate) const CLAIMED: &str = "claimed";
pub(crate) const DELIVERING: &str = "delivering";
pub(crate) const WAITING_USER: &str = "waiting_user";
pub(crate) const DELIVERED: &str = "delivered";
pub(crate) const CANCELLED: &str = "cancelled";
pub(crate) const FAILED: &str = "failed";

/// `agent_mail.direction`. A request goes app → agent; a notice or a question
/// goes agent → user.
pub(crate) const APP_TO_AGENT: &str = "app_to_agent";
pub(crate) const AGENT_TO_USER: &str = "agent_to_user";

/// `agent_mail.kind`. Only a `request` is claimable work (§4.1, the `next`
/// selection names both this and the direction).
pub(crate) const KIND_REQUEST: &str = "request";
pub(crate) const KIND_NOTICE: &str = "notice";
pub(crate) const KIND_QUESTION: &str = "question";

/// `agent_mail.origin`. Attribution for the user's eyes, never a boundary
/// (§10): `agent_direct` is a write that arrived with no mail behind it.
pub(crate) const ORIGIN_USER: &str = "user";
pub(crate) const ORIGIN_APP: &str = "app";
pub(crate) const ORIGIN_AGENT: &str = "agent";
pub(crate) const ORIGIN_AGENT_DIRECT: &str = "agent_direct";
pub(crate) const ORIGIN_WORKFLOW: &str = "workflow";

/// `agent_mail.reason` — the closed vocabulary §4 lists. A reason is written
/// by the move that ends the row, never inferred by a reader.
pub(crate) const REASON_UNCOLLECTED: &str = "uncollected";
pub(crate) const REASON_CAPACITY: &str = "capacity";
pub(crate) const REASON_CLAIM_MISMATCH: &str = "claim_mismatch";
pub(crate) const REASON_LATE_REFUSED: &str = "late_refused";
pub(crate) const REASON_ORPHAN_REFUSED: &str = "orphan_refused";
pub(crate) const REASON_CLAIM_REQUIRED: &str = "claim_required";
pub(crate) const REASON_SUBJECT_GONE: &str = "subject_gone";
pub(crate) const REASON_REFUSED: &str = "refused";
pub(crate) const REASON_ANSWERED_LATE: &str = "answered_late";
pub(crate) const REASON_UNANSWERED: &str = "unanswered";
pub(crate) const REASON_SUPERSEDED: &str = "superseded";
pub(crate) const REASON_ATTEMPTS_EXHAUSTED: &str = "attempts_exhausted";
/// Not stored: the journal's word for "the clock ended this lease" (§4.1).
pub(crate) const REASON_LEASE_EXPIRED: &str = "lease_expired";

// ============================================================
// Numbers
// ============================================================

/// The work lease: `expires_at = now + 10 min` on a claim (T3/T4).
pub(crate) const LEASE_MS: i64 = 600_000;
/// The write lease: 2 minutes while `delivering`, because what remains is one
/// row write (T5).
pub(crate) const WRITE_LEASE_MS: i64 = 120_000;
/// A question's answer deadline. Not a work lease — nothing is held while
/// waiting (T9).
pub(crate) const ANSWER_DEADLINE_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// `attempts >= cap` ends the row (T12).
pub(crate) const ATTEMPT_CAP: i64 = 3;
/// Touch-on-use is hard-capped at this many leases from the claim, so a loop
/// that keeps citing a token cannot hold an item forever (§4.1 step 4).
pub(crate) const HOLD_CAP_LEASES: i64 = 3;
/// Per-resource capacity, as the two-column `VALUES` list §4.1 binds into
/// every counting statement. A resource absent from this list is uncapped —
/// which is how `llm` stays free while `stt` is one.
pub(crate) const CAPACITY: &[(&str, i64)] = &[("stt", 1), ("tts", 1)];
/// `claimed_by` is untrusted text shown to the user *and* echoed to other
/// agents in `claim_required`, so it is sanitised at ingestion (§10).
pub(crate) const MAX_LABEL: usize = 64;

/// Wall-clock milliseconds, the unit every time column uses (§4).
pub(crate) fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
