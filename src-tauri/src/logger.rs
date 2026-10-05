//! Centralized application logger.
//!
//! Implements the `log::Log` trait so that all `log::info!`, `log::warn!`,
//! `log::error!`, `log::debug!` calls throughout the Rust backend are captured,
//! buffered, emitted to the frontend via Tauri events, and persisted to a
//! rotating log file under `<data_root>/logs/`.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Mutex};

use log::{Level, Log, Metadata, Record};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Maximum number of log entries kept in the ring buffer.
const MAX_BUFFER_SIZE: usize = 1000;

/// Rotate the log file into the `.1` backup once it exceeds this size.
const MAX_LOG_FILE_BYTES: u64 = 5 * 1024 * 1024;

/// A single log entry sent to the frontend.
#[derive(Clone, Serialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub message: String,
    pub module: String,
}

/// Shared log buffer accessible from both the logger and Tauri commands.
#[derive(Clone)]
pub struct LogBuffer {
    pub entries: Arc<Mutex<VecDeque<LogEntry>>>,
    pub app: Arc<Mutex<Option<AppHandle>>>,
    /// Lazily-opened append handle for the persistent log file, with the
    /// number of bytes in the current (unrotated) file.
    log_file: Arc<Mutex<Option<(std::fs::File, u64)>>>,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(VecDeque::with_capacity(MAX_BUFFER_SIZE))),
            app: Arc::new(Mutex::new(None)),
            log_file: Arc::new(Mutex::new(None)),
        }
    }

    /// Path of the persistent log file. Resolved lazily on every access so the
    /// platform-correct `app_paths` base (only established in the setup hook)
    /// is honored regardless of when the first log line arrives.
    pub fn file_path() -> std::path::PathBuf {
        crate::app_paths::data_subdir("logs").join("fms-app.log")
    }

    /// Buffer, persist to file, and emit an entry. Single path shared by the
    /// backend `log::` macros and the frontend log command.
    pub fn record(&self, entry: LogEntry) {
        let mut buf = self.entries.lock().unwrap();
        if buf.len() >= MAX_BUFFER_SIZE {
            buf.pop_front();
        }
        buf.push_back(entry.clone());
        drop(buf);

        self.append_to_file(&entry);
        self.emit_entry(&entry);
    }

    /// Set the app handle for event emission.
    pub fn set_app(&self, app: AppHandle) {
        let mut handle = self.app.lock().unwrap();
        *handle = Some(app);
    }

    /// Emit a log entry to the frontend.
    fn emit_entry(&self, entry: &LogEntry) {
        let handle = self.app.lock().unwrap();
        if let Some(app) = handle.as_ref() {
            let _ = app.emit("app-log", entry);
        }
    }

    /// Get a snapshot of all buffered log entries.
    pub fn get_history(&self) -> Vec<LogEntry> {
        let buf = self.entries.lock().unwrap();
        buf.iter().cloned().collect()
    }

    /// Clear all buffered log entries. The log file is deliberately left
    /// untouched — it is the archive that survives restarts and clears.
    pub fn clear(&self) {
        let mut buf = self.entries.lock().unwrap();
        buf.clear();
    }

    /// Append one entry to the log file, rotating into `fms-app.log.1` when
    /// the size cap is hit. Best-effort: I/O failures are swallowed (a logger
    /// that logs its own errors would recurse).
    fn append_to_file(&self, entry: &LogEntry) {
        let line = format!(
            "{} {} [{}] {}\n",
            entry.timestamp, entry.level, entry.module, entry.message
        );
        let mut guard = self.log_file.lock().unwrap();
        if guard.is_none() {
            let path = Self::file_path();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                Ok(file) => {
                    let bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
                    *guard = Some((file, bytes));
                }
                Err(_) => return,
            }
        }
        let (file, bytes) = guard.as_mut().unwrap();
        if file.write_all(line.as_bytes()).is_err() {
            return;
        }
        let _ = file.flush();
        *bytes += line.len() as u64;
        if *bytes >= MAX_LOG_FILE_BYTES {
            // Dropping the handle releases the file lock so the rename can
            // proceed (Windows keeps opened files locked for renaming).
            drop(std::mem::replace(&mut *guard, None));
            let path = Self::file_path();
            let _ = std::fs::rename(&path, path.with_file_name("fms-app.log.1"));
        }
    }

    /// Parse the persisted log file(s) back into entries, oldest first. The
    /// rotated `.1` backup is read before the current file so the archive is
    /// contiguous. Lines that don't match the header format are treated as
    /// continuation lines of the previous entry (multi-line messages).
    pub fn read_file_history(&self, limit: Option<usize>) -> Vec<LogEntry> {
        let path = Self::file_path();
        let paths = [
            path.with_file_name("fms-app.log.1"),
            path,
        ];
        let mut entries: Vec<LogEntry> = Vec::new();
        for p in paths {
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            for line in text.lines() {
                match parse_log_line(line) {
                    Some(entry) => entries.push(entry),
                    None => {
                        if let Some(last) = entries.last_mut() {
                            if !line.trim().is_empty() {
                                last.message.push('\n');
                                last.message.push_str(line);
                            }
                        }
                    }
                }
            }
        }
        if let Some(limit) = limit {
            if limit < entries.len() {
                entries.drain(..entries.len() - limit);
            }
        }
        entries
    }
}

