//! In-app updates: notice a newer release, and install it only when asked.
//!
//! Notify-first by construction — nothing here installs anything until
//! `app_update_install` is called, from Settings → About or by an agent
//! through MCP. The check itself goes through the updater plugin's configured
//! endpoints — GitHub first, the website mirror as fallback — so a machine
//! that cannot reach GitHub still learns of (and downloads) the newest
//! release. Whichever host answers, the download is verified against the
//! minisign public key in `tauri.conf.json`: the signature covers the bytes,
//! not the URL, so a compromised host cannot ship a binary.
//!
//! Desktop only, and only where the plugin can actually replace the installed
//! app — see [`platform_support`]. Desktop builds register these two commands
//! and the plugin; the mobile builds do neither.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

/// Progress events while an update downloads. `done` flips once the bytes are
/// verified and the installer takes over, which is the last thing the webview
/// hears from this process on Windows (the NSIS installer has to replace the
/// running executable, so the plugin quits the app).
const EVENT_PROGRESS: &str = "update://progress";

/// Where a human goes when the newest version is not known yet.
const RELEASES_PAGE: &str = "https://github.com/wsva/fms-app/releases/latest";

/// The website's own download section — the path for a human (and the
/// updater's fallback endpoint) when GitHub is not reachable from here.
const DOWNLOAD_PAGE: &str = "https://lusworkshop.site/#download";

/// Per-endpoint request budget. A firewall that blackholes GitHub produces a
/// hang rather than a refusal, and the plugin only moves to the fallback
/// endpoint when the first one errors or times out — the 30s default would
/// make every check feel like the failure.
const ENDPOINT_TIMEOUT: Duration = Duration::from_secs(8);

/// What `app_update_check` reports. `hint` is the actionable half — an agent
/// reads it instead of guessing what to do next.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateStatus {
    /// `CARGO_PKG_VERSION`: the CI-stamped package version the updater compares
    /// against, i.e. what an installed copy actually says it is.
    pub current: String,
    /// Newest advertised version; empty when unknown (unsupported platform,
    /// development build) or equal to `current` when already up to date.
    pub latest: String,
    pub available: bool,
    /// Release notes from `latest.json`.
    pub notes: String,
    /// Release page for `latest`, or the releases index when none is known.
    pub release_url: String,
    /// False when this build cannot install updates itself.
    pub supported: bool,
    /// Machine-readable reason for `supported: false`.
    pub unsupported_reason: Option<String>,
    /// True for a `tauri dev` run, where checking and installing are off.
    pub dev: bool,
    pub hint: String,
}

/// Download progress for the About panel.
#[derive(Debug, Clone, Serialize)]
struct UpdateProgress {
    downloaded: u64,
    /// `None` until the server sends a `Content-Length`.
    total: Option<u64>,
    done: bool,
}

/// Can this build install an update itself?
///
/// - **Windows**: yes. The NSIS installer replaces the install in place, and
///   `installMode: passive` drives it without user interaction.
/// - **Linux**: only when running the AppImage — the plugin swaps that single
///   writable file. A deb/rpm install has nothing it may overwrite.
/// - **macOS**: no. The plugin refuses an unsigned bundle to apply an update,
///   and this build ships unsigned; see docs if Developer ID signing lands.
fn platform_support() -> (bool, Option<&'static str>) {
    if cfg!(windows) {
        return (true, None);
    }
    if cfg!(target_os = "linux") {
        return if std::env::var_os("APPIMAGE").is_some() {
            (true, None)
        } else {
            (false, Some("not_appimage"))
        };
    }
    (false, Some("requires_signing"))
}

/// A `tauri dev` run reports the committed `0.1.0` placeholder, so it would
/// always look stale against the published manifest — and installing over a
/// running dev build is never what anyone wants. That takes it out of
/// `available`, not out of the check: the newest published version is still
/// what a maintainer wants to read in Settings → About.
fn is_dev_build() -> bool {
    cfg!(debug_assertions)
}

/// Why installing right now would throw work away. The Windows installer force
/// quits the process, so an in-flight transcription or model download would be
/// silently discarded; on other platforms the same quit happens via restart.
fn busy_reason(app: &AppHandle) -> Option<&'static str> {
    let state = app.state::<crate::models::ModelState>();
    let transcribing = state.transcribe_lock.try_lock().is_err();
    let downloading = state
        .cancel_flags
        .lock()
        .map(|flags| !flags.is_empty())
        .unwrap_or(false);

    if transcribing {
        Some("stt_running")
    } else if downloading {
        Some("model_downloading")
    } else {
        None
    }
}

fn tag_url(version: &str) -> String {
    format!("https://github.com/wsva/fms-app/releases/tag/v{version}")
}

