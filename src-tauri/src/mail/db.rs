//! The mailbox's two edges to the rest of the app: where the app-level database
//! lives, the cross-database read the reconcile needs, and the subject a claim
//! hands its holder.
//!
//! Everything else in this module tree takes a `Connection` and a `now`, which
//! is what keeps the ledger testable against `:memory:`. This file is the
//! exception on purpose — it is the whole of what a claim knows about books,
//! and the ledger itself names no dataset.

use rusqlite::Connection;

use crate::settings::SettingsState;

use super::store::{self, Mail};
use super::Reconciled;

/// Where the app-level database lives: the workspace's own file when one is
/// selected, the app data root otherwise. Both are the same file the todo and
/// XP ledgers use.
pub fn path(settings: &SettingsState) -> Result<std::path::PathBuf, String> {
    let dir = match settings.workspace_dir.lock().unwrap().as_ref() {
        Some(ws_dir) => ws_dir.clone(),
        None => crate::app_paths::data_root(),
    };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("app.sqlite3"))
}

/// Open the app-level SQLite (`{workspace_dir}/app.sqlite3`) and make sure the
/// mailbox schema exists.
///
/// Mirrors [`crate::todo`]'s opener rather than reusing a dictation helper:
/// the mailbox compiles on the mobile thin client, where the dictation stack
/// is not present. Both open the same file and every `CREATE` is
/// `IF NOT EXISTS`, so the duplication is idempotent.
pub fn open(settings: &SettingsState) -> Result<Connection, String> {
    let conn = Connection::open(path(settings)?).map_err(|e| e.to_string())?;
    // Mail is polled. A poll that meets another writer must wait like every
    // other app-DB writer does, not fail the agent's turn.
    conn.busy_timeout(std::time::Duration::from_secs(30))
        .map_err(|e| e.to_string())?;
    store::ensure_schema(&conn).map_err(|e| e.to_string())?;
    Ok(conn)
}

/// Does the artifact at this row's `target_uuid` already carry its `mail_uuid`?
///
/// This is §4.1's Phase 0d: the one read a claim makes outside the app-DB
/// write lock. Three answers, never two, because "I could not look" must not
/// read as "nothing is there" — the first closes the row, the second makes it
/// claimable, and the third fails it (`subject_gone`).
///
/// Today only book artifacts are addressed by the ledger, so a subject of any
/// other kind has nothing that could have landed and answers `Absent`. Once
/// §12 step 3 lands, `book_artifact` carries `mail_uuid` and this check starts
/// finding real answers without a change here.
pub fn reconcile_read(settings: &SettingsState, mail: &Mail) -> Reconciled {
    let book_uuid = match (mail.ref_kind.as_deref(), mail.ref_id.as_deref()) {
        (Some("book"), Some(id)) => id,
        _ => return Reconciled::Absent,
    };
    let dir = match crate::datasets::book::find_book_dir(settings, book_uuid) {
        Ok(dir) => dir,
        // Deleted, moved out of the workspace, or unreadable: the request can
        // never be delivered now, which is a failure the user must see.
        Err(_) => return Reconciled::SubjectGone,
    };
    let conn = match crate::datasets::book::open_book_db(&dir) {
        Ok(conn) => conn,
        Err(_) => return Reconciled::SubjectGone,
    };
    let target = match mail.target_uuid.as_deref() {
        Some(t) if !t.is_empty() => t,
        _ => return Reconciled::Absent,
    };
    // A dataset that has not been through the step-3 migration has no
    // `mail_uuid` column, so no artifact in it can be attributed to mail.
    if !has_column(&conn, "book_artifact", "mail_uuid") {
        return Reconciled::Absent;
    }
    match conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM book_artifact WHERE uuid = ?1 AND mail_uuid = ?2)",
        [target, mail.uuid.as_str()],
        |r| r.get::<_, i64>(0),
    ) {
        Ok(landed) if landed != 0 => Reconciled::Landed,
        Ok(_) => Reconciled::Absent,
        Err(_) => Reconciled::SubjectGone,
    }
}

/// The work behind a request, resolved **now**, for the `data` envelope a claim
/// returns (§5.2 rule 2, §9 "payloads are links").
///
/// The text is read at claim time rather than copied into the row, so an edit
/// made after the request is visible instead of frozen, and the hash the
/// request was made against travels with it so the holder can see the work now
/// reads against different text. `None` means this app cannot resolve the
/// subject — an unknown `ref_kind`, or a book deleted since — and the claim says
/// so rather than shipping an empty string that reads like an empty scope.
pub fn resolve_data(settings: &SettingsState, mail: &Mail) -> Option<serde_json::Value> {
    match mail.ref_kind.as_deref() {
        Some("book") => crate::datasets::book_mail::envelope(settings, mail),
        _ => None,
    }
}

fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    let sql = format!("PRAGMA table_info({table})");
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return false;
    };
    let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(1)) else {
        return false;
    };
    for name in rows.flatten() {
        if name == column {
            return true;
        }
    }
    false
}
