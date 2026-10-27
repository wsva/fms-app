//! The guard that makes first-write-wins true for a non-SQLite source's row
//! write: the app database is attached for exactly one statement, and that
//! statement is an `INSERT ... SELECT ... WHERE EXISTS (...)` over the claim
//! it is delivering (`docs/design/agent/mailbox.md` §5.1).
//!
//! This module is deliberately Tauri-free and knows nothing about which table
//! is being written: callers compose the statement and splice [`claim_guard`]
//! in. That is what lets the two-process test in the harness exercise the real
//! predicate and the real attach against a real file database, rather than a
//! copy of them.
//!
//! The guard's strength is a **snapshot check, not a lock** (§5.1): it stops
//! every writer displaced before the statement began; one displaced inside the
//! statement's microseconds lands a write the conflict clause orders and the
//! reconcile resolves.

use std::path::Path;

use rusqlite::{Connection, ToSql};

/// The predicate a guarded write splices into its `WHERE`. It is true only
/// while the row named by `?{mail_param}` is `delivering` under exactly this
/// claim: the 2-minute write lease that `begin_delivery` set (§4.1 T5).
pub fn claim_guard(mail_param: usize, claim_param: usize) -> String {
    format!(
        "EXISTS (SELECT 1 FROM app.agent_mail \
          WHERE uuid = ?{mail_param} AND claim_id = ?{claim_param} AND status = 'delivering')"
    )
}

/// Run one statement with the app database attached read-only.
///
/// The attach is per statement on purpose: ordinary dataset work keeps a
/// single-database connection, and the guard's read costs the same order of
/// magnitude as the write it protects. `mode=ro` bounds the lock the attach
/// can take, because the guard never writes the ledger; URI filenames are only
/// honoured when the connection was opened with `SQLITE_OPEN_URI`, which
/// rusqlite's default flags do, so a build that refuses that form falls back to
/// the plain path, which reads the same table.
pub fn write(
    conn: &Connection,
    app_path: &Path,
    sql: &str,
    params: &[&dyn ToSql],
) -> Result<usize, String> {
    attach_app(conn, app_path)?;
    let result = conn.execute(sql, params);
    let _ = conn.execute_batch("DETACH DATABASE app");
    result.map_err(|e| e.to_string())
}

/// Attach the mailbox database as `app` on `conn`.
pub fn attach_app(conn: &Connection, app_path: &Path) -> Result<(), String> {
    let uri = format!(
        "file:{}?mode=ro",
        app_path.to_string_lossy().replace('\\', "/")
    );
    let plain = app_path.to_string_lossy().to_string();
    if conn.execute("ATTACH DATABASE ?1 AS app", [&uri]).is_err()
        && conn
            .execute("ATTACH DATABASE ?1 AS app", [plain.as_str()])
            .is_err()
    {
        return Err("could not attach the mailbox database to the write".into());
    }
    Ok(())
}

/// Detach the mailbox database again. Best-effort: a failed detach leaves the
/// connection usable, and the next attach replaces nothing but its own alias.
pub fn detach_app(conn: &Connection) {
    let _ = conn.execute_batch("DETACH DATABASE app");
}