/// Ask the configured endpoint whether a newer release exists.
///
/// A development build reads the manifest like any other — the newest published
/// version is the number to compare a tag against — but never reports it as
/// available: the version such a build reports is the committed `0.1.0`
/// placeholder, so every real release would look newer than it, and `install`
/// refuses a dev build anyway. Reporting the placeholder *as* the latest release
/// (which is what copying `current` into `latest` did) is the one answer that is
/// wrong in both directions.
pub async fn check(app: &AppHandle) -> Result<UpdateStatus, String> {
    let (supported, unsupported_reason) = platform_support();
    let dev = is_dev_build();
    let mut status = UpdateStatus {
        current: env!("CARGO_PKG_VERSION").to_string(),
        latest: String::new(),
        available: false,
        notes: String::new(),
        release_url: RELEASES_PAGE.to_string(),
        supported,
        unsupported_reason: unsupported_reason.map(str::to_string),
        dev,
        hint: String::new(),
    };

    if !supported {
        status.hint = format!(
            "This build cannot update itself; download the newest installer from {DOWNLOAD_PAGE} or the releases page instead."
        );
        return Ok(status);
    }

    let update = app
        .updater_builder()
        .timeout(ENDPOINT_TIMEOUT)
        .build()
        .map_err(|e| format!("Updater unavailable: {e}"))?
        .check()
        .await
        .map_err(|e| format!("Update check failed: {e}"))?;

    match update {
        Some(update) => {
            status.latest = update.version.clone();
            status.notes = update.body.unwrap_or_default();
            status.release_url = tag_url(&update.version);
            status.available = !dev;
            status.hint = if dev {
                format!(
                    "Development build: the newest published release is {}. This build reports the {} placeholder and will not install an update; use a released build for that.",
                    update.version, status.current
                )
            } else {
                format!(
                    "FmS {} is available (installed: {}). Call app_update_install to download and install it, or tell the user it is in Settings > About.",
                    update.version, update.current_version
                )
            };
        }
        None => {
            status.hint = if dev {
                format!(
                    "Development build: nothing newer than {} is published, and this build will not install an update.",
                    status.current
                )
            } else {
                status.latest = status.current.clone();
                format!("Already on the newest release ({}).", status.current)
            };
        }
    }
    Ok(status)
}

/// Download and install the newest release.
///
/// Returns only where the installer can be applied without leaving the
/// process: on Windows the plugin exits first, so the caller never sees a
/// result there. That hand-over is a race the app cannot win by exiting
/// faster — the installer starts while this process is still tearing down,
/// and its Restart Manager shutdown of a dying process fails. So the installer
/// waits for this process to disappear itself; see `nsis/installer-hooks.nsh`.
pub async fn install(app: &AppHandle) -> Result<(), String> {
    let (supported, _) = platform_support();
    if !supported {
        return Err(format!(
            "This build cannot install updates itself; download the newest installer from {DOWNLOAD_PAGE} (or the releases page) instead."
        ));
    }
    if is_dev_build() {
        return Err("Update install is disabled in development builds.".to_string());
    }
    match busy_reason(app) {
        Some("stt_running") => {
            return Err(
                "A transcription is running. Finish or cancel it, then retry the update.".to_string(),
            )
        }
        Some("model_downloading") => {
            return Err(
                "A model download is running. Finish or cancel it, then retry the update."
                    .to_string(),
            )
        }
        Some(other) => return Err(format!("The app is busy ({other}); retry the update later.")),
        None => {}
    }

    let updater = app
        .updater_builder()
        .timeout(ENDPOINT_TIMEOUT)
        .on_before_exit(|| {
            log::info!("[updater] exiting so the installer can replace the running app");
        })
        .build()
        .map_err(|e| format!("Updater unavailable: {e}"))?;

    let Some(update) = updater
        .check()
        .await
        .map_err(|e| format!("Update check failed: {e}"))?
    else {
        return Err("No newer release is available.".to_string());
    };

    log::info!(
        "[updater] installing {} (currently {})",
        update.version,
        update.current_version
    );

    let downloaded = Arc::new(AtomicU64::new(0));
    let total = Arc::new(AtomicU64::new(0));
    let chunk_done = Arc::clone(&downloaded);
    let chunk_total = Arc::clone(&total);
    let finish_done = Arc::clone(&downloaded);
    let finish_total = Arc::clone(&total);
    let app_for_chunks = app.clone();
    let app_for_finish = app.clone();

    update
        .download_and_install(
            move |chunk_length: usize, content_length: Option<u64>| {
                if let Some(content_length) = content_length {
                    chunk_total.store(content_length, Ordering::Relaxed);
                }
                let received = chunk_done.fetch_add(chunk_length as u64, Ordering::Relaxed)
                    + chunk_length as u64;
                let total = chunk_total.load(Ordering::Relaxed);
                let _ = app_for_chunks.emit(
                    EVENT_PROGRESS,
                    UpdateProgress {
                        downloaded: received,
                        total: (total > 0).then_some(total),
                        done: false,
                    },
                );
            },
            move || {
                log::info!("[updater] download complete, handing off to the installer");
                let total = finish_total.load(Ordering::Relaxed);
                let _ = app_for_finish.emit(
                    EVENT_PROGRESS,
                    UpdateProgress {
                        downloaded: finish_done.load(Ordering::Relaxed),
                        total: (total > 0).then_some(total),
                        done: true,
                    },
                );
            },
        )
        .await
        .map_err(|e| format!("Update install failed: {e}"))?;

    // Windows never gets here (see the event comment above). Elsewhere the new
    // bundle is in place and `request_restart` re-executes the app: it asks the
    // loop to exit with the restart code rather than killing the process, so
    // RunEvent handlers still run.
    #[cfg(not(windows))]
    {
        log::info!("[updater] restarting into {}", update.version);
        app.request_restart();
    }

    Ok(())
}

/// Report whether a newer release exists. Never downloads anything: this is the
/// notify half of notify-first, and it is what Settings → About calls on mount.
#[tauri::command]
pub async fn app_update_check(app: AppHandle) -> Result<UpdateStatus, String> {
    check(&app).await
}

/// What the *Download & install* button (or an agent that was told to go ahead)
/// calls: fetch, verify the signature against `tauri.conf.json`'s pubkey,
/// install, restart.
///
/// Two refusals are worth wording separately for the UI: busy (finish the work
/// in flight and retry) and unsupported (nothing to do here, follow
/// `release_url` instead; see [`platform_support`]).
#[tauri::command]
pub async fn app_update_install(app: AppHandle) -> Result<(), String> {
    install(&app).await
}
