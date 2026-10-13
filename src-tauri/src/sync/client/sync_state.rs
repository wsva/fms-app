#![allow(unused_imports)]
use std::io::{BufReader, Write as _};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use futures_util::StreamExt;
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tar::Archive;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use ring::signature::{Ed25519KeyPair, KeyPair};

use crate::datasets;
use crate::settings::{self, SettingsState};
use super::*;



pub(crate) fn ensure_sync_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS writeback_queue (
            id           TEXT PRIMARY KEY,
            kind         TEXT NOT NULL,
            dataset_uuid TEXT NOT NULL,
            payload      TEXT NOT NULL,
            queued_at    TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS dataset_sync_state (
            dataset_uuid TEXT PRIMARY KEY,
            overall_hash TEXT NOT NULL,
            synced_at    TEXT NOT NULL,
            bytes        INTEGER NOT NULL DEFAULT 0,
            file_count   INTEGER NOT NULL DEFAULT 0
        );
        /* Single-row cursor for the app-wide journal scope (per-user dictation
           progress + XP). Kept out of `dataset_sync_state` deliberately: that
           table is keyed by real dataset uuids and the round prunes rows whose
           dataset vanished from the hub catalog, which would delete this.
           `backfilled_at` records that the state-based history read (§3.6) has
           run once, so the round does not re-download a whole ledger every round.
           `events_backfilled_at` is the same stamp for the practice-event half
           (reviews + attempts, added at protocol v6): kept separate so a device
           whose dictation/xp already settled can still recover practice history
           once its hub upgrades past v6, without re-pulling the whole ledger. */
        CREATE TABLE IF NOT EXISTS app_sync_state (
            id            INTEGER PRIMARY KEY CHECK (id = 1),
            cursor_seq    INTEGER,
            backfilled_at TEXT,
            events_backfilled_at TEXT,
            updated_at    TEXT NOT NULL
        );",
    )
    .map_err(|e| e.to_string())?;
    // Migration (§3.2): promote `dataset_sync_state` with a nullable incremental
    // row-log cursor. Added with **no DEFAULT** so every pre-existing row stays
    // NULL — a NULL cursor means "this copy predates incremental sync", so the
    // first `/changes` pull is told `resync_required` and the follower takes one
    // fresh snapshot, after which the cursor is a real hub seq.
    let has_cursor: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('dataset_sync_state') WHERE name = 'cursor'")
        .and_then(|mut s| s.exists([]))
        .unwrap_or(false);
    if !has_cursor {
        conn.execute("ALTER TABLE dataset_sync_state ADD COLUMN \"cursor\" INTEGER", [])
            .map_err(|e| e.to_string())?;
    }
    // Migration (Phase 5, §3.4): promote `writeback_queue` with `edit_time` (the
    // device-local action stamp that drives later-wins conflicts on the hub) and
    // `object_id` (the row a *state* change mutates, so coalescing can fold
    // repeat edits to one row). Added nullable so pre-existing rows keep
    // working; `enqueue_change` backfills both on every new insert.
    let has_edit_time: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('writeback_queue') WHERE name = 'edit_time'")
        .and_then(|mut s| s.exists([]))
        .unwrap_or(false);
    if !has_edit_time {
        conn.execute("ALTER TABLE writeback_queue ADD COLUMN \"edit_time\" TEXT", [])
            .map_err(|e| e.to_string())?;
    }
    let has_object_id: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('writeback_queue') WHERE name = 'object_id'")
        .and_then(|mut s| s.exists([]))
        .unwrap_or(false);
    if !has_object_id {
        conn.execute("ALTER TABLE writeback_queue ADD COLUMN object_id TEXT", [])
            .map_err(|e| e.to_string())?;
    }
    // Migration: `app_sync_state` shipped without `backfilled_at`; a device that
    // pulled the app scope before the history read existed gets the column here
    // (NULL = never backfilled, which is the truth for those rows).
    let has_backfilled_at: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('app_sync_state') WHERE name = 'backfilled_at'")
        .and_then(|mut s| s.exists([]))
        .unwrap_or(false);
    if !has_backfilled_at {
        conn.execute("ALTER TABLE app_sync_state ADD COLUMN backfilled_at TEXT", [])
            .map_err(|e| e.to_string())?;
    }
    // Migration (protocol v6): the practice-event half of the backfill gets its own
    // stamp, added nullable so a device that already folded dictation/xp reads it as
    // "events never backfilled" — the truth, and what re-triggers the event half.
    ensure_column(conn, "app_sync_state", "events_backfilled_at", "TEXT")?;
    // Migration (sync-v2.md §6.1, phase 1): provenance columns. A cursor is only
    // meaningful against the seq space of the host that minted it, so every row
    // records which host that was. `upstream_kind` defaults to `desktop_hub` for
    // pre-existing rows because a LAN hub was the only origin writing this table
    // before the migration; `upstream_origin` defaults to empty and is treated as
    // "unknown" — the first round after migration NULLs the cursor so the row
    // re-establishes itself against the currently connected host (INV-7, INV-19).
    ensure_column(conn, "dataset_sync_state", "row_hash", "TEXT")?;
    ensure_column(conn, "dataset_sync_state", "upstream_kind", "TEXT NOT NULL DEFAULT 'desktop_hub'")?;
    ensure_column(conn, "dataset_sync_state", "upstream_origin", "TEXT NOT NULL DEFAULT ''")?;
    ensure_column(conn, "dataset_sync_state", "published_version", "INTEGER")?;
    ensure_column(conn, "dataset_sync_state", "availability", "TEXT NOT NULL DEFAULT 'listed'")?;
    ensure_column(conn, "app_sync_state", "upstream_kind", "TEXT NOT NULL DEFAULT 'desktop_hub'")?;
    ensure_column(conn, "app_sync_state", "upstream_origin", "TEXT NOT NULL DEFAULT ''")?;
    // Migration (sync-v2.md §19.2): persist the last round's result JSON so the
    // status surface can show "what the previous round did" without running one.
    // Nullable — NULL until the first round completes on this build.
    ensure_column(conn, "app_sync_state", "last_round_json", "TEXT")?;
    ensure_column(conn, "app_sync_state", "last_round_at", "TEXT")?;
    // Migration (sync-v2.md §19.2): record *why* a copy owes a resync, so the
    // status surface can say "retention gap" / "journal regressed" instead of a
    // bare `needs_resync`. Nullable — NULL means nothing is owed (or the reason
    // was never recorded, which `classify_resync_reason` reads as `no_cursor`).
    // A successful `dataset_sync_snapshot` clears it: its INSERT OR REPLACE does
    // not list this column, so the re-inserted row defaults it back to NULL.
    ensure_column(conn, "dataset_sync_state", "resync_reason", "TEXT")?;
    Ok(())
}