/// Parse one persisted line: `2026-10-05 12:34:56.789 INFO [module] message`.
/// The timestamp produced by `"%Y-%m-%d %H:%M:%S%.3f"` is fixed-width ASCII
/// (23 chars), so byte-index sniffing is safe without a regex dependency.
fn parse_log_line(line: &str) -> Option<LogEntry> {
    let bytes = line.as_bytes();
    if bytes.len() < 24
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b' '
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'.'
    {
        return None;
    }
    let rest = line[23..].strip_prefix(' ')?;
    let space = rest.find(' ')?;
    let level = &rest[..space];
    if !matches!(level, "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR") {
        return None;
    }
    let open = rest[space + 1..].strip_prefix('[')?;
    let close = open.find(']')?;
    let module = open[..close].to_string();
    let message = open[close + 1..].strip_prefix(' ').unwrap_or("").to_string();
    Some(LogEntry {
        timestamp: line[..23].to_string(),
        level: level.to_string(),
        message,
        module,
    })
}

/// Custom logger that buffers entries, persists them to file, and emits
/// Tauri events.
struct AppLogger {
    buffer: LogBuffer,
}

impl Log for AppLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Debug
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let now = chrono::Local::now();
        let entry = LogEntry {
            timestamp: now.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
            level: record.level().to_string(),
            message: format!("{}", record.args()),
            module: record.module_path().unwrap_or("unknown").to_string(),
        };

        self.buffer.record(entry);
    }

    fn flush(&self) {}
}

/// Initialize the global logger.
///
/// Must be called once during Tauri setup. Returns the `LogBuffer` handle
/// so it can be stored as Tauri state for command access.
pub fn init_logger(app: AppHandle) -> LogBuffer {
    let buffer = LogBuffer::new();
    buffer.set_app(app);

    log::set_boxed_logger(Box::new(AppLogger {
        buffer: buffer.clone(),
    }))
        .expect("Failed to set global logger");
    log::set_max_level(log::LevelFilter::Debug);

    buffer
}

/// Tauri command: get all buffered log entries.
#[tauri::command]
pub fn log_get_history(buffer: tauri::State<'_, LogBuffer>) -> Vec<LogEntry> {
    buffer.get_history()
}

/// Tauri command: clear the log buffer. The persisted log file keeps the
/// history, so entries remain readable via `log_read_file_history`.
#[tauri::command]
pub fn log_clear(buffer: tauri::State<'_, LogBuffer>) {
    buffer.clear();
}

/// Tauri command: path of the persistent log file (for display or opening
/// externally).
#[tauri::command]
pub fn log_get_file_path() -> String {
    LogBuffer::file_path().to_string_lossy().into_owned()
}

/// Tauri command: read archived entries from the persistent log file (oldest
/// first, optional newest-`limit` window). Unlike the in-memory buffer this
/// survives app restarts and `log_clear`.
#[tauri::command]
pub fn log_read_file_history(
    buffer: tauri::State<'_, LogBuffer>,
    limit: Option<usize>,
) -> Vec<LogEntry> {
    buffer.read_file_history(limit)
}

/// Tauri command: log a message from the frontend.
#[tauri::command]
pub fn log_frontend_message(
    buffer: tauri::State<'_, LogBuffer>,
    message: String,
    level: Option<String>,
    module: Option<String>,
) {
    let now = chrono::Local::now();
    let entry = LogEntry {
        timestamp: now.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        level: level.unwrap_or_else(|| "INFO".to_string()),
        message,
        module: module.unwrap_or_else(|| "frontend".to_string()),
    };

    // Buffer, persist, and emit for real-time display
    buffer.record(entry);
}
