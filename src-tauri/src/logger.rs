//! Centralized application logger.
//!
//! Implements the `log::Log` trait so that all `log::info!`, `log::warn!`,
//! `log::error!`, `log::debug!` calls throughout the Rust backend are captured,
//! buffered, and emitted to the frontend via Tauri events.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use log::{Level, Log, Metadata, Record};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Maximum number of log entries kept in the ring buffer.
const MAX_BUFFER_SIZE: usize = 1000;

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
}

impl LogBuffer {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(VecDeque::with_capacity(MAX_BUFFER_SIZE))),
        }
    }

    /// Get a snapshot of all buffered log entries.
    pub fn get_history(&self) -> Vec<LogEntry> {
        let buf = self.entries.lock().unwrap();
        buf.iter().cloned().collect()
    }

    /// Clear all buffered log entries.
    pub fn clear(&self) {
        let mut buf = self.entries.lock().unwrap();
        buf.clear();
    }
}

/// Custom logger that buffers entries and emits Tauri events.
struct AppLogger {
    app: AppHandle,
    buffer: Arc<Mutex<VecDeque<LogEntry>>>,
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

        // Buffer the entry (ring buffer: drop oldest when full)
        let mut buf = self.buffer.lock().unwrap();
        if buf.len() >= MAX_BUFFER_SIZE {
            buf.pop_front();
        }
        buf.push_back(entry.clone());
        drop(buf);

        // Emit to frontend (non-blocking; ignore errors if no listener)
        let _ = self.app.emit("app-log", &entry);
    }

    fn flush(&self) {}
}

/// Initialize the global logger.
///
/// Must be called once during Tauri setup. Returns the `LogBuffer` handle
/// so it can be stored as Tauri state for command access.
pub fn init_logger(app: AppHandle) -> LogBuffer {
    let buffer = LogBuffer::new();
    let logger = AppLogger {
        app,
        buffer: buffer.entries.clone(),
    };

    log::set_boxed_logger(Box::new(logger))
        .expect("Failed to set global logger");
    log::set_max_level(log::LevelFilter::Debug);

    buffer
}

/// Tauri command: get all buffered log entries.
#[tauri::command]
pub fn log_get_history(buffer: tauri::State<'_, LogBuffer>) -> Vec<LogEntry> {
    buffer.get_history()
}

/// Tauri command: clear the log buffer.
#[tauri::command]
pub fn log_clear(buffer: tauri::State<'_, LogBuffer>) {
    buffer.clear();
}
