//! Database helper utilities with logging for write contention diagnosis.
//!
//! Provides wrapper functions around rusqlite operations that log:
//! - When a database connection is opened
//! - When a transaction begins
//! - When a transaction commits or rolls back
//! - When waiting for a database lock (via busy handler)

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, Arc};
use std::time::{Duration, Instant};

use rusqlite::Connection;

/// Global counter for active database writers.
static ACTIVE_WRITERS: AtomicUsize = AtomicUsize::new(0);

// Global set of datasets currently being adjusted (to prevent concurrent operations).
lazy_static::lazy_static! {
    static ref ACTIVE_ADJUSTMENTS: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
}

/// Check if a dataset is currently being adjusted.
#[allow(dead_code)]
pub fn is_dataset_being_adjusted(dataset_uuid: &str) -> bool {
    let active = ACTIVE_ADJUSTMENTS.lock().unwrap();
    active.contains(dataset_uuid)
}

/// Mark a dataset as being adjusted. Returns false if already being adjusted.
pub fn mark_dataset_adjustment_start(dataset_uuid: &str) -> bool {
    let mut active = ACTIVE_ADJUSTMENTS.lock().unwrap();
    if active.contains(dataset_uuid) {
        return false;
    }
    active.insert(dataset_uuid.to_string());
    log::info!("[DB] Dataset '{}' marked as being adjusted (active_count={})", dataset_uuid, active.len());
    true
}

/// Mark a dataset as no longer being adjusted.
pub fn mark_dataset_adjustment_end(dataset_uuid: &str) {
    let mut active = ACTIVE_ADJUSTMENTS.lock().unwrap();
    active.remove(dataset_uuid);
    log::info!("[DB] Dataset '{}' adjustment completed (active_count={})", dataset_uuid, active.len());
}

/// RAII guard that ensures a dataset's adjustment mark is released when dropped.
/// This guarantees cleanup even if the operation panics.
pub struct AdjustmentGuard {
    dataset_uuid: String,
}

impl AdjustmentGuard {
    /// Create a new guard. Returns None if the dataset is already being adjusted.
    pub fn try_acquire(dataset_uuid: &str) -> Option<Self> {
        if mark_dataset_adjustment_start(dataset_uuid) {
            Some(Self { dataset_uuid: dataset_uuid.to_string() })
        } else {
            None
        }
    }
}

impl Drop for AdjustmentGuard {
    fn drop(&mut self) {
        mark_dataset_adjustment_end(&self.dataset_uuid);
    }
}

/// Open a database connection with write contention logging.
///
/// Logs when the connection is opened and sets up a busy handler that logs
/// when waiting for a lock.
pub fn open_db_with_logging(db_path: &Path, context: &str) -> Result<Connection, String> {
    let start = Instant::now();
    log::info!(
        "[DB] Opening database for '{}' (active_writers={})",
        context,
        ACTIVE_WRITERS.load(Ordering::Relaxed)
    );

    let conn = Connection::open(db_path).map_err(|e| {
        log::error!("[DB] Failed to open database for '{}': {}", context, e);
        e.to_string()
    })?;

    // Set a busy timeout so SQLite will wait rather than fail immediately.
    conn.busy_timeout(Duration::from_secs(30))
        .map_err(|e| e.to_string())?;

    log::info!(
        "[DB] Database opened for '{}' in {:?}",
        context,
        start.elapsed()
    );

    Ok(conn)
}

/// Begin a transaction with logging.
///
/// Logs when the transaction begins and increments the active writer counter.
pub fn begin_transaction_with_logging<'a>(
    conn: &'a mut Connection,
    context: &str,
) -> Result<rusqlite::Transaction<'a>, String> {
    let writer_id = ACTIVE_WRITERS.fetch_add(1, Ordering::Relaxed) + 1;
    log::info!(
        "[DB] Beginning transaction for '{}' (writer_id={}, active_writers={})",
        context,
        writer_id,
        ACTIVE_WRITERS.load(Ordering::Relaxed)
    );

    let start = Instant::now();
    let tx = conn.transaction().map_err(|e| {
        ACTIVE_WRITERS.fetch_sub(1, Ordering::Relaxed);
        log::error!(
            "[DB] Failed to begin transaction for '{}': {} (waited {:?})",
            context,
            e,
            start.elapsed()
        );
        e.to_string()
    })?;

    log::info!(
        "[DB] Transaction started for '{}' (writer_id={}, took {:?})",
        context,
        writer_id,
        start.elapsed()
    );

    Ok(tx)
}

/// Commit a transaction with logging.
///
/// Logs when the transaction commits and decrements the active writer counter.
pub fn commit_transaction_with_logging(
    tx: rusqlite::Transaction,
    context: &str,
    writer_id: usize,
) -> Result<(), String> {
    let start = Instant::now();
    log::info!(
        "[DB] Committing transaction for '{}' (writer_id={})",
        context,
        writer_id
    );

    tx.commit().map_err(|e| {
        log::error!(
            "[DB] Failed to commit transaction for '{}': {} (waited {:?})",
            context,
            e,
            start.elapsed()
        );
        e.to_string()
    })?;

    ACTIVE_WRITERS.fetch_sub(1, Ordering::Relaxed);

    log::info!(
        "[DB] Transaction committed for '{}' (writer_id={}, took {:?}, active_writers={})",
        context,
        writer_id,
        start.elapsed(),
        ACTIVE_WRITERS.load(Ordering::Relaxed)
    );

    Ok(())
}

/// Execute a write statement with logging.
///
/// Logs the SQL statement and execution time.
#[allow(dead_code)]
pub fn execute_with_logging(
    conn: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
    context: &str,
) -> Result<usize, String> {
    let start = Instant::now();
    let result = conn.execute(sql, params).map_err(|e| {
        log::error!(
            "[DB] Execute failed for '{}' (sql: {}): {} (waited {:?})",
            context,
            truncate_sql(sql, 100),
            e,
            start.elapsed()
        );
        e.to_string()
    })?;

    let elapsed = start.elapsed();
    if elapsed > Duration::from_millis(100) {
        log::warn!(
            "[DB] Slow execute for '{}' (sql: {}, rows_affected: {}, took {:?})",
            context,
            truncate_sql(sql, 100),
            result,
            elapsed
        );
    } else {
        log::debug!(
            "[DB] Execute for '{}' (sql: {}, rows_affected: {}, took {:?})",
            context,
            truncate_sql(sql, 100),
            result,
            elapsed
        );
    }

    Ok(result)
}

/// Truncate SQL for logging purposes.
#[allow(dead_code)]
fn truncate_sql(sql: &str, max_len: usize) -> String {
    let sql = sql.trim().replace('\n', " ").replace('\r', "");
    if sql.len() > max_len {
        format!("{}...", &sql[..max_len])
    } else {
        sql
    }
}

/// Get the current number of active writers.
#[allow(dead_code)]
pub fn get_active_writers() -> usize {
    ACTIVE_WRITERS.load(Ordering::Relaxed)
}