/// Idempotent `ALTER TABLE ... ADD COLUMN` guarded by a `pragma_table_info`
/// check — the same pattern the surrounding migrations use. Kept as a helper so
/// the phase-1 provenance additions read as one block.
pub(crate) fn ensure_column(conn: &Connection, table: &str, name: &str, decl: &str) -> Result<(), String> {
    let exists: bool = conn
        .prepare(&format!(
            "SELECT 1 FROM pragma_table_info('{table}') WHERE name = '{name}'"
        ))
        .and_then(|mut s| s.exists([]))
        .unwrap_or(false);
    if exists {
        return Ok(());
    }
    conn.execute(&format!("ALTER TABLE {table} ADD COLUMN \"{name}\" {decl}"), [])
        .map_err(|e| e.to_string())
        .map(|_| ())
}

/// One-time cursor reset for pre-migration rows: a `dataset_sync_state` row whose
/// `upstream_origin` is empty was written before provenance existed, so its
/// `cursor` belongs to an unknown host's seq space. NULLing it forces the
/// existing "NULL cursor → one snapshot resync" path (documented at
/// [client.rs#L398-L410]) to re-establish the cursor against whichever host the
/// device is now talking to. Runs at most once per row (the migration stamps
/// `upstream_origin` from then on). Returns the number of rows reset so the
/// caller can log it.
pub(crate) fn reset_cursors_without_provenance(conn: &Connection) -> Result<usize, String> {
    let n = conn
        .execute(
            "UPDATE dataset_sync_state SET \"cursor\" = NULL, resync_reason = 'provenance_reset' \
             WHERE (upstream_origin IS NULL OR upstream_origin = '') AND \"cursor\" IS NOT NULL",
            [],
        )
        .map_err(|e| e.to_string())?;
    Ok(n)
}

/// Provenance stamp for the currently connected host, derived from its base URL.
/// `upstream_kind` is the wire-visible value from sync-v2.md §6.1
/// (`desktop_hub | cloud_hub | registry | local`); phase 1 only ever writes the
/// first two — `registry` lands with the adoption flow in phase 3.
pub(crate) fn upstream_of(base: &str) -> (&'static str, String) {
    match crate::sync::hub_kind(base) {
        crate::sync::HubKind::Website => ("cloud_hub", base.trim_end_matches('/').to_string()),
        crate::sync::HubKind::LanPc => ("desktop_hub", base.trim_end_matches('/').to_string()),
    }
}

