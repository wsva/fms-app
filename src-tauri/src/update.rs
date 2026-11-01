//! In-app updates: notice a newer release, and install it only when asked.
//!
//! Self-update lives here; *checking* is cross-platform. Notify-first by
//! construction — nothing installs until `app_update_install` is called, from
//! Settings → About or by an agent through MCP. On desktop the check goes
//! through the updater plugin's configured endpoint (the `latest.json` asset on
//! the newest published release), so the download is verified against the
//! minisign public key in `tauri.conf.json`: a compromised host cannot ship a
//! binary. Mobile builds have no installer to drive, so neither the plugin nor
//! `app_update_install` compiles there — but `app_update_check` is registered
//! and answers the manifest over plain HTTP (see [`check_mobile`]), so Android
//! can still report a newer release and point at its APK.

#[cfg(feature = "desktop")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "desktop")]
use std::sync::Arc;

use serde::Serialize;
use tauri::AppHandle;
#[cfg(feature = "desktop")]
use tauri::{Emitter, Manager};
#[cfg(feature = "desktop")]
use tauri_plugin_updater::UpdaterExt;

/// Progress events while an update downloads. `done` flips once the bytes are
/// verified and the installer takes over, which is the last thing the webview
/// hears from this process on Windows (the NSIS installer has to replace the
/// running executable, so the plugin quits the app).
#[cfg(feature = "desktop")]
const EVENT_PROGRESS: &str = "update://progress";

/// Where a human goes when the newest version is not known yet.
const RELEASES_PAGE: &str = "https://github.com/wsva/fms-app/releases/latest";

/// The release manifest the updater plugin is configured with
/// (`tauri.conf.json` → `plugins.updater.endpoints[0]`). A mobile build reads
/// the same URL itself, because the plugin is not compiled there.
const MANIFEST_URL: &str = "https://github.com/wsva/fms-app/releases/latest/download/latest.json";

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
#[cfg(feature = "desktop")]
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
/// - **Android / iOS**: no. There is no installer to replace; a release
///   reaches the phone as an APK the user installs by hand. The *check* is
///   still worth answering there — see [`check_mobile`].
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
    if cfg!(any(target_os = "android", target_os = "ios")) {
        return (false, Some("mobile"));
    }
    (false, Some("requires_signing"))
}

/// Why a build that cannot install an update itself still cannot report one.
/// `None` = the manifest is worth asking for. Mobile is the one unsupported
/// platform that asks: a newer release ships an APK the user can install by
/// hand, so "what is the newest version" is an answerable, useful question.
/// macOS/deb/rpm never reach the manifest, and `latest.json` carries no mobile
/// artifacts anyway.
fn check_unsupported_reason() -> Option<&'static str> {
    if cfg!(any(target_os = "android", target_os = "ios")) {
        return None;
    }
    Some("no_self_update")
}

/// Ask `latest.json` directly, without the updater plugin.
///
/// Signature verification is the plugin's, and it is desktop-only — which is
/// fine here precisely because nothing installs: the webview only renders a
/// version number and a link out to the release page. The version compare is
/// the same strictly-newer semver rule the plugin applies, segment by segment.
async fn check_mobile(current: &str) -> Result<(String, String), String> {
    let resp = reqwest::get(MANIFEST_URL)
        .await
        .map_err(|e| format!("Update check failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Update check failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("Update check failed: manifest is not valid JSON ({e})"))?;
    let latest = resp["version"].as_str().unwrap_or_default().to_string();
    let notes = resp["notes"].as_str().unwrap_or_default().to_string();
    Ok((if is_newer(&latest, current) { latest } else { String::new() }, notes))
}

/// Dot-separated numeric compare: `candidate` counts as newer only when it is
/// strictly greater than `installed` segment by segment (missing segments are
/// zero, so `0.2` equals `0.2.0`). A non-numeric segment bails out to "not
/// newer" — better to stay quiet than to nag about a version we cannot read.
fn is_newer(candidate: &str, installed: &str) -> bool {
    let segments = |v: &str| {
        v.trim_start_matches('v')
            .split('.')
            .map(|s| s.parse::<u64>().ok())
            .collect::<Option<Vec<_>>>()
    };
    let (Some(candidate), Some(installed)) = (segments(candidate), segments(installed)) else {
        return false;
    };
    for i in 0..candidate.len().max(installed.len()) {
        let a = *candidate.get(i).unwrap_or(&0);
        let b = *installed.get(i).unwrap_or(&0);
        if a != b {
            return a > b;
        }
    }
    false
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
#[cfg(feature = "desktop")]
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
#[cfg_attr(not(feature = "desktop"), allow(unused_variables))]
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

    // A build with no self-update path *and* no readable manifest (macOS,
    // deb/rpm) answers without a network round-trip. Mobile is the exception:
    // it cannot install, but a newer release is still a fact worth reporting,
    // so it reads the manifest itself below.
    if !supported && check_unsupported_reason().is_some() {
        status.hint = "This build cannot update itself; download the newest installer from the releases page instead.".to_string();
        return Ok(status);
    }

    // (newer-version-or-empty, notes): the plugin answers `None` unless the
    // manifest is strictly newer; the mobile reader applies the same semver
    // rule itself. Dev builds keep the answer but not `available` — see below.
    let found = if supported {
        #[cfg(feature = "desktop")]
        {
            let update = app
                .updater()
                .map_err(|e| format!("Updater unavailable: {e}"))?
                .check()
                .await
                .map_err(|e| format!("Update check failed: {e}"))?;
            match update {
                Some(update) => (Some(update.version), update.body.unwrap_or_default()),
                None => (None, String::new()),
            }
        }
        #[cfg(not(feature = "desktop"))]
        {
            // `supported` is only true on desktop platforms, where the plugin
            // is compiled; a non-desktop build never takes this branch.
            unreachable!("self-update requires a desktop platform")
        }
    } else {
        let (latest, notes) = check_mobile(&status.current).await?;
        ((!latest.is_empty()).then_some(latest), notes)
    };

    match found {
        (Some(latest), notes) => {
            status.latest = latest.clone();
            status.notes = notes;
            status.release_url = tag_url(&latest);
            status.available = supported && !dev;
            status.hint = if !supported {
                format!(
                    "FmS {} is available (installed: {}). This build cannot update itself; download and install it from the release page.",
                    latest, status.current
                )
            } else if dev {
                format!(
                    "Development build: the newest published release is {}. This build reports the {} placeholder and will not install an update; use a released build for that.",
                    latest, status.current
                )
            } else {
                format!(
                    "FmS {} is available (installed: {}). Call app_update_install to download and install it, or tell the user it is in Settings > About.",
                    latest, status.current
                )
            };
        }
        (None, _) => {
            status.hint = if dev {
                format!(
                    "Development build: nothing newer than {} is published, and this build will not install an update.",
                    status.current
                )
            } else if !supported {
                format!(
                    "This build cannot update itself; {} is already the newest published release.",
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
#[cfg(feature = "desktop")]
pub async fn install(app: &AppHandle) -> Result<(), String> {
    let (supported, _) = platform_support();
    if !supported {
        return Err(
            "This build cannot install updates itself; download the newest installer from the releases page instead."
                .to_string(),
        );
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
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn app_update_install(app: AppHandle) -> Result<(), String> {
    install(&app).await
}
