//! Centralized, platform-aware base directories for all persistent app data.
//!
//! The whole app anchors its writable state — datasets, workspaces, the
//! app-level SQLite DB, models, wiki, auth tokens and the global `settings.json`
//! — under a single `fms-app` base directory. Historically every module called
//! `dirs::data_dir()/fms-app` (or `dirs::config_dir()/fms-app`) directly, which
//! works on the desktop but is unusable on Android/iOS where the `dirs` crate has
//! no writable HOME/XDG and the OS enforces scoped storage.
//!
//! Routing everything through this module gives one lever to relocate storage:
//!
//! - **Desktop** (Windows/macOS/Linux): base stays `dirs::data_dir()/fms-app` and
//!   config stays `dirs::config_dir()/fms-app`, byte-for-byte the same as before,
//!   so existing installs are unaffected.
//! - **Mobile** (Android/iOS): base is Tauri's app-private
//!   `app_data_dir()/fms-app`, the only writable location under scoped storage.
//!   Datasets are pulled into it by the PC-sync thin-client flow rather than
//!   picked from arbitrary paths.
//!
//! [`init`] must be called once from the Tauri `setup` hook — the first place an
//! `AppHandle` exists. Before `init` runs (builder-time `Default` impls execute
//! first) the accessors fall back to the desktop paths; on mobile the in-memory
//! defaults are reloaded after `init` so commands never observe the placeholder
//! paths.

use std::path::PathBuf;
use std::sync::OnceLock;

use tauri::AppHandle;
// `app.path()` (used to resolve the mobile app-private dir) comes from Manager.
#[cfg(any(target_os = "android", target_os = "ios"))]
use tauri::Manager;

static DATA_ROOT: OnceLock<PathBuf> = OnceLock::new();
static CONFIG_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Desktop `dirs`-based data root. Also used as the pre-`init` fallback.
fn desktop_data_root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("fms-app")
}

/// Desktop `dirs`-based config root. Also used as the pre-`init` fallback.
fn desktop_config_root() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("fms-app")
}

/// Capture the platform-correct base directories. Idempotent (first call wins).
/// Call once at the very top of the Tauri `setup` closure, before any component
/// reads a path or writes a log.
pub fn init(app: &AppHandle) {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        if let Ok(dir) = app.path().app_data_dir() {
            let base = dir.join("fms-app");
            let _ = std::fs::create_dir_all(&base);
            let _ = DATA_ROOT.set(base.clone());
            let _ = CONFIG_ROOT.set(base);
        }
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        let _ = DATA_ROOT.set(desktop_data_root());
        let _ = CONFIG_ROOT.set(desktop_config_root());
    }
}

/// App-private data base directory (`.../fms-app`).
pub fn data_root() -> PathBuf {
    DATA_ROOT
        .get()
        .cloned()
        .unwrap_or_else(desktop_data_root)
}

/// Config base directory holding the global `settings.json`.
pub fn config_root() -> PathBuf {
    CONFIG_ROOT
        .get()
        .cloned()
        .unwrap_or_else(desktop_config_root)
}

/// A named sub-directory of the data root, i.e. `<data_root>/<sub>`.
pub fn data_subdir(sub: &str) -> PathBuf {
    data_root().join(sub)
}