/// Read this device's cursor into the app-wide journal scope (`None` = never
/// pulled). The single-row table is inserted on first advance, so a fresh device
/// simply has no row yet.
pub(crate) fn app_cursor(conn: &Connection) -> Option<i64> {
    conn.query_row("SELECT cursor_seq FROM app_sync_state WHERE id = 1", [], |r| {
        r.get::<_, Option<i64>>(0)
    })
    .unwrap_or(None)
}

/// Advance the app-wide journal cursor to the hub's high-water `seq`.
pub(crate) fn set_app_cursor(conn: &Connection, seq: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO app_sync_state (id, cursor_seq, updated_at) \
         VALUES (1, ?1, datetime('now')) \
         ON CONFLICT(id) DO UPDATE SET cursor_seq = excluded.cursor_seq, updated_at = excluded.updated_at",
        params![seq],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Stamp provenance on the single-row `app_sync_state` (sync-v2.md §6.1). Only
/// fills an empty origin — a row already stamped for a different host is left
/// alone, so a partial round against a foreign host cannot silently rewrite the
/// `@app` scope's upstream (INV-19). Returns `true` when the row was stamped,
/// `false` when the existing stamp was kept.
pub(crate) fn stamp_app_provenance(
    conn: &Connection,
    upstream_kind: &str,
    upstream_origin: &str,
) -> Result<bool, String> {
    // Make sure the row exists so the UPDATE has something to touch. The
    // ON CONFLICT branch preserves any cursor/backfill stamp already there.
    conn.execute(
        "INSERT INTO app_sync_state (id, updated_at) VALUES (1, datetime('now')) \
         ON CONFLICT(id) DO NOTHING",
        [],
    )
    .map_err(|e| e.to_string())?;
    let n = conn
        .execute(
            "UPDATE app_sync_state \
             SET upstream_kind = ?1, upstream_origin = ?2 \
             WHERE id = 1 AND (upstream_origin IS NULL OR upstream_origin = '')",
            params![upstream_kind, upstream_origin],
        )
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

/// Read the recorded `@app` upstream, if any; a fresh install returns `None`.
/// This is the read side of the [`stamp_app_provenance`] stamp. Phase 1 only
/// *writes* `@app` provenance (so a later round can see which host minted the
/// cursor); the INV-19 check that refuses an `@app` round against a foreign host
/// reads it in a later phase, so today it is exercised by the unit tests only.
#[allow(dead_code)]
pub(crate) fn app_provenance(conn: &Connection) -> Option<(String, String)> {
    conn.query_row(
        "SELECT upstream_kind, upstream_origin FROM app_sync_state WHERE id = 1",
        [],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .ok()
    .filter(|(_, origin)| !origin.is_empty())
}

/// When this device last folded the hub's *whole* app-data history into its own
/// (`None` = never). See [`backfill_app_history`].
pub(crate) fn app_history_backfilled(conn: &Connection) -> Option<String> {
    conn.query_row(
        "SELECT backfilled_at FROM app_sync_state WHERE id = 1",
        [],
        |r| r.get::<_, Option<String>>(0),
    )
    .unwrap_or(None)
}

/// Stamp the history backfill as done. Kept separate from `set_app_cursor` rather
/// than folded into it: a journal pull advances the cursor on every round, while
/// the backfill is a once-per-device (or once-per-gap) event, and conflating the two
/// would silently stop the retry after a *truncated* fetch.
pub(crate) fn set_app_history_backfilled(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "INSERT INTO app_sync_state (id, backfilled_at, updated_at) \
         VALUES (1, datetime('now'), datetime('now')) \
         ON CONFLICT(id) DO UPDATE SET backfilled_at = excluded.backfilled_at, updated_at = excluded.updated_at",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// When this device last folded the hub's *whole* practice-event history
/// (reviews + attempts) into its own (`None` = never, or the hub predated v6).
/// See [`backfill_app_history`].
pub(crate) fn app_events_history_backfilled(conn: &Connection) -> Option<String> {
    conn.query_row(
        "SELECT events_backfilled_at FROM app_sync_state WHERE id = 1",
        [],
        |r| r.get::<_, Option<String>>(0),
    )
    .unwrap_or(None)
}

/// Stamp the practice-event backfill as done, separately from the dictation/xp
/// stamp so a hub that only grew the event streams at v6 re-triggers exactly that
/// half (not the ledger) once it upgrades.
pub(crate) fn set_app_events_history_backfilled(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "INSERT INTO app_sync_state (id, events_backfilled_at, updated_at) \
         VALUES (1, datetime('now'), datetime('now')) \
         ON CONFLICT(id) DO UPDATE SET events_backfilled_at = excluded.events_backfilled_at, updated_at = excluded.updated_at",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Persist the round-result JSON (§19.2) so the status surface can report the
/// last round without running one. Single-row upsert; a failure here must never
/// fail the round itself (the caller logs at most).
pub(crate) fn set_last_round(conn: &Connection, result: &Value) -> Result<(), String> {
    conn.execute(
        "INSERT INTO app_sync_state (id, last_round_json, last_round_at, updated_at) \
         VALUES (1, ?1, datetime('now'), datetime('now')) \
         ON CONFLICT(id) DO UPDATE SET last_round_json = excluded.last_round_json, \
             last_round_at = excluded.last_round_at, updated_at = excluded.updated_at",
        params![result.to_string()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Read back the persisted round result: `(json, stamped_at)`. A stored blob that
/// no longer parses yields `None` for the JSON but keeps the stamp, so a corrupt
/// row degrades to "a round ran at T" rather than breaking the surface.
pub(crate) fn last_round(conn: &Connection) -> (Option<Value>, Option<String>) {
    conn.query_row(
        "SELECT last_round_json, last_round_at FROM app_sync_state WHERE id = 1",
        [],
        |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
    )
    .ok()
    .map(|(j, at)| (j.and_then(|s| serde_json::from_str::<Value>(&s).ok()), at))
    .unwrap_or((None, None))
}



/// Per-dataset local sync state, for the phone's dataset list.
#[derive(Serialize)]
pub struct SyncStateEntry {
    pub dataset_uuid: String,
    pub overall_hash: String,
    pub synced_at: String,
    pub bytes: i64,
    pub file_count: i64,
    /// Incremental row-log cursor (§3.2). `None` = a NULL cursor, i.e. this copy
    /// predates incremental sync and is due one resync before deltas apply.
    pub cursor: Option<i64>,
    /// Provenance of the host this copy came from (§6.1): `"desktop_hub"` (a LAN
    /// mirror) | `"registry"` (a website adoption) | `"cloud_hub"`, and its origin
    /// URL (empty = unknown / pre-migration). Surfaced on the status page (§19.2).
    pub upstream_kind: String,
    pub upstream_origin: String,
    /// Registry availability: `"listed"` | `"withdrawn"` (INV-22 — a withdrawn
    /// upstream still leaves the local copy practicable).
    pub availability: String,
    /// The registry version this copy was adopted at (`None` for a LAN mirror).
    pub published_version: Option<i64>,
    /// Why this copy currently owes a resync (§19.2), or `None` when it does not:
    /// `"retention_gap"` | `"journal_regressed"` | `"provenance_reset"`. A NULL
    /// cursor with no recorded reason is surfaced as `"no_cursor"` by
    /// [`classify_resync_reason`].
    pub resync_reason: Option<String>,
}

/// Read every `dataset_sync_state` row, including the phase-1/3 provenance
/// columns. Extracted so both the [`dataset_sync_state`] command and
/// [`sync_status_detail`] share one query (and one unit test covers it).
pub(crate) fn local_sync_entries(conn: &Connection) -> Result<Vec<SyncStateEntry>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT dataset_uuid, overall_hash, synced_at, bytes, file_count, \"cursor\", \
                    upstream_kind, upstream_origin, availability, published_version, resync_reason \
             FROM dataset_sync_state",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(SyncStateEntry {
                dataset_uuid: r.get(0)?,
                overall_hash: r.get(1)?,
                synced_at: r.get(2)?,
                bytes: r.get(3)?,
                file_count: r.get(4)?,
                cursor: r.get(5)?,
                upstream_kind: r.get(6)?,
                upstream_origin: r.get(7)?,
                availability: r.get(8)?,
                published_version: r.get(9)?,
                resync_reason: r.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Record *why* a copy owes a resync (sync-v2.md §19.2). Written at the moment
/// the round detects the condition, before the snapshot that should heal it: a
/// successful `dataset_sync_snapshot` clears the column again (its INSERT OR
/// REPLACE omits it), a failed one leaves the reason visible on the next status
/// read. A no-op when the row does not exist yet — the snapshot will create it.
pub(crate) fn set_resync_reason(conn: &Connection, uuid: &str, reason: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE dataset_sync_state SET resync_reason = ?2 WHERE dataset_uuid = ?1",
        params![uuid, reason],
    )
    .map_err(|e| e.to_string())
    .map(|_| ())
}

/// Decide what `DatasetStatus.resync_reason` should read for a dataset in `state`
/// (§19.2). Pure so the mapping is unit-testable with no DB. Only a `needs_resync`
/// copy carries a reason: the stored one when the round recorded it, else
/// `"no_cursor"` (a NULL cursor that predates incremental sync, or a copy never
/// pulled against a host that records reasons). Every other state gets `None`.
pub(crate) fn classify_resync_reason(state: &str, stored: Option<String>) -> Option<String> {
    if state != "needs_resync" {
        return None;
    }
    Some(stored.unwrap_or_else(|| "no_cursor".to_string()))
}

